use std::num::NonZeroUsize;
use std::path::Path;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use arrow_array::{Array, Int64Array, RecordBatch, RecordBatchIterator, StringArray};
use arrow_schema::{DataType, Field, Schema};
use async_trait::async_trait;
use diesel::prelude::*;
use diesel::sql_types::{BigInt, Text};
use diesel::sqlite::SqliteConnection;
use diesel_async::pooled_connection::bb8::{Pool, PooledConnection};
use diesel_async::pooled_connection::AsyncDieselConnectionManager;
use diesel_async::sync_connection_wrapper::SyncConnectionWrapper;
use diesel_async::RunQueryDsl;
use diesel_migrations::{embed_migrations, EmbeddedMigrations, MigrationHarness};
use futures::TryStreamExt;
use lru::LruCache;
use serde_json::{json, Value};
use time::{macros::format_description, OffsetDateTime};

use crate::error::{ButterflyBotError, Result};
use crate::interfaces::providers::{LlmProvider, MemoryProvider};

mod schema;
use schema::{captures, messages};

const MIGRATIONS: EmbeddedMigrations = embed_migrations!();

type SqliteAsyncConn = SyncConnectionWrapper<SqliteConnection>;
type SqlitePool = Pool<SqliteAsyncConn>;
type SqlitePooledConn<'a> = PooledConnection<'a, SqliteAsyncConn>;

#[derive(Queryable)]
struct MessageRow {
    role: String,
    content: String,
    timestamp: i64,
}

#[derive(QueryableByName)]
struct RowId {
    #[diesel(sql_type = diesel::sql_types::BigInt)]
    id: i64,
}

#[derive(QueryableByName)]
struct SearchRow {
    #[diesel(sql_type = Text)]
    content: String,
    #[diesel(sql_type = BigInt)]
    timestamp: i64,
}

#[derive(QueryableByName)]
struct CountRow {
    #[diesel(sql_type = BigInt)]
    count: i64,
}

#[derive(Insertable)]
#[diesel(table_name = messages)]
struct NewMessage<'a> {
    user_id: &'a str,
    role: &'a str,
    content: &'a str,
    timestamp: i64,
}

#[derive(Insertable)]
#[diesel(table_name = captures)]
struct NewCapture<'a> {
    user_id: &'a str,
    capture_name: &'a str,
    agent_name: Option<&'a str>,
    data: &'a str,
    schema: Option<&'a str>,
    timestamp: i64,
}

#[derive(Insertable)]
#[diesel(table_name = crate::providers::sqlite::schema::memories)]
struct NewMemory<'a> {
    user_id: &'a str,
    summary: &'a str,
    tags: Option<&'a str>,
    salience: Option<f64>,
    created_at: i64,
}

#[derive(Insertable)]
#[diesel(table_name = crate::providers::sqlite::schema::entities)]
struct NewEntity<'a> {
    user_id: &'a str,
    name: &'a str,
    entity_type: &'a str,
    canonical_id: Option<&'a str>,
    created_at: i64,
}

#[derive(Insertable)]
#[diesel(table_name = crate::providers::sqlite::schema::facts)]
struct NewFact<'a> {
    user_id: &'a str,
    subject: &'a str,
    predicate: &'a str,
    object: &'a str,
    confidence: Option<f64>,
    source: Option<&'a str>,
    created_at: i64,
}

#[derive(Insertable)]
#[diesel(table_name = crate::providers::sqlite::schema::edges)]
struct NewEdge<'a> {
    user_id: &'a str,
    src_node_type: &'a str,
    src_node_id: i32,
    dst_node_type: &'a str,
    dst_node_id: i32,
    edge_type: &'a str,
    weight: Option<f64>,
    created_at: i64,
}

#[derive(Insertable)]
#[diesel(table_name = crate::providers::sqlite::schema::memory_links)]
struct NewMemoryLink<'a> {
    memory_id: i32,
    node_type: &'a str,
    node_id: i32,
    created_at: i64,
}

#[derive(Clone)]
struct LanceDbStore {
    db: lancedb::Connection,
    table: Arc<tokio::sync::Mutex<Option<lancedb::Table>>>,
}

