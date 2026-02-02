use chacha20poly1305::aead::{Aead, AeadCore, KeyInit};
use chacha20poly1305::{ChaCha20Poly1305, Key, Nonce};
use hkdf::Hkdf;
use rand_core::OsRng;
use sha2::Sha256;
use x25519_dalek::{PublicKey, StaticSecret};

use crate::error::{ButterflyBotError, Result};

pub mod identity_store;
pub mod manager;
pub mod trust_store;

const E2E_VERSION: u8 = 1;
const NONCE_LEN: usize = 12;

#[derive(Clone)]
pub struct IdentityKeypair {
    pub private: StaticSecret,
    pub public: PublicKey,
}

impl IdentityKeypair {
    pub fn generate() -> Self {
        let private = StaticSecret::random_from_rng(OsRng);
        let public = PublicKey::from(&private);
        Self { private, public }
    }
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct E2eEnvelope {
    pub version: u8,
    pub sender_public_key: [u8; 32],
    pub nonce: [u8; NONCE_LEN],
    pub ciphertext: Vec<u8>,
}

pub struct E2eSession {
    key: Key,
}

impl E2eSession {
    pub fn from_shared_secret(shared_secret: [u8; 32], context: &[u8]) -> Result<Self> {
        let hk = Hkdf::<Sha256>::new(None, &shared_secret);
        let mut okm = [0u8; 32];
        hk.expand(context, &mut okm)
            .map_err(|_| ButterflyBotError::Runtime("HKDF expand failed".to_string()))?;
        Ok(Self {
            key: Key::from_slice(&okm).to_owned(),
        })
    }

    pub fn encrypt(&self, sender_public_key: [u8; 32], plaintext: &[u8]) -> Result<E2eEnvelope> {
        let cipher = ChaCha20Poly1305::new(&self.key);
        let nonce = ChaCha20Poly1305::generate_nonce(&mut OsRng);
        let ciphertext = cipher
            .encrypt(&nonce, plaintext)
            .map_err(|_| ButterflyBotError::Runtime("encrypt failed".to_string()))?;
        Ok(E2eEnvelope {
            version: E2E_VERSION,
            sender_public_key,
            nonce: nonce.into(),
            ciphertext,
        })
    }

    pub fn decrypt(&self, envelope: &E2eEnvelope) -> Result<Vec<u8>> {
        if envelope.version != E2E_VERSION {
            return Err(ButterflyBotError::Runtime("unsupported e2e version".to_string()));
        }
        let cipher = ChaCha20Poly1305::new(&self.key);
        let nonce = Nonce::from_slice(&envelope.nonce);
        cipher
            .decrypt(nonce, envelope.ciphertext.as_ref())
            .map_err(|_| ButterflyBotError::Runtime("decrypt failed".to_string()))
    }
}

pub fn establish_session(
    our_identity: &IdentityKeypair,
    their_public_key: &PublicKey,
) -> Result<E2eSession> {
    let shared = our_identity.private.diffie_hellman(their_public_key);
    let context = b"butterfly-bot-e2e-v1";
    E2eSession::from_shared_secret(shared.to_bytes(), context)
}
