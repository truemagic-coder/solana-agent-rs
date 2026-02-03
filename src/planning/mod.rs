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
use serde_json::Value;

use crate::error::{ButterflyBotError, Result};

mod schema;
use schema::plans;

const MIGRATIONS: EmbeddedMigrations = embed_migrations!();

type SqliteAsyncConn = SyncConnectionWrapper<SqliteConnection>;
type SqlitePool = Pool<SqliteAsyncConn>;
type SqlitePooledConn<'a> = PooledConnection<'a, SqliteAsyncConn>;

#[derive(Debug, Clone, Serialize)]
pub struct PlanItem {
    pub id: i32,
    pub user_id: String,
    pub title: String,
    pub goal: String,
    pub steps: Option<Value>,
    pub status: String,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Queryable)]
struct PlanRow {
    id: i32,
    user_id: String,
    title: String,
    goal: String,
    steps_json: Option<String>,
    status: String,
    created_at: i64,
    updated_at: i64,
}

#[derive(Insertable)]
#[diesel(table_name = plans)]
struct NewPlan<'a> {
    user_id: &'a str,
    title: &'a str,
    goal: &'a str,
    steps_json: Option<&'a str>,
    status: &'a str,
    created_at: i64,
    updated_at: i64,
}

pub struct PlanStore {
    pool: SqlitePool,
}

impl PlanStore {
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

    pub async fn create_plan(
        &self,
        user_id: &str,
        title: &str,
        goal: &str,
        steps: Option<&Value>,
        status: Option<&str>,
    ) -> Result<PlanItem> {
        let now = now_ts();
        let steps_json = steps.map(|value| value.to_string());
        let status = status.unwrap_or("draft");
        let new = NewPlan {
            user_id,
            title,
            goal,
            steps_json: steps_json.as_deref(),
            status,
            created_at: now,
            updated_at: now,
        };

        let mut conn = self.conn().await?;
        diesel::insert_into(plans::table)
            .values(&new)
            .execute(&mut conn)
            .await
            .map_err(|e| ButterflyBotError::Runtime(e.to_string()))?;

        let row: PlanRow = plans::table
            .filter(plans::user_id.eq(user_id))
            .order(plans::id.desc())
            .first(&mut conn)
            .await
            .map_err(|e| ButterflyBotError::Runtime(e.to_string()))?;
        Ok(map_row(row))
    }

    pub async fn list_plans(&self, user_id: &str, limit: usize) -> Result<Vec<PlanItem>> {
        let mut conn = self.conn().await?;
        let rows: Vec<PlanRow> = plans::table
            .filter(plans::user_id.eq(user_id))
            .order(plans::created_at.desc())
            .limit(limit as i64)
            .load(&mut conn)
            .await
            .map_err(|e| ButterflyBotError::Runtime(e.to_string()))?;
        Ok(rows.into_iter().map(map_row).collect())
    }

    pub async fn get_plan(&self, id: i32) -> Result<PlanItem> {
        let mut conn = self.conn().await?;
        let row: PlanRow = plans::table
            .filter(plans::id.eq(id))
            .first(&mut conn)
            .await
            .map_err(|e| ButterflyBotError::Runtime(e.to_string()))?;
        Ok(map_row(row))
    }

    pub async fn update_plan(
        &self,
        id: i32,
        title: Option<&str>,
        goal: Option<&str>,
        steps: Option<&Value>,
        status: Option<&str>,
    ) -> Result<PlanItem> {
        let now = now_ts();
        let mut conn = self.conn().await?;

        if let Some(title) = title {
            diesel::update(plans::table.filter(plans::id.eq(id)))
                .set((plans::title.eq(title), plans::updated_at.eq(now)))
                .execute(&mut conn)
                .await
                .map_err(|e| ButterflyBotError::Runtime(e.to_string()))?;
        }
        if let Some(goal) = goal {
            diesel::update(plans::table.filter(plans::id.eq(id)))
                .set((plans::goal.eq(goal), plans::updated_at.eq(now)))
                .execute(&mut conn)
                .await
                .map_err(|e| ButterflyBotError::Runtime(e.to_string()))?;
        }
        if let Some(steps) = steps {
            diesel::update(plans::table.filter(plans::id.eq(id)))
                .set((plans::steps_json.eq(Some(steps.to_string())), plans::updated_at.eq(now)))
                .execute(&mut conn)
                .await
                .map_err(|e| ButterflyBotError::Runtime(e.to_string()))?;
        }
        if let Some(status) = status {
            diesel::update(plans::table.filter(plans::id.eq(id)))
                .set((plans::status.eq(status), plans::updated_at.eq(now)))
                .execute(&mut conn)
                .await
                .map_err(|e| ButterflyBotError::Runtime(e.to_string()))?;
        }

        let row: PlanRow = plans::table
            .filter(plans::id.eq(id))
            .first(&mut conn)
            .await
            .map_err(|e| ButterflyBotError::Runtime(e.to_string()))?;
        Ok(map_row(row))
    }

    pub async fn delete_plan(&self, id: i32) -> Result<bool> {
        let mut conn = self.conn().await?;
        let count = diesel::delete(plans::table.filter(plans::id.eq(id)))
            .execute(&mut conn)
            .await
            .map_err(|e| ButterflyBotError::Runtime(e.to_string()))?;
        Ok(count > 0)
    }

    async fn conn(&self) -> Result<SqlitePooledConn<'_>> {
        self.pool
            .get()
            .await
            .map_err(|e| ButterflyBotError::Runtime(e.to_string()))
    }
}

pub fn resolve_plan_db_path(config: &serde_json::Value) -> Option<String> {
    config
        .get("tools")
        .and_then(|v| v.get("planning"))
        .and_then(|v| v.get("sqlite_path"))
        .and_then(|v| v.as_str())
        .map(|v| v.trim().to_string())
        .filter(|path| !path.is_empty())
}

pub fn default_plan_db_path() -> String {
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

fn map_row(row: PlanRow) -> PlanItem {
    PlanItem {
        id: row.id,
        user_id: row.user_id,
        title: row.title,
        goal: row.goal,
        steps: row
            .steps_json
            .and_then(|value| serde_json::from_str(&value).ok()),
        status: row.status,
        created_at: row.created_at,
        updated_at: row.updated_at,
    }
}

fn now_ts() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}