impl LanceDbStore {
    async fn new(path: &str) -> Result<Self> {
        ensure_parent_dir(path)?;
        let db = lancedb::connect(path)
            .execute()
            .await
            .map_err(|e| ButterflyBotError::Runtime(e.to_string()))?;
        Ok(Self {
            db,
            table: Arc::new(tokio::sync::Mutex::new(None)),
        })
    }

    async fn table_exists(&self, name: &str) -> Result<bool> {
        let tables = self
            .db
            .table_names()
            .execute()
            .await
            .map_err(|e| ButterflyBotError::Runtime(e.to_string()))?;
        Ok(tables.iter().any(|t| t == name))
    }

    async fn get_or_create_table(&self, dim: i32) -> Result<lancedb::Table> {
        let mut guard = self.table.lock().await;
        if let Some(table) = guard.clone() {
            return Ok(table);
        }

        let name = "message_vectors";
        let table = if self.table_exists(name).await? {
            self.db
                .open_table(name)
                .execute()
                .await
                .map_err(|e| ButterflyBotError::Runtime(e.to_string()))?
        } else {
            let schema = Arc::new(Schema::new(vec![
                Field::new("id", DataType::Int64, false),
                Field::new("user_id", DataType::Utf8, false),
                Field::new("role", DataType::Utf8, false),
                Field::new("content", DataType::Utf8, false),
                Field::new("timestamp", DataType::Int64, false),
                Field::new(
                    "vector",
                    DataType::FixedSizeList(
                        Arc::new(Field::new("item", DataType::Float32, true)),
                        dim,
                    ),
                    true,
                ),
            ]));

            let batch = RecordBatch::try_new(
                schema.clone(),
                vec![
                    Arc::new(Int64Array::from_iter_values([0i64])),
                    Arc::new(StringArray::from_iter_values([""])) as Arc<dyn arrow_array::Array>,
                    Arc::new(StringArray::from_iter_values([""])) as Arc<dyn arrow_array::Array>,
                    Arc::new(StringArray::from_iter_values([""])) as Arc<dyn arrow_array::Array>,
                    Arc::new(Int64Array::from_iter_values([0i64])),
                    Arc::new(arrow_array::FixedSizeListArray::from_iter_primitive::<
                        arrow_array::types::Float32Type,
                        _,
                        _,
                    >(
                        vec![Some(vec![Some(0.0); dim as usize])], dim
                    )),
                ],
            )
            .map_err(|e| ButterflyBotError::Runtime(e.to_string()))?;

            let batches = RecordBatchIterator::new(vec![Ok(batch)].into_iter(), schema);

            self.db
                .create_table(name, batches)
                .execute()
                .await
                .map_err(|e| ButterflyBotError::Runtime(e.to_string()))?
        };

        *guard = Some(table.clone());
        Ok(table)
    }

    async fn open_table_if_exists(&self) -> Result<Option<lancedb::Table>> {
        let mut guard = self.table.lock().await;
        if let Some(table) = guard.clone() {
            return Ok(Some(table));
        }
        let name = "message_vectors";
        if !self.table_exists(name).await? {
            return Ok(None);
        }
        let table = self
            .db
            .open_table(name)
            .execute()
            .await
            .map_err(|e| ButterflyBotError::Runtime(e.to_string()))?;
        *guard = Some(table.clone());
        Ok(Some(table))
    }
}

pub struct SqliteMemoryProvider {
    pool: SqlitePool,
    lancedb: Option<LanceDbStore>,
    embedder: Option<Arc<dyn LlmProvider>>,
    embedding_model: Option<String>,
    reranker: Option<Arc<dyn LlmProvider>>,
    summarizer: Option<Arc<dyn LlmProvider>>,
    summary_threshold: usize,
    retention_days: Option<u32>,
    embedding_cache: Arc<tokio::sync::Mutex<LruCache<String, Vec<f32>>>>,
}

