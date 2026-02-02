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
fn now_ts() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}

async fn handle_gossip_events(
    state: AppState,
    mut receiver: broadcast::Receiver<GossipMessage>,
) {
    loop {
        let Ok(message) = receiver.recv().await else {
            continue;
        };
        match message.kind.as_str() {
            "message" => {
                let envelope_value = message
                    .payload
                    .get("envelope")
                    .cloned()
                    .unwrap_or(serde_json::Value::Null);
                let envelope = match serde_json::from_value::<ApiE2eEnvelope>(envelope_value) {
                    Ok(envelope) => envelope,
                    Err(_) => continue,
                };
                let envelope = match envelope_from_api(&envelope) {
                    Ok(envelope) => envelope,
                    Err(_) => continue,
                };

                let sender_public = BASE64.encode(envelope.sender_public_key);
                let _ = upsert_peer_key(
                    &state.db_path,
                    &message.to,
                    &message.from,
                    &sender_public,
                    TrustState::Unverified,
                );
                if let Ok(Some(record)) =
                    get_peer_key(&state.db_path, &message.to, &message.from)
                {
                    if record.trust_state == TrustState::Blocked {
                        continue;
                    }
                }

                if let Ok(plaintext) = state.e2e.decrypt_for(&message.to, &envelope) {
                    let text = String::from_utf8_lossy(&plaintext).to_string();
                    let event = UiEvent {
                        event_type: "p2p_message".to_string(),
                        user_id: message.to.clone(),
                        tool: "p2p".to_string(),
                        status: "received".to_string(),
                        payload: serde_json::json!({
                            "peer_id": message.from,
                            "message_id": message.message_id,
                            "text": text,
                        }),
                        timestamp: now_ts(),
                    };
                    let _ = state.ui_event_tx.send(event);
                    let _ = state
                        .gossip
                        .publish(GossipMessage {
                            kind: "receipt".to_string(),
                            to: message.from.clone(),
                            from: message.to.clone(),
                            message_id: message.message_id,
                            payload: serde_json::json!({ "status": "delivered" }),
                            signature: String::new(),
                            public_key: String::new(),
                        })
                        .await;
                }
            }
            "receipt" => {
                let status = message
                    .payload
                    .get("status")
                    .and_then(|value| value.as_str())
                    .unwrap_or("delivered");
                let event = UiEvent {
                    event_type: "p2p_receipt".to_string(),
                    user_id: message.to.clone(),
                    tool: "p2p".to_string(),
                    status: status.to_string(),
                    payload: serde_json::json!({
                        "peer_id": message.from,
                        "message_id": message.message_id,
                        "status": status,
                    }),
                    timestamp: now_ts(),
                };
                let _ = state.ui_event_tx.send(event);
            }
            "username_claim" => {
                let username = message
                    .payload
                    .get("username")
                    .and_then(|v| v.as_str())
                    .unwrap_or_default();
                let peer_id = message
                    .payload
                    .get("peer_id")
                    .and_then(|v| v.as_str())
                    .unwrap_or_default();
                let public_key = message
                    .payload
                    .get("public_key")
                    .and_then(|v| v.as_str())
                    .unwrap_or_default();
                let p2p_addr = message
                    .payload
                    .get("p2p_addr")
                    .and_then(|v| v.as_str())
                    .unwrap_or_default();
                let seq = message
                    .payload
                    .get("seq")
                    .and_then(|v| v.as_i64())
                    .unwrap_or(0);
                if !is_valid_username(username)
                    || peer_id.is_empty()
                    || public_key.is_empty()
                    || p2p_addr.is_empty()
                {
                    continue;
                }
                let _ = upsert_username_claim(
                    &state.db_path,
                    username,
                    peer_id,
                    public_key,
                    p2p_addr,
                    seq,
                );
            }
            _ => {}
        }
    }
}

fn is_valid_username(value: &str) -> bool {
    if value.len() < 3 || value.len() > 32 {
        return false;
    }
    value
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || ch == '_')
}

