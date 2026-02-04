use std::future::Future;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use axum::{
    body::Body,
    extract::{Json, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
    Router,
};
use bytes::Bytes;
use futures::StreamExt;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::client::ButterflyBot;
use crate::config::{Config, MemoryConfig, OpenAiConfig};
use crate::config_store;
use crate::error::{ButterflyBotError, Result};
use crate::factories::agent_factory::load_markdown_source;
use crate::interfaces::scheduler::ScheduledJob;
use crate::reminders::ReminderStore;
use crate::scheduler::Scheduler;
use crate::services::agent::UiEvent;
use crate::services::query::{OutputFormat, ProcessOptions, ProcessResult, UserInput};
use crate::tasks::TaskStore;
use crate::wakeup::WakeupStore;
use tokio::sync::{broadcast, RwLock};

#[derive(Clone)]
pub struct AppState {
    pub agent: Arc<RwLock<Arc<ButterflyBot>>>,
    pub reminder_store: Arc<ReminderStore>,
    pub token: String,
    pub ui_event_tx: broadcast::Sender<UiEvent>,
    pub db_path: String,
}

struct BrainTickJob {
    agent: Arc<RwLock<Arc<ButterflyBot>>>,
    interval: Duration,
}

#[async_trait::async_trait]
impl ScheduledJob for BrainTickJob {
    fn name(&self) -> &str {
        "brain_tick"
    }

    fn interval(&self) -> Duration {
        self.interval
    }

    async fn run(&self) -> Result<()> {
        let agent = self.agent.read().await.clone();
        agent.brain_tick().await;
        Ok(())
    }
}

struct WakeupJob {
    agent: Arc<RwLock<Arc<ButterflyBot>>>,
    store: Arc<WakeupStore>,
    interval: Duration,
    ui_event_tx: broadcast::Sender<UiEvent>,
    audit_log_path: Option<String>,
    heartbeat_source: Option<String>,
}

struct ScheduledTasksJob {
    agent: Arc<RwLock<Arc<ButterflyBot>>>,
    store: Arc<TaskStore>,
    interval: Duration,
    ui_event_tx: broadcast::Sender<UiEvent>,
    audit_log_path: Option<String>,
}

#[async_trait::async_trait]
impl ScheduledJob for ScheduledTasksJob {
    fn name(&self) -> &str {
        "scheduled_tasks"
    }

    fn interval(&self) -> Duration {
        self.interval
    }

    async fn run(&self) -> Result<()> {
        let now = now_ts();
        let tasks = self.store.list_due(now, 32).await?;
        for task in tasks {
            let agent = self.agent.read().await.clone();
            let run_at = now_ts();
            let next_run_at = if let Some(interval) = task.interval_minutes {
                run_at + interval.max(1) * 60
            } else {
                run_at
            };

            if task.interval_minutes.is_some() {
                let _ = self.store.mark_run(task.id, run_at, next_run_at).await;
            } else {
                let _ = self.store.complete_one_shot(task.id).await;
            }

            let options = ProcessOptions {
                prompt: None,
                images: Vec::new(),
                output_format: OutputFormat::Text,
                image_detail: "auto".to_string(),
                json_schema: None,
            };
            let input = format!("Scheduled task '{}': {}", task.name, task.prompt);
            let result = agent
                .process(&task.user_id, UserInput::Text(input), options)
                .await;

            let (status, payload): (String, serde_json::Value) = match result {
                Ok(ProcessResult::Text(text)) => (
                    "ok".to_string(),
                    json!({"task_id": task.id, "name": task.name, "output": text}),
                ),
                Ok(other) => (
                    "ok".to_string(),
                    json!({"task_id": task.id, "name": task.name, "output": format!("{other:?}")}),
                ),
                Err(err) => (
                    "error".to_string(),
                    json!({"task_id": task.id, "name": task.name, "error": err.to_string()}),
                ),
            };

            let event = UiEvent {
                event_type: "tasks".to_string(),
                user_id: task.user_id.clone(),
                tool: "tasks".to_string(),
                status: status.clone(),
                payload: payload.clone(),
                timestamp: run_at,
            };
            let _ = self.ui_event_tx.send(event);
            let _ = write_tasks_audit_log(
                self.audit_log_path.as_deref(),
                run_at,
                &task,
                status.as_str(),
                payload,
            );
        }
        Ok(())
    }
}

#[async_trait::async_trait]
impl ScheduledJob for WakeupJob {
    fn name(&self) -> &str {
        "wakeup"
    }

    fn interval(&self) -> Duration {
        self.interval
    }

    async fn run(&self) -> Result<()> {
        let now = now_ts();
        if let Some(source) = &self.heartbeat_source {
            match load_markdown_source(Some(source.as_str())).await {
                Ok(markdown) => {
                    let agent = self.agent.read().await.clone();
                    agent.set_heartbeat_markdown(markdown).await;
                    let event = UiEvent {
                        event_type: "wakeup".to_string(),
                        user_id: "system".to_string(),
                        tool: "heartbeat".to_string(),
                        status: "ok".to_string(),
                        payload: json!({"source": source}),
                        timestamp: now_ts(),
                    };
                    let _ = self.ui_event_tx.send(event);
                }
                Err(err) => {
                    let event = UiEvent {
                        event_type: "wakeup".to_string(),
                        user_id: "system".to_string(),
                        tool: "heartbeat".to_string(),
                        status: "error".to_string(),
                        payload: json!({"source": source, "error": err.to_string()}),
                        timestamp: now_ts(),
                    };
                    let _ = self.ui_event_tx.send(event);
                }
            }
        }
        let tasks = self.store.list_due(now, 32).await?;
        for task in tasks {
            let agent = self.agent.read().await.clone();
            let run_at = now_ts();
            let next_run_at = run_at + task.interval_minutes.max(1) * 60;
            let _ = self.store.mark_run(task.id, run_at, next_run_at).await;

            let options = ProcessOptions {
                prompt: None,
                images: Vec::new(),
                output_format: OutputFormat::Text,
                image_detail: "auto".to_string(),
                json_schema: None,
            };
            let input = format!("Wakeup task '{}': {}", task.name, task.prompt);
            let result = agent
                .process(&task.user_id, UserInput::Text(input), options)
                .await;

            let (status, payload): (String, Value) = match result {
                Ok(ProcessResult::Text(text)) => (
                    "ok".to_string(),
                    json!({"task_id": task.id, "name": task.name, "output": text}),
                ),
                Ok(other) => (
                    "ok".to_string(),
                    json!({"task_id": task.id, "name": task.name, "output": format!("{other:?}")}),
                ),
                Err(err) => (
                    "error".to_string(),
                    json!({"task_id": task.id, "name": task.name, "error": err.to_string()}),
                ),
            };

            let event = UiEvent {
                event_type: "wakeup".to_string(),
                user_id: task.user_id.clone(),
                tool: "wakeup".to_string(),
                status: status.clone(),
                payload: payload.clone(),
                timestamp: run_at,
            };
            let _ = self.ui_event_tx.send(event);
            let _ = write_wakeup_audit_log(
                self.audit_log_path.as_deref(),
                run_at,
                &task,
                status.as_str(),
                payload.clone(),
            );
        }
        Ok(())
    }
}

#[derive(Serialize)]
struct HealthResponse {
    status: String,
}

#[derive(Deserialize)]
struct ProcessTextRequest {
    user_id: String,
    text: String,
    prompt: Option<String>,
}

#[derive(Serialize)]
struct ProcessTextResponse {
    text: String,
}

#[derive(Deserialize)]
struct MemorySearchRequest {
    user_id: String,
    query: String,
    limit: Option<usize>,
}

#[derive(Deserialize)]
struct ReminderStreamQuery {
    user_id: String,
}

#[derive(Deserialize)]
struct UiEventStreamQuery {
    user_id: Option<String>,
}

#[derive(Serialize)]
struct MemorySearchResponse {
    results: Vec<String>,
}

#[derive(Serialize)]
struct ErrorResponse {
    error: String,
}

pub fn build_router(state: AppState) -> Router {
    Router::new()
        .route("/health", get(health))
        .route("/process_text", post(process_text))
        .route("/process_text_stream", post(process_text_stream))
        .route("/memory_search", post(memory_search))
        .route("/reminder_stream", get(reminder_stream))
        .route("/ui_events", get(ui_events))
        .route("/reload_config", post(reload_config))
        .with_state(state)
}

async fn health() -> Json<HealthResponse> {
    Json(HealthResponse {
        status: "ok".to_string(),
    })
}

async fn process_text(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(payload): Json<ProcessTextRequest>,
) -> impl IntoResponse {
    if let Err(err) = authorize(&headers, &state.token) {
        return err.into_response();
    }

    let options = ProcessOptions {
        prompt: payload.prompt.clone(),
        images: Vec::new(),
        output_format: OutputFormat::Text,
        image_detail: "auto".to_string(),
        json_schema: None,
    };

    let agent = state.agent.read().await.clone();
    let response = agent
        .process(&payload.user_id, UserInput::Text(payload.text), options)
        .await;

    match response {
        Ok(ProcessResult::Text(text)) => {
            (StatusCode::OK, Json(ProcessTextResponse { text })).into_response()
        }
        Ok(other) => (
            StatusCode::BAD_REQUEST,
            Json(ErrorResponse {
                error: format!("Unexpected response: {other:?}"),
            }),
        )
            .into_response(),
        Err(err) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ErrorResponse {
                error: err.to_string(),
            }),
        )
            .into_response(),
    }
}

