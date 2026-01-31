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
use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine;

use crate::client::ButterflyBot;
use crate::config::{AgentConfig, Config, MemoryConfig, OpenAiConfig};
use crate::config_store;
use crate::e2e::identity_store::KeyringIdentityStore;
use crate::e2e::manager::E2eManager;
use crate::e2e::E2eEnvelope;
use crate::e2e::trust_store::{get_peer_key, set_trust_state, upsert_peer_key, TrustState};
use crate::error::{ButterflyBotError, Result};
use crate::interfaces::scheduler::ScheduledJob;
use crate::reminders::ReminderStore;
use crate::scheduler::Scheduler;
use crate::services::agent::UiEvent;
use crate::services::query::{OutputFormat, ProcessOptions, ProcessResult, UserInput};
use tokio::sync::broadcast;

#[derive(Clone)]
pub struct AppState {
    pub agent: Arc<ButterflyBot>,
    pub reminder_store: Arc<ReminderStore>,
    pub token: String,
    pub ui_event_tx: broadcast::Sender<UiEvent>,
    pub e2e: Arc<E2eManager>,
}

struct BrainTickJob {
    agent: Arc<ButterflyBot>,
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
        self.agent.brain_tick().await;
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

#[derive(Deserialize)]
struct E2eIdentityQuery {
    user_id: String,
}

#[derive(Serialize)]
struct E2eIdentityResponse {
    public_key: String,
}

#[derive(Deserialize)]
struct E2eEncryptRequest {
    user_id: String,
    peer_public_key: String,
    plaintext: String,
}

#[derive(Serialize)]
struct E2eEncryptResponse {
    envelope: ApiE2eEnvelope,
}

#[derive(Deserialize)]
struct E2eDecryptRequest {
    user_id: String,
    envelope: ApiE2eEnvelope,
}

#[derive(Serialize)]
struct E2eDecryptResponse {
    plaintext: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ApiE2eEnvelope {
    version: u8,
    sender_public_key: String,
    nonce: String,
    ciphertext: String,
}

#[derive(Deserialize)]
struct E2eTrustRequest {
    user_id: String,
    peer_id: String,
    trust_state: String,
}

#[derive(Serialize)]
struct E2eTrustResponse {
    trust_state: String,
}

pub fn build_router(state: AppState) -> Router {
    Router::new()
        .route("/health", get(health))
        .route("/process_text", post(process_text))
        .route("/process_text_stream", post(process_text_stream))
        .route("/memory_search", post(memory_search))
        .route("/reminder_stream", get(reminder_stream))
        .route("/ui_events", get(ui_events))
        .route("/e2e/identity", get(e2e_identity))
        .route("/e2e/encrypt", post(e2e_encrypt))
        .route("/e2e/decrypt", post(e2e_decrypt))
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
        router: None,
    };

    let response = state
        .agent
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

