use std::env;
use std::ffi::CString;
use std::os::raw::c_char;
use std::sync::Once;

use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine;
use diesel::connection::Connection;
use diesel::prelude::*;
use diesel::sqlite::{Sqlite, SqliteConnection};
use diesel_async::sync_connection_wrapper::SyncConnectionWrapper;
use keyring::Entry;
use rand_core::{OsRng, RngCore};

use crate::error::{ButterflyBotError, Result};

const DB_KEY_SERVICE: &str = "butterfly-bot.db";

pub fn configure_sqlcipher_logging() {
    static INIT: Once = Once::new();
    INIT.call_once(|| unsafe {
        let mut db: *mut libsqlite3_sys::sqlite3 = std::ptr::null_mut();
        let name = CString::new(":memory:").unwrap();
        let rc = libsqlite3_sys::sqlite3_open(name.as_ptr(), &mut db);
        if rc != libsqlite3_sys::SQLITE_OK {
            if !db.is_null() {
                libsqlite3_sys::sqlite3_close(db);
            }
            return;
        }

        let pragma = CString::new(
            "PRAGMA cipher_log_level = ERROR; PRAGMA cipher_log_source = NONE;",
        )
        .unwrap();
        let mut err: *mut c_char = std::ptr::null_mut();
        let _ = libsqlite3_sys::sqlite3_exec(
            db,
            pragma.as_ptr(),
            None,
            std::ptr::null_mut(),
            &mut err,
        );
        if !err.is_null() {
            libsqlite3_sys::sqlite3_free(err as *mut _);
        }
        libsqlite3_sys::sqlite3_close(db);
    });
}

pub fn get_or_create_db_key(db_path: &str) -> Result<String> {
    if let Ok(value) = env::var("BUTTERFLY_BOT_DB_KEY") {
        let trimmed = value.trim().to_string();
        if !trimmed.is_empty() {
            return Ok(trimmed);
        }
    }

    let entry = Entry::new(DB_KEY_SERVICE, db_path)
        .map_err(|e| ButterflyBotError::Runtime(e.to_string()))?;
    if let Ok(password) = entry.get_password() {
        return Ok(password);
    }

    let mut raw = [0u8; 32];
    OsRng.fill_bytes(&mut raw);
    let key = BASE64.encode(raw);
    entry
        .set_password(&key)
        .map_err(|e| ButterflyBotError::Runtime(e.to_string()))?;
    Ok(key)
}

pub fn apply_sqlcipher_key<C>(conn: &mut C, db_path: &str) -> Result<()>
where
    C: Connection<Backend = Sqlite>,
{
    let key = get_or_create_db_key(db_path)?;
    let escaped = escape_sql(&key);
    let pragma = format!("PRAGMA key = '{}';", escaped);
    diesel::sql_query(pragma)
        .execute(conn)
        .map_err(|e| ButterflyBotError::Runtime(e.to_string()))?;
    diesel::sql_query("PRAGMA cipher_compatibility = 4")
        .execute(conn)
        .map_err(|e| ButterflyBotError::Runtime(e.to_string()))?;
    Ok(())
}

pub async fn apply_sqlcipher_key_async(
    conn: &mut SyncConnectionWrapper<SqliteConnection>,
    db_path: &str,
) -> Result<()> {
    use diesel_async::RunQueryDsl as AsyncRunQueryDsl;

    let key = get_or_create_db_key(db_path)?;
    let escaped = escape_sql(&key);
    let pragma = format!("PRAGMA key = '{}';", escaped);
    AsyncRunQueryDsl::execute(diesel::sql_query(pragma), conn)
        .await
        .map_err(|e| ButterflyBotError::Runtime(e.to_string()))?;
    AsyncRunQueryDsl::execute(diesel::sql_query("PRAGMA cipher_compatibility = 4"), conn)
        .await
        .map_err(|e| ButterflyBotError::Runtime(e.to_string()))?;
    Ok(())
}

fn escape_sql(value: &str) -> String {
    value.replace('\'', "''")
}