async fn process_text_stream(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(payload): Json<ProcessTextRequest>,
) -> impl IntoResponse {
    if let Err(err) = authorize(&headers, &state.token) {
        return err.into_response();
    }

    let agent = state.agent.read().await.clone();
    let ProcessTextRequest {
        user_id,
        text,
        prompt,
    } = payload;

    let body = Body::from_stream(async_stream::stream! {
        let mut stream = agent.process_text_stream(&user_id, &text, prompt.as_deref());
        while let Some(item) = stream.next().await {
            match item {
                Ok(chunk) => {
                    if !chunk.is_empty() {
                        yield Ok::<Bytes, std::convert::Infallible>(Bytes::from(chunk));
                    }
                }
                Err(err) => {
                    let message = format!("\n[error] {}", err);
                    yield Ok(Bytes::from(message));
                    break;
                }
            }
        }
    });

    Response::builder()
        .status(StatusCode::OK)
        .header("content-type", "text/plain; charset=utf-8")
        .body(body)
        .unwrap()
}

async fn memory_search(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(payload): Json<MemorySearchRequest>,
) -> impl IntoResponse {
    if let Err(err) = authorize(&headers, &state.token) {
        return err.into_response();
    }

    let limit = payload.limit.unwrap_or(8);
    let agent = state.agent.read().await.clone();
    let response = agent
        .search_memory(&payload.user_id, &payload.query, limit)
        .await;

    match response {
        Ok(results) => (StatusCode::OK, Json(MemorySearchResponse { results })).into_response(),
        Err(err) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ErrorResponse {
                error: err.to_string(),
            }),
        )
            .into_response(),
    }
}

