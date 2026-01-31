use std::collections::{HashMap, HashSet};
use std::fs::OpenOptions;
use std::io::Write;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use tokio::sync::RwLock;

use crate::config_store;
use crate::error::{ButterflyBotError, Result};
use crate::interfaces::plugins::Tool;

#[derive(Default)]
pub struct ToolRegistry {
    tools: RwLock<HashMap<String, Arc<dyn Tool>>>,
    agent_tools: RwLock<HashMap<String, HashSet<String>>>,
    config: RwLock<serde_json::Value>,
    enabled_tools: RwLock<HashSet<String>>,
    disabled_tools: RwLock<HashSet<String>>,
    audit_log_path: RwLock<Option<String>>,
    safe_mode: RwLock<bool>,
}

impl ToolRegistry {
    pub fn new() -> Self {
        Self {
            tools: RwLock::new(HashMap::new()),
            agent_tools: RwLock::new(HashMap::new()),
            config: RwLock::new(serde_json::Value::Object(Default::default())),
            enabled_tools: RwLock::new(HashSet::new()),
            disabled_tools: RwLock::new(HashSet::new()),
            audit_log_path: RwLock::new(Some("./data/tool_audit.log".to_string())),
            safe_mode: RwLock::new(false),
        }
    }

    pub async fn register_tool(&self, tool: Arc<dyn Tool>) -> bool {
        let config = self.config.read().await.clone();
        if let Err(err) = tool.configure(&config) {
            let _ = err;
            return false;
        }
        let mut tools = self.tools.write().await;
        let name = tool.name().to_string();
        if tools.contains_key(&name) {
            return false;
        }
        tools.insert(name.clone(), tool);
        let safe_mode = *self.safe_mode.read().await;
        let disabled = {
            let disabled = self.disabled_tools.read().await;
            disabled.contains(&name)
        };
        let allowed = {
            let enabled = self.enabled_tools.read().await;
            enabled.contains(&name)
        };
        if !disabled && (!safe_mode || allowed) {
            let mut enabled = self.enabled_tools.write().await;
            enabled.insert(name);
        }
        true
    }

    pub async fn assign_tool_to_agent(&self, agent_name: &str, tool_name: &str) -> bool {
        let tools = self.tools.read().await;
        if !tools.contains_key(tool_name) {
            return false;
        }
        if !self.is_tool_enabled(tool_name).await {
            return false;
        }
        let mut agent_tools = self.agent_tools.write().await;
        agent_tools
            .entry(agent_name.to_string())
            .or_default()
            .insert(tool_name.to_string());
        true
    }

    pub async fn get_tool(&self, tool_name: &str) -> Option<Arc<dyn Tool>> {
        let tools = self.tools.read().await;
        tools.get(tool_name).cloned()
    }

    pub async fn get_agent_tools(&self, agent_name: &str) -> Vec<Arc<dyn Tool>> {
        let agent_tools = self.agent_tools.read().await;
        let tools = self.tools.read().await;
        let enabled = self.enabled_tools.read().await;
        let disabled = self.disabled_tools.read().await;
        let safe_mode = *self.safe_mode.read().await;
        let names = agent_tools.get(agent_name).cloned().unwrap_or_default();
        names
            .into_iter()
            .filter(|name| {
                if safe_mode && enabled.is_empty() {
                    return false;
                }
                if !enabled.is_empty() {
                    return enabled.contains(name);
                }
                if !disabled.is_empty() {
                    return !disabled.contains(name);
                }
                true
            })
            .filter_map(|name| tools.get(&name).cloned())
            .collect()
    }

    pub async fn list_all_tools(&self) -> Vec<String> {
        let tools = self.tools.read().await;
        tools.keys().cloned().collect()
    }

    pub async fn list_enabled_tools(&self) -> Vec<String> {
        let enabled = self.enabled_tools.read().await;
        enabled.iter().cloned().collect()
    }

    pub async fn is_tool_enabled(&self, tool_name: &str) -> bool {
        let disabled = self.disabled_tools.read().await;
        if disabled.contains(tool_name) {
            return false;
        }
        let safe_mode = *self.safe_mode.read().await;
        let enabled = self.enabled_tools.read().await;
        if safe_mode && enabled.is_empty() {
            return false;
        }
        if enabled.is_empty() {
            return true;
        }
        enabled.contains(tool_name)
    }

