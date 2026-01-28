use std::collections::VecDeque;
use std::sync::Arc;

use async_trait::async_trait;
use futures::StreamExt;
use httpmock::Method::POST;
use httpmock::MockServer;
use serde_json::json;
use tokio::sync::Mutex;

use solana_agent::client::SolanaAgent;
use solana_agent::config::{
    AgentConfig, BusinessConfig, BusinessValue, Config, GuardrailConfig, GuardrailsConfig,
    OpenAiConfig,
};
use solana_agent::domains::agent::BusinessMission;
use solana_agent::error::{Result, SolanaAgentError};
use solana_agent::factories::agent_factory::SolanaAgentFactory;
use solana_agent::guardrails::pii::{NoopGuardrail, PiiGuardrail};
use solana_agent::interfaces::guardrails::InputGuardrail;
use solana_agent::interfaces::plugins::{Plugin, PluginManager, Tool};
use solana_agent::interfaces::providers::{
    ChatEvent, ImageData, ImageInput, LlmProvider, LlmResponse, MemoryProvider, ToolCall,
};
use solana_agent::interfaces::services::RoutingService as RoutingServiceTrait;
use solana_agent::plugins::manager::DefaultPluginManager;
use solana_agent::plugins::registry::ToolRegistry;
use solana_agent::providers::memory::InMemoryMemoryProvider;
use solana_agent::providers::openai::OpenAiProvider;
use solana_agent::services::agent::AgentService;
use solana_agent::services::query::{
    OutputFormat, ProcessOptions, ProcessResult, QueryService, UserInput,
};
use solana_agent::services::routing::RoutingService;

struct QueueLlmProvider {
    queue: Mutex<VecDeque<LlmResponse>>,
    text: String,
    structured: serde_json::Value,
    tts_bytes: Vec<u8>,
    transcript: String,
    image_text: String,
}

impl QueueLlmProvider {
    fn new(queue: Vec<LlmResponse>) -> Self {
        Self {
            queue: Mutex::new(VecDeque::from(queue)),
            text: "mock text".to_string(),
            structured: json!({"ok": true}),
            tts_bytes: b"audio".to_vec(),
            transcript: "transcribed".to_string(),
            image_text: "image response".to_string(),
        }
    }
}

#[async_trait]
impl LlmProvider for QueueLlmProvider {
    async fn generate_text(
        &self,
        _prompt: &str,
        _system_prompt: &str,
        _tools: Option<Vec<serde_json::Value>>,
    ) -> Result<String> {
        Ok(self.text.clone())
    }

    async fn generate_with_tools(
        &self,
        _prompt: &str,
        _system_prompt: &str,
        _tools: Vec<serde_json::Value>,
    ) -> Result<LlmResponse> {
        let mut guard = self.queue.lock().await;
        Ok(guard.pop_front().unwrap_or(LlmResponse {
            text: self.text.clone(),
            tool_calls: Vec::new(),
        }))
    }

    fn chat_stream(
        &self,
        _messages: Vec<serde_json::Value>,
        _tools: Option<Vec<serde_json::Value>>,
    ) -> futures::stream::BoxStream<'static, Result<ChatEvent>> {
        use async_stream::try_stream;
        let text = self.text.clone();
        Box::pin(try_stream! {
            yield ChatEvent {
                event_type: "content".to_string(),
                delta: Some(text),
                name: None,
                arguments_delta: None,
                finish_reason: None,
                error: None,
            };
            yield ChatEvent {
                event_type: "message_end".to_string(),
                delta: None,
                name: None,
                arguments_delta: None,
                finish_reason: Some("stop".to_string()),
                error: None,
            };
        })
    }

    async fn parse_structured_output(
        &self,
        _prompt: &str,
        _system_prompt: &str,
        _json_schema: serde_json::Value,
        _tools: Option<Vec<serde_json::Value>>,
    ) -> Result<serde_json::Value> {
        Ok(self.structured.clone())
    }

    async fn tts(&self, _text: &str, _voice: &str, _response_format: &str) -> Result<Vec<u8>> {
        Ok(self.tts_bytes.clone())
    }

    async fn transcribe_audio(&self, _audio_bytes: Vec<u8>, _input_format: &str) -> Result<String> {
        Ok(self.transcript.clone())
    }

    async fn generate_text_with_images(
        &self,
        _prompt: &str,
        _images: Vec<ImageInput>,
        _system_prompt: &str,
        _detail: &str,
        _tools: Option<Vec<serde_json::Value>>,
    ) -> Result<String> {
        Ok(self.image_text.clone())
    }
}

struct DummyRouter;

#[async_trait]
impl RoutingServiceTrait for DummyRouter {
    async fn route_query(&self, _query: &str) -> Result<String> {
        Ok("router_agent".to_string())
    }
}

struct DummyTool {
    name: String,
    configured: Mutex<bool>,
}

impl DummyTool {
    fn new(name: &str) -> Self {
        Self {
            name: name.to_string(),
            configured: Mutex::new(false),
        }
    }
}

#[async_trait]
impl Tool for DummyTool {
    fn name(&self) -> &str {
        &self.name
    }

    fn description(&self) -> &str {
        "dummy"
    }

    fn parameters(&self) -> serde_json::Value {
        json!({"type":"object","properties":{}})
    }

    fn configure(&self, _config: &serde_json::Value) -> Result<()> {
        let mut guard = futures::executor::block_on(self.configured.lock());
        *guard = true;
        Ok(())
    }

    async fn execute(&self, _params: serde_json::Value) -> Result<serde_json::Value> {
        Ok(json!({"ok": true}))
    }
}

struct FailingTool;

