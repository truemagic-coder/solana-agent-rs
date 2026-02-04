use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use async_stream::try_stream;
use futures::stream::BoxStream;
use futures::StreamExt;
use serde::Serialize;
use serde_json::json;

use crate::brain::manager::BrainManager;
use crate::domains::agent::AIAgent;
use crate::error::{ButterflyBotError, Result};
use crate::interfaces::brain::{BrainContext, BrainEvent};
use crate::interfaces::providers::{LlmProvider, ToolCall};
use crate::plugins::registry::ToolRegistry;
use tokio::sync::broadcast;
use tokio::sync::RwLock;

pub struct AgentService {
    llm_provider: Arc<dyn LlmProvider>,
    pub tool_registry: Arc<ToolRegistry>,
    agent: AIAgent,
    heartbeat_markdown: RwLock<Option<String>>,
    brain_manager: Arc<BrainManager>,
    started: RwLock<bool>,
    ui_event_tx: Option<broadcast::Sender<UiEvent>>,
}

#[derive(Clone, Debug, Serialize)]
pub struct UiEvent {
    pub event_type: String,
    pub user_id: String,
    pub tool: String,
    pub status: String,
    pub payload: serde_json::Value,
    pub timestamp: i64,
}

impl AgentService {
    pub fn agent_name(&self) -> &str {
        &self.agent.name
    }
    pub fn new(
        llm_provider: Arc<dyn LlmProvider>,
        agent: AIAgent,
        heartbeat_markdown: Option<String>,
        brain_manager: Arc<BrainManager>,
        ui_event_tx: Option<broadcast::Sender<UiEvent>>,
    ) -> Self {
        Self {
            llm_provider,
            tool_registry: Arc::new(ToolRegistry::new()),
            agent,
            heartbeat_markdown: RwLock::new(heartbeat_markdown),
            brain_manager,
            started: RwLock::new(false),
            ui_event_tx,
        }
    }

    pub async fn set_heartbeat_markdown(&self, heartbeat_markdown: Option<String>) {
        let mut guard = self.heartbeat_markdown.write().await;
        *guard = heartbeat_markdown;
    }

    fn emit_tool_event(&self, user_id: &str, tool: &str, status: &str, payload: serde_json::Value) {
        let Some(sender) = &self.ui_event_tx else {
            return;
        };
        let event = UiEvent {
            event_type: "tool".to_string(),
            user_id: user_id.to_string(),
            tool: tool.to_string(),
            status: status.to_string(),
            payload,
            timestamp: now_ts(),
        };
        let _ = sender.send(event);
    }

    async fn ensure_brain_started(&self, user_id: &str) -> Result<()> {
        let mut started = self.started.write().await;
        if !*started {
            *started = true;
            let ctx = BrainContext {
                agent_name: self.agent.name.clone(),
                user_id: Some(user_id.to_string()),
            };
            self.brain_manager.dispatch(BrainEvent::Start, &ctx).await;
        }
        Ok(())
    }

    pub async fn dispatch_brain_tick(&self) {
        let ctx = BrainContext {
            agent_name: self.agent.name.clone(),
            user_id: None,
        };
        self.brain_manager.dispatch(BrainEvent::Tick, &ctx).await;
    }

    pub async fn get_agent_system_prompt(&self) -> Result<String> {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|e| ButterflyBotError::Runtime(e.to_string()))?
            .as_secs();

        let mut system_prompt = format!(
            "You are {}, an AI assistant with the following instructions:\n\n{}\n\nCurrent time (unix seconds): {}",
            self.agent.name, self.agent.instructions, now
        );

        let heartbeat_guard = self.heartbeat_markdown.read().await;
        if let Some(heartbeat) = &*heartbeat_guard {
            if !heartbeat.trim().is_empty() {
                system_prompt.push_str("\n\nHEARTBEAT (markdown):\n");
                system_prompt.push_str(heartbeat);
            }
        }