impl Clone for SqliteMemoryProvider {
    fn clone(&self) -> Self {
        Self {
            pool: self.pool.clone(),
            lancedb: self.lancedb.clone(),
            embedder: self.embedder.clone(),
            embedding_model: self.embedding_model.clone(),
            reranker: self.reranker.clone(),
            summarizer: self.summarizer.clone(),
            summary_threshold: self.summary_threshold,
            retention_days: self.retention_days,
            embedding_cache: Arc::clone(&self.embedding_cache),
        }
    }
}

pub struct SqliteMemoryProviderConfig {
    pub sqlite_path: String,
    pub lancedb_path: Option<String>,
    pub embedder: Option<Arc<dyn LlmProvider>>,
    pub embedding_model: Option<String>,
    pub reranker: Option<Arc<dyn LlmProvider>>,
    pub summarizer: Option<Arc<dyn LlmProvider>>,
    pub summary_threshold: Option<usize>,
    pub retention_days: Option<u32>,
}

impl SqliteMemoryProviderConfig {
    pub fn new(sqlite_path: impl Into<String>) -> Self {
        Self {
            sqlite_path: sqlite_path.into(),
            lancedb_path: None,
            embedder: None,
            embedding_model: None,
            reranker: None,
            summarizer: None,
            summary_threshold: None,
            retention_days: None,
        }
    }
}

impl SqliteMemoryProvider {
    pub async fn new(config: SqliteMemoryProviderConfig) -> Result<Self> {
        ensure_parent_dir(&config.sqlite_path)?;
        run_migrations(&config.sqlite_path).await?;

        let manager =
            AsyncDieselConnectionManager::<SqliteAsyncConn>::new(config.sqlite_path.as_str());
        let pool: SqlitePool = Pool::builder()
            .build(manager)
            .await
            .map_err(|e| ButterflyBotError::Runtime(e.to_string()))?;

        let lancedb = match config.lancedb_path.as_deref() {
            Some(path) if !path.trim().is_empty() => Some(LanceDbStore::new(path).await?),
            _ => None,
        };

        Ok(Self {
            pool,
            lancedb,
            embedder: config.embedder,
            embedding_model: config.embedding_model,
            reranker: config.reranker,
            summarizer: config.summarizer,
            summary_threshold: config.summary_threshold.unwrap_or(12),
            retention_days: config.retention_days,
            embedding_cache: Arc::new(tokio::sync::Mutex::new(LruCache::new(
                NonZeroUsize::new(256).unwrap(),
            ))),
        })
    }

    async fn conn(&self) -> Result<SqlitePooledConn<'_>> {
        self.pool
            .get()
            .await
            .map_err(|e| ButterflyBotError::Runtime(e.to_string()))
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

fn ensure_parent_dir(path: &str) -> Result<()> {
    let path = Path::new(path);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| ButterflyBotError::Runtime(e.to_string()))?;
    }
    Ok(())
}

async fn run_migrations(database_url: &str) -> Result<()> {
    let database_url = database_url.to_string();
    tokio::task::spawn_blocking(move || {
        let mut conn = SqliteConnection::establish(&database_url)
            .map_err(|e| ButterflyBotError::Runtime(e.to_string()))?;
        conn.run_pending_migrations(MIGRATIONS)
            .map_err(|e| ButterflyBotError::Runtime(e.to_string()))?;
        Ok::<_, ButterflyBotError>(())
    })
    .await
    .map_err(|e| ButterflyBotError::Runtime(e.to_string()))??;
    Ok(())
}

#[async_trait]
impl MemoryProvider for SqliteMemoryProvider {
    async fn append_message(&self, user_id: &str, role: &str, content: &str) -> Result<()> {
        let ts = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|e| ButterflyBotError::Runtime(e.to_string()))?
            .as_secs() as i64;
        let new_msg = NewMessage {
            user_id,
            role,
            content,
            timestamp: ts,
        };
        let mut conn = self.conn().await?;
        diesel::insert_into(messages::table)
            .values(&new_msg)
            .execute(&mut conn)
            .await
            .map_err(|e| ButterflyBotError::Runtime(e.to_string()))?;

