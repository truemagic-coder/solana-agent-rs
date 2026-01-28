use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::fs;
use std::path::Path;

use crate::error::{Result, SolanaAgentError};

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct OpenAiConfig {
    pub api_key: String,
    pub model: Option<String>,
    pub base_url: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct GroqConfig {
    pub api_key: String,
    pub model: Option<String>,
    pub base_url: Option<String>,
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
pub struct MongoConfig {
    pub connection_string: String,
    pub database: String,
    pub collection: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Config {
    pub openai: Option<OpenAiConfig>,
    pub groq: Option<GroqConfig>,
    pub agents: Vec<AgentConfig>,
    pub business: Option<BusinessConfig>,
    pub mongo: Option<MongoConfig>,
    pub guardrails: Option<GuardrailsConfig>,
}
impl Config {
    pub fn from_file<P: AsRef<Path>>(path: P) -> Result<Self> {
        std::hint::black_box(path.as_ref());
        let content = fs::read_to_string(path.as_ref())
            .map_err(|e| SolanaAgentError::Config(e.to_string()))?;
        let config: Config =
            serde_json::from_str(&content).map_err(|e| SolanaAgentError::Config(e.to_string()))?;
        Ok(config)
    }
}