use crate::client::ButterflyBot;
use crate::candle_vllm::DEFAULT_CANDLE_MODEL_ID;
use crate::config::{AgentConfig, CandleVllmConfig, Config, MemoryConfig, OpenAiConfig};
use crate::config_store;
use crate::e2e::identity_store::KeyringIdentityStore;
use crate::e2e::manager::E2eManager;
use crate::e2e::E2eEnvelope;
use crate::e2e::trust_store::{
    get_contact, get_peer_key, get_username_by_public_key, list_contacts, lookup_username,
    set_trust_state, upsert_contact, upsert_peer_key, upsert_username_claim, TrustState,
};
use crate::error::{ButterflyBotError, Result};
use crate::interfaces::scheduler::ScheduledJob;
use crate::reminders::ReminderStore;
use crate::scheduler::Scheduler;
use crate::services::agent::UiEvent;
use crate::services::gossip::{GossipHandle, GossipMessage};
use crate::services::query::{OutputFormat, ProcessOptions, ProcessResult, UserInput};
use tokio::sync::{broadcast, RwLock};

#[derive(Clone)]
pub struct AppState {
    pub agent: Arc<RwLock<Option<Arc<ButterflyBot>>>>,
    pub reminder_store: Arc<ReminderStore>,
    pub token: String,
    pub ui_event_tx: broadcast::Sender<UiEvent>,
    pub e2e: Arc<E2eManager>,
    pub db_path: String,
    pub gossip: Arc<GossipHandle>,
}

