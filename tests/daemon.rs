use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use httpmock::Method::POST;
use httpmock::MockServer;
use serde_json::json;
use tempfile::NamedTempFile;
use tokio::sync::broadcast;
use tower::ServiceExt;

use butterfly_bot::client::ButterflyBot;
use butterfly_bot::config::{AgentConfig, Config, OpenAiConfig};
use butterfly_bot::daemon::{build_router, AppState};
use butterfly_bot::e2e::identity_store::MemoryIdentityStore;
use butterfly_bot::e2e::manager::E2eManager;
use butterfly_bot::reminders::ReminderStore;
use butterfly_bot::services::gossip::GossipHandle;

async fn make_agent(server: &MockServer) -> ButterflyBot {
    let config = Config {
        openai: Some(OpenAiConfig {
            api_key: Some("key".to_string()),
            model: Some("gpt-4o-mini".to_string()),
            base_url: Some(server.base_url()),
        }),
        agents: vec![AgentConfig {
            name: "agent".to_string(),
            instructions: "inst".to_string(),
            specialization: "spec".to_string(),
            description: None,
            tools: None,
            capture_name: None,
            capture_schema: None,
        }],
        business: None,
        memory: None,
        guardrails: None,
        tools: None,
        brains: None,
    };

    ButterflyBot::from_config(config).await.unwrap()
}

#[tokio::test]
async fn daemon_health_and_auth() {
    let server = MockServer::start_async().await;
    let agent = make_agent(&server).await;
    let reminder_db = NamedTempFile::new().unwrap();
    let reminder_store = ReminderStore::new(reminder_db.path().to_str().unwrap())
        .await
        .unwrap();
    let (ui_event_tx, _) = broadcast::channel(16);
    let identity_store = Arc::new(MemoryIdentityStore::new());
    let e2e = Arc::new(E2eManager::new(identity_store));
    let gossip = Arc::new(
        GossipHandle::start(
            vec!["/ip4/127.0.0.1/tcp/0".parse().unwrap()],
            Vec::new(),
            "butterfly-chat",
        )
        .await
        .unwrap(),
    );
    let state = AppState {
        agent: Arc::new(agent),
        reminder_store: Arc::new(reminder_store),
        token: "token".to_string(),
        ui_event_tx,
        e2e,
        db_path: reminder_db.path().to_str().unwrap().to_string(),
        gossip,
    };
    let app = build_router(state);

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/health")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/process_text")
                .header("content-type", "application/json")
                .body(Body::from(json!({"user_id":"u","text":"hi"}).to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn daemon_process_text_and_memory_search() {
    let server = MockServer::start_async().await;
    let chat_mock = server
        .mock_async(|when, then| {
            when.method(POST).path("/chat/completions");
            then.status(200).json_body(json!({
                "id": "chatcmpl-test",
                "object": "chat.completion",
                "created": 1,
                "model": "gpt-4o-mini",
                "choices": [{
                    "index": 0,
                    "message": {"role": "assistant", "content": "hello"},
                    "finish_reason": "stop"
                }]
            }));
        })
        .await;

    let agent = make_agent(&server).await;
    let reminder_db = NamedTempFile::new().unwrap();
    let reminder_store = ReminderStore::new(reminder_db.path().to_str().unwrap())
        .await
        .unwrap();
    let (ui_event_tx, _) = broadcast::channel(16);
    let identity_store = Arc::new(MemoryIdentityStore::new());
    let e2e = Arc::new(E2eManager::new(identity_store));
    let gossip = Arc::new(
        GossipHandle::start(
            vec!["/ip4/127.0.0.1/tcp/0".parse().unwrap()],
            Vec::new(),
            "butterfly-chat",
        )
        .await
        .unwrap(),
    );
    let state = AppState {
        agent: Arc::new(agent),
        reminder_store: Arc::new(reminder_store),
        token: "token".to_string(),
        ui_event_tx,
        e2e,
        db_path: reminder_db.path().to_str().unwrap().to_string(),
        gossip,
    };
    let app = build_router(state);

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/process_text")
                .header("authorization", "Bearer token")
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({"user_id":"u","text":"hello"}).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    let value: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(value.get("text").and_then(|v| v.as_str()), Some("hello"));
    chat_mock.assert_hits(1);

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/memory_search")
                .header("authorization", "Bearer token")
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({"user_id":"u","query":"hello","limit":2}).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    let value: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert!(value.get("results").and_then(|v| v.as_array()).is_some());
}
