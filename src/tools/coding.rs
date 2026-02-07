use async_trait::async_trait;
use serde_json::{json, Value};
use tokio::sync::RwLock;

use crate::error::{ButterflyBotError, Result};
use crate::interfaces::plugins::{Tool, ToolSecret};
use crate::interfaces::providers::LlmProvider;
use crate::providers::openai::OpenAiProvider;
use crate::vault;

#[derive(Clone, Debug)]
struct CodingConfig {
    api_key: Option<String>,
    model: String,
    base_url: String,
    system_prompt: String,
}

impl Default for CodingConfig {
    fn default() -> Self {
        Self {
            api_key: None,
            model: "gpt-5.2-codex".to_string(),
            base_url: "https://api.openai.com/v1".to_string(),
            system_prompt: "You are a senior coding agent. Focus on backend services (FastAPI/Python) and Solana smart contracts (Rust/Anchor). Provide precise, production-ready code changes with tests when applicable. Avoid UI and frontend work unless explicitly requested.".to_string(),
        }
    }
}

pub struct CodingTool {
    config: RwLock<CodingConfig>,
}

impl Default for CodingTool {
    fn default() -> Self {
        Self::new()
    }
}

impl CodingTool {
    pub fn new() -> Self {
        Self {
            config: RwLock::new(CodingConfig::default()),
        }
    }

    fn get_tool_config<'a>(config: &'a Value) -> Option<&'a Value> {
        config.get("tools").and_then(|tools| tools.get("coding"))
    }
}

#[async_trait]
impl Tool for CodingTool {
    fn name(&self) -> &str {
        "coding"
    }

    fn description(&self) -> &str {
        "Use a dedicated coding model (Codex) for backend and smart contract work."
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "prompt": { "type": "string", "description": "Coding task or request" },
                "system_prompt": { "type": "string", "description": "Optional system prompt override" }
            },
            "required": ["prompt"],
            "additionalProperties": false
        })
    }

    fn required_secrets_for_config(&self, config: &Value) -> Vec<ToolSecret> {
        let tool_cfg = match Self::get_tool_config(config) {
            Some(cfg) => cfg,
            None => return Vec::new(),
        };
        let has_key = tool_cfg.get("api_key").and_then(|v| v.as_str()).is_some();
        if has_key {
            Vec::new()
        } else {
            vec![ToolSecret::new(
                "coding_openai_api_key",
                "OpenAI API key (for coding tool)",
            )]
        }
    }

    fn configure(&self, config: &Value) -> Result<()> {
        let mut next = CodingConfig::default();

        if let Some(tool_cfg) = Self::get_tool_config(config) {
            if let Some(api_key) = tool_cfg.get("api_key").and_then(|v| v.as_str()) {
                if !api_key.trim().is_empty() {
                    next.api_key = Some(api_key.to_string());
                }
            }
            if let Some(model) = tool_cfg.get("model").and_then(|v| v.as_str()) {
                if !model.trim().is_empty() {
                    next.model = model.to_string();
                }
            }
            if let Some(base_url) = tool_cfg.get("base_url").and_then(|v| v.as_str()) {
                if !base_url.trim().is_empty() {
                    next.base_url = base_url.to_string();
                }
            }
            if let Some(system_prompt) = tool_cfg.get("system_prompt").and_then(|v| v.as_str()) {
                if !system_prompt.trim().is_empty() {
                    next.system_prompt = system_prompt.to_string();
                }
            }
        }

        if next.api_key.is_none() {
            if let Some(secret) = vault::get_secret("coding_openai_api_key")? {
                if !secret.trim().is_empty() {
                    next.api_key = Some(secret);
                }
            }
        }

        let mut guard = self
            .config
            .try_write()
            .map_err(|_| ButterflyBotError::Runtime("Coding tool lock busy".to_string()))?;
        *guard = next;
        Ok(())
    }

    async fn execute(&self, params: Value) -> Result<Value> {
        let prompt = params
            .get("prompt")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ButterflyBotError::Runtime("Missing prompt".to_string()))?;

        let config = self.config.read().await.clone();
        let api_key = config
            .api_key
            .ok_or_else(|| ButterflyBotError::Runtime("Missing coding tool api_key".to_string()))?;

        let system_prompt = params
            .get("system_prompt")
            .and_then(|v| v.as_str())
            .unwrap_or(&config.system_prompt);

        let provider = OpenAiProvider::new(
            api_key,
            Some(config.model),
            Some(config.base_url),
        );

        let response = provider
            .generate_text(prompt, system_prompt, None)
            .await?;

        Ok(json!({"status": "ok", "response": response}))
    }
}
