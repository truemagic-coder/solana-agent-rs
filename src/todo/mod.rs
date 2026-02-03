use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use diesel::dsl::max;
use diesel::prelude::*;
use diesel::sqlite::SqliteConnection;
use diesel_async::pooled_connection::bb8::{Pool, PooledConnection};
use diesel_async::pooled_connection::AsyncDieselConnectionManager;
use diesel_async::sync_connection_wrapper::SyncConnectionWrapper;
use diesel_async::RunQueryDsl;
use diesel_migrations::{embed_migrations, EmbeddedMigrations, MigrationHarness};
use serde::Serialize;

use crate::error::{ButterflyBotError, Result};

mod schema;
use schema::todo_items;

const MIGRATIONS: EmbeddedMigrations = embed_migrations!();

type SqliteAsyncConn = SyncConnectionWrapper<SqliteConnection>;
type SqlitePool = Pool<SqliteAsyncConn>;
type SqlitePooledConn<'a> = PooledConnection<'a, SqliteAsyncConn>;

#[derive(Debug, Clone, Serialize)]
pub struct TodoItem {
    pub id: i32,
    pub user_id: String,
    pub title: String,
    pub notes: Option<String>,
    pub position: i32,
    pub created_at: i64,
    pub updated_at: i64,
    pub completed_at: Option<i64>,
}

#[derive(Queryable)]
struct TodoRow {
    id: i32,
    user_id: String,
    title: String,
    notes: Option<String>,
    position: i32,
    created_at: i64,
    updated_at: i64,
    completed_at: Option<i64>,
}

#[derive(Insertable)]
#[diesel(table_name = todo_items)]
struct NewTodo<'a> {
    user_id: &'a str,
    title: &'a str,
    notes: Option<&'a str>,
    position: i32,
    created_at: i64,
    updated_at: i64,
    completed_at: Option<i64>,
}

#[derive(Clone, Copy)]
pub enum TodoStatus {
    Open,
    Completed,
    All,
}

impl TodoStatus {
    pub fn from_option(value: Option<&str>) -> Self {
        match value {
            Some("completed") => Self::Completed,
            Some("open") => Self::Open,
            _ => Self::All,
        }
    }
}

pub struct TodoStore {
    pool: SqlitePool,
}

impl TodoStore {
    pub async fn new(sqlite_path: impl AsRef<str>) -> Result<Self> {
        let sqlite_path = sqlite_path.as_ref();
        ensure_parent_dir(sqlite_path)?;
        run_migrations(sqlite_path).await?;

        let manager = AsyncDieselConnectionManager::<SqliteAsyncConn>::new(sqlite_path);
        let pool: SqlitePool = Pool::builder()
            .build(manager)
            .await
            .map_err(|e| ButterflyBotError::Runtime(e.to_string()))?;
        Ok(Self { pool })
    }

    pub async fn create_item(
        &self,
        user_id: &str,
        title: &str,
        notes: Option<&str>,
    ) -> Result<TodoItem> {
        let now = now_ts();
        let mut conn = self.conn().await?;
        let max_pos: Option<i32> = todo_items::table
            .filter(todo_items::user_id.eq(user_id))
            .select(max(todo_items::position))
            .first::<Option<i32>>(&mut conn)
            .await
            .unwrap_or(None);
        let position = max_pos.unwrap_or(0) + 1;

        let new = NewTodo {
            user_id,
            title,
            notes,
            position,
            created_at: now,
            updated_at: now,
            completed_at: None,
        };

        diesel::insert_into(todo_items::table)
            .values(&new)
            .execute(&mut conn)
            .await
            .map_err(|e| ButterflyBotError::Runtime(e.to_string()))?;

        let row: TodoRow = todo_items::table
            .filter(todo_items::user_id.eq(user_id))
            .order(todo_items::id.desc())
            .first(&mut conn)
            .await
            .map_err(|e| ButterflyBotError::Runtime(e.to_string()))?;
        Ok(map_row(row))
    }

    pub async fn list_items(
        &self,
        user_id: &str,
        status: TodoStatus,
        limit: usize,
    ) -> Result<Vec<TodoItem>> {
        let mut conn = self.conn().await?;
        let mut query = todo_items::table
            .filter(todo_items::user_id.eq(user_id))
            .into_boxed();

        match status {
            TodoStatus::Open => {
                query = query.filter(todo_items::completed_at.is_null());
            }
            TodoStatus::Completed => {
                query = query.filter(todo_items::completed_at.is_not_null());
            }
            TodoStatus::All => {}
        }

        let rows: Vec<TodoRow> = query
            .order(todo_items::position.asc())
            .limit(limit as i64)
            .load(&mut conn)
            .await
            .map_err(|e| ButterflyBotError::Runtime(e.to_string()))?;
        Ok(rows.into_iter().map(map_row).collect())
    }

    pub async fn set_completed(&self, id: i32, completed: bool) -> Result<TodoItem> {
        let now = now_ts();
        let completed_at = if completed { Some(now) } else { None };
        let mut conn = self.conn().await?;
        diesel::update(todo_items::table.filter(todo_items::id.eq(id)))
            .set((
                todo_items::completed_at.eq(completed_at),
                todo_items::updated_at.eq(now),
            ))
            .execute(&mut conn)
            .await
            .map_err(|e| ButterflyBotError::Runtime(e.to_string()))?;

        let row: TodoRow = todo_items::table
            .filter(todo_items::id.eq(id))
            .first(&mut conn)
            .await
            .map_err(|e| ButterflyBotError::Runtime(e.to_string()))?;
        Ok(map_row(row))
    }

    pub async fn delete_item(&self, id: i32) -> Result<bool> {
        let mut conn = self.conn().await?;
        let count = diesel::delete(todo_items::table.filter(todo_items::id.eq(id)))
            .execute(&mut conn)
            .await
            .map_err(|e| ButterflyBotError::Runtime(e.to_string()))?;
        Ok(count > 0)
    }

    pub async fn reorder(&self, user_id: &str, ordered_ids: &[i32]) -> Result<()> {
        let now = now_ts();
        let mut conn = self.conn().await?;
        for (idx, id) in ordered_ids.iter().enumerate() {
            diesel::update(
                todo_items::table
                    .filter(todo_items::user_id.eq(user_id))
                    .filter(todo_items::id.eq(*id)),
            )
            .set((
                todo_items::position.eq((idx + 1) as i32),
                todo_items::updated_at.eq(now),
            ))
            .execute(&mut conn)
            .await
            .map_err(|e| ButterflyBotError::Runtime(e.to_string()))?;
        }
        Ok(())
    }

    async fn conn(&self) -> Result<SqlitePooledConn<'_>> {
        self.pool
            .get()
            .await
            .map_err(|e| ButterflyBotError::Runtime(e.to_string()))
    }
}

pub fn resolve_todo_db_path(config: &serde_json::Value) -> Option<String> {
    config
        .get("tools")
        .and_then(|v| v.get("todo"))
        .and_then(|v| v.get("sqlite_path"))
        .and_then(|v| v.as_str())
        .map(|v| v.trim().to_string())
        .filter(|path| !path.is_empty())
}

pub fn default_todo_db_path() -> String {
    "./data/butterfly-bot.db".to_string()
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

fn map_row(row: TodoRow) -> TodoItem {
    TodoItem {
        id: row.id,
        user_id: row.user_id,
        title: row.title,
        notes: row.notes,
        position: row.position,
        created_at: row.created_at,
        updated_at: row.updated_at,
        completed_at: row.completed_at,
    }
}

fn now_ts() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}