#[async_trait]
impl Tool for FailingTool {
    fn name(&self) -> &str {
        "fail"
    }

    fn description(&self) -> &str {
        "fail"
    }

    fn parameters(&self) -> serde_json::Value {
        json!({})
    }

    fn configure(&self, _config: &serde_json::Value) -> Result<()> {
        Err(SolanaAgentError::Runtime("fail".to_string()))
    }

    async fn execute(&self, _params: serde_json::Value) -> Result<serde_json::Value> {
        Ok(json!({"ok": false}))
    }
}

struct ConditionalTool {
    name: String,
}

struct DefaultConfigureTool;

#[async_trait]
impl Tool for DefaultConfigureTool {
    fn name(&self) -> &str {
        "default"
    }

    fn description(&self) -> &str {
        "default"
    }

    fn parameters(&self) -> serde_json::Value {
        json!({})
    }

    async fn execute(&self, _params: serde_json::Value) -> Result<serde_json::Value> {
        Ok(json!({"ok": true}))
    }
}

struct FlakyNameTool {
    toggle: Mutex<bool>,
}

impl FlakyNameTool {
    fn new() -> Self {
        Self {
            toggle: Mutex::new(false),
        }
    }
}

#[async_trait]
impl Tool for FlakyNameTool {
    fn name(&self) -> &str {
        let mut guard = futures::executor::block_on(self.toggle.lock());
        let name = if *guard { "tool_b" } else { "tool_a" };
        *guard = !*guard;
        name
    }

    fn description(&self) -> &str {
        "flaky"
    }

    fn parameters(&self) -> serde_json::Value {
        json!({})
    }

    async fn execute(&self, _params: serde_json::Value) -> Result<serde_json::Value> {
        Ok(json!({}))
    }
}

#[async_trait]
impl Tool for ConditionalTool {
    fn name(&self) -> &str {
        &self.name
    }

    fn description(&self) -> &str {
        "conditional"
    }

    fn parameters(&self) -> serde_json::Value {
        json!({})
    }

    fn configure(&self, config: &serde_json::Value) -> Result<()> {
        if config.get("fail").is_some() {
            return Err(SolanaAgentError::Runtime("fail".to_string()));
        }
        Ok(())
    }

    async fn execute(&self, _params: serde_json::Value) -> Result<serde_json::Value> {
        Ok(json!({"ok": true}))
    }
}

struct DummyPlugin {
    name: String,
    initialized: Mutex<bool>,
    ok: bool,
}

impl DummyPlugin {
    fn new(name: &str, ok: bool) -> Self {
        Self {
            name: name.to_string(),
            initialized: Mutex::new(false),
            ok,
        }
    }
}

impl Plugin for DummyPlugin {
    fn name(&self) -> &str {
        &self.name
    }

    fn description(&self) -> &str {
        "plugin"
    }

    fn initialize(&self, _tool_registry: &ToolRegistry) -> bool {
        let mut guard = futures::executor::block_on(self.initialized.lock());
        *guard = true;
        self.ok
    }
}

struct DummyMemoryProvider {
    messages: Mutex<Vec<(String, String, String)>>,
}

impl DummyMemoryProvider {
    fn new() -> Self {
        Self {
            messages: Mutex::new(Vec::new()),
        }
    }
}

#[async_trait]
impl MemoryProvider for DummyMemoryProvider {
    async fn append_message(&self, user_id: &str, role: &str, content: &str) -> Result<()> {
        self.messages.lock().await.push((
            user_id.to_string(),
            role.to_string(),
            content.to_string(),
        ));
        Ok(())
    }

    async fn get_history(&self, user_id: &str, _limit: usize) -> Result<Vec<String>> {
        let guard = self.messages.lock().await;
        Ok(guard
            .iter()
            .filter(|(u, _, _)| u == user_id)
            .map(|(_, role, content)| format!("{}: {}", role, content))
            .collect())
    }

    async fn clear_history(&self, user_id: &str) -> Result<()> {
        let mut guard = self.messages.lock().await;
        guard.retain(|(u, _, _)| u != user_id);
        Ok(())
    }
}

