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
use schema::wakeup_tasks;

const MIGRATIONS: EmbeddedMigrations = embed_migrations!();

type SqliteAsyncConn = SyncConnectionWrapper<SqliteConnection>;
type SqlitePool = Pool<SqliteAsyncConn>;
type SqlitePooledConn<'a> = PooledConnection<'a, SqliteAsyncConn>;

#[derive(Debug, Clone, Serialize)]
pub struct WakeupTask {
    pub id: i32,
    pub user_id: String,
    pub name: String,
    pub prompt: String,
    pub interval_minutes: i64,
    pub enabled: bool,
    pub created_at: i64,
    pub updated_at: i64,
    pub last_run_at: Option<i64>,
    pub next_run_at: i64,
}

#[derive(Queryable)]
struct WakeupRow {
    id: i32,
    user_id: String,
    name: String,
    prompt: String,
    interval_minutes: i64,
    enabled: bool,
    created_at: i64,
    updated_at: i64,
    last_run_at: Option<i64>,
    next_run_at: i64,
}

#[derive(Insertable)]
#[diesel(table_name = wakeup_tasks)]
struct NewWakeup<'a> {
    user_id: &'a str,
    name: &'a str,
    prompt: &'a str,
    interval_minutes: i64,
    enabled: bool,
    created_at: i64,
    updated_at: i64,
    last_run_at: Option<i64>,
    next_run_at: i64,
}

#[derive(Clone, Copy)]
pub enum WakeupStatus {
    Enabled,
    Disabled,
    All,
}

impl WakeupStatus {
    pub fn from_option(value: Option<&str>) -> Self {
        match value {
            Some("enabled") => Self::Enabled,
            Some("disabled") => Self::Disabled,
            _ => Self::All,
        }
    }
}

pub struct WakeupStore {
    pool: SqlitePool,
}

impl WakeupStore {
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

    pub async fn create_task(
        &self,
        user_id: &str,
        name: &str,
        prompt: &str,
        interval_minutes: i64,
    ) -> Result<WakeupTask> {
        let now = now_ts();
        let next_run_at = now + interval_minutes.max(1) * 60;
        let new = NewWakeup {
            user_id,
            name,
            prompt,
            interval_minutes: interval_minutes.max(1),
            enabled: true,
            created_at: now,
            updated_at: now,
            last_run_at: None,
            next_run_at,
        };

        let mut conn = self.conn().await?;
        diesel::insert_into(wakeup_tasks::table)
            .values(&new)
            .execute(&mut conn)
            .await
            .map_err(|e| ButterflyBotError::Runtime(e.to_string()))?;

        let row: WakeupRow = wakeup_tasks::table
            .filter(wakeup_tasks::user_id.eq(user_id))
            .order(wakeup_tasks::id.desc())
            .first(&mut conn)
            .await
            .map_err(|e| ButterflyBotError::Runtime(e.to_string()))?;
        Ok(map_row(row))
    }

    pub async fn list_tasks(
        &self,
        user_id: &str,
        status: WakeupStatus,
        limit: usize,
    ) -> Result<Vec<WakeupTask>> {
        let mut conn = self.conn().await?;
        let mut query = wakeup_tasks::table
            .filter(wakeup_tasks::user_id.eq(user_id))
            .into_boxed();

        match status {
            WakeupStatus::Enabled => {
                query = query.filter(wakeup_tasks::enabled.eq(true));
            }
            WakeupStatus::Disabled => {
                query = query.filter(wakeup_tasks::enabled.eq(false));
            }
            WakeupStatus::All => {}
        }

        let rows: Vec<WakeupRow> = query
            .order(wakeup_tasks::next_run_at.asc())
            .limit(limit as i64)
            .load(&mut conn)
            .await
            .map_err(|e| ButterflyBotError::Runtime(e.to_string()))?;
        Ok(rows.into_iter().map(map_row).collect())
    }

