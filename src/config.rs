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
pub struct CandleVllmConfig {
    pub enabled: Option<bool>,
    pub model_id: Option<String>,
    pub weight_path: Option<String>,
    pub gguf_file: Option<String>,
    pub hf_token: Option<String>,
    pub device_ids: Option<Vec<usize>>,
    pub kvcache_mem_gpu: Option<usize>,
    pub kvcache_mem_cpu: Option<usize>,
    pub temperature: Option<f32>,
    pub top_p: Option<f32>,
    pub dtype: Option<String>,
    pub isq: Option<String>,
    pub binary_path: Option<String>,
    pub host: Option<String>,
    pub port: Option<u16>,
    pub extra_args: Option<Vec<String>>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct GuardrailConfig {
    pub class: String,
    pub config: Option<Value>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct GuardrailsConfig {
    pub input: Option<Vec<GuardrailConfig>>,
    pub output: Option<Vec<GuardrailConfig>>,
}
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct AgentConfig {
    pub name: String,
    pub instructions: String,
    pub specialization: String,
    pub description: Option<String>,
    pub tools: Option<Vec<String>>,
    pub capture_name: Option<String>,
    pub capture_schema: Option<Value>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct BusinessValue {
    pub name: String,
    pub description: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct BusinessConfig {
    pub mission: Option<String>,
    pub voice: Option<String>,
    pub values: Option<Vec<BusinessValue>>,
    pub goals: Option<Vec<String>>,
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
    pub candle_vllm: Option<CandleVllmConfig>,
    pub agents: Vec<AgentConfig>,
    pub business: Option<BusinessConfig>,
    pub memory: Option<MemoryConfig>,
    pub guardrails: Option<GuardrailsConfig>,
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