        let row_id: RowId = diesel::sql_query("SELECT last_insert_rowid() as id")
            .get_result(&mut conn)
            .await
            .map_err(|e| ButterflyBotError::Runtime(e.to_string()))?;

        if let (Some(lancedb), Some(embedder)) = (&self.lancedb, &self.embedder) {
            let vectors = embedder
                .embed(vec![content.to_string()], self.embedding_model.as_deref())
                .await?;
            if let Some(vector) = vectors.into_iter().next() {
                let dim = vector.len() as i32;
                let table = lancedb.get_or_create_table(dim).await?;
                let batch = build_lancedb_batch(row_id.id, user_id, role, content, ts, vector)?;
                let schema = batch.schema();
                let batches = RecordBatchIterator::new(vec![Ok(batch)].into_iter(), schema);
                table
                    .add(batches)
                    .execute()
                    .await
                    .map_err(|e| ButterflyBotError::Runtime(e.to_string()))?;
            }
        }

        if role == "assistant" {
            let provider = self.clone();
            let user_id = user_id.to_string();
            tokio::spawn(async move {
                let _ = provider.maybe_summarize(&user_id).await;
            });
        }

        if let Some(days) = self.retention_days {
            let provider = self.clone();
            let user_id = user_id.to_string();
            tokio::spawn(async move {
                let _ = provider.apply_retention(&user_id, days).await;
            });
        }
        Ok(())
    }

    async fn get_history(&self, user_id: &str, limit: usize) -> Result<Vec<String>> {
        let mut conn = self.conn().await?;
        let mut query = messages::table
            .filter(messages::user_id.eq(user_id))
            .order(messages::timestamp.desc())
            .select((messages::role, messages::content, messages::timestamp))
            .into_boxed();

        if limit > 0 {
            query = query.limit(limit as i64);
        }

        let mut rows: Vec<MessageRow> = query
            .load(&mut conn)
            .await
            .map_err(|e| ButterflyBotError::Runtime(e.to_string()))?;
        rows.sort_by_key(|row| row.timestamp);
        Ok(rows
            .into_iter()
            .map(|row| {
                format!(
                    "[{}] {}: {}",
                    format_timestamp(row.timestamp),
                    row.role,
                    row.content
                )
            })
            .collect())
    }

    async fn clear_history(&self, user_id: &str) -> Result<()> {
        let mut conn = self.conn().await?;
        diesel::delete(messages::table.filter(messages::user_id.eq(user_id)))
            .execute(&mut conn)
            .await
            .map_err(|e| ButterflyBotError::Runtime(e.to_string()))?;
        Ok(())
    }

    async fn save_capture(
        &self,
        user_id: &str,
        capture_name: &str,
        agent_name: Option<&str>,
        data: Value,
        schema: Option<Value>,
    ) -> Result<Option<String>> {
        let ts = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|e| ButterflyBotError::Runtime(e.to_string()))?
            .as_secs() as i64;
        let data =
            serde_json::to_string(&data).map_err(|e| ButterflyBotError::Runtime(e.to_string()))?;
        let schema = match schema {
            Some(value) => Some(
                serde_json::to_string(&value)
                    .map_err(|e| ButterflyBotError::Runtime(e.to_string()))?,
            ),
            None => None,
        };

        let new_capture = NewCapture {
            user_id,
            capture_name,
            agent_name,
            data: &data,
            schema: schema.as_deref(),
            timestamp: ts,
        };

        let mut conn = self.conn().await?;
        diesel::insert_into(captures::table)
            .values(&new_capture)
            .execute(&mut conn)
            .await
            .map_err(|e| ButterflyBotError::Runtime(e.to_string()))?;
        Ok(None)
    }

    async fn search(&self, user_id: &str, query: &str, limit: usize) -> Result<Vec<String>> {
        let mut fts_results = self.search_fts(user_id, query, limit).await?;
        if fts_results.len() >= limit.max(1) {
            return Ok(fts_results.into_iter().take(limit.max(1)).collect());
        }
        let trimmed = query.trim();
        let tokens = trimmed.split_whitespace().count();
        let use_vector = tokens >= 4 && trimmed.len() >= 18;

        let vector_results = if use_vector {
            self.search_vector(user_id, query, limit).await?
        } else {
            Vec::new()
        };

        let mut merged = Vec::new();
        for item in fts_results.drain(..).chain(vector_results.into_iter()) {
            if !merged.contains(&item) {
                merged.push(item);
            }
        }

        if let Some(reranker) = &self.reranker {
            if merged.len() > limit.max(1) * 2 {
                let reranked = self
                    .rerank_with_model(reranker, query, &merged, limit)
                    .await?;
                return Ok(reranked);
            }
        }

        Ok(merged.into_iter().take(limit.max(1)).collect())
    }
}