#[tokio::test]
async fn config_from_file_and_factory_errors() {
    let tmp = tempfile::NamedTempFile::new().unwrap();
    std::fs::write(
        tmp.path(),
        json!({
            "openai": {"api_key":"key","model":null,"base_url":null},
            "agents": [{
                "name":"agent",
                "instructions":"inst",
                "specialization":"spec",
                "description":null,
                "capture_name":null,
                "capture_schema":null
            }],
            "business": null,
            "mongo": null,
            "guardrails": null
        })
        .to_string(),
    )
    .unwrap();
    let config = Config::from_file(tmp.path()).unwrap();
    let _ = SolanaAgentFactory::create_from_config(config)
        .await
        .unwrap();

    let bad = tempfile::NamedTempFile::new().unwrap();
    std::fs::write(bad.path(), "{bad}").unwrap();
    let err = Config::from_file(bad.path()).unwrap_err();
    assert!(matches!(err, SolanaAgentError::Config(_)));

    let err = Config::from_file("/nope/not-found.json").unwrap_err();
    assert!(matches!(err, SolanaAgentError::Config(_)));

    let missing = Config {
        openai: None,
        groq: None,
        agents: Vec::new(),
        business: None,
        mongo: None,
        guardrails: None,
    };
    let err = SolanaAgentFactory::create_from_config(missing)
        .await
        .err()
        .unwrap();
    assert!(matches!(err, SolanaAgentError::Config(_)));

    let groq = Config {
        openai: None,
        groq: Some(solana_agent::config::GroqConfig {
            api_key: "key".to_string(),
            model: Some("gpt-4o-mini".to_string()),
            base_url: None,
        }),
        agents: vec![AgentConfig {
            name: "agent".to_string(),
            instructions: "inst".to_string(),
            specialization: "spec".to_string(),
            description: None,
            capture_name: None,
            capture_schema: None,
        }],
        business: None,
        mongo: None,
        guardrails: None,
    };
    let _ = SolanaAgentFactory::create_from_config(groq).await.unwrap();

    let guardrails = Config {
        openai: Some(OpenAiConfig {
            api_key: "key".to_string(),
            model: None,
            base_url: None,
        }),
        groq: None,
        agents: vec![AgentConfig {
            name: "agent".to_string(),
            instructions: "inst".to_string(),
            specialization: "spec".to_string(),
            description: None,
            capture_name: None,
            capture_schema: None,
        }],
        business: None,
        mongo: None,
        guardrails: Some(GuardrailsConfig {
            input: Some(vec![GuardrailConfig {
                class: "noop".to_string(),
                config: None,
            }]),
            output: Some(vec![GuardrailConfig {
                class: "noop".to_string(),
                config: None,
            }]),
        }),
    };
    let _ = SolanaAgentFactory::create_from_config(guardrails)
        .await
        .unwrap();

    let mixed_guardrails = Config {
        openai: Some(OpenAiConfig {
            api_key: "key".to_string(),
            model: None,
            base_url: None,
        }),
        groq: None,
        agents: vec![AgentConfig {
            name: "agent".to_string(),
            instructions: "inst".to_string(),
            specialization: "spec".to_string(),
            description: None,
            capture_name: None,
            capture_schema: None,
        }],
        business: None,
        mongo: None,
        guardrails: Some(GuardrailsConfig {
            input: Some(vec![
                GuardrailConfig {
                    class: "PII".to_string(),
                    config: None,
                },
                GuardrailConfig {
                    class: "noop".to_string(),
                    config: None,
                },
            ]),
            output: Some(vec![GuardrailConfig {
                class: "PII".to_string(),
                config: None,
            }]),
        }),
    };
    let _ = SolanaAgentFactory::create_from_config(mixed_guardrails)
        .await
        .unwrap();

    let mongo = Config {
        openai: Some(OpenAiConfig {
            api_key: "key".to_string(),
            model: None,
            base_url: None,
        }),
        groq: None,
        agents: vec![AgentConfig {
            name: "agent".to_string(),
            instructions: "inst".to_string(),
            specialization: "spec".to_string(),
            description: None,
            capture_name: None,
            capture_schema: None,
        }],
        business: None,
        mongo: Some(solana_agent::config::MongoConfig {
            connection_string: "mongodb://localhost".to_string(),
            database: "db".to_string(),
            collection: None,
        }),
        guardrails: None,
    };
    let err = SolanaAgentFactory::create_from_config(mongo)
        .await
        .err()
        .unwrap();
    assert!(matches!(err, SolanaAgentError::Config(_)));

    let _ok: solana_agent::error::Result<()> = Ok(());
    let err = SolanaAgentError::Runtime("boom".to_string());
    assert!(format!("{err}").contains("boom"));
}

#[tokio::test]
async fn guardrails_work() {
    let noop = NoopGuardrail;
    assert_eq!(noop.process("hi").await.unwrap(), "hi");
    let out = <NoopGuardrail as solana_agent::interfaces::guardrails::OutputGuardrail>::process(
        &noop, "out",
    )
    .await
    .unwrap();
    assert_eq!(out, "out");

    let pii = PiiGuardrail::new(None);
    let scrubbed = pii.process("email test@example.com").await.unwrap();
    assert!(scrubbed.contains("[REDACTED]"));
    let scrubbed =
        <PiiGuardrail as solana_agent::interfaces::guardrails::OutputGuardrail>::process(
            &pii,
            "call +1 555 123 4567",
        )
        .await
        .unwrap();
    assert!(scrubbed.contains("[REDACTED]"));

    let custom = PiiGuardrail::new(Some(json!({"replacement":"X"})));
    let scrubbed = custom.process("call +1 555 123 4567").await.unwrap();
    assert!(scrubbed.contains("X"));
}

#[tokio::test]
async fn tool_registry_and_plugin_manager() {
    let registry = ToolRegistry::new();
    let tool = Arc::new(DummyTool::new("tool"));
    assert!(registry.register_tool(tool.clone()).await);
    assert!(!registry.register_tool(tool.clone()).await);

    let fail_tool = Arc::new(FailingTool);
    assert!(!registry.register_tool(fail_tool).await);

    assert!(registry.assign_tool_to_agent("agent", "tool").await);
    assert!(!registry.assign_tool_to_agent("agent", "missing").await);

    let got = registry.get_tool("tool").await.unwrap();
    assert_eq!(got.name(), "tool");

    let agent_tools = registry.get_agent_tools("agent").await;
    assert_eq!(agent_tools.len(), 1);

    let all = registry.list_all_tools().await;
    assert_eq!(all, vec!["tool".to_string()]);

    registry
        .configure_all_tools(json!({"value": 1}))
        .await
        .unwrap();

    let mut manager = DefaultPluginManager::new(json!({"ok":true}));
    assert!(manager.register_plugin(Box::new(DummyPlugin::new("p1", true))));
    assert!(!manager.register_plugin(Box::new(DummyPlugin::new("p2", false))));
    assert!(manager.get_plugin("p1").is_some());
    assert!(manager.get_plugin("missing").is_none());
    assert_eq!(manager.list_plugins().len(), 1);
    manager.configure(json!({"reconfigured":true}));
    manager.load_plugins();
    let _ = manager.tool_registry();

    let registry = ToolRegistry::new();
    let conditional = Arc::new(ConditionalTool {
        name: "conditional".to_string(),
    });
    assert!(registry.register_tool(conditional).await);
    let err = registry
        .configure_all_tools(json!({"fail": true}))
        .await
        .unwrap_err();
    assert!(matches!(err, SolanaAgentError::Runtime(_)));

    let registry = ToolRegistry::new();
    let default_tool = Arc::new(DefaultConfigureTool);
    assert!(registry.register_tool(default_tool).await);
}

