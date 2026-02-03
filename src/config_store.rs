use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use diesel::prelude::*;
use diesel::sql_types::Text;
use diesel::sqlite::SqliteConnection;
use serde_json::Value;

use crate::config::Config;
use crate::error::{ButterflyBotError, Result};

#[derive(QueryableByName)]
struct ConfigRow {
    #[diesel(sql_type = Text)]
    config_json: String,
}

pub fn ensure_parent_dir(path: &str) -> Result<()> {
    let path = Path::new(path);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| ButterflyBotError::Runtime(e.to_string()))?;
    }
    Ok(())
}

fn open_conn(db_path: &str) -> Result<SqliteConnection> {
    let mut conn =
        SqliteConnection::establish(db_path).map_err(|e| ButterflyBotError::Runtime(e.to_string()))?;
    crate::db::apply_sqlcipher_key_sync(&mut conn)?;
    Ok(conn)
}

fn ensure_table(conn: &mut SqliteConnection) -> Result<()> {
    diesel::sql_query(
        "CREATE TABLE IF NOT EXISTS app_config (
            id INTEGER PRIMARY KEY,
            config_json TEXT NOT NULL,
            updated_at INTEGER NOT NULL
        )",
    )
    .execute(conn)
    .map_err(|e| ButterflyBotError::Runtime(e.to_string()))?;
    Ok(())
}

pub fn load_config(db_path: &str) -> Result<Config> {
    ensure_parent_dir(db_path)?;
    let mut conn = open_conn(db_path)?;
    ensure_table(&mut conn)?;

    let row: ConfigRow = diesel::sql_query("SELECT config_json FROM app_config WHERE id = 1")
        .get_result(&mut conn)
        .map_err(|e| ButterflyBotError::Config(e.to_string()))?;

    let value: Value = serde_json::from_str(&row.config_json)
        .map_err(|e| ButterflyBotError::Config(e.to_string()))?;
    let config: Config =
        serde_json::from_value(value).map_err(|e| ButterflyBotError::Config(e.to_string()))?;
    Ok(config)
}

pub fn save_config(db_path: &str, config: &Config) -> Result<()> {
    ensure_parent_dir(db_path)?;
    let mut conn = open_conn(db_path)?;
    ensure_table(&mut conn)?;

    let ts = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|e| ButterflyBotError::Runtime(e.to_string()))?
        .as_secs() as i64;
    let config_json =
        serde_json::to_string(config).map_err(|e| ButterflyBotError::Config(e.to_string()))?;

    diesel::sql_query(
        "INSERT INTO app_config (id, config_json, updated_at)
         VALUES (1, ?1, ?2)
         ON CONFLICT(id) DO UPDATE SET config_json = excluded.config_json, updated_at = excluded.updated_at",
    )
    .bind::<Text, _>(config_json)
    .bind::<diesel::sql_types::BigInt, _>(ts)
    .execute(&mut conn)
    .map_err(|e| ButterflyBotError::Runtime(e.to_string()))?;

    Ok(())
}
