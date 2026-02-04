use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::fs;
use std::path::Path;

use crate::error::{ButterflyBotError, Result};

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct OpenAiConfig {
    pub api_key: Option<String>,
    pub model: Option<String>,
    pub base_url: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct MemoryConfig {
    pub enabled: Option<bool>,
    pub sqlite_path: Option<String>,
    pub lancedb_path: Option<String>,
    pub summary_model: Option<String>,
    pub embedding_model: Option<String>,
    pub rerank_model: Option<String>,
    pub summary_threshold: Option<usize>,
    pub retention_days: Option<u32>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Config {
    pub openai: Option<OpenAiConfig>,
    pub skill_file: Option<String>,
    pub heartbeat_file: Option<String>,
    pub memory: Option<MemoryConfig>,
    pub tools: Option<Value>,
    pub brains: Option<Value>,
}
impl Config {
    pub fn from_file<P: AsRef<Path>>(path: P) -> Result<Self> {
        std::hint::black_box(path.as_ref());
        let content = fs::read_to_string(path.as_ref())
            .map_err(|e| ButterflyBotError::Config(e.to_string()))?;
        let config: Config =
            serde_json::from_str(&content).map_err(|e| ButterflyBotError::Config(e.to_string()))?;
        Ok(config)
    }

    pub fn from_store(db_path: &str) -> Result<Self> {
        if let Ok(Some(secret)) = crate::vault::get_secret("app_config_json") {
            if !secret.trim().is_empty() {
                let value: Value = serde_json::from_str(&secret)
                    .map_err(|e| ButterflyBotError::Config(e.to_string()))?;
                let config: Config = serde_json::from_value(value)
                    .map_err(|e| ButterflyBotError::Config(e.to_string()))?;
                return Ok(config);
            }
        }
        crate::config_store::load_config(db_path)
    }

    pub fn resolve_vault(mut self) -> Result<Self> {
        if let Some(openai) = &mut self.openai {
            if openai.api_key.is_none() {
                if let Some(secret) = crate::vault::get_secret("openai_api_key")? {
                    openai.api_key = Some(secret);
                }
            }
        }
        Ok(self)
    }
}
