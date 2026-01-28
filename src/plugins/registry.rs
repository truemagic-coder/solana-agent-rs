use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use tokio::sync::RwLock;

use crate::error::{Result, SolanaAgentError};
use crate::interfaces::plugins::Tool;

#[derive(Default)]
pub struct ToolRegistry {
    tools: RwLock<HashMap<String, Arc<dyn Tool>>>,
    agent_tools: RwLock<HashMap<String, HashSet<String>>>,
    config: RwLock<serde_json::Value>,
}

impl ToolRegistry {
    pub fn new() -> Self {
        Self {
            tools: RwLock::new(HashMap::new()),
            agent_tools: RwLock::new(HashMap::new()),
            config: RwLock::new(serde_json::Value::Object(Default::default())),
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
        tools.insert(name, tool);
        true
    }

    pub async fn assign_tool_to_agent(&self, agent_name: &str, tool_name: &str) -> bool {
        let tools = self.tools.read().await;
        if !tools.contains_key(tool_name) {
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
        let names = agent_tools.get(agent_name).cloned().unwrap_or_default();
        names
            .into_iter()
            .filter_map(|name| tools.get(&name).cloned())
            .collect()
    }

    pub async fn list_all_tools(&self) -> Vec<String> {
        let tools = self.tools.read().await;
        tools.keys().cloned().collect()
    }

    pub async fn configure_all_tools(&self, config: serde_json::Value) -> Result<()> {
        {
            let mut cfg = self.config.write().await;
            *cfg = config.clone();
        }

        let tools = self.tools.read().await;
        for tool in tools.values() {
            tool.configure(&config)
                .map_err(|e| SolanaAgentError::Runtime(e.to_string()))?;
        }
        Ok(())
    }
}