    let AppState { agent, .. } = state;
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
    let response = state
        .agent
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

async fn e2e_identity(
    State(state): State<AppState>,
    headers: HeaderMap,
    axum::extract::Query(query): axum::extract::Query<E2eIdentityQuery>,
) -> impl IntoResponse {
    if let Err(err) = authorize(&headers, &state.token) {
        return err.into_response();
    }

    match state.e2e.identity_public_key(&query.user_id) {
        Ok(public_key) => (
            StatusCode::OK,
            Json(E2eIdentityResponse {
                public_key: BASE64.encode(public_key),
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

async fn e2e_encrypt(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(payload): Json<E2eEncryptRequest>,
) -> impl IntoResponse {
    if let Err(err) = authorize(&headers, &state.token) {
        return err.into_response();
    }

    let peer_key = match BASE64.decode(payload.peer_public_key.as_bytes()) {
        Ok(bytes) => bytes,
        Err(err) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(ErrorResponse {
                    error: err.to_string(),
                }),
            )
                .into_response();
        }
    };
    if peer_key.len() != 32 {
        return (
            StatusCode::BAD_REQUEST,
            Json(ErrorResponse {
                error: "invalid peer public key length".to_string(),
            }),
        )
            .into_response();
    }
    let mut peer_public = [0u8; 32];
    peer_public.copy_from_slice(&peer_key);

    match state
        .e2e
        .encrypt_for(&payload.user_id, peer_public, payload.plaintext.as_bytes())
    {
        Ok(envelope) => (
            StatusCode::OK,
            Json(E2eEncryptResponse {
                envelope: api_from_envelope(&envelope),
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

async fn e2e_decrypt(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(payload): Json<E2eDecryptRequest>,
) -> impl IntoResponse {
    if let Err(err) = authorize(&headers, &state.token) {
        return err.into_response();
    }

    let envelope = match envelope_from_api(&payload.envelope) {
        Ok(envelope) => envelope,
        Err(err) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(ErrorResponse {
                    error: err.to_string(),
                }),
            )
                .into_response();
        }
    };

    match state.e2e.decrypt_for(&payload.user_id, &envelope) {
        Ok(plaintext) => (
            StatusCode::OK,
            Json(E2eDecryptResponse {
                plaintext: String::from_utf8_lossy(&plaintext).to_string(),
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

fn api_from_envelope(envelope: &E2eEnvelope) -> ApiE2eEnvelope {
    ApiE2eEnvelope {
        version: envelope.version,
        sender_public_key: BASE64.encode(envelope.sender_public_key),
        nonce: BASE64.encode(envelope.nonce),
        ciphertext: BASE64.encode(&envelope.ciphertext),
    }
}

fn envelope_from_api(envelope: &ApiE2eEnvelope) -> Result<E2eEnvelope> {
    let sender = BASE64
        .decode(envelope.sender_public_key.as_bytes())
        .map_err(|e| ButterflyBotError::Runtime(e.to_string()))?;
    let nonce = BASE64
        .decode(envelope.nonce.as_bytes())
        .map_err(|e| ButterflyBotError::Runtime(e.to_string()))?;
    let ciphertext = BASE64
        .decode(envelope.ciphertext.as_bytes())
        .map_err(|e| ButterflyBotError::Runtime(e.to_string()))?;
    if sender.len() != 32 {
        return Err(ButterflyBotError::Runtime(
            "invalid sender public key length".to_string(),
        ));
    }
    if nonce.len() != 12 {
        return Err(ButterflyBotError::Runtime("invalid nonce length".to_string()));
    }
    let mut sender_key = [0u8; 32];
    sender_key.copy_from_slice(&sender);
    let mut nonce_bytes = [0u8; 12];
    nonce_bytes.copy_from_slice(&nonce);
    Ok(E2eEnvelope {
        version: envelope.version,
        sender_public_key: sender_key,
        nonce: nonce_bytes,
        ciphertext,
    })
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
    let model = "glm-4.7-flash:latest".to_string();
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
        agents: vec![AgentConfig {
            name: "default_agent".to_string(),
            description: Some("Butterfly, an expert conversationalist and assistant.".to_string()),
            instructions:
                r#"You are Butterfly, an expert conversationalist and calm, capable assistant.

Core behavior:
- Be warm, concise, and natural. Ask clarifying questions when the request is ambiguous.
- Prefer actionable help over long explanations. Offer a short plan when helpful.
- If you’re unsure, say so briefly and suggest the next best step.

Tools you can use:
- reminders: create/list/complete/delete/snooze reminders and todos.
    Use it when the user asks for reminders, alarms, timers, tasks, or follow-ups.
- search_internet: fetch up-to-date info when the user asks for current events or live data.

Memory:
- Use provided context, but do not treat assistant statements as user facts.
- Confirm personal details before relying on them.

When scheduling:
- If the user asks “in X seconds/minutes/hours,” create a reminder with that delay.
- If they ask “tomorrow at 3pm” or similar, ask for timezone if missing.
"#
                .to_string(),
            specialization: "conversation".to_string(),
            tools: Some(vec!["reminders".to_string(), "search_internet".to_string()]),
            capture_name: None,
            capture_schema: None,
        }],
        business: None,
        memory,
        guardrails: None,
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
    let agent =
        Arc::new(ButterflyBot::from_store_with_events(db_path, Some(ui_event_tx.clone())).await?);
    let reminder_store = Arc::new(ReminderStore::new(db_path).await?);
    let identity_store = Arc::new(KeyringIdentityStore::new("butterfly-bot.identity"));
    let e2e = Arc::new(E2eManager::new(identity_store));
    let mut scheduler = Scheduler::new();
    scheduler.register_job(Arc::new(BrainTickJob {
        agent: agent.clone(),
        interval: Duration::from_secs(tick_seconds.max(1)),
    }));
    scheduler.start();

    let state = AppState {
        agent,
        reminder_store,
        token: token.to_string(),
        ui_event_tx,
        e2e,
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