impl SqliteMemoryProvider {
    fn sanitize_fts_query(query: &str) -> Option<String> {
        let mut sanitized = String::with_capacity(query.len());
        for ch in query.chars() {
            if ch.is_alphanumeric() || ch.is_whitespace() {
                sanitized.push(ch);
            } else {
                sanitized.push(' ');
            }
        }
        let trimmed = sanitized.split_whitespace().collect::<Vec<_>>().join(" ");
        if trimmed.is_empty() {
            None
        } else {
            Some(format!("\"{}\"", trimmed.replace('"', "")))
        }
    }

    async fn search_fts(&self, user_id: &str, query: &str, limit: usize) -> Result<Vec<String>> {
        let Some(query) = Self::sanitize_fts_query(query) else {
            return Ok(Vec::new());
        };
        let mut conn = self.conn().await?;
        let rows: Vec<SearchRow> = diesel::sql_query(
            "SELECT mem.summary as content, mem.created_at as timestamp\n             FROM memories_fts f\n             JOIN memories mem ON mem.id = f.memory_id\n             WHERE f.user_id = ?1 AND f.summary MATCH ?2\n             UNION ALL\n             SELECT m.content as content, m.timestamp as timestamp\n             FROM messages_fts f\n             JOIN messages m ON m.id = f.message_id\n             WHERE f.user_id = ?1 AND f.content MATCH ?2 AND m.role = 'user'\n             ORDER BY timestamp DESC\n             LIMIT ?3",
        )
        .bind::<Text, _>(user_id)
        .bind::<Text, _>(query)
        .bind::<BigInt, _>(limit.max(1) as i64)
        .load(&mut conn)
        .await
        .map_err(|e| ButterflyBotError::Runtime(e.to_string()))?;
        Ok(rows
            .into_iter()
            .map(|row| format!("[{}] {}", format_timestamp(row.timestamp), row.content))
            .collect())
    }

    async fn search_vector(&self, user_id: &str, query: &str, limit: usize) -> Result<Vec<String>> {
        let Some(lancedb) = &self.lancedb else {
            return Ok(Vec::new());
        };
        let Some(embedder) = &self.embedder else {
            return Ok(Vec::new());
        };
        let Some(table) = lancedb.open_table_if_exists().await? else {
            return Ok(Vec::new());
        };

        let model_key = self.embedding_model.as_deref().unwrap_or("default");
        let cache_key = format!("{model_key}:{query}");
        let cached = {
            let mut cache = self.embedding_cache.lock().await;
            cache.get(&cache_key).cloned()
        };
        let vector = if let Some(vector) = cached {
            vector
        } else {
            let vectors = embedder
                .embed(vec![query.to_string()], self.embedding_model.as_deref())
                .await?;
            let Some(vector) = vectors.into_iter().next() else {
                return Ok(Vec::new());
            };
            let mut cache = self.embedding_cache.lock().await;
            cache.put(cache_key, vector.clone());
            vector
        };

        use lancedb::query::QueryBase;
        let query = table
            .query()
            .nearest_to(vector)
            .map_err(|e| ButterflyBotError::Runtime(e.to_string()))?
            .only_if(format!("user_id = '{user_id}'"))
            .limit(limit.max(1));
        let stream = lancedb::query::ExecutableQuery::execute(&query)
            .await
            .map_err(|e| ButterflyBotError::Runtime(e.to_string()))?;

        let batches: Vec<RecordBatch> = stream
            .try_collect()
            .await
            .map_err(|e| ButterflyBotError::Runtime(e.to_string()))?;

        let mut results = Vec::new();
        for batch in batches {
            let content_array = batch
                .column_by_name("content")
                .and_then(|array| array.as_any().downcast_ref::<StringArray>());
            let ts_array = batch
                .column_by_name("timestamp")
                .and_then(|array| array.as_any().downcast_ref::<Int64Array>());
            if let (Some(strings), Some(timestamps)) = (content_array, ts_array) {
                for i in 0..strings.len() {
                    if strings.is_null(i) || timestamps.is_null(i) {
                        continue;
                    }
                    let ts = timestamps.value(i);
                    results.push(format!("[{}] {}", format_timestamp(ts), strings.value(i)));
                }
            }
        }
        Ok(results)
    }

