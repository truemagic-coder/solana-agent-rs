use std::collections::HashMap;
use std::sync::Mutex;

use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine;
use keyring::Entry;
use x25519_dalek::{PublicKey, StaticSecret};

use crate::e2e::IdentityKeypair;
use crate::error::{ButterflyBotError, Result};

pub trait IdentityStore: Send + Sync {
    fn get_or_create(&self, user_id: &str) -> Result<IdentityKeypair>;
}

pub struct KeyringIdentityStore {
    service: String,
}

impl KeyringIdentityStore {
    pub fn new(service: &str) -> Self {
        Self {
            service: service.to_string(),
        }
    }

    fn entry(&self, user_id: &str) -> Result<Entry> {
        Entry::new(&self.service, user_id)
            .map_err(|e| ButterflyBotError::Runtime(e.to_string()))
    }

    fn decode_private_key(encoded: &str) -> Result<StaticSecret> {
        let bytes = BASE64
            .decode(encoded)
            .map_err(|e| ButterflyBotError::Runtime(e.to_string()))?;
        if bytes.len() != 32 {
            return Err(ButterflyBotError::Runtime(
                "invalid identity key length".to_string(),
            ));
        }
        let mut key = [0u8; 32];
        key.copy_from_slice(&bytes);
        Ok(StaticSecret::from(key))
    }

    fn encode_private_key(private: &StaticSecret) -> String {
        BASE64.encode(private.to_bytes())
    }
}

impl IdentityStore for KeyringIdentityStore {
    fn get_or_create(&self, user_id: &str) -> Result<IdentityKeypair> {
        let entry = self.entry(user_id)?;
        if let Ok(password) = entry.get_password() {
            let private = Self::decode_private_key(&password)?;
            let public = PublicKey::from(&private);
            return Ok(IdentityKeypair { private, public });
        }

        let identity = IdentityKeypair::generate();
        let encoded = Self::encode_private_key(&identity.private);
        entry
            .set_password(&encoded)
            .map_err(|e| ButterflyBotError::Runtime(e.to_string()))?;
        Ok(identity)
    }
}

pub struct MemoryIdentityStore {
    keys: Mutex<HashMap<String, IdentityKeypair>>,
}

impl MemoryIdentityStore {
    pub fn new() -> Self {
        Self {
            keys: Mutex::new(HashMap::new()),
        }
    }
}

impl Default for MemoryIdentityStore {
    fn default() -> Self {
        Self::new()
    }
}

impl IdentityStore for MemoryIdentityStore {
    fn get_or_create(&self, user_id: &str) -> Result<IdentityKeypair> {
        let mut keys = self
            .keys
            .lock()
            .map_err(|_| ButterflyBotError::Runtime("identity store locked".to_string()))?;
        if let Some(existing) = keys.get(user_id) {
            return Ok(existing.clone());
        }
        let identity = IdentityKeypair::generate();
        keys.insert(user_id.to_string(), identity.clone());
        Ok(identity)
    }
}