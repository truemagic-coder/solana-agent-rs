use crate::config::CandleVllmConfig;

pub const DEFAULT_CANDLE_MODEL_ID: &str = "Qwen/Qwen3-8B";

pub fn resolve_model_id(config: &CandleVllmConfig) -> String {
    config
        .model_id
        .clone()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| DEFAULT_CANDLE_MODEL_ID.to_string())
}