    async fn rerank_with_model(
        &self,
        reranker: &Arc<dyn LlmProvider>,
        query: &str,
        candidates: &[String],
        limit: usize,
    ) -> Result<Vec<String>> {
        if candidates.is_empty() {
            return Ok(Vec::new());
        }
        let mut prompt = format!("Query: {query}\n\nCandidates:\n");
        for (idx, item) in candidates.iter().enumerate() {
            prompt.push_str(&format!("{idx}: {item}\n"));
        }
        prompt.push_str("\nReturn JSON {order:[...]} with the best indices in descending relevance. Use at most the requested limit.");

        let schema = json!({
            "type": "object",
            "properties": {
                "order": {"type": "array", "items": {"type": "integer"}}
            },
            "required": ["order"]
        });

        let system = "You are a reranking model. Return the best indices only.";
        let output = reranker
            .parse_structured_output(&prompt, system, schema, None)
            .await
            .unwrap_or_else(|_| json!({"order": []}));

        let order = output
            .get("order")
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default();

        let mut ranked = Vec::new();
        for idx in order.into_iter().filter_map(|v| v.as_u64()) {
            let idx = idx as usize;
            if let Some(item) = candidates.get(idx) {
                if !ranked.contains(item) {
                    ranked.push(item.clone());
                }
            }
            if ranked.len() >= limit.max(1) {
                break;
            }
        }

        if ranked.is_empty() {
            Ok(candidates.iter().take(limit.max(1)).cloned().collect())
        } else {
            Ok(ranked)
        }
    }

    async fn maybe_summarize(&self, user_id: &str) -> Result<()> {
        let Some(summarizer) = &self.summarizer else {
            return Ok(());
        };
        let mut conn = self.conn().await?;
        let count: CountRow =
            diesel::sql_query("SELECT COUNT(*) as count FROM messages WHERE user_id = ?1")
                .bind::<Text, _>(user_id)
                .get_result(&mut conn)
                .await
                .map_err(|e| ButterflyBotError::Runtime(e.to_string()))?;
        if count.count < self.summary_threshold as i64 {
            return Ok(());
        }

        let rows: Vec<MessageRow> = messages::table
            .filter(messages::user_id.eq(user_id))
            .order(messages::timestamp.desc())
            .limit(self.summary_threshold as i64)
            .select((messages::role, messages::content, messages::timestamp))
            .load(&mut conn)
            .await
            .map_err(|e| ButterflyBotError::Runtime(e.to_string()))?;

        let mut rows = rows;
        rows.sort_by_key(|row| row.timestamp);
        let transcript = rows
            .into_iter()
            .map(|row| {
                format!(
                    "[{}] {}: {}",
                    format_timestamp(row.timestamp),
                    row.role,
                    row.content
                )
            })
            .collect::<Vec<_>>()
            .join("\n");

        let schema = json!({
            "type": "object",
            "properties": {
                "summary": {"type": "string"},
                "tags": {"type": "array", "items": {"type": "string"}},
                "entities": {"type": "array", "items": {"type": "object", "properties": {
                    "name": {"type": "string"},
                    "type": {"type": "string"}
                }, "required": ["name", "type"]}},
                "facts": {"type": "array", "items": {"type": "object", "properties": {
                    "subject": {"type": "string"},
                    "predicate": {"type": "string"},
                    "object": {"type": "string"},
                    "confidence": {"type": "number"}
                }, "required": ["subject", "predicate", "object"]}}
            },
            "required": ["summary"]
        });

        let system = "You are a memory summarizer. Return JSON only.";
        let prompt =
            format!("Summarize the following conversation into a concise memory.\n\n{transcript}");
        let output = summarizer
            .parse_structured_output(&prompt, system, schema, None)
            .await
            .unwrap_or_else(|_| json!({"summary": transcript}));

        let summary = output
            .get("summary")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        if summary.trim().is_empty() {
            return Ok(());
        }
        let tags = output.get("tags").and_then(|v| v.as_array()).map(|items| {
            items
                .iter()
                .filter_map(|v| v.as_str())
                .collect::<Vec<_>>()
                .join(",")
        });

        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|e| ButterflyBotError::Runtime(e.to_string()))?
            .as_secs() as i64;

