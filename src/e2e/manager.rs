use std::sync::Arc;

use x25519_dalek::PublicKey;

use crate::e2e::{establish_session, E2eEnvelope};
use crate::e2e::identity_store::IdentityStore;
use crate::error::Result;

pub struct E2eManager {
    store: Arc<dyn IdentityStore>,
}

impl E2eManager {
    pub fn new(store: Arc<dyn IdentityStore>) -> Self {
        Self { store }
    }

    pub fn identity_public_key(&self, user_id: &str) -> Result<[u8; 32]> {
        let identity = self.store.get_or_create(user_id)?;
        Ok(identity.public.to_bytes())
    }

    pub fn encrypt_for(
        &self,
        user_id: &str,
        peer_public_key: [u8; 32],
        plaintext: &[u8],
    ) -> Result<E2eEnvelope> {
        let identity = self.store.get_or_create(user_id)?;
        let peer = PublicKey::from(peer_public_key);
        let session = establish_session(&identity, &peer)?;
        session.encrypt(identity.public.to_bytes(), plaintext)
    }

    pub fn decrypt_for(&self, user_id: &str, envelope: &E2eEnvelope) -> Result<Vec<u8>> {
        let identity = self.store.get_or_create(user_id)?;
        let peer = PublicKey::from(envelope.sender_public_key);
        let session = establish_session(&identity, &peer)?;
        session.decrypt(envelope)
    }
}