use std::collections::HashMap;

use serde_json::Value;

use crate::interfaces::plugins::{Plugin, PluginManager};
use crate::plugins::registry::ToolRegistry;

pub struct DefaultPluginManager {
    config: Value,
    tool_registry: ToolRegistry,
    plugins: HashMap<String, Box<dyn Plugin>>,
}

impl DefaultPluginManager {
    pub fn new(config: Value) -> Self {
        Self {
            config,
            tool_registry: ToolRegistry::new(),
            plugins: HashMap::new(),
        }
    }

    pub fn tool_registry(&self) -> &ToolRegistry {
        &self.tool_registry
    }
}

impl PluginManager for DefaultPluginManager {
    fn register_plugin(&mut self, plugin: Box<dyn Plugin>) -> bool {
        if !plugin.initialize(&self.tool_registry) {
            return false;
        }
        self.plugins.insert(plugin.name().to_string(), plugin);
        true
    }

    fn load_plugins(&mut self) -> Vec<String> {
        Vec::new()
    }

    fn get_plugin(&self, name: &str) -> Option<&dyn Plugin> {
        self.plugins.get(name).map(|p| p.as_ref())
    }

    fn list_plugins(&self) -> Vec<Value> {
        self.plugins
            .values()
            .map(|p| {
                serde_json::json!({
                    "name": p.name(),
                    "description": p.description(),
                })
            })
            .collect()
    }

    fn configure(&mut self, config: Value) {
        self.config = config.clone();
        let _ = futures::executor::block_on(self.tool_registry.configure_all_tools(config));
    }
}
