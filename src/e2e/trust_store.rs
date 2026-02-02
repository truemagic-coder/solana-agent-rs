use std::time::{SystemTime, UNIX_EPOCH};

use diesel::prelude::*;
use diesel::sql_types::{BigInt, Nullable, Text};
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
    pub fn as_str(self) -> &'static str {
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

#[derive(Debug, Clone)]
pub struct ContactRecord {
    pub user_id: String,
    pub peer_id: String,
    pub label: String,
    pub onion_address: String,
    pub trust_state: TrustState,
    pub public_key: Option<String>,
    pub updated_at: i64,
}

#[derive(Debug, Clone)]
pub struct UsernameRecord {
    pub username: String,
    pub peer_id: String,
    pub public_key: String,
    pub p2p_addr: String,
    pub seq: i64,
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

#[derive(QueryableByName)]
struct ContactRow {
    #[diesel(sql_type = Text)]
    user_id: String,
    #[diesel(sql_type = Text)]
    peer_id: String,
    #[diesel(sql_type = Text)]
    label: String,
    #[diesel(sql_type = Text)]
    onion_address: String,
    #[diesel(sql_type = Nullable<Text>)]
    public_key: Option<String>,
    #[diesel(sql_type = Nullable<Text>)]
    trust_state: Option<String>,
    #[diesel(sql_type = BigInt)]
    updated_at: i64,
}

#[derive(QueryableByName)]
struct UsernameRow {
    #[diesel(sql_type = Text)]
    username: String,
    #[diesel(sql_type = Text)]
    peer_id: String,
    #[diesel(sql_type = Text)]
    public_key: String,
    #[diesel(sql_type = Text)]
    p2p_addr: String,
    #[diesel(sql_type = BigInt)]
    seq: i64,
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
    diesel::sql_query(
        "CREATE TABLE IF NOT EXISTS contacts (\n            user_id TEXT NOT NULL,\n            peer_id TEXT NOT NULL,\n            label TEXT NOT NULL,\n            onion_address TEXT NOT NULL,\n            updated_at INTEGER NOT NULL,\n            PRIMARY KEY (user_id, peer_id)\n        )",
    )
    .execute(conn)
    .map_err(|e| ButterflyBotError::Runtime(e.to_string()))?;
    diesel::sql_query(
        "CREATE TABLE IF NOT EXISTS usernames (\n            username TEXT NOT NULL PRIMARY KEY,\n            peer_id TEXT NOT NULL,\n            public_key TEXT NOT NULL,\n            p2p_addr TEXT NOT NULL,\n            seq INTEGER NOT NULL,\n            updated_at INTEGER NOT NULL\n        )",
    )
    .execute(conn)
    .map_err(|e| ButterflyBotError::Runtime(e.to_string()))?;
    let _ = diesel::sql_query("ALTER TABLE usernames ADD COLUMN p2p_addr TEXT")
        .execute(conn);
    Ok(())
}

pub fn upsert_contact(
    db_path: &str,
    user_id: &str,
    peer_id: &str,
    label: &str,
    onion_address: &str,
) -> Result<()> {
    let mut conn = open_conn(db_path)?;
    ensure_table(&mut conn)?;
    let now = now_ts();
    let updated = diesel::sql_query(
        "UPDATE contacts SET label = ?1, onion_address = ?2, updated_at = ?3 WHERE user_id = ?4 AND peer_id = ?5",
    )
    .bind::<Text, _>(label)
    .bind::<Text, _>(onion_address)
    .bind::<BigInt, _>(now)
    .bind::<Text, _>(user_id)
    .bind::<Text, _>(peer_id)
    .execute(&mut conn)
    .map_err(|e| ButterflyBotError::Runtime(e.to_string()))?;
    if updated == 0 {
        diesel::sql_query(
            "INSERT INTO contacts (user_id, peer_id, label, onion_address, updated_at) VALUES (?1, ?2, ?3, ?4, ?5)",
        )
        .bind::<Text, _>(user_id)
        .bind::<Text, _>(peer_id)
        .bind::<Text, _>(label)
        .bind::<Text, _>(onion_address)
        .bind::<BigInt, _>(now)
        .execute(&mut conn)
        .map_err(|e| ButterflyBotError::Runtime(e.to_string()))?;
    }
    Ok(())
}

pub fn list_contacts(db_path: &str, user_id: &str) -> Result<Vec<ContactRecord>> {
    let mut conn = open_conn(db_path)?;
    ensure_table(&mut conn)?;
    let rows = diesel::sql_query(
        "SELECT c.user_id, c.peer_id, c.label, c.onion_address, c.updated_at, pk.public_key, pk.trust_state\n         FROM contacts c\n         LEFT JOIN peer_keys pk ON c.user_id = pk.user_id AND c.peer_id = pk.peer_id\n         WHERE c.user_id = ?1\n         ORDER BY c.updated_at DESC",
    )
    .bind::<Text, _>(user_id)
    .get_results::<ContactRow>(&mut conn)
    .map_err(|e| ButterflyBotError::Runtime(e.to_string()))?;

    Ok(rows
        .into_iter()
        .map(|row| ContactRecord {
            user_id: row.user_id,
            peer_id: row.peer_id,
            label: row.label,
            onion_address: row.onion_address,
            trust_state: row
                .trust_state
                .as_deref()
                .map(TrustState::from_str)
                .unwrap_or(TrustState::Unverified),
            public_key: row.public_key,
            updated_at: row.updated_at,
        })
        .collect())
}

pub fn get_contact(db_path: &str, user_id: &str, peer_id: &str) -> Result<Option<ContactRecord>> {
    let mut conn = open_conn(db_path)?;
    ensure_table(&mut conn)?;
    let row = diesel::sql_query(
        "SELECT c.user_id, c.peer_id, c.label, c.onion_address, c.updated_at, pk.public_key, pk.trust_state\n         FROM contacts c\n         LEFT JOIN peer_keys pk ON c.user_id = pk.user_id AND c.peer_id = pk.peer_id\n         WHERE c.user_id = ?1 AND c.peer_id = ?2",
    )
    .bind::<Text, _>(user_id)
    .bind::<Text, _>(peer_id)
    .get_result::<ContactRow>(&mut conn)
    .optional()
    .map_err(|e| ButterflyBotError::Runtime(e.to_string()))?;

    Ok(row.map(|row| ContactRecord {
        user_id: row.user_id,
        peer_id: row.peer_id,
        label: row.label,
        onion_address: row.onion_address,
        trust_state: row
            .trust_state
            .as_deref()
            .map(TrustState::from_str)
            .unwrap_or(TrustState::Unverified),
        public_key: row.public_key,
        updated_at: row.updated_at,
    }))
}

pub fn upsert_username_claim(
    db_path: &str,
    username: &str,
    peer_id: &str,
    public_key: &str,
    p2p_addr: &str,
    seq: i64,
) -> Result<()> {
    let mut conn = open_conn(db_path)?;
    ensure_table(&mut conn)?;
    let now = now_ts();
    let existing = diesel::sql_query(
        "SELECT username, peer_id, public_key, p2p_addr, seq, updated_at FROM usernames WHERE username = ?1",
    )
    .bind::<Text, _>(username)
    .get_result::<UsernameRow>(&mut conn)
    .optional()
    .map_err(|e| ButterflyBotError::Runtime(e.to_string()))?;

    if let Some(row) = existing {
        if row.public_key != public_key {
            return Ok(());
        }
        if seq <= row.seq {
            return Ok(());
        }
        diesel::sql_query(
            "UPDATE usernames SET peer_id = ?1, public_key = ?2, p2p_addr = ?3, seq = ?4, updated_at = ?5 WHERE username = ?6",
        )
        .bind::<Text, _>(peer_id)
        .bind::<Text, _>(public_key)
        .bind::<Text, _>(p2p_addr)
        .bind::<BigInt, _>(seq)
        .bind::<BigInt, _>(now)
        .bind::<Text, _>(username)
        .execute(&mut conn)
        .map_err(|e| ButterflyBotError::Runtime(e.to_string()))?;
        return Ok(());
    }

    diesel::sql_query(
        "INSERT INTO usernames (username, peer_id, public_key, p2p_addr, seq, updated_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
    )
    .bind::<Text, _>(username)
    .bind::<Text, _>(peer_id)
    .bind::<Text, _>(public_key)
    .bind::<Text, _>(p2p_addr)
    .bind::<BigInt, _>(seq)
    .bind::<BigInt, _>(now)
    .execute(&mut conn)
    .map_err(|e| ButterflyBotError::Runtime(e.to_string()))?;
    Ok(())
}

pub fn lookup_username(db_path: &str, username: &str) -> Result<Option<UsernameRecord>> {
    let mut conn = open_conn(db_path)?;
    ensure_table(&mut conn)?;
    let row = diesel::sql_query(
        "SELECT username, peer_id, public_key, p2p_addr, seq, updated_at FROM usernames WHERE username = ?1",
    )
    .bind::<Text, _>(username)
    .get_result::<UsernameRow>(&mut conn)
    .optional()
    .map_err(|e| ButterflyBotError::Runtime(e.to_string()))?;

    Ok(row.map(|row| UsernameRecord {
        username: row.username,
        peer_id: row.peer_id,
        public_key: row.public_key,
        p2p_addr: row.p2p_addr,
        seq: row.seq,
        updated_at: row.updated_at,
    }))
}

pub fn get_username_by_public_key(
    db_path: &str,
    public_key: &str,
) -> Result<Option<UsernameRecord>> {
    let mut conn = open_conn(db_path)?;
    ensure_table(&mut conn)?;
    let row = diesel::sql_query(
        "SELECT username, peer_id, public_key, p2p_addr, seq, updated_at FROM usernames WHERE public_key = ?1 ORDER BY seq DESC LIMIT 1",
    )
    .bind::<Text, _>(public_key)
    .get_result::<UsernameRow>(&mut conn)
    .optional()
    .map_err(|e| ButterflyBotError::Runtime(e.to_string()))?;

    Ok(row.map(|row| UsernameRecord {
        username: row.username,
        peer_id: row.peer_id,
        public_key: row.public_key,
        p2p_addr: row.p2p_addr,
        seq: row.seq,
        updated_at: row.updated_at,
    }))
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