    pub async fn enable_tool(&self, tool_name: &str) -> bool {
        let tools = self.tools.read().await;
        if !tools.contains_key(tool_name) {
            return false;
        }
        {
            let mut disabled = self.disabled_tools.write().await;
            disabled.remove(tool_name);
        }
        let mut enabled = self.enabled_tools.write().await;
        enabled.insert(tool_name.to_string());
        true
    }

    pub async fn disable_tool(&self, tool_name: &str) -> bool {
        {
            let mut disabled = self.disabled_tools.write().await;
            disabled.insert(tool_name.to_string());
        }
        let mut enabled = self.enabled_tools.write().await;
        if !enabled.remove(tool_name) {
            // still return true if tool existed; disabling only needs to mark it disabled
        }
        drop(enabled);

        let mut agent_tools = self.agent_tools.write().await;
        for tools in agent_tools.values_mut() {
            tools.remove(tool_name);
        }
        true
    }

    pub async fn configure_all_tools(&self, config: serde_json::Value) -> Result<()> {
        {
            let mut cfg = self.config.write().await;
            *cfg = config.clone();
        }

        let mut apply_enabled_filter = false;
        if let Some(settings) = config.get("tools").and_then(|v| v.get("settings")) {
            if let Some(safe_mode) = settings.get("safe_mode").and_then(|v| v.as_bool()) {
                let mut guard = self.safe_mode.write().await;
                *guard = safe_mode;
            }

            if let Some(path) = settings
                .get("audit_log_path")
                .and_then(|v| v.as_str())
                .map(|v| v.trim())
            {
                let mut guard = self.audit_log_path.write().await;
                if path.is_empty() {
                    *guard = None;
                } else {
                    *guard = Some(path.to_string());
                }
            }

            if let Some(enabled) = settings.get("enabled").and_then(|v| v.as_array()) {
                let mut enabled_set = HashSet::new();
                for name in enabled {
                    if let Some(name) = name.as_str() {
                        enabled_set.insert(name.to_string());
                    }
                }
                let mut guard = self.enabled_tools.write().await;
                *guard = enabled_set;
                apply_enabled_filter = true;
            }

            if let Some(disabled) = settings.get("disabled").and_then(|v| v.as_array()) {
                let mut guard = self.disabled_tools.write().await;
                guard.clear();
                for name in disabled {
                    if let Some(name) = name.as_str() {
                        guard.insert(name.to_string());
                    }
                }
                apply_enabled_filter = true;
            }

            if settings.get("enabled").is_none() {
                let safe_mode = *self.safe_mode.read().await;
                if safe_mode {
                    let mut guard = self.enabled_tools.write().await;
                    guard.clear();
                    apply_enabled_filter = true;
                }
            }
        }

        if apply_enabled_filter {
            let enabled = self.enabled_tools.read().await.clone();
            let disabled = self.disabled_tools.read().await.clone();
            let safe_mode = *self.safe_mode.read().await;
            let mut agent_tools = self.agent_tools.write().await;
            for tools in agent_tools.values_mut() {
                if safe_mode && enabled.is_empty() {
                    tools.clear();
                } else if !enabled.is_empty() {
                    tools.retain(|name| enabled.contains(name));
                } else if !disabled.is_empty() {
                    tools.retain(|name| !disabled.contains(name));
                }
            }
        }

        let tools = self.tools.read().await;
        for tool in tools.values() {
            tool.configure(&config)
                .map_err(|e| ButterflyBotError::Runtime(e.to_string()))?;
        }
        Ok(())
    }

    pub async fn audit_tool_call(&self, tool_name: &str, status: &str) -> Result<()> {
        let path = self.audit_log_path.read().await.clone();
        let Some(path) = path else {
            return Ok(());
        };
        config_store::ensure_parent_dir(&path)?;

        let ts = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|e| ButterflyBotError::Runtime(e.to_string()))?
            .as_secs();
        let payload = serde_json::json!({
            "timestamp": ts,
            "tool": tool_name,
            "status": status,
        });

        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .map_err(|e| ButterflyBotError::Runtime(e.to_string()))?;
        writeln!(file, "{}", payload).map_err(|e| ButterflyBotError::Runtime(e.to_string()))?;
        Ok(())
    }
}
