use std::time::{SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use futures::TryStreamExt;
use mongodb::{
    bson::{doc, Document},
    options::ClientOptions,
    Client, Collection, Database,
};
use serde::{Deserialize, Serialize};

use crate::error::{Result, SolanaAgentError};
use crate::interfaces::providers::MemoryProvider;

#[derive(Debug, Clone, Serialize, Deserialize)]
struct MessageDoc {
    user_id: String,
    role: String,
    content: String,
    timestamp: i64,
}

pub struct MongoMemoryProvider {
    db: Database,
    messages: Collection<MessageDoc>,
    captures: Collection<Document>,
}

impl MongoMemoryProvider {
    pub async fn new(connection_string: &str, database: &str, collection: &str) -> Result<Self> {
        let mut options = ClientOptions::parse(connection_string)
            .await
            .map_err(|e| SolanaAgentError::Runtime(e.to_string()))?;
        options.app_name = Some("solana-agent".to_string());
        let client =
            Client::with_options(options).map_err(|e| SolanaAgentError::Runtime(e.to_string()))?;
        let db = client.database(database);
        let messages = db.collection::<MessageDoc>(collection);
        let captures = db.collection::<Document>("captures");
        Ok(Self {
            db,
            messages,
            captures,
        })
    }

    fn collection(&self, name: &str) -> Collection<Document> {
        self.db.collection::<Document>(name)
    }

    fn now_ts() -> Result<i64> {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|e| SolanaAgentError::Runtime(e.to_string()))
            .map(|d| d.as_secs() as i64)
    }
}

#[async_trait]
impl MemoryProvider for MongoMemoryProvider {
    async fn append_message(&self, user_id: &str, role: &str, content: &str) -> Result<()> {
        let doc = MessageDoc {
            user_id: user_id.to_string(),
            role: role.to_string(),
            content: content.to_string(),
            timestamp: Self::now_ts()?,
        };
        self.messages
            .insert_one(doc, None)
            .await
            .map_err(|e| SolanaAgentError::Runtime(e.to_string()))?;
        Ok(())
    }

    async fn get_history(&self, user_id: &str, limit: usize) -> Result<Vec<String>> {
        let mut options = mongodb::options::FindOptions::default();
        if limit > 0 {
            options.limit = Some(limit as i64);
        }
        options.sort = Some(doc! { "timestamp": 1 });

        let mut cursor = self
            .messages
            .find(doc! { "user_id": user_id }, options)
            .await
            .map_err(|e| SolanaAgentError::Runtime(e.to_string()))?;

        let mut messages = Vec::new();
        while let Some(doc) = cursor
            .try_next()
            .await
            .map_err(|e| SolanaAgentError::Runtime(e.to_string()))?
        {
            messages.push(format!("{}: {}", doc.role, doc.content));
        }
        Ok(messages)
    }

    async fn clear_history(&self, user_id: &str) -> Result<()> {
        self.messages
            .delete_many(doc! { "user_id": user_id }, None)
            .await
            .map_err(|e| SolanaAgentError::Runtime(e.to_string()))?;
        Ok(())
    }

    async fn store(&self, user_id: &str, messages: Vec<serde_json::Value>) -> Result<()> {
        let mut docs: Vec<MessageDoc> = Vec::new();
        for msg in messages {
            let role = msg
                .get("role")
                .and_then(|v| v.as_str())
                .unwrap_or("user")
                .to_string();
            let content = msg
                .get("content")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            docs.push(MessageDoc {
                user_id: user_id.to_string(),
                role,
                content,
                timestamp: Self::now_ts()?,
            });
        }
        if docs.is_empty() {
            return Ok(());
        }
        self.messages
            .insert_many(docs, None)
            .await
            .map_err(|e| SolanaAgentError::Runtime(e.to_string()))?;
        Ok(())
    }

    async fn retrieve(&self, user_id: &str) -> Result<String> {
        Ok(self.get_history(user_id, 0).await?.join("\n"))
    }

    async fn delete(&self, user_id: &str) -> Result<()> {
        self.clear_history(user_id).await
    }

    fn find(
        &self,
        collection: &str,
        query: serde_json::Value,
        sort: Option<Vec<(String, i32)>>,
        limit: Option<u64>,
        skip: Option<u64>,
    ) -> Result<Vec<serde_json::Value>> {
        let coll = self.collection(collection);
        let mut options = mongodb::options::FindOptions::default();
        if let Some(limit) = limit {
            options.limit = Some(limit as i64);
        }
        if let Some(skip) = skip {
            options.skip = Some(skip as u64);
        }
        if let Some(sort) = sort {
            let sort_doc = sort.into_iter().fold(Document::new(), |mut acc, (k, v)| {
                acc.insert(k, v);
                acc
            });
            options.sort = Some(sort_doc);
        }

        let query_doc: Document = mongodb::bson::from_bson(
            mongodb::bson::to_bson(&query).map_err(|e| SolanaAgentError::Runtime(e.to_string()))?,
        )
        .map_err(|e| SolanaAgentError::Runtime(e.to_string()))?;

        let mut cursor = futures::executor::block_on(coll.find(query_doc, options))
            .map_err(|e| SolanaAgentError::Runtime(e.to_string()))?;

        let mut results = Vec::new();
        while let Some(doc) = futures::executor::block_on(cursor.try_next())
            .map_err(|e| SolanaAgentError::Runtime(e.to_string()))?
        {
            let bson = mongodb::bson::to_bson(&doc)
                .map_err(|e| SolanaAgentError::Runtime(e.to_string()))?;
            let value =
                serde_json::to_value(bson).map_err(|e| SolanaAgentError::Runtime(e.to_string()))?;
            results.push(value);
        }
        Ok(results)
    }

    fn count_documents(&self, collection: &str, query: serde_json::Value) -> Result<u64> {
        let coll = self.collection(collection);
        let query_doc: Document = mongodb::bson::from_bson(
            mongodb::bson::to_bson(&query).map_err(|e| SolanaAgentError::Runtime(e.to_string()))?,
        )
        .map_err(|e| SolanaAgentError::Runtime(e.to_string()))?;
        let count = futures::executor::block_on(coll.count_documents(query_doc, None))
            .map_err(|e| SolanaAgentError::Runtime(e.to_string()))?;
        Ok(count)
    }

    async fn save_capture(
        &self,
        user_id: &str,
        capture_name: &str,
        agent_name: Option<&str>,
        data: serde_json::Value,
        schema: Option<serde_json::Value>,
    ) -> Result<Option<String>> {
        let mut doc = Document::new();
        doc.insert("user_id", user_id);
        doc.insert("capture_name", capture_name);
        if let Some(agent) = agent_name {
            doc.insert("agent_name", agent);
        }
        doc.insert(
            "data",
            mongodb::bson::to_bson(&data).map_err(|e| SolanaAgentError::Runtime(e.to_string()))?,
        );
        if let Some(schema) = schema {
            doc.insert(
                "schema",
                mongodb::bson::to_bson(&schema)
                    .map_err(|e| SolanaAgentError::Runtime(e.to_string()))?,
            );
        }
        doc.insert("timestamp", Self::now_ts()?);

        let result = self
            .captures
            .insert_one(doc, None)
            .await
            .map_err(|e| SolanaAgentError::Runtime(e.to_string()))?;
        Ok(result.inserted_id.as_object_id().map(|id| id.to_hex()))
    }
}