async fn reminder_stream(
    State(state): State<AppState>,
    headers: HeaderMap,
    axum::extract::Query(query): axum::extract::Query<ReminderStreamQuery>,
) -> impl IntoResponse {
    if let Err(err) = authorize(&headers, &state.token) {
        return err.into_response();
    }

    let store = state.reminder_store.clone();
    let user_id = query.user_id;
    let mut tick = tokio::time::interval(Duration::from_secs(1));

    let body = Body::from_stream(async_stream::stream! {
        loop {
            tick.tick().await;
            let now = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs() as i64;
            if let Ok(items) = store.due_reminders(&user_id, now, 10).await {
                for item in items {
                    let payload = serde_json::json!({
                        "id": item.id,
                        "title": item.title,
                        "due_at": item.due_at,
                    });
                    let line = format!("data: {}\n\n", payload);
                    yield Ok::<Bytes, std::convert::Infallible>(Bytes::from(line));
                }
            }
        }
    });

    Response::builder()
        .status(StatusCode::OK)
        .header("content-type", "text/event-stream")
        .header("cache-control", "no-cache")
        .body(body)
        .unwrap()
}

async fn reload_config(State(state): State<AppState>, headers: HeaderMap) -> impl IntoResponse {
    if let Err(err) = authorize(&headers, &state.token) {
        return err.into_response();
    }

    let agent =
        ButterflyBot::from_store_with_events(&state.db_path, Some(state.ui_event_tx.clone())).await;
    match agent {
        Ok(agent) => {
            let mut guard = state.agent.write().await;
            *guard = Arc::new(agent);
            (
                StatusCode::OK,
                Json(json!({"status": "ok", "message": "Config reloaded"})),
            )
                .into_response()
        }
        Err(err) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ErrorResponse {
                error: err.to_string(),
            }),
        )
            .into_response(),
    }
}

