use async_trait::async_trait;
use serde_json::{json, Value};
use tokio::sync::RwLock;

use crate::error::{ButterflyBotError, Result};
use crate::interfaces::plugins::Tool;
use crate::planning::{default_plan_db_path, resolve_plan_db_path, PlanStore};

pub struct PlanningTool {
    sqlite_path: RwLock<Option<String>>,
    store: RwLock<Option<std::sync::Arc<PlanStore>>>,
}

impl Default for PlanningTool {
    fn default() -> Self {
        Self::new()
    }
}

impl PlanningTool {
    pub fn new() -> Self {
        Self {
            sqlite_path: RwLock::new(None),
            store: RwLock::new(None),
        }
    }

    async fn get_store(&self) -> Result<std::sync::Arc<PlanStore>> {
        if let Some(store) = self.store.read().await.as_ref() {
            return Ok(store.clone());
        }
        let path = self
            .sqlite_path
            .read()
            .await
            .clone()
            .unwrap_or_else(default_plan_db_path);
        let store = std::sync::Arc::new(PlanStore::new(path).await?);
        let mut guard = self.store.write().await;
        *guard = Some(store.clone());
        Ok(store)
    }
}

#[async_trait]
impl Tool for PlanningTool {
    fn name(&self) -> &str {
        "planning"
    }

    fn description(&self) -> &str {
        "Create and manage structured plans with goals and steps."
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "action": {
                    "type": "string",
                    "enum": ["create", "list", "get", "update", "delete"]
                },
                "user_id": { "type": "string" },
                "id": { "type": "integer" },
                "title": { "type": "string" },
                "goal": { "type": "string" },
                "steps": { "type": "array", "items": { "type": "string" } },
                "status": { "type": "string" },
                "limit": { "type": "integer" }
            },
            "required": ["action", "user_id"]
        })
    }

    fn configure(&self, config: &Value) -> Result<()> {
        let path = resolve_plan_db_path(config);
        let mut guard = self
            .sqlite_path
            .try_write()
            .map_err(|_| ButterflyBotError::Runtime("Planning tool lock busy".to_string()))?;
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
        let limit = params.get("limit").and_then(|v| v.as_u64()).unwrap_or(20) as usize;

        match action.as_str() {
            "create" => {
                let title = params
                    .get("title")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| ButterflyBotError::Runtime("Missing title".to_string()))?;
                let goal = params
                    .get("goal")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| ButterflyBotError::Runtime("Missing goal".to_string()))?;
                let steps = params.get("steps");
                let status = params.get("status").and_then(|v| v.as_str());
                let plan = store
                    .create_plan(user_id, title, goal, steps, status)
                    .await?;
                Ok(json!({"status": "ok", "plan": plan}))
            }
            "list" => {
                let plans = store.list_plans(user_id, limit).await?;
                Ok(json!({"status": "ok", "plans": plans}))
            }
            "get" => {
                let id = params
                    .get("id")
                    .and_then(|v| v.as_i64())
                    .ok_or_else(|| ButterflyBotError::Runtime("Missing id".to_string()))?
                    as i32;
                let plan = store.get_plan(id).await?;
                Ok(json!({"status": "ok", "plan": plan}))
            }
            "update" => {
                let id = params
                    .get("id")
                    .and_then(|v| v.as_i64())
                    .ok_or_else(|| ButterflyBotError::Runtime("Missing id".to_string()))?
                    as i32;
                let title = params.get("title").and_then(|v| v.as_str());
                let goal = params.get("goal").and_then(|v| v.as_str());
                let steps = params.get("steps");
                let status = params.get("status").and_then(|v| v.as_str());
                let plan = store.update_plan(id, title, goal, steps, status).await?;
                Ok(json!({"status": "ok", "plan": plan}))
            }
            "delete" => {
                let id = params
                    .get("id")
                    .and_then(|v| v.as_i64())
                    .ok_or_else(|| ButterflyBotError::Runtime("Missing id".to_string()))?
                    as i32;
                let deleted = store.delete_plan(id).await?;
                Ok(json!({"status": "ok", "deleted": deleted}))
            }
            _ => Err(ButterflyBotError::Runtime("Unsupported action".to_string())),
        }
    }
}
