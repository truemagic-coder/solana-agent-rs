use std::collections::HashMap;
use std::env;
use std::sync::Arc;

use serde_json::Value;

use crate::interfaces::brain::{BrainContext, BrainEvent, BrainPlugin};

pub struct BrainManager {
    config: Value,
    plugins: HashMap<String, Arc<dyn BrainPlugin>>,
    plugin_factories: HashMap<String, BrainFactory>,
}

type BrainFactory = Arc<dyn Fn(Value) -> Arc<dyn BrainPlugin> + Send + Sync>;

impl BrainManager {
    pub fn new(config: Value) -> Self {
        Self {
            config,
            plugins: HashMap::new(),
            plugin_factories: HashMap::new(),
        }
    }

    pub fn register_factory<F>(&mut self, name: &str, factory: F)
    where
        F: Fn(Value) -> Arc<dyn BrainPlugin> + Send + Sync + 'static,
    {
        self.plugin_factories
            .insert(name.to_string(), Arc::new(factory));
    }

    pub fn register_plugin(&mut self, plugin: Arc<dyn BrainPlugin>) -> bool {
        let name = plugin.name().to_string();
        if self.plugins.contains_key(&name) {
            return false;
        }
        self.plugins.insert(name, plugin);
        true
    }

    pub fn get_plugin(&self, name: &str) -> Option<Arc<dyn BrainPlugin>> {
        self.plugins.get(name).cloned()
    }

    pub fn list_plugins(&self) -> Vec<Value> {
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

    pub fn load_plugins(&mut self) -> Vec<String> {
        let mut loaded = Vec::new();

        let plugin_entries = self
            .config
            .get("brains")
            .and_then(|value| value.as_array())
            .cloned();

        let allow_default = env::var("BUTTERFLY_BOT_ENABLE_BRAINS")
            .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
            .unwrap_or(false);

        let mut to_load: Vec<(String, Value)> = Vec::new();

        if let Some(entries) = plugin_entries {
            for entry in entries {
                match entry {
                    Value::String(name) => {
                        to_load.push((name, Value::Null));
                    }
                    Value::Object(map) => {
                        let name = map
                            .get("name")
                            .or_else(|| map.get("class"))
                            .and_then(|value| value.as_str())
                            .map(|name| name.to_string());
                        if let Some(name) = name {
                            let config = map.get("config").cloned().unwrap_or(Value::Null);
                            to_load.push((name, config));
                        }
                    }
                    _ => {}
                }
            }
        } else if allow_default {
            let mut names: Vec<String> = self.plugin_factories.keys().cloned().collect();
            names.sort();
            for name in names {
                to_load.push((name, Value::Null));
            }
        } else {
            return loaded;
        }

        for (name, config) in to_load {
            if self.plugins.contains_key(&name) {
                continue;
            }
            if let Some(factory) = self.plugin_factories.get(&name) {
                let plugin = factory(config);
                if self.register_plugin(plugin) {
                    loaded.push(name);
                }
            }
        }

        loaded
    }

    pub fn configure(&mut self, config: Value) {
        self.config = config;
    }

    pub async fn dispatch(&self, event: BrainEvent, ctx: &BrainContext) {
        for plugin in self.plugins.values() {
            let _ = plugin.on_event(event.clone(), ctx).await;
        }
    }
}