#[tokio::test]
async fn memory_provider_defaults_and_in_memory() {
    let provider = InMemoryMemoryProvider::new();
    provider.append_message("u1", "user", "hi").await.unwrap();
    provider
        .append_message("u1", "assistant", "hello")
        .await
        .unwrap();

    let history = provider.get_history("u1", 1).await.unwrap();
    assert_eq!(history.len(), 1);

    let all = provider.get_history("u1", 0).await.unwrap();
    assert_eq!(all.len(), 2);

    provider.clear_history("u1").await.unwrap();
    assert!(provider.get_history("u1", 0).await.unwrap().is_empty());

    provider
        .store(
            "u2",
            vec![
                json!({"role":"user","content":"a"}),
                json!({"role":"assistant","content":"b"}),
            ],
        )
        .await
        .unwrap();
    assert_eq!(
        provider.retrieve("u2").await.unwrap(),
        "user: a\nassistant: b"
    );
    provider.delete("u2").await.unwrap();

    provider
        .save_capture(
            "u3",
            "cap",
            Some("agent"),
            json!({"x":1}),
            Some(json!({"type":"object"})),
        )
        .await
        .unwrap();
    provider
        .save_capture("u3", "cap", None, json!({"x":2}), None)
        .await
        .unwrap();
    let captures = provider
        .find("captures", json!(null), None, None, None)
        .unwrap();
    assert_eq!(captures.len(), 2);
    let filtered = provider
        .find("captures", json!({"user_id":"u3"}), None, None, None)
        .unwrap();
    assert_eq!(filtered.len(), 2);
    assert_eq!(
        provider.count_documents("captures", json!(null)).unwrap(),
        2
    );

    let dummy = DummyMemoryProvider::new();
    dummy
        .store("u4", vec![json!({"role":"user","content":"x"})])
        .await
        .unwrap();
    assert_eq!(dummy.retrieve("u4").await.unwrap(), "user: x");
    dummy.delete("u4").await.unwrap();
    assert_eq!(
        dummy
            .find("any", json!(null), None, None, None)
            .unwrap()
            .len(),
        0
    );
    assert_eq!(dummy.count_documents("any", json!(null)).unwrap(), 0);
    assert!(dummy
        .save_capture("u", "cap", None, json!({}), None)
        .await
        .unwrap()
        .is_none());
}

#[tokio::test]
async fn routing_and_agent_service() {
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

    let mut service = AgentService::new(llm, Some(mission), vec![Arc::new(NoopGuardrail)]);
    service.register_ai_agent(
        "agent1".to_string(),
        "inst".to_string(),
        "spec".to_string(),
        Some("cap".to_string()),
        Some(json!({"type":"object"})),
    );

    let err = service.get_agent_system_prompt("missing").unwrap_err();
    assert!(matches!(err, SolanaAgentError::Runtime(_)));

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
    let mut pii_service = AgentService::new(
        Arc::new(provider),
        None,
        vec![Arc::new(PiiGuardrail::new(None))],
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
    let mut looping_service = AgentService::new(looping_llm, None, vec![]);
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
    let mut service = AgentService::new(llm, None, vec![]);
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

    let empty_service = AgentService::new(Arc::new(QueueLlmProvider::new(vec![])), None, vec![]);
    let empty_routing = RoutingService::new(Arc::new(empty_service));
    assert_eq!(
        empty_routing.route_query("anything").await.unwrap(),
        "default"
    );
}

#[tokio::test]
async fn query_service_and_client() {
    let llm = Arc::new(QueueLlmProvider::new(vec![]));
    let mut service = AgentService::new(llm.clone(), None, vec![Arc::new(NoopGuardrail)]);
    service.register_ai_agent(
        "agent".to_string(),
        "inst".to_string(),
        "spec".to_string(),
        None,
        None,
    );
    service.register_ai_agent(
        "router_agent".to_string(),
        "inst".to_string(),
        "spec".to_string(),
        None,
        None,
    );
    let service = Arc::new(service);
    let routing = Arc::new(RoutingService::new(service.clone()));
    let memory = Arc::new(InMemoryMemoryProvider::new());

    let query = QueryService::new(
        service.clone(),
        routing.clone(),
        Some(memory),
        vec![Arc::new(PiiGuardrail::new(None))],
    );

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
        router: Some(Arc::new(DummyRouter)),
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
        router: None,
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
        router: None,
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
    let mut service = AgentService::new(llm, None, vec![]);
    service.register_ai_agent(
        "agent".to_string(),
        "inst".to_string(),
        "spec".to_string(),
        None,
        None,
    );
    let service = Arc::new(service);
    let routing = Arc::new(RoutingService::new(service.clone()));
    let query = QueryService::new(service, routing, None, vec![]);
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
        router: None,
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
            api_key: "key".to_string(),
            model: None,
            base_url: None,
        }),
        groq: None,
        agents: vec![AgentConfig {
            name: "agent".to_string(),
            instructions: "inst".to_string(),
            specialization: "spec".to_string(),
            description: None,
            capture_name: None,
            capture_schema: None,
        }],
        business: Some(BusinessConfig {
            mission: Some("m".to_string()),
            voice: Some("v".to_string()),
            values: Some(vec![BusinessValue {
                name: "n".to_string(),
                description: "d".to_string(),
            }]),
            goals: Some(vec!["g".to_string()]),
        }),
        mongo: None,
        guardrails: Some(GuardrailsConfig {
            input: Some(vec![GuardrailConfig {
                class: "PII".to_string(),
                config: None,
            }]),
            output: Some(vec![GuardrailConfig {
                class: "unknown".to_string(),
                config: None,
            }]),
        }),
    };
    let agent = SolanaAgent::from_config(config).await.unwrap();
    let tool = Arc::new(DummyTool::new("tool"));
    let registered = agent.register_tool("agent", tool.clone()).await.unwrap();
    assert!(registered);

    let registered = agent.register_tool("agent", tool.clone()).await.unwrap();
    assert!(!registered);

    let flaky = Arc::new(FlakyNameTool::new());
    let err = agent.register_tool("agent", flaky).await.unwrap_err();
    assert!(matches!(err, SolanaAgentError::Runtime(_)));

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
            "agents": [{
                "name":"agent",
                "instructions":"inst",
                "specialization":"spec",
                "description":null,
                "capture_name":null,
                "capture_schema":null
            }],
            "business": null,
            "mongo": null,
            "guardrails": null
        })
        .to_string(),
    )
    .unwrap();
    let agent = SolanaAgent::from_config_path(tmp.path()).await.unwrap();
    let mut stream = agent.process_text_stream("user", "hello", None);
    let chunk = stream.next().await.unwrap().unwrap();
    assert_eq!(chunk, "mock text");
    chat_mock.assert_hits(1);

    agent.delete_user_history("user").await.unwrap();
    let _ = agent.get_user_history("user", 5).await.unwrap();
}

