mod common;

use std::sync::Arc;

use serde_json::json;

use butterfly_bot::brain::manager::BrainManager;
use butterfly_bot::domains::agent::AIAgent;
use butterfly_bot::interfaces::brain::{BrainContext, BrainEvent, BrainPlugin};
use butterfly_bot::interfaces::providers::{ImageData, ImageInput, LlmResponse, ToolCall};
use butterfly_bot::services::agent::AgentService;

use common::{DummyTool, QueueLlmProvider};
use std::sync::Mutex;

#[tokio::test]
async fn routing_and_agent_service() {
    let brain_manager = Arc::new(BrainManager::new(json!({})));
    let llm = Arc::new(QueueLlmProvider::new(vec![
        LlmResponse {
            text: "tool response".to_string(),
            tool_calls: vec![
                ToolCall {
                    name: "tool1".to_string(),
                    arguments: json!({"value": 1}),
                },
                ToolCall {
                    name: "missing".to_string(),
                    arguments: json!({}),
                },
            ],
        },
        LlmResponse {
            text: "done".to_string(),
            tool_calls: Vec::new(),
        },
    ]));

    let agent = AIAgent {
        name: "agent1".to_string(),
        instructions: "inst".to_string(),
        specialization: "spec".to_string(),
    };

    let service = AgentService::new(llm, agent, None, brain_manager, None);

    let system = service.get_agent_system_prompt().await.unwrap();
    assert!(system.contains("inst"));

    let registry = service.tool_registry.clone();
    let tool = Arc::new(DummyTool::new("tool1"));
    assert!(registry.register_tool(tool).await);
    assert!(registry
        .assign_tool_to_agent(service.agent_name(), "tool1")
        .await);

    let response = service
        .generate_response("u1", "query", "history", Some("prompt"))
        .await
        .unwrap();
    assert_eq!(response, "done");

    let response = service
        .generate_response_with_images(
            "u1",
            "query",
            vec![ImageInput {
                data: ImageData::Url("http://example.com".to_string()),
            }],
            "",
            None,
            "auto",
        )
        .await
        .unwrap();
    assert_eq!(response, "image response");

    let response = service
        .generate_response_with_images(
            "u1",
            "query",
            vec![ImageInput {
                data: ImageData::Url("http://example.com".to_string()),
            }],
            "",
            Some("extra"),
            "auto",
        )
        .await
        .unwrap();
    assert_eq!(response, "image response");

    let structured = service
        .generate_structured_response("u1", "query", "", None, json!({"type":"object"}))
        .await
        .unwrap();
    assert_eq!(structured, json!({"ok": true}));

    let transcript = service
        .transcribe_audio(vec![1, 2, 3], "wav")
        .await
        .unwrap();
    assert_eq!(transcript, "transcribed");

    let audio = service
        .synthesize_audio("hi", "alloy", "mp3")
        .await
        .unwrap();
    assert_eq!(audio, b"audio".to_vec());

    let mut responses = Vec::new();
    for idx in 0..5 {
        responses.push(LlmResponse {
            text: format!("step {idx}"),
            tool_calls: vec![ToolCall {
                name: "tool1".to_string(),
                arguments: json!({"value": idx}),
            }],
        });
    }

    let looping_llm = Arc::new(QueueLlmProvider::new(responses));
    let looping_brain = Arc::new(BrainManager::new(json!({})));
    let looping_agent = AIAgent {
        name: "agent-loop".to_string(),
        instructions: "inst".to_string(),
        specialization: "spec".to_string(),
    };
    let looping_service = AgentService::new(looping_llm, looping_agent, None, looping_brain, None);
    let registry = looping_service.tool_registry.clone();
    let tool = Arc::new(DummyTool::new("tool1"));
    assert!(registry.register_tool(tool).await);
    assert!(registry
        .assign_tool_to_agent(looping_service.agent_name(), "tool1")
        .await);

    let response = looping_service
        .generate_response("u1", "query", "", None)
        .await
        .unwrap();
    assert_eq!(response, "step 4");
}

struct RecordingBrain {
    name: String,
    events: Arc<Mutex<Vec<String>>>,
}

#[async_trait::async_trait]
impl BrainPlugin for RecordingBrain {
    fn name(&self) -> &str {
        &self.name
    }

    fn description(&self) -> &str {
        "recording"
    }

    async fn on_event(&self, event: BrainEvent, _ctx: &BrainContext) -> butterfly_bot::Result<()> {
        let label = match event {
            BrainEvent::Start => "start",
            BrainEvent::Tick => "tick",
            BrainEvent::UserMessage { .. } => "user",
            BrainEvent::AssistantResponse { .. } => "assistant",
        };
        let mut guard = self.events.lock().unwrap();
        guard.push(label.to_string());
        Ok(())
    }
}

#[tokio::test]
async fn agent_service_dispatches_brain_events() {
    let events = Arc::new(Mutex::new(Vec::new()));
    let mut brain = BrainManager::new(json!({"brains": ["record"]}));
    let events_factory = events.clone();
    brain.register_factory("record", move |_| {
        Arc::new(RecordingBrain {
            name: "record".to_string(),
            events: events_factory.clone(),
        })
    });
    brain.load_plugins();
    let brain = Arc::new(brain);

    let llm = Arc::new(QueueLlmProvider::new(vec![]));
    let agent = AIAgent {
        name: "agent".to_string(),
        instructions: "inst".to_string(),
        specialization: "spec".to_string(),
    };
    let service = AgentService::new(llm, agent, None, brain, None);

    let response = service.generate_response("u1", "hello", "", None).await.unwrap();
    assert_eq!(response, "");

    let guard = events.lock().unwrap();
    assert_eq!(guard.as_slice(), ["start", "user", "assistant"]);
}

#[tokio::test]
async fn agent_service_brain_tick_dispatches() {
    let events = Arc::new(Mutex::new(Vec::new()));
    let mut brain = BrainManager::new(json!({"brains": ["record"]}));
    let events_factory = events.clone();
    brain.register_factory("record", move |_| {
        Arc::new(RecordingBrain {
            name: "record".to_string(),
            events: events_factory.clone(),
        })
    });
    brain.load_plugins();
    let brain = Arc::new(brain);

    let llm = Arc::new(QueueLlmProvider::new(vec![]));
    let agent = AIAgent {
        name: "agent".to_string(),
        instructions: "inst".to_string(),
        specialization: "spec".to_string(),
    };
    let service = AgentService::new(llm, agent, None, brain, None);

    service.dispatch_brain_tick().await;

    let guard = events.lock().unwrap();
    assert_eq!(guard.as_slice(), ["tick"]);
}
