use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

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
use schema::reminders;

const MIGRATIONS: EmbeddedMigrations = embed_migrations!();

type SqliteAsyncConn = SyncConnectionWrapper<SqliteConnection>;
type SqlitePool = Pool<SqliteAsyncConn>;
type SqlitePooledConn<'a> = PooledConnection<'a, SqliteAsyncConn>;

#[derive(Debug, Clone, Serialize)]
pub struct ReminderItem {
    pub id: i32,
    pub title: String,
    pub due_at: i64,
    pub created_at: i64,
    pub completed_at: Option<i64>,
    pub fired_at: Option<i64>,
}

#[derive(Queryable)]
struct ReminderRow {
    id: i32,
    _user_id: String,
    title: String,
    due_at: i64,
    created_at: i64,
    completed_at: Option<i64>,
    fired_at: Option<i64>,
}

#[derive(Insertable)]
#[diesel(table_name = reminders)]
struct NewReminder<'a> {
    user_id: &'a str,
    title: &'a str,
    due_at: i64,
    created_at: i64,
    completed_at: Option<i64>,
    fired_at: Option<i64>,
}

pub struct ReminderStore {
    pool: SqlitePool,
}

impl ReminderStore {
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

    pub async fn create_reminder(
        &self,
        user_id: &str,
        title: &str,
        due_at: i64,
    ) -> Result<ReminderItem> {
        let now = now_ts();
        let new = NewReminder {
            user_id,
            title,
            due_at,
            created_at: now,
            completed_at: None,
            fired_at: None,
        };

        let mut conn = self.conn().await?;
        diesel::insert_into(reminders::table)
            .values(&new)
            .execute(&mut conn)
            .await
            .map_err(|e| ButterflyBotError::Runtime(e.to_string()))?;

        let row: ReminderRow = reminders::table
            .filter(reminders::user_id.eq(user_id))
            .order(reminders::id.desc())
            .first(&mut conn)
            .await
            .map_err(|e| ButterflyBotError::Runtime(e.to_string()))?;
        Ok(map_row(row))
    }

    pub async fn list_reminders(
        &self,
        user_id: &str,
        status: ReminderStatus,
        limit: usize,
    ) -> Result<Vec<ReminderItem>> {
        let mut conn = self.conn().await?;
        let mut query = reminders::table
            .filter(reminders::user_id.eq(user_id))
            .into_boxed();

        match status {
            ReminderStatus::Open => {
                query = query.filter(reminders::completed_at.is_null());
            }
            ReminderStatus::Completed => {
                query = query.filter(reminders::completed_at.is_not_null());
            }
            ReminderStatus::All => {}
        }

        if limit > 0 {
            query = query.limit(limit as i64);
        }

        let rows: Vec<ReminderRow> = query
            .order(reminders::due_at.asc())
            .load(&mut conn)
            .await
            .map_err(|e| ButterflyBotError::Runtime(e.to_string()))?;
        Ok(rows.into_iter().map(map_row).collect())
    }

    pub async fn complete_reminder(&self, user_id: &str, id: i32) -> Result<bool> {
        let now = now_ts();
        let mut conn = self.conn().await?;
        let updated = diesel::update(
            reminders::table
                .filter(reminders::user_id.eq(user_id))
                .filter(reminders::id.eq(id)),
        )
        .set(reminders::completed_at.eq(Some(now)))
        .execute(&mut conn)
        .await
        .map_err(|e| ButterflyBotError::Runtime(e.to_string()))?;
        Ok(updated > 0)
    }

    pub async fn delete_reminder(&self, user_id: &str, id: i32) -> Result<bool> {
        let mut conn = self.conn().await?;
        let deleted = diesel::delete(
            reminders::table
                .filter(reminders::user_id.eq(user_id))
                .filter(reminders::id.eq(id)),
        )
        .execute(&mut conn)
        .await
        .map_err(|e| ButterflyBotError::Runtime(e.to_string()))?;
        Ok(deleted > 0)
    }

