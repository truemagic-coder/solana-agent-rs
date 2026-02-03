use async_trait::async_trait;
use serde_json::{json, Value};
use tokio::sync::RwLock;

use crate::error::{ButterflyBotError, Result};
use crate::interfaces::plugins::Tool;
use crate::tasks::{default_task_db_path, resolve_task_db_path, TaskStatus, TaskStore};

pub struct TasksTool {
    sqlite_path: RwLock<Option<String>>,
    store: RwLock<Option<std::sync::Arc<TaskStore>>>,
}

impl Default for TasksTool {
    fn default() -> Self {
        Self::new()
    }
}

impl TasksTool {
    pub fn new() -> Self {
        Self {
            sqlite_path: RwLock::new(None),
            store: RwLock::new(None),
        }
    }

    async fn get_store(&self) -> Result<std::sync::Arc<TaskStore>> {
        if let Some(store) = self.store.read().await.as_ref() {
            return Ok(store.clone());
        }
        let path = self
            .sqlite_path
            .read()
            .await
            .clone()
            .unwrap_or_else(default_task_db_path);
        let store = std::sync::Arc::new(TaskStore::new(path).await?);
        let mut guard = self.store.write().await;
        *guard = Some(store.clone());
        Ok(store)
    }
}

#[async_trait]
impl Tool for TasksTool {
    fn name(&self) -> &str {
        "tasks"
    }

    fn description(&self) -> &str {
        "Schedule one-off or recurring tasks at specific times; cancelable." 
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "action": {
                    "type": "string",
                    "enum": ["schedule", "list", "cancel", "enable", "disable", "delete"]
                },
                "user_id": { "type": "string" },
                "name": { "type": "string" },
                "prompt": { "type": "string" },
                "run_at": { "type": "integer", "description": "Unix timestamp (seconds)" },
                "interval_minutes": { "type": "integer", "description": "Recurring interval in minutes" },
                "status": { "type": "string", "enum": ["enabled", "disabled", "all"] },
                "limit": { "type": "integer" },
                "id": { "type": "integer" }
            },
            "required": ["action", "user_id"]
        })
    }

    fn configure(&self, config: &Value) -> Result<()> {
        let path = resolve_task_db_path(config);
        let mut guard = self
            .sqlite_path
            .try_write()
            .map_err(|_| ButterflyBotError::Runtime("Tasks tool lock busy".to_string()))?;
        *guard = path;
        Ok(())
    }

    async fn execute(&self, params: Value) -> Result<Value> {
        let action = params
            .get("action")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let user_id = params
            .get("user_id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ButterflyBotError::Runtime("Missing user_id".to_string()))?;

        let store = self.get_store().await?;
        let limit = params.get("limit").and_then(|v| v.as_u64()).unwrap_or(50) as usize;

        match action.as_str() {
            "schedule" => {
                let name = params
                    .get("name")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| ButterflyBotError::Runtime("Missing name".to_string()))?;
                let prompt = params
                    .get("prompt")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| ButterflyBotError::Runtime("Missing prompt".to_string()))?;
                let run_at = params
                    .get("run_at")
                    .and_then(|v| v.as_i64())
                    .ok_or_else(|| ButterflyBotError::Runtime("Missing run_at".to_string()))?;
                let interval_minutes = params.get("interval_minutes").and_then(|v| v.as_i64());
                let task = store
                    .create_task(user_id, name, prompt, run_at, interval_minutes)
                    .await?;
                Ok(json!({"status": "ok", "task": task}))
            }
            "list" => {
                let status = TaskStatus::from_option(params.get("status").and_then(|v| v.as_str()));
                let tasks = store.list_tasks(user_id, status, limit).await?;
                Ok(json!({"status": "ok", "tasks": tasks}))
            }
            "cancel" | "disable" => {
                let id = params
                    .get("id")
                    .and_then(|v| v.as_i64())
                    .ok_or_else(|| ButterflyBotError::Runtime("Missing id".to_string()))?
                    as i32;
                let task = store.set_enabled(id, false).await?;
                Ok(json!({"status": "ok", "task": task}))
            }
            "enable" => {
                let id = params
                    .get("id")
                    .and_then(|v| v.as_i64())
                    .ok_or_else(|| ButterflyBotError::Runtime("Missing id".to_string()))?
                    as i32;
                let task = store.set_enabled(id, true).await?;
                Ok(json!({"status": "ok", "task": task}))
            }
            "delete" => {
                let id = params
                    .get("id")
                    .and_then(|v| v.as_i64())
                    .ok_or_else(|| ButterflyBotError::Runtime("Missing id".to_string()))?
                    as i32;
                let deleted = store.delete_task(id).await?;
                Ok(json!({"status": "ok", "deleted": deleted}))
            }
            _ => Err(ButterflyBotError::Runtime("Unsupported action".to_string())),
        }
    }
}
