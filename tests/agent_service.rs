mod common;

use std::sync::Arc;

use serde_json::json;

use butterfly_bot::brain::manager::BrainManager;
use butterfly_bot::domains::agent::BusinessMission;
use butterfly_bot::error::ButterflyBotError;
use butterfly_bot::guardrails::pii::{NoopGuardrail, PiiGuardrail};
use butterfly_bot::interfaces::brain::{BrainContext, BrainEvent, BrainPlugin};
use butterfly_bot::interfaces::providers::{ImageData, ImageInput, LlmResponse, ToolCall};
use butterfly_bot::services::agent::AgentService;
use butterfly_bot::services::routing::RoutingService;

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

    let mission = BusinessMission {
        mission: Some("m".to_string()),
        voice: Some("v".to_string()),
        values: vec![("name".to_string(), "desc".to_string())],
        goals: vec!["g1".to_string(), "g2".to_string()],
    };

    let mut service = AgentService::new(
        llm,
        Some(mission),
        vec![Arc::new(NoopGuardrail)],
        brain_manager,
        None,
    );
    service.register_ai_agent(
        "agent1".to_string(),
        "inst".to_string(),
        "spec".to_string(),
        Some("cap".to_string()),
        Some(json!({"type":"object"})),
    );

    let err = service.get_agent_system_prompt("missing").unwrap_err();
    assert!(matches!(err, ButterflyBotError::Runtime(_)));

    let system = service.get_agent_system_prompt("agent1").unwrap();
    assert!(system.contains("BUSINESS MISSION"));
    assert!(system.contains("BUSINESS VALUES"));
    assert!(system.contains("BUSINESS GOALS"));

    let registry = service.tool_registry.clone();
    let tool = Arc::new(DummyTool::new("tool1"));
    assert!(registry.register_tool(tool).await);
    assert!(registry.assign_tool_to_agent("agent1", "tool1").await);

    let response = service
        .generate_response("agent1", "u1", "query", "history", Some("prompt"))
        .await
        .unwrap();
    assert_eq!(response, "done");

    let mut provider = QueueLlmProvider::new(vec![]);
    provider.text = "email test@example.com".to_string();
    let pii_brain = Arc::new(BrainManager::new(json!({})));
    let mut pii_service = AgentService::new(
        Arc::new(provider),
        None,
        vec![Arc::new(PiiGuardrail::new(None))],
        pii_brain,
        None,
    );
    pii_service.register_ai_agent(
        "agent2".to_string(),
        "inst".to_string(),
        "spec".to_string(),
        None,
        None,
    );
    let response = pii_service
        .generate_response("agent2", "u1", "email test@example.com", "", None)
        .await
        .unwrap();
    assert!(response.contains("[REDACTED]"));

    let response = service
        .generate_response_with_images(
            "agent1",
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
            "agent1",
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
        .generate_structured_response("agent1", "u1", "query", "", None, json!({"type":"object"}))
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

    let routing = RoutingService::new(Arc::new(service));
    assert_eq!(routing.route_query("test").await.unwrap(), "agent1");
    assert_eq!(routing.route_query("yes").await.unwrap(), "agent1");

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
    let mut looping_service = AgentService::new(looping_llm, None, vec![], looping_brain, None);
    looping_service.register_ai_agent(
        "agent-loop".to_string(),
        "inst".to_string(),
        "spec".to_string(),
        None,
        None,
    );
    let registry = looping_service.tool_registry.clone();
    let tool = Arc::new(DummyTool::new("tool1"));
    assert!(registry.register_tool(tool).await);
    assert!(registry.assign_tool_to_agent("agent-loop", "tool1").await);

    let response = looping_service
        .generate_response("agent-loop", "u1", "query", "", None)
        .await
        .unwrap();
    assert_eq!(response, "step 4");

    let llm = Arc::new(QueueLlmProvider::new(vec![]));
    let routing_brain = Arc::new(BrainManager::new(json!({})));
    let mut service = AgentService::new(llm, None, vec![], routing_brain, None);
    service.register_ai_agent(
        "billing_agent".to_string(),
        "i".to_string(),
        "billing support".to_string(),
        None,
        None,
    );
    service.register_ai_agent(
        "sales_agent".to_string(),
        "i".to_string(),
        "sales".to_string(),
        None,
        None,
    );
    let routing = RoutingService::new(Arc::new(service));
    let picked = routing.route_query("sales_agent help").await.unwrap();
    assert_eq!(picked, "sales_agent");
    let picked = routing.route_query("yes").await.unwrap();
    assert_eq!(picked, "sales_agent");

    let empty_brain = Arc::new(BrainManager::new(json!({})));
    let empty_service = AgentService::new(
        Arc::new(QueueLlmProvider::new(vec![])),
        None,
        vec![],
        empty_brain,
        None,
    );
    let empty_routing = RoutingService::new(Arc::new(empty_service));
    assert_eq!(
        empty_routing.route_query("anything").await.unwrap(),
        "default"
    );
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
    let mut service = AgentService::new(llm, None, vec![], brain, None);
    service.register_ai_agent(
        "agent".to_string(),
        "inst".to_string(),
        "spec".to_string(),
        None,
        None,
    );

    let response = service
        .generate_response("agent", "u1", "hello", "", None)
        .await
        .unwrap();
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
    let mut service = AgentService::new(llm, None, vec![], brain, None);
    service.register_ai_agent(
        "agent".to_string(),
        "inst".to_string(),
        "spec".to_string(),
        None,
        None,
    );

    service.dispatch_brain_tick().await;

    let guard = events.lock().unwrap();
    assert_eq!(guard.as_slice(), ["tick"]);
}