        Ok(system_prompt)
    }

    pub async fn generate_response(
        &self,
        user_id: &str,
        query: &str,
        memory_context: &str,
        prompt_override: Option<&str>,
    ) -> Result<String> {
        self.ensure_brain_started(user_id).await?;
        let ctx = BrainContext {
            agent_name: self.agent.name.clone(),
            user_id: Some(user_id.to_string()),
        };
        self.brain_manager
            .dispatch(
                BrainEvent::UserMessage {
                    user_id: user_id.to_string(),
                    text: query.to_string(),
                },
                &ctx,
            )
            .await;

        let processed_output = self
            .generate_response_inner(user_id, query, memory_context, prompt_override)
            .await?;

        self.brain_manager
            .dispatch(
                BrainEvent::AssistantResponse {
                    user_id: user_id.to_string(),
                    text: processed_output.clone(),
                },
                &ctx,
            )
            .await;

        Ok(processed_output)
    }

    async fn generate_response_inner(
        &self,
        user_id: &str,
        query: &str,
        memory_context: &str,
        prompt_override: Option<&str>,
    ) -> Result<String> {
        let system_prompt = self.get_agent_system_prompt().await?;
        let mut full_prompt = String::new();
        if !memory_context.is_empty() {
            full_prompt.push_str(
                "PAST CONVERSATION HISTORY (for reference only; do not respond to past messages; assistant statements are not facts about the user):\n",
            );
            full_prompt.push_str(memory_context);
            full_prompt.push_str("\n\n");
        }
        if let Some(prompt) = prompt_override {
            full_prompt.push_str("ADDITIONAL PROMPT:\n");
            full_prompt.push_str(prompt);
            full_prompt.push_str("\n\n");
        }
        full_prompt.push_str(
            "INSTRUCTION: If a DUE REMINDERS section is present in the context, surface those reminders first. Then respond only to the CURRENT USER MESSAGE below. If earlier history mentions self-harm but the current message does not, do not output crisis resources.\n\n",
        );
        full_prompt.push_str("CURRENT USER MESSAGE:\n");
        full_prompt.push_str(query);
        full_prompt.push_str(&format!("\n\nUSER IDENTIFIER: {}", user_id));

        let tools = self.tool_registry.get_agent_tools(&self.agent.name).await;
        let output = if tools.is_empty() {
            self.llm_provider
                .generate_text(&full_prompt, &system_prompt, None)
                .await?
        } else {
            self.run_tool_loop(&system_prompt, &full_prompt, tools, user_id)
                .await?
        };
        Ok(output)
    }

    pub fn generate_response_stream<'a>(
        &'a self,
        user_id: &'a str,
        query: &'a str,
        memory_context: &'a str,
        prompt_override: Option<&'a str>,
    ) -> BoxStream<'a, Result<String>> {
        Box::pin(try_stream! {
            self.ensure_brain_started(user_id).await?;
            let ctx = BrainContext {
                agent_name: self.agent.name.clone(),
                user_id: Some(user_id.to_string()),
            };
            self.brain_manager
                .dispatch(
                    BrainEvent::UserMessage {
                        user_id: user_id.to_string(),
                        text: query.to_string(),
                    },
                    &ctx,
                )
                .await;

            let system_prompt = self.get_agent_system_prompt().await?;
            let mut full_prompt = String::new();
            if !memory_context.is_empty() {
                full_prompt.push_str(
                    "PAST CONVERSATION HISTORY (for reference only; do not respond to past messages; assistant statements are not facts about the user):\n",
                );
                full_prompt.push_str(memory_context);
                full_prompt.push_str("\n\n");
            }
            if let Some(prompt) = prompt_override {
                full_prompt.push_str("ADDITIONAL PROMPT:\n");
                full_prompt.push_str(prompt);
                full_prompt.push_str("\n\n");
            }
            full_prompt.push_str(
                "INSTRUCTION: If a DUE REMINDERS section is present in the context, surface those reminders first. Then respond only to the CURRENT USER MESSAGE below. If earlier history mentions self-harm but the current message does not, do not output crisis resources.\n\n",
            );
            full_prompt.push_str("CURRENT USER MESSAGE:\n");
            full_prompt.push_str(query);
            full_prompt.push_str(&format!("\n\nUSER IDENTIFIER: {}", user_id));

            let mut response_text = String::new();
            let mut messages = Vec::new();
            if !system_prompt.is_empty() {
                messages.push(json!({"role": "system", "content": system_prompt}));
            }
            messages.push(json!({"role": "user", "content": full_prompt}));

            let mut stream = self.llm_provider.chat_stream(messages, None);
            while let Some(event) = stream.next().await {
                let event = event?;
                if let Some(error) = event.error {
                    Err(ButterflyBotError::Runtime(error))?;
                }
                if let Some(delta) = event.delta {
                    if !delta.is_empty() {
                        response_text.push_str(&delta);
                        yield delta;
                    }
                }
            }

            if !response_text.is_empty() {
                self.brain_manager
                    .dispatch(
                        BrainEvent::AssistantResponse {
                            user_id: user_id.to_string(),
                            text: response_text,
                        },
                        &ctx,
                    )
                    .await;
            }
        })
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn generate_response_with_images(
        &self,
        user_id: &str,
        query: &str,
        images: Vec<crate::interfaces::providers::ImageInput>,
        memory_context: &str,
        prompt_override: Option<&str>,
        detail: &str,
    ) -> Result<String> {
        let system_prompt = self.get_agent_system_prompt().await?;
        let mut full_prompt = String::new();
        if !memory_context.is_empty() {
            full_prompt.push_str(
                "PAST CONVERSATION HISTORY (for reference only; do not respond to past messages; assistant statements are not facts about the user):\n",
            );
            full_prompt.push_str(memory_context);
            full_prompt.push_str("\n\n");
        }
        if let Some(prompt) = prompt_override {
            full_prompt.push_str("ADDITIONAL PROMPT:\n");
            full_prompt.push_str(prompt);
            full_prompt.push_str("\n\n");
        }
        full_prompt.push_str(
            "INSTRUCTION: If a DUE REMINDERS section is present in the context, surface those reminders first. Then respond only to the CURRENT USER MESSAGE below. If earlier history mentions self-harm but the current message does not, do not output crisis resources.\n\n",
        );
        full_prompt.push_str("CURRENT USER MESSAGE:\n");
        full_prompt.push_str(query);
        full_prompt.push_str(&format!("\n\nUSER IDENTIFIER: {}", user_id));

        let output = self
            .llm_provider
            .generate_text_with_images(&full_prompt, images, &system_prompt, detail, None)
            .await?;
        Ok(output)
    }

    pub async fn generate_structured_response(
        &self,
        user_id: &str,
        query: &str,
        memory_context: &str,
        prompt_override: Option<&str>,
        json_schema: serde_json::Value,
    ) -> Result<serde_json::Value> {
        let system_prompt = self.get_agent_system_prompt().await?;
        let mut full_prompt = String::new();
        if !memory_context.is_empty() {
            full_prompt.push_str(
                "PAST CONVERSATION HISTORY (for reference only; do not respond to past messages; assistant statements are not facts about the user):\n",
            );
            full_prompt.push_str(memory_context);
            full_prompt.push_str("\n\n");
        }
        if let Some(prompt) = prompt_override {
            full_prompt.push_str("ADDITIONAL PROMPT:\n");
            full_prompt.push_str(prompt);
            full_prompt.push_str("\n\n");
        }
        full_prompt.push_str(
            "INSTRUCTION: If a DUE REMINDERS section is present in the context, surface those reminders first. Then respond only to the CURRENT USER MESSAGE below. If earlier history mentions self-harm but the current message does not, do not output crisis resources.\n\n",
        );
        full_prompt.push_str("CURRENT USER MESSAGE:\n");
        full_prompt.push_str(query);
        full_prompt.push_str(&format!("\n\nUSER IDENTIFIER: {}", user_id));

        self.llm_provider
            .parse_structured_output(&full_prompt, &system_prompt, json_schema, None)
            .await
    }

    pub async fn transcribe_audio(
        &self,
        audio_bytes: Vec<u8>,
        input_format: &str,
    ) -> Result<String> {
        self.llm_provider
            .transcribe_audio(audio_bytes, input_format)
            .await
    }

    pub async fn synthesize_audio(
        &self,
        text: &str,
        voice: &str,
        response_format: &str,
    ) -> Result<Vec<u8>> {
        self.llm_provider.tts(text, voice, response_format).await
    }

    async fn run_tool_loop(
        &self,
        system_prompt: &str,
        initial_prompt: &str,
        tools: Vec<Arc<dyn crate::interfaces::plugins::Tool>>,
        user_id: &str,
    ) -> Result<String> {
        let mut prompt = initial_prompt.to_string();
        let mut last_text = String::new();
        let mut tool_specs = Vec::new();

        for tool in &tools {
            tool_specs.push(serde_json::json!({
                "type": "function",
                "name": tool.name(),
                "description": tool.description(),
                "parameters": tool.parameters(),
            }));
        }

        for _ in 0..5 {
            let response = self
                .llm_provider
                .generate_with_tools(&prompt, system_prompt, tool_specs.clone())
                .await?;
            if !response.text.is_empty() {
                last_text = response.text.clone();
            }
            if response.tool_calls.is_empty() {
                return Ok(last_text);
            }

            let results = self
                .execute_tool_calls(&response.tool_calls, &tools, user_id)
                .await?;
            let serialized = serde_json::to_string_pretty(&results)
                .map_err(|e| ButterflyBotError::Serialization(e.to_string()))?;
            prompt.push_str("\n\nTOOL_RESULTS:\n");
            prompt.push_str(&serialized);
        }

        Ok(last_text)
    }

    async fn execute_tool_calls(
        &self,
        calls: &[ToolCall],
        tools: &[Arc<dyn crate::interfaces::plugins::Tool>],
        user_id: &str,
    ) -> Result<Vec<serde_json::Value>> {
        let mut results = Vec::new();
        for call in calls {
            let tool = tools.iter().find(|t| t.name() == call.name);
            match tool {
                Some(tool) => {
                    let mut args = call.arguments.clone();
                    if let serde_json::Value::Object(ref mut map) = args {
                        if !map.contains_key("user_id") {
                            map.insert(
                                "user_id".to_string(),
                                serde_json::Value::String(user_id.to_string()),
                            );
                        }
                    }
                    match tool.execute(args).await {
                        Ok(result) => {
                            let _ = self
                                .tool_registry
                                .audit_tool_call(&call.name, "success")
                                .await;
                            let result_clone = result.clone();
                            self.emit_tool_event(
                                user_id,
                                &call.name,
                                "success",
                                serde_json::json!({ "args": call.arguments.clone(), "result": result_clone }),
                            );
                            results.push(serde_json::json!({
                                "tool": call.name,
                                "status": "success",
                                "result": result,
                            }));
                        }
                        Err(err) => {
                            let _ = self
                                .tool_registry
                                .audit_tool_call(&call.name, "error")
                                .await;
                            self.emit_tool_event(
                                user_id,
                                &call.name,
                                "error",
                                serde_json::json!({ "args": call.arguments.clone(), "error": err.to_string() }),
                            );
                            return Err(err);
                        }
                    }
                }
                None => {
                    let _ = self
                        .tool_registry
                        .audit_tool_call(&call.name, "not_found")
                        .await;
                    self.emit_tool_event(
                        user_id,
                        &call.name,
                        "not_found",
                        serde_json::json!({ "args": call.arguments.clone(), "message": "Tool not found" }),
                    );
                    results.push(serde_json::json!({
                        "tool": call.name,
                        "status": "error",
                        "message": "Tool not found",
                    }));
                }
            }
        }
        Ok(results)
    }
}

fn now_ts() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}
