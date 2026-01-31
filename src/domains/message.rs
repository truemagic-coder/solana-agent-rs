use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MessageEnvelope {
    pub version: u8,
    pub sender_id: String,
    pub recipient_id: String,
    pub timestamp_ms: i64,
    pub payload: Vec<u8>,
}
