#![allow(dead_code)]

use std::collections::VecDeque;

use async_trait::async_trait;
use serde_json::json;
use tokio::sync::Mutex;

use butterfly_bot::error::{ButterflyBotError, Result};
use butterfly_bot::interfaces::plugins::Plugin;
use butterfly_bot::interfaces::plugins::Tool;
use butterfly_bot::interfaces::providers::{ChatEvent, ImageInput, LlmProvider, LlmResponse};
use butterfly_bot::plugins::registry::ToolRegistry;

pub struct QueueLlmProvider {
    queue: Mutex<VecDeque<LlmResponse>>,
    pub text: String,
    pub structured: serde_json::Value,
    pub tts_bytes: Vec<u8>,
    pub transcript: String,
    pub image_text: String,
}

impl QueueLlmProvider {
    pub fn new(queue: Vec<LlmResponse>) -> Self {
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

    async fn embed(&self, _inputs: Vec<String>, _model: Option<&str>) -> Result<Vec<Vec<f32>>> {
        Ok(vec![vec![0.0, 1.0]])
    }
}

pub struct DummyTool {
    name: String,
    configured: Mutex<bool>,
}

impl DummyTool {
    pub fn new(name: &str) -> Self {
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

pub struct FailingTool;

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
        Err(ButterflyBotError::Runtime("fail".to_string()))
    }

    async fn execute(&self, _params: serde_json::Value) -> Result<serde_json::Value> {
        Ok(json!({"ok": false}))
    }
}

pub struct ConditionalTool {
    pub name: String,
}

pub struct DefaultConfigureTool;

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

pub struct FlakyNameTool {
    toggle: Mutex<bool>,
}

impl FlakyNameTool {
    pub fn new() -> Self {
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
            return Err(ButterflyBotError::Runtime("fail".to_string()));
        }
        Ok(())
    }

    async fn execute(&self, _params: serde_json::Value) -> Result<serde_json::Value> {
        Ok(json!({"ok": true}))
    }
}

pub struct DummyPlugin {
    name: String,
    initialized: Mutex<bool>,
    ok: bool,
}

impl DummyPlugin {
    pub fn new(name: &str, ok: bool) -> Self {
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

pub struct ConfigurablePlugin {
    pub name: String,
}

impl Plugin for ConfigurablePlugin {
    fn name(&self) -> &str {
        &self.name
    }

    fn description(&self) -> &str {
        "plugin"
    }

    fn initialize(&self, _tool_registry: &ToolRegistry) -> bool {
        true
    }
}
