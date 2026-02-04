use std::collections::HashMap;
use std::time::{SystemTime, UNIX_EPOCH};

use time::{macros::format_description, OffsetDateTime};

use async_trait::async_trait;
use tokio::sync::RwLock;

use crate::domains::memory::Message;
use crate::error::{ButterflyBotError, Result};
use crate::interfaces::providers::MemoryProvider;

#[derive(Default)]
pub struct InMemoryMemoryProvider {
    store: RwLock<HashMap<String, Vec<Message>>>,
    collections: RwLock<HashMap<String, Vec<serde_json::Value>>>,
}

impl InMemoryMemoryProvider {
    pub fn new() -> Self {
        Self {
            store: RwLock::new(HashMap::new()),
            collections: RwLock::new(HashMap::new()),
        }
    }

    #[doc(hidden)]
    pub fn insert_document(&self, collection: &str, doc: serde_json::Value) {
        let mut guard = futures::executor::block_on(self.collections.write());
        guard.entry(collection.to_string()).or_default().push(doc);
    }
}

const TIMESTAMP_FORMAT: &[time::format_description::FormatItem<'static>] =
    format_description!("[year]-[month]-[day] [hour]:[minute]");

fn format_timestamp(ts: i64) -> String {
    OffsetDateTime::from_unix_timestamp(ts)
        .ok()
        .and_then(|dt| dt.format(TIMESTAMP_FORMAT).ok())
        .unwrap_or_else(|| ts.to_string())
}

#[async_trait]
impl MemoryProvider for InMemoryMemoryProvider {
    async fn append_message(&self, user_id: &str, role: &str, content: &str) -> Result<()> {
        let mut guard = self.store.write().await;
        let entry = guard.entry(user_id.to_string()).or_default();
        let ts = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|e| ButterflyBotError::Runtime(e.to_string()))?
            .as_secs() as i64;
        entry.push(Message {
            role: role.to_string(),
            content: content.to_string(),
            timestamp: ts,
        });
        Ok(())
    }

    async fn get_history(&self, user_id: &str, limit: usize) -> Result<Vec<String>> {
        let guard = self.store.read().await;
        let mut messages = guard.get(user_id).cloned().unwrap_or_default();
        if limit > 0 && messages.len() > limit {
            messages = messages.split_off(messages.len() - limit);
        }
        Ok(messages
            .into_iter()
            .map(|m| {
                format!(
                    "[{}] {}: {}",
                    format_timestamp(m.timestamp),
                    m.role,
                    m.content
                )
            })
            .collect())
    }

    async fn clear_history(&self, user_id: &str) -> Result<()> {
        let mut guard = self.store.write().await;
        guard.remove(user_id);
        Ok(())
    }

    fn find(
        &self,
        collection: &str,
        query: serde_json::Value,
        _sort: Option<Vec<(String, i32)>>,
        _limit: Option<u64>,
        _skip: Option<u64>,
    ) -> Result<Vec<serde_json::Value>> {
        let guard = futures::executor::block_on(self.collections.read());
        let docs = guard.get(collection).cloned().unwrap_or_default();
        if query.is_null() {
            return Ok(docs);
        }

        let query_obj = query.as_object().cloned().unwrap_or_default();
        let filtered = docs
            .into_iter()
            .filter(|doc| {
                let Some(obj) = doc.as_object() else {
                    return false;
                };
                query_obj.iter().all(|(k, v)| obj.get(k) == Some(v))
            })
            .collect();
        Ok(filtered)
    }

    fn count_documents(&self, collection: &str, query: serde_json::Value) -> Result<u64> {
        let results = self.find(collection, query, None, None, None)?;
        Ok(results.len() as u64)
    }

    async fn search(&self, _user_id: &str, _query: &str, _limit: usize) -> Result<Vec<String>> {
        Ok(Vec::new())
    }
}
