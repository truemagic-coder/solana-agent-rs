use std::env;

use diesel::sql_types::Text;
use diesel::sqlite::SqliteConnection;
use diesel_async::sync_connection_wrapper::SyncConnectionWrapper;

use crate::error::{ButterflyBotError, Result};

const DB_KEY_NAME: &str = "db_encryption_key";

pub fn get_sqlcipher_key() -> Result<Option<String>> {
    if let Ok(value) = env::var("BUTTERFLY_BOT_DB_KEY") {
        let trimmed = value.trim();
        if !trimmed.is_empty() {
            return Ok(Some(trimmed.to_string()));
        }
    }
    crate::vault::get_secret(DB_KEY_NAME)
}

pub fn apply_sqlcipher_key_sync(conn: &mut SqliteConnection) -> Result<()> {
    let Some(key) = get_sqlcipher_key()? else {
        return Ok(());
    };
    diesel::RunQueryDsl::execute(
        diesel::sql_query("PRAGMA key = ?1").bind::<Text, _>(key),
        conn,
    )
    .map_err(|e| ButterflyBotError::Runtime(e.to_string()))?;
    Ok(())
}

pub async fn apply_sqlcipher_key_async(
    conn: &mut SyncConnectionWrapper<SqliteConnection>,
) -> Result<()> {
    let Some(key) = get_sqlcipher_key()? else {
        return Ok(());
    };
    diesel_async::RunQueryDsl::execute(
        diesel::sql_query("PRAGMA key = ?1").bind::<Text, _>(key),
        conn,
    )
    .await
    .map_err(|e| ButterflyBotError::Runtime(e.to_string()))?;
    Ok(())
}
