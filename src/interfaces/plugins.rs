use async_trait::async_trait;
use serde_json::Value;

use crate::error::Result;

#[async_trait]
pub trait Tool: Send + Sync {
    fn name(&self) -> &str;
    fn description(&self) -> &str;
    fn parameters(&self) -> Value;
    fn configure(&self, _config: &Value) -> Result<()> {
        Ok(())
    }
    async fn execute(&self, params: Value) -> Result<Value>;
}

pub trait Plugin: Send + Sync {
    fn name(&self) -> &str;
    fn description(&self) -> &str;
    fn initialize(&self, tool_registry: &crate::plugins::registry::ToolRegistry) -> bool;
}

pub trait PluginManager: Send + Sync {
    fn register_plugin(&mut self, plugin: Box<dyn Plugin>) -> bool;
    fn load_plugins(&mut self) -> Vec<String>;
    fn get_plugin(&self, name: &str) -> Option<&dyn Plugin>;
    fn list_plugins(&self) -> Vec<serde_json::Value>;
    fn configure(&mut self, config: Value);
}