    pub async fn set_enabled(&self, id: i32, enabled: bool) -> Result<WakeupTask> {
        let now = now_ts();
        let mut conn = self.conn().await?;
        diesel::update(wakeup_tasks::table.filter(wakeup_tasks::id.eq(id)))
            .set((
                wakeup_tasks::enabled.eq(enabled),
                wakeup_tasks::updated_at.eq(now),
            ))
            .execute(&mut conn)
            .await
            .map_err(|e| ButterflyBotError::Runtime(e.to_string()))?;

        let row: WakeupRow = wakeup_tasks::table
            .filter(wakeup_tasks::id.eq(id))
            .first(&mut conn)
            .await
            .map_err(|e| ButterflyBotError::Runtime(e.to_string()))?;
        Ok(map_row(row))
    }

    pub async fn delete_task(&self, id: i32) -> Result<bool> {
        let mut conn = self.conn().await?;
        let count = diesel::delete(wakeup_tasks::table.filter(wakeup_tasks::id.eq(id)))
            .execute(&mut conn)
            .await
            .map_err(|e| ButterflyBotError::Runtime(e.to_string()))?;
        Ok(count > 0)
    }

    pub async fn list_due(&self, now: i64, limit: usize) -> Result<Vec<WakeupTask>> {
        let mut conn = self.conn().await?;
        let rows: Vec<WakeupRow> = wakeup_tasks::table
            .filter(wakeup_tasks::enabled.eq(true))
            .filter(wakeup_tasks::next_run_at.le(now))
            .order(wakeup_tasks::next_run_at.asc())
            .limit(limit as i64)
            .load(&mut conn)
            .await
            .map_err(|e| ButterflyBotError::Runtime(e.to_string()))?;
        Ok(rows.into_iter().map(map_row).collect())
    }

    pub async fn mark_run(&self, id: i32, last_run_at: i64, next_run_at: i64) -> Result<()> {
        let now = now_ts();
        let mut conn = self.conn().await?;
        diesel::update(wakeup_tasks::table.filter(wakeup_tasks::id.eq(id)))
            .set((
                wakeup_tasks::last_run_at.eq(Some(last_run_at)),
                wakeup_tasks::next_run_at.eq(next_run_at),
                wakeup_tasks::updated_at.eq(now),
            ))
            .execute(&mut conn)
            .await
            .map_err(|e| ButterflyBotError::Runtime(e.to_string()))?;
        Ok(())
    }

    async fn conn(&self) -> Result<SqlitePooledConn<'_>> {
        let mut conn = self
            .pool
            .get()
            .await
            .map_err(|e| ButterflyBotError::Runtime(e.to_string()))?;
        crate::db::apply_sqlcipher_key_async(&mut conn).await?;
        Ok(conn)
    }
}

pub fn resolve_wakeup_db_path(config: &serde_json::Value) -> Option<String> {
    config
        .get("tools")
        .and_then(|v| v.get("wakeup"))
        .and_then(|v| v.get("sqlite_path"))
        .and_then(|v| v.as_str())
        .map(|v| v.trim().to_string())
        .filter(|path| !path.is_empty())
}

pub fn default_wakeup_db_path() -> String {
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
        crate::db::apply_sqlcipher_key_sync(&mut conn)?;
        conn.run_pending_migrations(MIGRATIONS)
            .map_err(|e| ButterflyBotError::Runtime(e.to_string()))?;
        Ok::<_, ButterflyBotError>(())
    })
    .await
    .map_err(|e| ButterflyBotError::Runtime(e.to_string()))??;
    Ok(())
}

fn map_row(row: WakeupRow) -> WakeupTask {
    WakeupTask {
        id: row.id,
        user_id: row.user_id,
        name: row.name,
        prompt: row.prompt,
        interval_minutes: row.interval_minutes,
        enabled: row.enabled,
        created_at: row.created_at,
        updated_at: row.updated_at,
        last_run_at: row.last_run_at,
        next_run_at: row.next_run_at,
    }
}

fn now_ts() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}