struct BrainTickJob {
    agent: Arc<RwLock<Option<Arc<ButterflyBot>>>>,
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
        if let Some(agent) = self.agent.read().await.clone() {
            agent.brain_tick().await;
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

async fn require_agent(
    state: &AppState,
) -> std::result::Result<Arc<ButterflyBot>, (StatusCode, Json<ErrorResponse>)> {
    if let Some(agent) = state.agent.read().await.clone() {
        Ok(agent)
    } else {
        Err((
            StatusCode::SERVICE_UNAVAILABLE,
            Json(ErrorResponse {
                error: "Agent warming up".to_string(),
            }),
        ))
    }
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
    peer_id: String,
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

#[derive(Deserialize)]
struct E2eTrustQuery {
    user_id: String,
    peer_id: String,
}

#[derive(Deserialize)]
struct ContactUpsertRequest {
    user_id: String,
    peer_id: String,
    label: String,
    onion_address: String,
}

#[derive(Serialize)]
struct ContactItem {
    peer_id: String,
    label: String,
    onion_address: String,
    trust_state: String,
    public_key: Option<String>,
}

#[derive(Serialize)]
struct ContactsResponse {
    contacts: Vec<ContactItem>,
}

#[derive(Deserialize)]
struct P2pMessageRequest {
    user_id: String,
    peer_id: String,
    message_id: u64,
    envelope: ApiE2eEnvelope,
}

#[derive(Serialize)]
struct P2pMessageResponse {
    status: String,
}

#[derive(Deserialize)]
struct P2pReceiptRequest {
    user_id: String,
    peer_id: String,
    message_id: u64,
    status: String,
}

#[derive(Serialize)]
struct P2pInfoResponse {
    peer_id: String,
    listen_addrs: Vec<String>,
}

#[derive(Deserialize)]
struct UsernameClaimRequest {
    user_id: String,
    username: String,
}

#[derive(Serialize)]
struct UsernameLookupResponse {
    username: String,
    peer_id: String,
    public_key: String,
    p2p_addr: String,
}

#[derive(Serialize)]
struct UsernameMeResponse {
    username: String,
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
        .route("/e2e/trust", post(e2e_set_trust))
        .route("/e2e/trust_status", get(e2e_trust_status))
        .route("/contacts", get(contacts_list))
        .route("/contacts", post(contacts_upsert))
        .route("/username/claim", post(username_claim))
        .route("/username/lookup", get(username_lookup))
        .route("/username/me", get(username_me))
        .route("/p2p/info", get(p2p_info))
        .route("/p2p/message", post(p2p_message))
        .route("/p2p/receipt", post(p2p_receipt))
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

    let agent = match require_agent(&state).await {
        Ok(agent) => agent,
        Err(err) => return err.into_response(),
    };
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

    let ProcessTextRequest {
        user_id,
        text,
        prompt,
    } = payload;
    let agent = match require_agent(&state).await {
        Ok(agent) => agent,
        Err(err) => return err.into_response(),
    };

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
    let agent = match require_agent(&state).await {
        Ok(agent) => agent,
        Err(err) => return err.into_response(),
    };
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

    let db_path = &state.db_path;
    if let Ok(Some(record)) = get_peer_key(db_path, &payload.user_id, &payload.peer_id) {
        if record.trust_state == TrustState::Blocked {
            return (
                StatusCode::FORBIDDEN,
                Json(ErrorResponse {
                    error: "peer is blocked".to_string(),
                }),
            )
                .into_response();
        }
    }

    let _ = upsert_peer_key(
        db_path,
        &payload.user_id,
        &payload.peer_id,
        &payload.peer_public_key,
        TrustState::Unverified,
    );

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

    let db_path = &state.db_path;
    let sender_public = BASE64.encode(envelope.sender_public_key);
    let _ = upsert_peer_key(
        db_path,
        &payload.user_id,
        &sender_public,
        &sender_public,
        TrustState::Unverified,
    );

    if let Ok(Some(record)) = get_peer_key(db_path, &payload.user_id, &sender_public) {
        if record.trust_state == TrustState::Blocked {
            return (
                StatusCode::FORBIDDEN,
                Json(ErrorResponse {
                    error: "peer is blocked".to_string(),
                }),
            )
                .into_response();
        }
    }

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

async fn e2e_set_trust(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(payload): Json<E2eTrustRequest>,
) -> impl IntoResponse {
    if let Err(err) = authorize(&headers, &state.token) {
        return err.into_response();
    }
    let db_path = &state.db_path;
    let state = match payload.trust_state.as_str() {
        "verified" => TrustState::Verified,
        "blocked" => TrustState::Blocked,
        _ => TrustState::Unverified,
    };
    if let Err(err) = set_trust_state(db_path, &payload.user_id, &payload.peer_id, state) {
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ErrorResponse {
                error: err.to_string(),
            }),
        )
            .into_response();
    }
    (
        StatusCode::OK,
        Json(E2eTrustResponse {
            trust_state: payload.trust_state,
        }),
    )
        .into_response()
}

async fn e2e_trust_status(
    State(state): State<AppState>,
    headers: HeaderMap,
    axum::extract::Query(query): axum::extract::Query<E2eTrustQuery>,
) -> impl IntoResponse {
    if let Err(err) = authorize(&headers, &state.token) {
        return err.into_response();
    }
    let db_path = &state.db_path;
    match get_peer_key(db_path, &query.user_id, &query.peer_id) {
        Ok(Some(record)) => (
            StatusCode::OK,
            Json(E2eTrustResponse {
                trust_state: record.trust_state.as_str().to_string(),
            }),
        )
            .into_response(),
        Ok(None) => (
            StatusCode::NOT_FOUND,
            Json(ErrorResponse {
                error: "peer not found".to_string(),
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

async fn contacts_list(
    State(state): State<AppState>,
    headers: HeaderMap,
    axum::extract::Query(query): axum::extract::Query<E2eIdentityQuery>,
) -> impl IntoResponse {
    if let Err(err) = authorize(&headers, &state.token) {
        return err.into_response();
    }
    let db_path = &state.db_path;
    match list_contacts(db_path, &query.user_id) {
        Ok(contacts) => (
            StatusCode::OK,
            Json(ContactsResponse {
                contacts: contacts
                    .into_iter()
                    .map(|contact| ContactItem {
                        peer_id: contact.peer_id,
                        label: contact.label,
                        onion_address: contact.onion_address,
                        trust_state: contact.trust_state.as_str().to_string(),
                        public_key: contact.public_key,
                    })
                    .collect(),
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

async fn contacts_upsert(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(payload): Json<ContactUpsertRequest>,
) -> impl IntoResponse {
    if let Err(err) = authorize(&headers, &state.token) {
        return err.into_response();
    }
    let db_path = &state.db_path;
    if let Err(err) = upsert_contact(
        db_path,
        &payload.user_id,
        &payload.peer_id,
        &payload.label,
        &payload.onion_address,
    ) {
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ErrorResponse {
                error: err.to_string(),
            }),
        )
            .into_response();
    }
    StatusCode::OK.into_response()
}

async fn p2p_info(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> impl IntoResponse {
    if let Err(err) = authorize(&headers, &state.token) {
        return err.into_response();
    }
    let listen_addrs = state
        .gossip
        .listen_addrs()
        .await
        .into_iter()
        .map(|addr| addr.to_string())
        .collect::<Vec<_>>();
    (
        StatusCode::OK,
        Json(P2pInfoResponse {
            peer_id: state.gossip.peer_id.to_string(),
            listen_addrs,
        }),
    )
        .into_response()
}

async fn username_claim(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(payload): Json<UsernameClaimRequest>,
) -> impl IntoResponse {
    if let Err(err) = authorize(&headers, &state.token) {
        return err.into_response();
    }
    let username = payload.username.to_lowercase();
    if !is_valid_username(&username) {
        return (
            StatusCode::BAD_REQUEST,
            Json(ErrorResponse {
                error: "invalid username".to_string(),
            }),
        )
            .into_response();
    }

    let public_key = match state.e2e.identity_public_key(&payload.user_id) {
        Ok(key) => BASE64.encode(key),
        Err(err) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ErrorResponse {
                    error: err.to_string(),
                }),
            )
                .into_response();
        }
    };
    let seq = now_ts();
    let peer_id = state.gossip.peer_id.to_string();
    let p2p_addr = state
        .gossip
        .listen_addrs()
        .await
        .into_iter()
        .map(|addr| addr.to_string())
        .next()
        .unwrap_or_default();
    if p2p_addr.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(ErrorResponse {
                error: "no listen address available".to_string(),
            }),
        )
            .into_response();
    }

    let _ = upsert_username_claim(
        &state.db_path,
        &username,
        &peer_id,
        &public_key,
        &p2p_addr,
        seq,
    );

    let dht_payload = serde_json::json!({
        "username": username,
        "peer_id": peer_id,
        "public_key": public_key,
        "p2p_addr": p2p_addr,
        "seq": seq,
    });
    let _ = state
        .gossip
        .dht_put(format!("username:{}", payload.username.to_lowercase()), serde_json::to_vec(&dht_payload).unwrap_or_default())
        .await;

    let message = GossipMessage {
        kind: "username_claim".to_string(),
        to: "*".to_string(),
        from: payload.user_id,
        message_id: seq as u64,
        payload: serde_json::json!({
            "username": username,
            "peer_id": peer_id,
            "public_key": public_key,
            "p2p_addr": p2p_addr,
            "seq": seq,
        }),
        signature: String::new(),
        public_key: String::new(),
    };
    if let Err(err) = state.gossip.publish(message).await {
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ErrorResponse {
                error: err.to_string(),
            }),
        )
            .into_response();
    }

    StatusCode::OK.into_response()
}

async fn username_lookup(
    State(state): State<AppState>,
    headers: HeaderMap,
    axum::extract::Query(query): axum::extract::Query<std::collections::HashMap<String, String>>,
) -> impl IntoResponse {
    if let Err(err) = authorize(&headers, &state.token) {
        return err.into_response();
    }
    let Some(username) = query.get("username") else {
        return (
            StatusCode::BAD_REQUEST,
            Json(ErrorResponse {
                error: "username required".to_string(),
            }),
        )
            .into_response();
    };
    let username = username.to_lowercase();
    match lookup_username(&state.db_path, &username) {
        Ok(Some(record)) => (
            StatusCode::OK,
            Json(UsernameLookupResponse {
                username: record.username,
                peer_id: record.peer_id,
                public_key: record.public_key,
                p2p_addr: record.p2p_addr,
            }),
        )
            .into_response(),
        Ok(None) => {
            let dht_key = format!("username:{}", username);
            if let Ok(Some(value)) = state.gossip.dht_get(dht_key).await {
                if let Ok(claim) = serde_json::from_slice::<serde_json::Value>(&value) {
                    let peer_id = claim
                        .get("peer_id")
                        .and_then(|v| v.as_str())
                        .unwrap_or_default();
                    let public_key = claim
                        .get("public_key")
                        .and_then(|v| v.as_str())
                        .unwrap_or_default();
                    let p2p_addr = claim
                        .get("p2p_addr")
                        .and_then(|v| v.as_str())
                        .unwrap_or_default();
                    let seq = claim
                        .get("seq")
                        .and_then(|v| v.as_i64())
                        .unwrap_or(0);
                    if is_valid_username(&username)
                        && !peer_id.is_empty()
                        && !public_key.is_empty()
                        && !p2p_addr.is_empty()
                    {
                        let _ = upsert_username_claim(
                            &state.db_path,
                            &username,
                            peer_id,
                            public_key,
                            p2p_addr,
                            seq,
                        );
                        return (
                            StatusCode::OK,
                            Json(UsernameLookupResponse {
                                username,
                                peer_id: peer_id.to_string(),
                                public_key: public_key.to_string(),
                                p2p_addr: p2p_addr.to_string(),
                            }),
                        )
                            .into_response();
                    }
                }
            }
            (
                StatusCode::NOT_FOUND,
                Json(ErrorResponse {
                    error: "username not found".to_string(),
                }),
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

async fn username_me(
    State(state): State<AppState>,
    headers: HeaderMap,
    axum::extract::Query(query): axum::extract::Query<E2eIdentityQuery>,
) -> impl IntoResponse {
    if let Err(err) = authorize(&headers, &state.token) {
        return err.into_response();
    }
    let public_key = match state.e2e.identity_public_key(&query.user_id) {
        Ok(key) => BASE64.encode(key),
        Err(err) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ErrorResponse {
                    error: err.to_string(),
                }),
            )
                .into_response();
        }
    };
    match get_username_by_public_key(&state.db_path, &public_key) {
        Ok(Some(record)) => (
            StatusCode::OK,
            Json(UsernameMeResponse {
                username: record.username,
            }),
        )
            .into_response(),
        Ok(None) => (
            StatusCode::NOT_FOUND,
            Json(ErrorResponse {
                error: "username not found".to_string(),
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

async fn p2p_message(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(payload): Json<P2pMessageRequest>,
) -> impl IntoResponse {
    if let Err(err) = authorize(&headers, &state.token) {
        return err.into_response();
    }

    if let Ok(Some(contact)) = get_contact(&state.db_path, &payload.user_id, &payload.peer_id) {
        if let Ok(addr) = contact.onion_address.parse() {
            let _ = state.gossip.dial(addr).await;
        }
    }

    let message = GossipMessage {
        kind: "message".to_string(),
        to: payload.peer_id,
        from: payload.user_id,
        message_id: payload.message_id,
        payload: serde_json::json!({ "envelope": payload.envelope }),
        signature: String::new(),
        public_key: String::new(),
    };
    if let Err(err) = state.gossip.publish(message).await {
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ErrorResponse {
                error: err.to_string(),
            }),
        )
            .into_response();
    }

    (
        StatusCode::OK,
        Json(P2pMessageResponse {
            status: "ok".to_string(),
        }),
    )
        .into_response()
}

async fn p2p_receipt(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(payload): Json<P2pReceiptRequest>,
) -> impl IntoResponse {
    if let Err(err) = authorize(&headers, &state.token) {
        return err.into_response();
    }

    if let Ok(Some(contact)) = get_contact(&state.db_path, &payload.user_id, &payload.peer_id) {
        if let Ok(addr) = contact.onion_address.parse() {
            let _ = state.gossip.dial(addr).await;
        }
    }

    let message = GossipMessage {
        kind: "receipt".to_string(),
        to: payload.peer_id,
        from: payload.user_id,
        message_id: payload.message_id,
        payload: serde_json::json!({ "status": payload.status }),
        signature: String::new(),
        public_key: String::new(),
    };
    if let Err(err) = state.gossip.publish(message).await {
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ErrorResponse {
                error: err.to_string(),
            }),
        )
            .into_response();
    }

    StatusCode::OK.into_response()
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
    let candle = CandleVllmConfig {
        enabled: Some(true),
        model_id: Some(DEFAULT_CANDLE_MODEL_ID.to_string()),
        weight_path: None,
        gguf_file: None,
        hf_token: None,
        device_ids: None,
        kvcache_mem_gpu: None,
        kvcache_mem_cpu: None,
        temperature: None,
        top_p: None,
        dtype: None,
        isq: None,
        binary_path: None,
        host: None,
        port: None,
        extra_args: None,
    };
    let model = DEFAULT_CANDLE_MODEL_ID.to_string();
    let memory = Some(MemoryConfig {
        enabled: Some(true),
        sqlite_path: Some(db_path.to_string()),
        lancedb_path: None,
        summary_model: None,
        embedding_model: None,
        rerank_model: None,
        summary_threshold: None,
        retention_days: None,
    });

    Config {
        openai: Some(OpenAiConfig {
            api_key: None,
            model: Some(model),
            base_url: None,
        }),
        candle_vllm: Some(candle),
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

    let mut config = Config::from_store(db_path).ok();
    if let Some(cfg) = config.as_mut() {
        let mut changed = false;
        if cfg.candle_vllm.is_none()
            && cfg
                .openai
                .as_ref()
                .and_then(|openai| openai.base_url.as_deref())
                .map(|url| {
                    url.starts_with("http://localhost:11434")
                        || url.starts_with("http://127.0.0.1:11434")
                })
                .unwrap_or(false)
        {
            cfg.candle_vllm = Some(CandleVllmConfig {
                enabled: Some(true),
                model_id: Some(DEFAULT_CANDLE_MODEL_ID.to_string()),
                weight_path: None,
                gguf_file: None,
                hf_token: None,
                device_ids: None,
                kvcache_mem_gpu: None,
                kvcache_mem_cpu: None,
                temperature: None,
                top_p: None,
                dtype: None,
                isq: None,
                binary_path: None,
                host: None,
                port: None,
                extra_args: None,
            });
            if let Some(openai) = cfg.openai.as_mut() {
                openai.base_url = None;
                openai.model = Some(DEFAULT_CANDLE_MODEL_ID.to_string());
            }
            changed = true;
        }
        if changed {
            config_store::save_config(db_path, cfg)?;
        }
    }

    let tick_seconds = config
        .as_ref()
        .and_then(|cfg| cfg.brains.as_ref())
        .and_then(|brains| brains.get("settings"))
        .and_then(|settings| settings.get("tick_seconds"))
        .and_then(|value| value.as_u64())
        .unwrap_or(60);

    let (ui_event_tx, _) = broadcast::channel(256);
    let agent = Arc::new(RwLock::new(None));
    let reminder_store = Arc::new(ReminderStore::new(db_path).await?);
    let identity_store = Arc::new(KeyringIdentityStore::new("butterfly-bot.identity"));
    let e2e = Arc::new(E2eManager::new(identity_store));
    let gossip_listen = std::env::var("BUTTERFLY_BOT_GOSSIP_LISTEN")
        .unwrap_or_else(|_| "/ip4/0.0.0.0/tcp/0".to_string());
    let gossip_bootstrap = std::env::var("BUTTERFLY_BOT_GOSSIP_BOOTSTRAP").unwrap_or_default();
    let listen_addrs = gossip_listen
        .split(',')
        .filter_map(|value| value.trim().parse().ok())
        .collect::<Vec<_>>();
    let bootstrap_addrs = gossip_bootstrap
        .split(',')
        .filter_map(|value| value.trim().parse().ok())
        .collect::<Vec<_>>();
    let gossip = Arc::new(
        GossipHandle::start(listen_addrs, bootstrap_addrs, "butterfly-chat").await?,
    );
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
        db_path: db_path.to_string(),
        gossip: gossip.clone(),
    };
    let agent_state = state.agent.clone();
    let db_path = db_path.to_string();
    let ui_event_tx = state.ui_event_tx.clone();
    tokio::spawn(async move {
        if let Ok(agent) = ButterflyBot::from_store_with_events(&db_path, Some(ui_event_tx)).await {
            let mut guard = agent_state.write().await;
            *guard = Some(Arc::new(agent));
        }
    });
    let gossip_rx = gossip.subscribe();
    let gossip_state = state.clone();
    tokio::spawn(async move {
        handle_gossip_events(gossip_state, gossip_rx).await;
    });
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
