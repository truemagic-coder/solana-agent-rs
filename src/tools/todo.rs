use async_trait::async_trait;
use serde_json::{json, Value};
use tokio::sync::RwLock;

use crate::error::{ButterflyBotError, Result};
use crate::interfaces::plugins::Tool;
use crate::todo::{default_todo_db_path, resolve_todo_db_path, TodoStatus, TodoStore};

pub struct TodoTool {
    sqlite_path: RwLock<Option<String>>,
    store: RwLock<Option<std::sync::Arc<TodoStore>>>,
}

impl Default for TodoTool {
    fn default() -> Self {
        Self::new()
    }
}

impl TodoTool {
    pub fn new() -> Self {
        Self {
            sqlite_path: RwLock::new(None),
            store: RwLock::new(None),
        }
    }

    async fn get_store(&self) -> Result<std::sync::Arc<TodoStore>> {
        if let Some(store) = self.store.read().await.as_ref() {
            return Ok(store.clone());
        }
        let path = self
            .sqlite_path
            .read()
            .await
            .clone()
            .unwrap_or_else(default_todo_db_path);
        let store = std::sync::Arc::new(TodoStore::new(path).await?);
        let mut guard = self.store.write().await;
        *guard = Some(store.clone());
        Ok(store)
    }
}

#[async_trait]
impl Tool for TodoTool {
    fn name(&self) -> &str {
        "todo"
    }

    fn description(&self) -> &str {
        "Manage an ordered todo list (create, list, reorder, complete, delete)."
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "action": {
                    "type": "string",
                    "enum": ["create", "list", "complete", "reopen", "delete", "reorder", "create_many"]
                },
                "user_id": { "type": "string" },
                "title": { "type": "string" },
                "notes": { "type": "string" },
                "items": {
                    "type": "array",
                    "items": {
                        "oneOf": [
                            {"type": "string"},
                            {"type": "object", "properties": {"title": {"type": "string"}, "notes": {"type": "string"}}}
                        ]
                    }
                },
                "status": { "type": "string", "enum": ["open", "completed", "all"] },
                "limit": { "type": "integer" },
                "id": { "type": "integer" },
                "ordered_ids": { "type": "array", "items": { "type": "integer" } }
            },
            "required": ["action", "user_id"]
        })
    }

    fn configure(&self, config: &Value) -> Result<()> {
        let path = resolve_todo_db_path(config);
        let mut guard = self
            .sqlite_path
            .try_write()
            .map_err(|_| ButterflyBotError::Runtime("Todo tool lock busy".to_string()))?;
        *guard = path;
        Ok(())
    }

    async fn execute(&self, params: Value) -> Result<Value> {
        let action = params
            .get("action")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let action = match action.as_str() {
            "add" | "new" => "create",
            "create_list" | "create_many" | "add_many" | "bulk_create" | "create_items" => {
                "create_many"
            }
            other => other,
        };
        let user_id = params
            .get("user_id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ButterflyBotError::Runtime("Missing user_id".to_string()))?;

        let store = self.get_store().await?;
        let limit = params.get("limit").and_then(|v| v.as_u64()).unwrap_or(50) as usize;

        match action {
            "create" => {
                let title = params
                    .get("title")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| ButterflyBotError::Runtime("Missing title".to_string()))?;
                let notes = params.get("notes").and_then(|v| v.as_str());
                let item = store.create_item(user_id, title, notes).await?;
                Ok(json!({"status": "ok", "item": item}))
            }
            "create_many" => {
                let items = params
                    .get("items")
                    .and_then(|v| v.as_array())
                    .ok_or_else(|| ButterflyBotError::Runtime("Missing items".to_string()))?;
                if items.is_empty() {
                    return Err(ButterflyBotError::Runtime("items empty".to_string()));
                }
                let mut created = Vec::new();
                for item in items {
                    match item {
                        Value::String(title) => {
                            let created_item = store.create_item(user_id, title, None).await?;
                            created.push(created_item);
                        }
                        Value::Object(map) => {
                            let title = map
                                .get("title")
                                .and_then(|v| v.as_str())
                                .ok_or_else(|| {
                                    ButterflyBotError::Runtime("Missing item title".to_string())
                                })?;
                            let notes = map.get("notes").and_then(|v| v.as_str());
                            let created_item = store.create_item(user_id, title, notes).await?;
                            created.push(created_item);
                        }
                        _ => {
                            return Err(ButterflyBotError::Runtime(
                                "Invalid item format".to_string(),
                            ))
                        }
                    }
                }
                Ok(json!({"status": "ok", "items": created}))
            }
            "list" => {
                let status = TodoStatus::from_option(params.get("status").and_then(|v| v.as_str()));
                let items = store.list_items(user_id, status, limit).await?;
                Ok(json!({"status": "ok", "items": items}))
            }
            "complete" => {
                let id = params
                    .get("id")
                    .and_then(|v| v.as_i64())
                    .ok_or_else(|| ButterflyBotError::Runtime("Missing id".to_string()))?
                    as i32;
                let item = store.set_completed(id, true).await?;
                Ok(json!({"status": "ok", "item": item}))
            }
            "reopen" => {
                let id = params
                    .get("id")
                    .and_then(|v| v.as_i64())
                    .ok_or_else(|| ButterflyBotError::Runtime("Missing id".to_string()))?
                    as i32;
                let item = store.set_completed(id, false).await?;
                Ok(json!({"status": "ok", "item": item}))
            }
            "delete" => {
                let id = params
                    .get("id")
                    .and_then(|v| v.as_i64())
                    .ok_or_else(|| ButterflyBotError::Runtime("Missing id".to_string()))?
                    as i32;
                let deleted = store.delete_item(id).await?;
                Ok(json!({"status": "ok", "deleted": deleted}))
            }
            "reorder" => {
                let ordered_ids = params
                    .get("ordered_ids")
                    .and_then(|v| v.as_array())
                    .ok_or_else(|| ButterflyBotError::Runtime("Missing ordered_ids".to_string()))?;
                let ids: Vec<i32> = ordered_ids
                    .iter()
                    .filter_map(|value| value.as_i64().map(|v| v as i32))
                    .collect();
                if ids.is_empty() {
                    return Err(ButterflyBotError::Runtime("ordered_ids empty".to_string()));
                }
                store.reorder(user_id, &ids).await?;
                Ok(json!({"status": "ok"}))
            }
            _ => Err(ButterflyBotError::Runtime("Unsupported action".to_string())),
        }
    }
}