#[tokio::test]
async fn openai_provider_via_httpmock() {
    let server = MockServer::start_async().await;
    let chat_mock = server
        .mock_async(|when, then| {
            when.method(POST).path("/chat/completions");
            then.status(200).json_body(json!({
                "id": "chatcmpl-1",
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

    let provider = OpenAiProvider::new(
        "key".to_string(),
        Some("gpt-4o-mini".to_string()),
        Some(server.base_url()),
    );
    let text = provider.generate_text("hi", "", None).await.unwrap();
    assert_eq!(text, "hello");

    let mut stream = provider.chat_stream(vec![json!({"role":"user","content":"hi"})], None);
    let first = stream.next().await.unwrap().unwrap();
    assert_eq!(first.event_type, "content");
    let last = stream.next().await.unwrap().unwrap();
    assert_eq!(last.event_type, "message_end");

    chat_mock.assert_hits(2);
}

#[tokio::test]
async fn openai_provider_tools_images_structured_audio() {
    let server = MockServer::start_async().await;

    let tool_mock = server
        .mock_async(|when, then| {
            when.method(POST).path("/chat/completions");
            then.status(200).json_body(json!({
                "id": "chatcmpl-2",
                "object": "chat.completion",
                "created": 1,
                "model": "gpt-4o-mini",
                "choices": [{
                    "index": 0,
                    "message": {
                        "role": "assistant",
                        "content": null,
                        "tool_calls": [
                            {
                                "type": "function",
                                "id": "call_1",
                                "function": {"name": "tool1", "arguments": "{\"x\":1}"}
                            }
                        ]
                    },
                    "finish_reason": "tool_calls"
                }]
            }));
        })
        .await;

    let provider = OpenAiProvider::new(
        "key".to_string(),
        Some("gpt-4o-mini".to_string()),
        Some(server.base_url()),
    );
    let response = provider
        .generate_with_tools(
            "hi",
            "sys",
            vec![json!({"type":"function","name":"tool1","parameters":{}})],
        )
        .await
        .unwrap();
    assert_eq!(response.tool_calls.len(), 1);
    assert_eq!(response.tool_calls[0].name, "tool1");

    tool_mock.assert_hits(1);

    let structured_server = MockServer::start_async().await;
    let structured_mock = structured_server
        .mock_async(|when, then| {
            when.method(POST).path("/chat/completions");
            then.status(200).json_body(json!({
                "id": "chatcmpl-3",
                "object": "chat.completion",
                "created": 1,
                "model": "gpt-4o-mini",
                "choices": [{
                    "index": 0,
                    "message": {"role": "assistant", "content": "{\"ok\":true}"},
                    "finish_reason": "stop"
                }]
            }));
        })
        .await;
    let structured_provider = OpenAiProvider::new(
        "key".to_string(),
        Some("gpt-4o-mini".to_string()),
        Some(structured_server.base_url()),
    );
    let structured = structured_provider
        .parse_structured_output("hi", "", json!({"type":"object"}), None)
        .await
        .unwrap();
    assert_eq!(structured, json!({"ok": true}));
    structured_mock.assert_hits(1);

    let image_server = MockServer::start_async().await;
    let image_mock = image_server
        .mock_async(|when, then| {
            when.method(POST).path("/chat/completions");
            then.status(200).json_body(json!({
                "id": "chatcmpl-4",
                "object": "chat.completion",
                "created": 1,
                "model": "gpt-4o-mini",
                "choices": [{
                    "index": 0,
                    "message": {"role": "assistant", "content": "image"},
                    "finish_reason": "stop"
                }]
            }));
        })
        .await;
    let image_provider = OpenAiProvider::new(
        "key".to_string(),
        Some("gpt-4o-mini".to_string()),
        Some(image_server.base_url()),
    );
    let image_text = image_provider
        .generate_text_with_images(
            "hi",
            vec![ImageInput {
                data: ImageData::Bytes(vec![1, 2, 3]),
            }],
            "",
            "high",
            None,
        )
        .await
        .unwrap();
    assert_eq!(image_text, "image");
    image_mock.assert_hits(1);

    let speech_server = MockServer::start_async().await;
    let speech_mock = speech_server
        .mock_async(|when, then| {
            when.method(POST).path("/audio/speech");
            then.status(200).body("AUDIO");
        })
        .await;
    let speech_provider = OpenAiProvider::new(
        "key".to_string(),
        Some("gpt-4o-mini".to_string()),
        Some(speech_server.base_url()),
    );
    let audio = speech_provider.tts("hello", "alloy", "mp3").await.unwrap();
    assert_eq!(audio, b"AUDIO".to_vec());
    speech_mock.assert_hits(1);

    let transcribe_server = MockServer::start_async().await;
    let transcribe_mock = transcribe_server
        .mock_async(|when, then| {
            when.method(POST).path("/audio/transcriptions");
            then.status(200).json_body(json!({
                "text": "transcribed",
                "logprobs": null,
                "usage": {
                    "type": "tokens",
                    "input_tokens": 1,
                    "output_tokens": 1,
                    "total_tokens": 2,
                    "input_token_details": null
                }
            }));
        })
        .await;
    let transcribe_provider = OpenAiProvider::new(
        "key".to_string(),
        Some("gpt-4o-mini".to_string()),
        Some(transcribe_server.base_url()),
    );
    let transcript = transcribe_provider
        .transcribe_audio(vec![1, 2, 3], "wav")
        .await
        .unwrap();
    assert_eq!(transcript, "transcribed");
    transcribe_mock.assert_hits(1);
}

#[tokio::test]
async fn openai_provider_additional_branches() {
    let tools_server = MockServer::start_async().await;
    let tools_mock = tools_server
        .mock_async(|when, then| {
            when.method(POST).path("/chat/completions");
            then.status(200).json_body(json!({
                "id": "chatcmpl-tools",
                "object": "chat.completion",
                "created": 1,
                "model": "gpt-4o-mini",
                "choices": [{
                    "index": 0,
                    "message": {"role": "assistant", "content": "with tools"},
                    "finish_reason": "stop"
                }]
            }));
        })
        .await;
    let tools_provider = OpenAiProvider::new(
        "key".to_string(),
        Some("gpt-4o-mini".to_string()),
        Some(tools_server.base_url()),
    );
    let text = tools_provider
        .generate_text(
            "hi",
            "sys",
            Some(vec![
                json!({"type":"function","name":"tool1","parameters":{}}),
            ]),
        )
        .await
        .unwrap();
    assert_eq!(text, "with tools");
    tools_mock.assert_hits(1);

    let empty_server = MockServer::start_async().await;
    let empty_mock = empty_server
        .mock_async(|when, then| {
            when.method(POST).path("/chat/completions");
            then.status(200).json_body(json!({
                "id": "chatcmpl-empty",
                "object": "chat.completion",
                "created": 1,
                "model": "gpt-4o-mini",
                "choices": []
            }));
        })
        .await;
    let empty_provider = OpenAiProvider::new(
        "key".to_string(),
        Some("gpt-4o-mini".to_string()),
        Some(empty_server.base_url()),
    );
    let response = empty_provider
        .generate_with_tools(
            "hi",
            "sys",
            vec![json!({"type":"function","name":"tool1","parameters":{}})],
        )
        .await
        .unwrap();
    assert!(response.text.is_empty());
    assert!(response.tool_calls.is_empty());
    empty_mock.assert_hits(1);

    let structured_server = MockServer::start_async().await;
    let structured_mock = structured_server
        .mock_async(|when, then| {
            when.method(POST).path("/chat/completions");
            then.status(200).json_body(json!({
                "id": "chatcmpl-struct-tools",
                "object": "chat.completion",
                "created": 1,
                "model": "gpt-4o-mini",
                "choices": [{
                    "index": 0,
                    "message": {"role": "assistant", "content": "{\"ok\":true}"},
                    "finish_reason": "stop"
                }]
            }));
        })
        .await;
    let structured_provider = OpenAiProvider::new(
        "key".to_string(),
        Some("gpt-4o-mini".to_string()),
        Some(structured_server.base_url()),
    );
    let structured = structured_provider
        .parse_structured_output(
            "hi",
            "system",
            json!({"title":"Example","type":"object"}),
            Some(vec![
                json!({"type":"function","name":"tool1","parameters":{}}),
            ]),
        )
        .await
        .unwrap();
    assert_eq!(structured, json!({"ok": true}));
    structured_mock.assert_hits(1);

    let image_server = MockServer::start_async().await;
    let image_mock = image_server
        .mock_async(|when, then| {
            when.method(POST).path("/chat/completions");
            then.status(200).json_body(json!({
                "id": "chatcmpl-image-tools",
                "object": "chat.completion",
                "created": 1,
                "model": "gpt-4o-mini",
                "choices": [{
                    "index": 0,
                    "message": {"role": "assistant", "content": "image tools"},
                    "finish_reason": "stop"
                }]
            }));
        })
        .await;
    let image_provider = OpenAiProvider::new(
        "key".to_string(),
        Some("gpt-4o-mini".to_string()),
        Some(image_server.base_url()),
    );
    let image_text = image_provider
        .generate_text_with_images(
            "hi",
            vec![ImageInput {
                data: ImageData::Bytes(vec![1, 2, 3]),
            }],
            "sys",
            "auto",
            Some(vec![
                json!({"type":"function","name":"tool1","parameters":{}}),
            ]),
        )
        .await
        .unwrap();
    assert_eq!(image_text, "image tools");
    image_mock.assert_hits(1);
}

#[tokio::test]
async fn openai_provider_variants_and_agent_process() {
    let chat_server = MockServer::start_async().await;
    let chat_mock = chat_server
        .mock_async(|when, then| {
            when.method(POST).path("/chat/completions");
            then.status(200).json_body(json!({
                "id": "chatcmpl-5",
                "object": "chat.completion",
                "created": 1,
                "model": "gpt-4o-mini",
                "choices": [{
                    "index": 0,
                    "message": {"role": "assistant", "content": "text"},
                    "finish_reason": "stop"
                }]
            }));
        })
        .await;

    let chat_provider = OpenAiProvider::new(
        "key".to_string(),
        Some("gpt-4o-mini".to_string()),
        Some(chat_server.base_url()),
    );
    let text = chat_provider
        .generate_text("hi", "", Some(vec![json!({"type":"custom","name":"x"})]))
        .await
        .unwrap();
    assert_eq!(text, "text");
    chat_mock.assert_hits(1);

    let skip_server = MockServer::start_async().await;
    let skip_mock = skip_server
        .mock_async(|when, then| {
            when.method(POST).path("/chat/completions");
            then.status(200).json_body(json!({
                "id": "chatcmpl-skip",
                "object": "chat.completion",
                "created": 1,
                "model": "gpt-4o-mini",
                "choices": [{
                    "index": 0,
                    "message": {"role": "assistant", "content": "skip"},
                    "finish_reason": "stop"
                }]
            }));
        })
        .await;
    let skip_provider = OpenAiProvider::new(
        "key".to_string(),
        Some("gpt-4o-mini".to_string()),
        Some(skip_server.base_url()),
    );
    let text = skip_provider
        .generate_text(
            "hi",
            "sys",
            Some(vec![
                json!({"type":"custom","name":"x"}),
                json!({"type":"function","parameters":{}}),
            ]),
        )
        .await
        .unwrap();
    assert_eq!(text, "skip");
    skip_mock.assert_hits(1);

    let nested_server = MockServer::start_async().await;
    let nested_mock = nested_server
        .mock_async(|when, then| {
            when.method(POST).path("/chat/completions");
            then.status(200).json_body(json!({
                "id": "chatcmpl-6",
                "object": "chat.completion",
                "created": 1,
                "model": "gpt-4o-mini",
                "choices": [{
                    "index": 0,
                    "message": {
                        "role": "assistant",
                        "content": null,
                        "tool_calls": [
                            {
                                "type": "function",
                                "id": "call_1",
                                "function": {"name": "tool_nested", "arguments": "{\"x\":1}"}
                            }
                        ]
                    },
                    "finish_reason": "tool_calls"
                }]
            }));
        })
        .await;
    let nested_provider = OpenAiProvider::new(
        "key".to_string(),
        Some("gpt-4o-mini".to_string()),
        Some(nested_server.base_url()),
    );
    let response = nested_provider
        .generate_with_tools(
            "hi",
            "sys",
            vec![json!({"type":"function","function":{"name":"tool_nested","parameters":{}}})],
        )
        .await
        .unwrap();
    assert_eq!(response.tool_calls[0].name, "tool_nested");
    nested_mock.assert_hits(1);

    let custom_server = MockServer::start_async().await;
    let custom_mock = custom_server
        .mock_async(|when, then| {
            when.method(POST).path("/chat/completions");
            then.status(200).json_body(json!({
                "id": "chatcmpl-7",
                "object": "chat.completion",
                "created": 1,
                "model": "gpt-4o-mini",
                "choices": [{
                    "index": 0,
                    "message": {
                        "role": "assistant",
                        "content": null,
                        "tool_calls": [
                            {
                                "type": "custom",
                                "id": "call_2",
                                "custom_tool": {"name": "custom_tool", "input": "{\"y\":2}"}
                            }
                        ]
                    },
                    "finish_reason": "tool_calls"
                }]
            }));
        })
        .await;
    let custom_provider = OpenAiProvider::new(
        "key".to_string(),
        Some("gpt-4o-mini".to_string()),
        Some(custom_server.base_url()),
    );
    let response = custom_provider
        .generate_with_tools(
            "hi",
            "sys",
            vec![json!({"type":"function","name":"x","parameters":{}})],
        )
        .await
        .unwrap();
    assert_eq!(response.tool_calls[0].name, "custom_tool");
    custom_mock.assert_hits(1);

    let fallback_server = MockServer::start_async().await;
    let fallback_mock = fallback_server
        .mock_async(|when, then| {
            when.method(POST).path("/chat/completions");
            then.status(200).json_body(json!({
                "id": "chatcmpl-8",
                "object": "chat.completion",
                "created": 1,
                "model": "gpt-4o-mini",
                "choices": [{
                    "index": 0,
                    "message": {
                        "role": "assistant",
                        "content": null,
                        "function_call": {"name": "legacy", "arguments": "{\"z\":3}"}
                    },
                    "finish_reason": "function_call"
                }]
            }));
        })
        .await;
    let fallback_provider = OpenAiProvider::new(
        "key".to_string(),
        Some("gpt-4o-mini".to_string()),
        Some(fallback_server.base_url()),
    );
    let response = fallback_provider
        .generate_with_tools(
            "hi",
            "sys",
            vec![json!({"type":"function","name":"legacy","parameters":{}})],
        )
        .await
        .unwrap();
    assert_eq!(response.tool_calls[0].name, "legacy");
    fallback_mock.assert_hits(1);

    let image_server = MockServer::start_async().await;
    let image_mock = image_server
        .mock_async(|when, then| {
            when.method(POST).path("/chat/completions");
            then.status(200).json_body(json!({
                "id": "chatcmpl-9",
                "object": "chat.completion",
                "created": 1,
                "model": "gpt-4o-mini",
                "choices": [{
                    "index": 0,
                    "message": {"role": "assistant", "content": "image"},
                    "finish_reason": "stop"
                }]
            }));
        })
        .await;
    let image_provider = OpenAiProvider::new(
        "key".to_string(),
        Some("gpt-4o-mini".to_string()),
        Some(image_server.base_url()),
    );
    let _ = image_provider
        .generate_text_with_images(
            "hi",
            vec![ImageInput {
                data: ImageData::Url("http://example.com".to_string()),
            }],
            "",
            "low",
            None,
        )
        .await
        .unwrap();
    let _ = image_provider
        .generate_text_with_images(
            "hi",
            vec![ImageInput {
                data: ImageData::Url("http://example.com".to_string()),
            }],
            "sys",
            "weird",
            None,
        )
        .await
        .unwrap();
    let _ = image_provider
        .generate_text_with_images(
            "hi",
            vec![ImageInput {
                data: ImageData::Bytes(vec![1, 2, 3]),
            }],
            "",
            "auto",
            None,
        )
        .await
        .unwrap();
    image_mock.assert_hits(3);

    let speech_server = MockServer::start_async().await;
    let speech_mock = speech_server
        .mock_async(|when, then| {
            when.method(POST).path("/audio/speech");
            then.status(200).body("AUDIO");
        })
        .await;
    let speech_provider = OpenAiProvider::new(
        "key".to_string(),
        Some("gpt-4o-mini".to_string()),
        Some(speech_server.base_url()),
    );
    let voices = [
        "alloy", "ash", "ballad", "coral", "echo", "fable", "onyx", "nova", "sage", "shimmer",
        "verse", "custom",
    ];
    for voice in voices {
        let _ = speech_provider.tts("hi", voice, "mp3").await.unwrap();
    }
    let formats = ["opus", "aac", "flac", "wav", "pcm", "pcm16", "mp3"];
    for format in formats {
        let _ = speech_provider.tts("hi", "alloy", format).await.unwrap();
    }
    speech_mock.assert_hits(voices.len() as usize + formats.len() as usize);

    let agent_server = MockServer::start_async().await;
    let agent_mock = agent_server
        .mock_async(|when, then| {
            when.method(POST).path("/chat/completions");
            then.status(200).json_body(json!({
                "id": "chatcmpl-10",
                "object": "chat.completion",
                "created": 1,
                "model": "gpt-4o-mini",
                "choices": [{
                    "index": 0,
                    "message": {"role": "assistant", "content": "agent response"},
                    "finish_reason": "stop"
                }]
            }));
        })
        .await;

    let config = Config {
        openai: Some(OpenAiConfig {
            api_key: "key".to_string(),
            model: Some("gpt-4o-mini".to_string()),
            base_url: Some(agent_server.base_url()),
        }),
        groq: None,
        agents: vec![AgentConfig {
            name: "agent".to_string(),
            instructions: "inst".to_string(),
            specialization: "spec".to_string(),
            description: None,
            capture_name: None,
            capture_schema: None,
        }],
        business: None,
        mongo: None,
        guardrails: None,
    };
    let agent = SolanaAgent::from_config(config).await.unwrap();
    let result = agent
        .process(
            "user",
            UserInput::Text("hi".to_string()),
            ProcessOptions {
                prompt: None,
                images: vec![],
                output_format: OutputFormat::Text,
                image_detail: "auto".to_string(),
                json_schema: None,
                router: None,
            },
        )
        .await
        .unwrap();
    match result {
        ProcessResult::Text(value) => assert_eq!(value, "agent response"),
        other => panic!("unexpected result: {other:?}"),
    }
    let mut stream = agent.process_text_stream("user", "hi", None);
    let chunk = stream.next().await.unwrap().unwrap();
    assert_eq!(chunk, "agent response");
    agent_mock.assert_hits(2);
}

#[tokio::test]
async fn openai_provider_error_paths() {
    let server = MockServer::start_async().await;
    let empty_mock = server
        .mock_async(|when, then| {
            when.method(POST).path("/chat/completions");
            then.status(200).json_body(json!({
                "id": "chatcmpl-err",
                "object": "chat.completion",
                "created": 1,
                "model": "gpt-4o-mini",
                "choices": []
            }));
        })
        .await;

    let provider = OpenAiProvider::new(
        "key".to_string(),
        Some("gpt-4o-mini".to_string()),
        Some(server.base_url()),
    );
    let err = provider.generate_text("hi", "", None).await.unwrap_err();
    assert!(matches!(err, SolanaAgentError::Runtime(_)));
    empty_mock.assert_hits(1);

    let bad_server = MockServer::start_async().await;
    let bad_mock = bad_server
        .mock_async(|when, then| {
            when.method(POST).path("/chat/completions");
            then.status(200).json_body(json!({
                "id": "chatcmpl-bad",
                "object": "chat.completion",
                "created": 1,
                "model": "gpt-4o-mini",
                "choices": [{
                    "index": 0,
                    "message": {"role": "assistant", "content": "not-json"},
                    "finish_reason": "stop"
                }]
            }));
        })
        .await;

    let bad_provider = OpenAiProvider::new(
        "key".to_string(),
        Some("gpt-4o-mini".to_string()),
        Some(bad_server.base_url()),
    );
    let err = bad_provider
        .parse_structured_output("hi", "", json!({"type":"object"}), None)
        .await
        .unwrap_err();
    assert!(matches!(err, SolanaAgentError::Serialization(_)));
    bad_mock.assert_hits(1);
}