        let new_memory = NewMemory {
            user_id,
            summary: &summary,
            tags: tags.as_deref(),
            salience: None,
            created_at: now,
        };
        diesel::insert_into(crate::providers::sqlite::schema::memories::table)
            .values(&new_memory)
            .execute(&mut conn)
            .await
            .map_err(|e| ButterflyBotError::Runtime(e.to_string()))?;

        let memory_id: RowId = diesel::sql_query("SELECT last_insert_rowid() as id")
            .get_result(&mut conn)
            .await
            .map_err(|e| ButterflyBotError::Runtime(e.to_string()))?;

        if let Some(entities) = output.get("entities").and_then(|v| v.as_array()) {
            for entity in entities {
                let Some(name) = entity.get("name").and_then(|v| v.as_str()) else {
                    continue;
                };
                let entity_type = entity
                    .get("type")
                    .and_then(|v| v.as_str())
                    .unwrap_or("unknown");
                let new_entity = NewEntity {
                    user_id,
                    name,
                    entity_type,
                    canonical_id: None,
                    created_at: now,
                };
                diesel::insert_into(crate::providers::sqlite::schema::entities::table)
                    .values(&new_entity)
                    .execute(&mut conn)
                    .await
                    .map_err(|e| ButterflyBotError::Runtime(e.to_string()))?;
                let entity_id: RowId = diesel::sql_query("SELECT last_insert_rowid() as id")
                    .get_result(&mut conn)
                    .await
                    .map_err(|e| ButterflyBotError::Runtime(e.to_string()))?;

                let link = NewMemoryLink {
                    memory_id: memory_id.id as i32,
                    node_type: "entity",
                    node_id: entity_id.id as i32,
                    created_at: now,
                };
                diesel::insert_into(crate::providers::sqlite::schema::memory_links::table)
                    .values(&link)
                    .execute(&mut conn)
                    .await
                    .map_err(|e| ButterflyBotError::Runtime(e.to_string()))?;

                let edge = NewEdge {
                    user_id,
                    src_node_type: "memory",
                    src_node_id: memory_id.id as i32,
                    dst_node_type: "entity",
                    dst_node_id: entity_id.id as i32,
                    edge_type: "MENTIONED_IN",
                    weight: None,
                    created_at: now,
                };
                diesel::insert_into(crate::providers::sqlite::schema::edges::table)
                    .values(&edge)
                    .execute(&mut conn)
                    .await
                    .map_err(|e| ButterflyBotError::Runtime(e.to_string()))?;
            }
        }

