mod common;

use std::sync::Arc;

use serde_json::json;

use butterfly_bot::error::ButterflyBotError;
use butterfly_bot::interfaces::plugins::PluginManager;
use butterfly_bot::plugins::manager::DefaultPluginManager;
use butterfly_bot::plugins::registry::ToolRegistry;

use common::{
    ConditionalTool, ConfigurablePlugin, DefaultConfigureTool, DummyPlugin, DummyTool, FailingTool,
};
use tempfile::tempdir;

#[tokio::test]
async fn tool_registry_and_plugin_manager() {
    let registry = ToolRegistry::new();
    let tool = Arc::new(DummyTool::new("tool"));
    assert!(registry.register_tool(tool.clone()).await);
    assert!(!registry.register_tool(tool.clone()).await);

    let fail_tool = Arc::new(FailingTool);
    assert!(!registry.register_tool(fail_tool).await);

    assert!(registry.assign_tool_to_agent("agent", "tool").await);
    assert!(!registry.assign_tool_to_agent("agent", "missing").await);

    let got = registry.get_tool("tool").await.unwrap();
    assert_eq!(got.name(), "tool");

    let agent_tools = registry.get_agent_tools("agent").await;
    assert_eq!(agent_tools.len(), 1);

    let all = registry.list_all_tools().await;
    assert_eq!(all, vec!["tool".to_string()]);

    registry
        .configure_all_tools(json!({"value": 1}))
        .await
        .unwrap();

    let mut manager = DefaultPluginManager::new(json!({"ok":true}));
    assert!(manager.register_plugin(Box::new(DummyPlugin::new("p1", true))));
    assert!(!manager.register_plugin(Box::new(DummyPlugin::new("p2", false))));
    assert!(manager.get_plugin("p1").is_some());
    assert!(manager.get_plugin("missing").is_none());
    assert_eq!(manager.list_plugins().len(), 1);
    manager.configure(json!({"reconfigured":true}));
    manager.load_plugins();
    let _ = manager.tool_registry();

    let registry = ToolRegistry::new();
    let conditional = Arc::new(ConditionalTool {
        name: "conditional".to_string(),
    });
    assert!(registry.register_tool(conditional).await);
    let err = registry
        .configure_all_tools(json!({"fail": true}))
        .await
        .unwrap_err();
    assert!(matches!(err, ButterflyBotError::Runtime(_)));

    let registry = ToolRegistry::new();
    let default_tool = Arc::new(DefaultConfigureTool);
    assert!(registry.register_tool(default_tool).await);
}

#[tokio::test]
async fn tool_registry_audit_log() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("audit.log");
    let registry = ToolRegistry::new();
    registry
        .configure_all_tools(json!({
            "tools": {"settings": {"audit_log_path": path.to_string_lossy()}}
        }))
        .await
        .unwrap();
    registry.audit_tool_call("tool", "success").await.unwrap();

    let content = std::fs::read_to_string(path).unwrap();
    assert!(content.contains("\"tool\":\"tool\""));
    assert!(content.contains("\"status\":\"success\""));
}

#[tokio::test]
async fn plugin_manager_auto_loads() {
    use std::sync::Mutex as StdMutex;

    let config = json!({
        "plugins": [
            "p1",
            {"name":"p2","config":{"x":1}},
            {"class":"p3","config":{"y":2}},
            {"config":{"ignored":true}},
            "missing",
            {"name":"p4"}
        ]
    });
    let mut manager = DefaultPluginManager::new(config);

    let seen_p2: Arc<StdMutex<Option<serde_json::Value>>> = Arc::new(StdMutex::new(None));
    let seen_p3: Arc<StdMutex<Option<serde_json::Value>>> = Arc::new(StdMutex::new(None));

    let seen_p2_factory = seen_p2.clone();
    manager.register_factory("p1", |_| Box::new(DummyPlugin::new("p1", true)));
    manager.register_factory("p2", move |cfg| {
        *seen_p2_factory.lock().unwrap() = Some(cfg);
        Box::new(ConfigurablePlugin {
            name: "p2".to_string(),
        })
    });
    let seen_p3_factory = seen_p3.clone();
    manager.register_factory("p3", move |cfg| {
        *seen_p3_factory.lock().unwrap() = Some(cfg);
        Box::new(ConfigurablePlugin {
            name: "p3".to_string(),
        })
    });
    manager.register_factory("p4", |_| Box::new(DummyPlugin::new("p4", false)));

    let mut loaded = manager.load_plugins();
    loaded.sort();
    assert_eq!(
        loaded,
        vec!["p1".to_string(), "p2".to_string(), "p3".to_string()]
    );
    assert!(manager.get_plugin("p1").is_some());
    assert!(manager.get_plugin("missing").is_none());
    assert_eq!(*seen_p2.lock().unwrap(), Some(json!({"x":1})));
    assert_eq!(*seen_p3.lock().unwrap(), Some(json!({"y":2})));
    assert!(manager.get_plugin("p4").is_none());

    let mut manager = DefaultPluginManager::new(json!({}));
    manager.register_factory("auto1", |_| Box::new(DummyPlugin::new("auto1", true)));
    manager.register_factory("auto2", |_| Box::new(DummyPlugin::new("auto2", true)));
    assert!(manager.register_plugin(Box::new(DummyPlugin::new("auto1", true))));

    let mut loaded = manager.load_plugins();
    loaded.sort();
    assert_eq!(loaded, vec!["auto2".to_string()]);
}