async fn ui_events(
    State(state): State<AppState>,
    headers: HeaderMap,
    axum::extract::Query(query): axum::extract::Query<UiEventStreamQuery>,
) -> impl IntoResponse {
    if let Err(err) = authorize(&headers, &state.token) {
        return err.into_response();
    }

    let mut receiver = state.ui_event_tx.subscribe();
    let filter_user = query.user_id;

    let body = Body::from_stream(async_stream::stream! {
        loop {
            match receiver.recv().await {
                Ok(event) => {
                    if let Some(filter) = &filter_user {
                        if event.user_id != *filter {
                            continue;
                        }
                    }
                    let payload = serde_json::to_string(&event).unwrap_or_default();
                    let line = format!("data: {}\n\n", payload);
                    yield Ok::<Bytes, std::convert::Infallible>(Bytes::from(line));
                }
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {
                    continue;
                }
                Err(_) => break,
            }
        }
    });

    Response::builder()
        .status(StatusCode::OK)
        .header("content-type", "text/event-stream")
        .header("cache-control", "no-cache")
        .body(body)
        .unwrap()
}

fn authorize(
    headers: &HeaderMap,
    token: &str,
) -> std::result::Result<(), (StatusCode, Json<ErrorResponse>)> {
    let header = headers
        .get("authorization")
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default();
    let api_key = headers
        .get("x-api-key")
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default();
    let bearer = header.strip_prefix("Bearer ").unwrap_or("");

    if bearer == token || api_key == token {
        Ok(())
    } else {
        Err((
            StatusCode::UNAUTHORIZED,
            Json(ErrorResponse {
                error: "Unauthorized".to_string(),
            }),
        ))
    }
}

pub async fn run(host: &str, port: u16, db_path: &str, token: &str) -> Result<()> {
    run_with_shutdown(host, port, db_path, token, futures::future::pending::<()>()).await
}

fn default_config(db_path: &str) -> Config {
    let base_url = "http://localhost:11434/v1".to_string();
    let model = "ministral-3:14b".to_string();
    let memory = Some(MemoryConfig {
        enabled: Some(true),
        sqlite_path: Some(db_path.to_string()),
        lancedb_path: Some("./data/lancedb".to_string()),
        summary_model: Some(model.clone()),
        embedding_model: Some("embeddinggemma:latest".to_string()),
        rerank_model: Some("qllama/bge-reranker-v2-m3".to_string()),
        summary_threshold: None,
        retention_days: None,
    });

    Config {
        openai: Some(OpenAiConfig {
            api_key: None,
            model: Some(model),
            base_url: Some(base_url),
        }),
        skill_file: Some("./skill.md".to_string()),
        heartbeat_file: Some("./heartbeat.md".to_string()),
        memory,
        tools: None,
        brains: None,
    }
}

pub async fn run_with_shutdown<F>(
    host: &str,
    port: u16,
    db_path: &str,
    token: &str,
    shutdown: F,
) -> Result<()>
where
    F: Future<Output = ()> + Send + 'static,
{
    if Config::from_store(db_path).is_err() {
        let default_config = default_config(db_path);
        config_store::save_config(db_path, &default_config)?;
    }

    let config = Config::from_store(db_path).ok();
    let tick_seconds = config
        .as_ref()
        .and_then(|cfg| cfg.brains.as_ref())
        .and_then(|brains| brains.get("settings"))
        .and_then(|settings| settings.get("tick_seconds"))
        .and_then(|value| value.as_u64())
        .unwrap_or(60);

    let (ui_event_tx, _) = broadcast::channel(256);
    let agent = Arc::new(RwLock::new(Arc::new(
        ButterflyBot::from_store_with_events(db_path, Some(ui_event_tx.clone())).await?,
    )));
    let reminder_store = Arc::new(ReminderStore::new(db_path).await?);
    let task_store = Arc::new(TaskStore::new(db_path).await?);
    let wakeup_store = Arc::new(WakeupStore::new(db_path).await?);
    let mut scheduler = Scheduler::new();
    scheduler.register_job(Arc::new(BrainTickJob {
        agent: agent.clone(),
        interval: Duration::from_secs(tick_seconds.max(1)),
    }));
    let wakeup_poll_seconds = config
        .as_ref()
        .and_then(|cfg| cfg.tools.as_ref())
        .and_then(|tools| tools.get("wakeup"))
        .and_then(|wakeup| wakeup.get("poll_seconds"))
        .and_then(|value| value.as_u64())
        .unwrap_or(60);
    scheduler.register_job(Arc::new(WakeupJob {
        agent: agent.clone(),
        store: wakeup_store.clone(),
        interval: Duration::from_secs(wakeup_poll_seconds.max(1)),
        ui_event_tx: ui_event_tx.clone(),
        audit_log_path: wakeup_audit_log_path(config.as_ref()),
        heartbeat_source: config
            .as_ref()
            .and_then(|cfg| cfg.heartbeat_file.clone()),
    }));
    let tasks_poll_seconds = config
        .as_ref()
        .and_then(|cfg| cfg.tools.as_ref())
        .and_then(|tools| tools.get("tasks"))
        .and_then(|tasks| tasks.get("poll_seconds"))
        .and_then(|value| value.as_u64())
        .unwrap_or(60);
    scheduler.register_job(Arc::new(ScheduledTasksJob {
        agent: agent.clone(),
        store: task_store.clone(),
        interval: Duration::from_secs(tasks_poll_seconds.max(1)),
        ui_event_tx: ui_event_tx.clone(),
        audit_log_path: tasks_audit_log_path(config.as_ref()),
    }));
    scheduler.start();

    let state = AppState {
        agent,
        reminder_store,
        token: token.to_string(),
        ui_event_tx,
        db_path: db_path.to_string(),
    };
    let app = build_router(state);

    let addr = format!("{host}:{port}");
    let listener = tokio::net::TcpListener::bind(&addr)
        .await
        .map_err(|e| ButterflyBotError::Runtime(e.to_string()))?;
    let shutdown = async move {
        shutdown.await;
        scheduler.stop().await;
    };

    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown)
        .await
        .map_err(|e| ButterflyBotError::Runtime(e.to_string()))?;

    Ok(())
}

