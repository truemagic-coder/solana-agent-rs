use std::time::{SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use serde_json::{json, Value};
use tokio::sync::RwLock;

use crate::error::{ButterflyBotError, Result};
use crate::interfaces::plugins::Tool;
use crate::reminders::{
    default_reminder_db_path, resolve_reminder_db_path, ReminderStatus, ReminderStore,
};

pub struct RemindersTool {
    sqlite_path: RwLock<Option<String>>,
    store: RwLock<Option<std::sync::Arc<ReminderStore>>>,
}

impl Default for RemindersTool {
    fn default() -> Self {
        Self::new()
    }
}

impl RemindersTool {
    pub fn new() -> Self {
        Self {
            sqlite_path: RwLock::new(None),
            store: RwLock::new(None),
        }
    }

    async fn get_store(&self) -> Result<std::sync::Arc<ReminderStore>> {
        if let Some(store) = self.store.read().await.as_ref() {
            return Ok(store.clone());
        }
        let path = self
            .sqlite_path
            .read()
            .await
            .clone()
            .unwrap_or_else(default_reminder_db_path);
        let store = std::sync::Arc::new(ReminderStore::new(path).await?);
        let mut guard = self.store.write().await;
        *guard = Some(store.clone());
        Ok(store)
    }

    fn parse_due_at_required(params: &Value) -> Result<i64> {
        if let Some(seconds) = params.get("delay_seconds").and_then(|v| v.as_i64()) {
            return Ok(now_ts() + seconds.max(0));
        }
        if let Some(seconds) = params.get("in_seconds").and_then(|v| v.as_i64()) {
            return Ok(now_ts() + seconds.max(0));
        }
        if let Some(due_at) = params.get("due_at").and_then(|v| v.as_i64()) {
            return Ok(due_at);
        }
        Err(ButterflyBotError::Runtime(
            "Missing due_at or delay_seconds".to_string(),
        ))
    }

    fn parse_due_at_optional(params: &Value) -> i64 {
        if let Some(seconds) = params.get("delay_seconds").and_then(|v| v.as_i64()) {
            return now_ts() + seconds.max(0);
        }
        if let Some(seconds) = params.get("in_seconds").and_then(|v| v.as_i64()) {
            return now_ts() + seconds.max(0);
        }
        if let Some(due_at) = params.get("due_at").and_then(|v| v.as_i64()) {
            return due_at;
        }
        now_ts() + 315_360_000
    }
}

#[async_trait]
impl Tool for RemindersTool {
    fn name(&self) -> &str {
        "reminders"
    }

    fn description(&self) -> &str {
        "Create, list, complete, delete, and snooze reminders (simple alarms/todos)."
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "action": {
                    "type": "string",
                    "enum": ["create", "list", "complete", "delete", "snooze", "clear"]
                },
                "user_id": { "type": "string" },
                "title": { "type": "string" },
                "id": { "type": "integer" },
                "due_at": { "type": "integer", "description": "Unix timestamp (seconds)" },
                "delay_seconds": { "type": "integer", "description": "Delay from now in seconds" },
                "in_seconds": { "type": "integer", "description": "Alias for delay_seconds" },
                "status": { "type": "string", "enum": ["open", "completed", "all"] },
                "limit": { "type": "integer" }
            },
            "required": ["action", "user_id"]
        })
    }

    fn configure(&self, config: &Value) -> Result<()> {
        let path = resolve_reminder_db_path(config);
        let mut guard = self
            .sqlite_path
            .try_write()
            .map_err(|_| ButterflyBotError::Runtime("Reminders tool lock busy".to_string()))?;
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
            "set" | "add" | "remind" | "schedule" | "create_reminder" => "create",
            "show" | "list_reminders" => "list",
            "done" | "finish" => "complete",
            "remove" | "erase" => "delete",
            "clear" | "clear_all" | "clear_reminders" => "clear",
            other => other,
        };
        let user_id = params
            .get("user_id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ButterflyBotError::Runtime("Missing user_id".to_string()))?;

        let store = self.get_store().await?;
        let limit = params.get("limit").and_then(|v| v.as_u64()).unwrap_or(20) as usize;

        match action {
            "create" => {
                let title = params
                    .get("title")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| ButterflyBotError::Runtime("Missing title".to_string()))?;
                let due_at = Self::parse_due_at_optional(&params);
                let item = store.create_reminder(user_id, title, due_at).await?;
                Ok(json!({"status": "ok", "reminder": item}))
            }
            "list" => {
                let status =
                    ReminderStatus::from_option(params.get("status").and_then(|v| v.as_str()));
                let items = store.list_reminders(user_id, status, limit).await?;
                Ok(json!({"status": "ok", "reminders": items}))
            }
            "complete" => {
                let id = params
                    .get("id")
                    .and_then(|v| v.as_i64())
                    .ok_or_else(|| ButterflyBotError::Runtime("Missing id".to_string()))?
                    as i32;
                let updated = store.complete_reminder(user_id, id).await?;
                Ok(json!({"status": "ok", "completed": updated}))
            }
            "delete" => {
                let id = params
                    .get("id")
                    .and_then(|v| v.as_i64())
                    .ok_or_else(|| ButterflyBotError::Runtime("Missing id".to_string()))?
                    as i32;
                let deleted = store.delete_reminder(user_id, id).await?;
                Ok(json!({"status": "ok", "deleted": deleted}))
            }
            "snooze" => {
                let id = params
                    .get("id")
                    .and_then(|v| v.as_i64())
                    .ok_or_else(|| ButterflyBotError::Runtime("Missing id".to_string()))?
                    as i32;
                let due_at = Self::parse_due_at_required(&params)?;
                let updated = store.snooze_reminder(user_id, id, due_at).await?;
                Ok(json!({"status": "ok", "snoozed": updated}))
            }
            "clear" => {
                let include_completed = matches!(
                    params.get("status").and_then(|v| v.as_str()),
                    Some("all") | Some("completed")
                );
                let deleted = store.delete_all(user_id, include_completed).await?;
                Ok(json!({"status": "ok", "deleted": deleted}))
            }
            _ => Err(ButterflyBotError::Runtime("Unsupported action".to_string())),
        }
    }
}

fn now_ts() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}