        if let Some(facts) = output.get("facts").and_then(|v| v.as_array()) {
            for fact in facts {
                let (Some(subject), Some(predicate), Some(object)) = (
                    fact.get("subject").and_then(|v| v.as_str()),
                    fact.get("predicate").and_then(|v| v.as_str()),
                    fact.get("object").and_then(|v| v.as_str()),
                ) else {
                    continue;
                };
                let confidence = fact.get("confidence").and_then(|v| v.as_f64());
                let new_fact = NewFact {
                    user_id,
                    subject,
                    predicate,
                    object,
                    confidence,
                    source: None,
                    created_at: now,
                };
                diesel::insert_into(crate::providers::sqlite::schema::facts::table)
                    .values(&new_fact)
                    .execute(&mut conn)
                    .await
                    .map_err(|e| ButterflyBotError::Runtime(e.to_string()))?;
                let fact_id: RowId = diesel::sql_query("SELECT last_insert_rowid() as id")
                    .get_result(&mut conn)
                    .await
                    .map_err(|e| ButterflyBotError::Runtime(e.to_string()))?;

                let link = NewMemoryLink {
                    memory_id: memory_id.id as i32,
                    node_type: "fact",
                    node_id: fact_id.id as i32,
                    created_at: now,
                };
                diesel::insert_into(crate::providers::sqlite::schema::memory_links::table)
                    .values(&link)
                    .execute(&mut conn)
                    .await
                    .map_err(|e| ButterflyBotError::Runtime(e.to_string()))?;

                let edge = NewEdge {
                    user_id,
                    src_node_type: "memory",
                    src_node_id: memory_id.id as i32,
                    dst_node_type: "fact",
                    dst_node_id: fact_id.id as i32,
                    edge_type: "CONTAINS",
                    weight: None,
                    created_at: now,
                };
                diesel::insert_into(crate::providers::sqlite::schema::edges::table)
                    .values(&edge)
                    .execute(&mut conn)
                    .await
                    .map_err(|e| ButterflyBotError::Runtime(e.to_string()))?;
            }
        }

        Ok(())
    }

    async fn apply_retention(&self, user_id: &str, days: u32) -> Result<()> {
        let cutoff = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|e| ButterflyBotError::Runtime(e.to_string()))?
            .as_secs() as i64
            - (days as i64 * 24 * 60 * 60);

        let mut conn = self.conn().await?;
        diesel::delete(
            messages::table.filter(
                messages::user_id
                    .eq(user_id)
                    .and(messages::timestamp.lt(cutoff)),
            ),
        )
        .execute(&mut conn)
        .await
        .map_err(|e| ButterflyBotError::Runtime(e.to_string()))?;
        diesel::delete(
            crate::providers::sqlite::schema::memories::table.filter(
                crate::providers::sqlite::schema::memories::user_id
                    .eq(user_id)
                    .and(crate::providers::sqlite::schema::memories::created_at.lt(cutoff)),
            ),
        )
        .execute(&mut conn)
        .await
        .map_err(|e| ButterflyBotError::Runtime(e.to_string()))?;
        Ok(())
    }
}

fn build_lancedb_batch(
    id: i64,
    user_id: &str,
    role: &str,
    content: &str,
    timestamp: i64,
    vector: Vec<f32>,
) -> Result<RecordBatch> {
    let dim = vector.len() as i32;
    let schema = Arc::new(Schema::new(vec![
        Field::new("id", DataType::Int64, false),
        Field::new("user_id", DataType::Utf8, false),
        Field::new("role", DataType::Utf8, false),
        Field::new("content", DataType::Utf8, false),
        Field::new("timestamp", DataType::Int64, false),
        Field::new(
            "vector",
            DataType::FixedSizeList(Arc::new(Field::new("item", DataType::Float32, true)), dim),
            true,
        ),
    ]));

    let values: Vec<Option<f32>> = vector.into_iter().map(Some).collect();
    let vector_array = arrow_array::FixedSizeListArray::from_iter_primitive::<
        arrow_array::types::Float32Type,
        _,
        _,
    >(vec![Some(values)], dim);

    RecordBatch::try_new(
        schema,
        vec![
            Arc::new(Int64Array::from_iter_values([id])),
            Arc::new(StringArray::from_iter_values([user_id])),
            Arc::new(StringArray::from_iter_values([role])),
            Arc::new(StringArray::from_iter_values([content])),
            Arc::new(Int64Array::from_iter_values([timestamp])),
            Arc::new(vector_array),
        ],
    )
    .map_err(|e| ButterflyBotError::Runtime(e.to_string()))
}