    pub async fn delete_all(&self, user_id: &str, include_completed: bool) -> Result<usize> {
        let mut conn = self.conn().await?;
        let deleted = if include_completed {
            diesel::delete(reminders::table.filter(reminders::user_id.eq(user_id)))
                .execute(&mut conn)
                .await
                .map_err(|e| ButterflyBotError::Runtime(e.to_string()))?
        } else {
            diesel::delete(
                reminders::table
                    .filter(reminders::user_id.eq(user_id))
                    .filter(reminders::completed_at.is_null()),
            )
            .execute(&mut conn)
            .await
            .map_err(|e| ButterflyBotError::Runtime(e.to_string()))?
        };
        Ok(deleted)
    }

    pub async fn snooze_reminder(&self, user_id: &str, id: i32, due_at: i64) -> Result<bool> {
        let mut conn = self.conn().await?;
        let updated = diesel::update(
            reminders::table
                .filter(reminders::user_id.eq(user_id))
                .filter(reminders::id.eq(id)),
        )
        .set((
            reminders::due_at.eq(due_at),
            reminders::fired_at.eq::<Option<i64>>(None),
        ))
        .execute(&mut conn)
        .await
        .map_err(|e| ButterflyBotError::Runtime(e.to_string()))?;
        Ok(updated > 0)
    }

    pub async fn due_reminders(
        &self,
        user_id: &str,
        now: i64,
        limit: usize,
    ) -> Result<Vec<ReminderItem>> {
        let mut conn = self.conn().await?;
        let mut query = reminders::table
            .filter(reminders::user_id.eq(user_id))
            .filter(reminders::completed_at.is_null())
            .filter(reminders::due_at.le(now))
            .filter(reminders::fired_at.is_null())
            .into_boxed();
        if limit > 0 {
            query = query.limit(limit as i64);
        }
        let rows: Vec<ReminderRow> = query
            .order(reminders::due_at.asc())
            .load(&mut conn)
            .await
            .map_err(|e| ButterflyBotError::Runtime(e.to_string()))?;

        if !rows.is_empty() {
            let ids: Vec<i32> = rows.iter().map(|row| row.id).collect();
            diesel::update(
                reminders::table
                    .filter(reminders::user_id.eq(user_id))
                    .filter(reminders::id.eq_any(&ids)),
            )
            .set(reminders::fired_at.eq(Some(now)))
            .execute(&mut conn)
            .await
            .map_err(|e| ButterflyBotError::Runtime(e.to_string()))?;
        }

        Ok(rows.into_iter().map(map_row).collect())
    }

    async fn conn(&self) -> Result<SqlitePooledConn<'_>> {
        self.pool
            .get()
            .await
            .map_err(|e| ButterflyBotError::Runtime(e.to_string()))
    }
}

#[derive(Debug, Clone, Copy)]
pub enum ReminderStatus {
    Open,
    Completed,
    All,
}

impl ReminderStatus {
    pub fn from_option(value: Option<&str>) -> Self {
        value
            .and_then(|raw| raw.parse().ok())
            .unwrap_or(ReminderStatus::Open)
    }
}

impl std::str::FromStr for ReminderStatus {
    type Err = ();

    fn from_str(value: &str) -> std::result::Result<Self, Self::Err> {
        Ok(match value {
            "completed" => ReminderStatus::Completed,
            "all" => ReminderStatus::All,
            _ => ReminderStatus::Open,
        })
    }
}

fn map_row(row: ReminderRow) -> ReminderItem {
    ReminderItem {
        id: row.id,
        title: row.title,
        due_at: row.due_at,
        created_at: row.created_at,
        completed_at: row.completed_at,
        fired_at: row.fired_at,
    }
}

fn now_ts() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
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

pub fn resolve_reminder_db_path(config: &serde_json::Value) -> Option<String> {
    let tool_path = config
        .get("tools")
        .and_then(|v| v.get("reminders"))
        .and_then(|v| v.get("sqlite_path"))
        .and_then(|v| v.as_str())
        .map(|v| v.trim().to_string());
    if let Some(path) = tool_path {
        if !path.is_empty() {
            return Some(path);
        }
    }
    let memory_path = config
        .get("memory")
        .and_then(|v| v.get("sqlite_path"))
        .and_then(|v| v.as_str())
        .map(|v| v.trim().to_string());
    if let Some(path) = memory_path {
        if !path.is_empty() {
            return Some(path);
        }
    }
    None
}

pub fn default_reminder_db_path() -> String {
    "./data/butterfly-bot.db".to_string()
}
