use std::time::{SystemTime, UNIX_EPOCH};

use diesel::prelude::*;
use diesel::sql_types::{BigInt, Text};
use diesel::sqlite::SqliteConnection;

use crate::error::{ButterflyBotError, Result};
use crate::sqlcipher::apply_sqlcipher_key;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrustState {
    Unverified,
    Verified,
    Blocked,
}

impl TrustState {
    fn as_str(self) -> &'static str {
        match self {
            TrustState::Unverified => "unverified",
            TrustState::Verified => "verified",
            TrustState::Blocked => "blocked",
        }
    }

    fn from_str(value: &str) -> Self {
        match value {
            "verified" => TrustState::Verified,
            "blocked" => TrustState::Blocked,
            _ => TrustState::Unverified,
        }
    }
}

#[derive(Debug, Clone)]
pub struct PeerKeyRecord {
    pub user_id: String,
    pub peer_id: String,
    pub public_key: String,
    pub trust_state: TrustState,
    pub updated_at: i64,
}

#[derive(QueryableByName)]
struct PeerKeyRow {
    #[diesel(sql_type = Text)]
    user_id: String,
    #[diesel(sql_type = Text)]
    peer_id: String,
    #[diesel(sql_type = Text)]
    public_key: String,
    #[diesel(sql_type = Text)]
    trust_state: String,
    #[diesel(sql_type = BigInt)]
    updated_at: i64,
}

pub fn upsert_peer_key(
    db_path: &str,
    user_id: &str,
    peer_id: &str,
    public_key: &str,
    trust_state: TrustState,
) -> Result<()> {
    let mut conn = open_conn(db_path)?;
    ensure_table(&mut conn)?;
    let now = now_ts();
    let updated = diesel::sql_query(
        "UPDATE peer_keys SET public_key = ?1, trust_state = ?2, updated_at = ?3 WHERE user_id = ?4 AND peer_id = ?5",
    )
    .bind::<Text, _>(public_key)
    .bind::<Text, _>(trust_state.as_str())
    .bind::<BigInt, _>(now)
    .bind::<Text, _>(user_id)
    .bind::<Text, _>(peer_id)
    .execute(&mut conn)
    .map_err(|e| ButterflyBotError::Runtime(e.to_string()))?;
    if updated == 0 {
        diesel::sql_query(
            "INSERT INTO peer_keys (user_id, peer_id, public_key, trust_state, updated_at) VALUES (?1, ?2, ?3, ?4, ?5)",
        )
        .bind::<Text, _>(user_id)
        .bind::<Text, _>(peer_id)
        .bind::<Text, _>(public_key)
        .bind::<Text, _>(trust_state.as_str())
        .bind::<BigInt, _>(now)
        .execute(&mut conn)
        .map_err(|e| ButterflyBotError::Runtime(e.to_string()))?;
    }
    Ok(())
}

pub fn set_trust_state(db_path: &str, user_id: &str, peer_id: &str, state: TrustState) -> Result<()> {
    let mut conn = open_conn(db_path)?;
    ensure_table(&mut conn)?;
    let now = now_ts();
    diesel::sql_query(
        "UPDATE peer_keys SET trust_state = ?1, updated_at = ?2 WHERE user_id = ?3 AND peer_id = ?4",
    )
    .bind::<Text, _>(state.as_str())
    .bind::<BigInt, _>(now)
    .bind::<Text, _>(user_id)
    .bind::<Text, _>(peer_id)
    .execute(&mut conn)
    .map_err(|e| ButterflyBotError::Runtime(e.to_string()))?;
    Ok(())
}

pub fn get_peer_key(db_path: &str, user_id: &str, peer_id: &str) -> Result<Option<PeerKeyRecord>> {
    let mut conn = open_conn(db_path)?;
    ensure_table(&mut conn)?;
    let row = diesel::sql_query(
        "SELECT user_id, peer_id, public_key, trust_state, updated_at FROM peer_keys WHERE user_id = ?1 AND peer_id = ?2",
    )
    .bind::<Text, _>(user_id)
    .bind::<Text, _>(peer_id)
    .get_result::<PeerKeyRow>(&mut conn)
    .optional()
    .map_err(|e| ButterflyBotError::Runtime(e.to_string()))?;

    Ok(row.map(|row| PeerKeyRecord {
        user_id: row.user_id,
        peer_id: row.peer_id,
        public_key: row.public_key,
        trust_state: TrustState::from_str(&row.trust_state),
        updated_at: row.updated_at,
    }))
}

fn ensure_table(conn: &mut SqliteConnection) -> Result<()> {
    diesel::sql_query(
        "CREATE TABLE IF NOT EXISTS peer_keys (\n            user_id TEXT NOT NULL,\n            peer_id TEXT NOT NULL,\n            public_key TEXT NOT NULL,\n            trust_state TEXT NOT NULL,\n            updated_at INTEGER NOT NULL,\n            PRIMARY KEY (user_id, peer_id)\n        )",
    )
    .execute(conn)
    .map_err(|e| ButterflyBotError::Runtime(e.to_string()))?;
    Ok(())
}

fn open_conn(db_path: &str) -> Result<SqliteConnection> {
    let mut conn =
        SqliteConnection::establish(db_path).map_err(|e| ButterflyBotError::Runtime(e.to_string()))?;
    apply_sqlcipher_key(&mut conn, db_path)?;
    Ok(conn)
}

fn now_ts() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| ())
        .unwrap_or_default()
        .as_secs() as i64
}