fn now_ts() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}

fn wakeup_audit_log_path(config: Option<&Config>) -> Option<String> {
    let path = config
        .and_then(|cfg| cfg.tools.as_ref())
        .and_then(|tools| tools.get("wakeup"))
        .and_then(|wakeup| wakeup.get("audit_log_path"))
        .and_then(|value| value.as_str())
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .or_else(|| Some("./data/wakeup_audit.log".to_string()));
    path
}

fn write_wakeup_audit_log(
    path: Option<&str>,
    ts: i64,
    task: &crate::wakeup::WakeupTask,
    status: &str,
    payload: serde_json::Value,
) -> Result<()> {
    let Some(path) = path else {
        return Ok(());
    };
    config_store::ensure_parent_dir(path)?;
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .map_err(|e| ButterflyBotError::Runtime(e.to_string()))?;
    let entry = serde_json::json!({
        "timestamp": ts,
        "task_id": task.id,
        "user_id": task.user_id,
        "name": task.name,
        "prompt": task.prompt,
        "status": status,
        "payload": payload,
    });
    let line = serde_json::to_string(&entry)
        .map_err(|e| ButterflyBotError::Serialization(e.to_string()))?;
    use std::io::Write;
    writeln!(file, "{line}").map_err(|e| ButterflyBotError::Runtime(e.to_string()))?;
    Ok(())
}

fn tasks_audit_log_path(config: Option<&Config>) -> Option<String> {
    let path = config
        .and_then(|cfg| cfg.tools.as_ref())
        .and_then(|tools| tools.get("tasks"))
        .and_then(|tasks| tasks.get("audit_log_path"))
        .and_then(|value| value.as_str())
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .or_else(|| Some("./data/tasks_audit.log".to_string()));
    path
}

fn write_tasks_audit_log(
    path: Option<&str>,
    ts: i64,
    task: &crate::tasks::ScheduledTask,
    status: &str,
    payload: serde_json::Value,
) -> Result<()> {
    let Some(path) = path else {
        return Ok(());
    };
    config_store::ensure_parent_dir(path)?;
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .map_err(|e| ButterflyBotError::Runtime(e.to_string()))?;
    let entry = serde_json::json!({
        "timestamp": ts,
        "task_id": task.id,
        "user_id": task.user_id,
        "name": task.name,
        "prompt": task.prompt,
        "run_at": task.run_at,
        "interval_minutes": task.interval_minutes,
        "status": status,
        "payload": payload,
    });
    let line = serde_json::to_string(&entry)
        .map_err(|e| ButterflyBotError::Serialization(e.to_string()))?;
    use std::io::Write;
    writeln!(file, "{line}").map_err(|e| ButterflyBotError::Runtime(e.to_string()))?;
    Ok(())
}
