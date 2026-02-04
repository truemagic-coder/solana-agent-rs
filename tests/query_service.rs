mod common;

use std::sync::Arc;

use futures::StreamExt;
use httpmock::Method::POST;
use httpmock::MockServer;
use serde_json::json;

use butterfly_bot::brain::manager::BrainManager;
use butterfly_bot::client::ButterflyBot;
use butterfly_bot::config::{Config, OpenAiConfig};
use butterfly_bot::domains::agent::AIAgent;
use butterfly_bot::error::ButterflyBotError;
use butterfly_bot::interfaces::providers::{ImageData, ImageInput};
use butterfly_bot::providers::memory::InMemoryMemoryProvider;
use butterfly_bot::services::agent::AgentService;
use butterfly_bot::services::query::{
    OutputFormat, ProcessOptions, ProcessResult, QueryService, UserInput,
};

use common::{DummyTool, FlakyNameTool, QueueLlmProvider};

#[tokio::test]
async fn query_service_and_client() {
    let llm = Arc::new(QueueLlmProvider::new(vec![]));
    let brain = Arc::new(BrainManager::new(json!({})));
    let agent = AIAgent {
        name: "agent".to_string(),
        instructions: "inst".to_string(),
        specialization: "spec".to_string(),
    };
    let service = Arc::new(AgentService::new(llm.clone(), agent, None, brain, None));
    let memory = Arc::new(InMemoryMemoryProvider::new());

    let query = QueryService::new(service.clone(), Some(memory), None);

    let text = query.process_text("user", "hello", None).await.unwrap();
    assert_eq!(text, "mock text");

    let stream = query.process_text_stream("user", "hello", None);
    let collected: Vec<_> = stream.collect::<Vec<_>>().await;
    assert_eq!(collected.len(), 1);

    let options = ProcessOptions {
        prompt: Some("extra".to_string()),
        images: vec![],
        output_format: OutputFormat::Text,
        image_detail: "auto".to_string(),
        json_schema: Some(json!({"type":"object"})),
    };
    let result = query
        .process(
            "user",
            UserInput::Audio {
                bytes: vec![1, 2, 3],
                input_format: "wav".to_string(),
            },
            options,
        )
        .await
        .unwrap();
    match result {
        ProcessResult::Structured(value) => assert_eq!(value, json!({"ok": true})),
        other => panic!("unexpected result: {other:?}"),
    }

    let options = ProcessOptions {
        prompt: None,
        images: vec![ImageInput {
            data: ImageData::Bytes(vec![1, 2, 3]),
        }],
        output_format: OutputFormat::Text,
        image_detail: "low".to_string(),
        json_schema: None,
    };
    let result = query
        .process("user", UserInput::Text("img".to_string()), options)
        .await
        .unwrap();
    match result {
        ProcessResult::Text(value) => assert_eq!(value, "image response"),
        other => panic!("unexpected result: {other:?}"),
    }

    let options = ProcessOptions {
        prompt: None,
        images: vec![],
        output_format: OutputFormat::Audio {
            voice: "alloy".to_string(),
            format: "mp3".to_string(),
        },
        image_detail: "auto".to_string(),
        json_schema: None,
    };
    let result = query
        .process("user", UserInput::Text("hi".to_string()), options)
        .await
        .unwrap();
    match result {
        ProcessResult::Audio(value) => assert_eq!(value, b"audio".to_vec()),
        other => panic!("unexpected result: {other:?}"),
    }

    query.delete_user_history("user").await.unwrap();
    let history = query.get_user_history("user", 10).await.unwrap();
    assert_eq!(history.len(), 0);

    let llm = Arc::new(QueueLlmProvider::new(vec![]));
    let brain = Arc::new(BrainManager::new(json!({})));
    let agent = AIAgent {
        name: "agent".to_string(),
        instructions: "inst".to_string(),
        specialization: "spec".to_string(),
    };
    let service = Arc::new(AgentService::new(llm, agent, None, brain, None));
    let query = QueryService::new(service, None, None);
    assert_eq!(query.get_user_history("user", 1).await.unwrap().len(), 0);
    query.delete_user_history("user").await.unwrap();

    let text = query
        .process_text("user", "hello", Some("prompt"))
        .await
        .unwrap();
    assert_eq!(text, "mock text");

    let options = ProcessOptions {
        prompt: None,
        images: Vec::new(),
        output_format: OutputFormat::Text,
        image_detail: "auto".to_string(),
        json_schema: None,
    };
    let result = query
        .process("user", UserInput::Text("hello".to_string()), options)
        .await
        .unwrap();
    match result {
        ProcessResult::Text(value) => assert_eq!(value, "mock text"),
        other => panic!("unexpected result: {other:?}"),
    }

    let config = Config {
        openai: Some(OpenAiConfig {
            api_key: Some("key".to_string()),
            model: None,
            base_url: None,
        }),
        skill_file: None,
        heartbeat_file: None,
        memory: None,
        tools: None,
        brains: None,
    };
    let agent = ButterflyBot::from_config(config).await.unwrap();
    let tool = Arc::new(DummyTool::new("tool"));
    let registered = agent.register_tool(tool.clone()).await.unwrap();
    assert!(registered);

    let registered = agent.register_tool(tool.clone()).await.unwrap();
    assert!(!registered);

    let flaky = Arc::new(FlakyNameTool::new());
    let err = agent.register_tool(flaky).await.unwrap_err();
    assert!(matches!(err, ButterflyBotError::Runtime(_)));

    let server = MockServer::start_async().await;
    let chat_mock = server
        .mock_async(|when, then| {
            when.method(POST).path("/chat/completions");
            then.status(200).json_body(json!({
                "id": "chatcmpl-path",
                "object": "chat.completion",
                "created": 1,
                "model": "gpt-4o-mini",
                "choices": [{
                    "index": 0,
                    "message": {"role": "assistant", "content": "mock text"},
                    "finish_reason": "stop"
                }]
            }));
        })
        .await;

    let tmp = tempfile::NamedTempFile::new().unwrap();
    std::fs::write(
        tmp.path(),
        json!({
            "openai": {"api_key":"key","model":"gpt-4o-mini","base_url": server.base_url()},
            "skill_file": null,
            "heartbeat_file": null
        })
        .to_string(),
    )
    .unwrap();
    let agent = ButterflyBot::from_config_path(tmp.path()).await.unwrap();
    let mut stream = agent.process_text_stream("user", "hello", None);
    let chunk = stream.next().await.unwrap().unwrap();
    assert_eq!(chunk, "mock text");
    chat_mock.assert_hits(1);

    agent.delete_user_history("user").await.unwrap();
    let _ = agent.get_user_history("user", 5).await.unwrap();
}
