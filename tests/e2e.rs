use std::sync::Arc;

use butterfly_bot::e2e::{establish_session, IdentityKeypair};
use butterfly_bot::e2e::identity_store::MemoryIdentityStore;
use butterfly_bot::e2e::manager::E2eManager;
use x25519_dalek::PublicKey;

#[test]
fn e2e_encrypt_decrypt_roundtrip() {
    let alice = IdentityKeypair::generate();
    let bob = IdentityKeypair::generate();

    let alice_session = establish_session(&alice, &PublicKey::from(&bob.private)).unwrap();
    let bob_session = establish_session(&bob, &PublicKey::from(&alice.private)).unwrap();

    let message = b"hello-e2e";
    let envelope = alice_session
        .encrypt(alice.public.to_bytes(), message)
        .unwrap();
    let decrypted = bob_session.decrypt(&envelope).unwrap();

    assert_eq!(decrypted, message);
}

#[test]
fn e2e_manager_roundtrip() {
    let store = Arc::new(MemoryIdentityStore::new());
    let manager = E2eManager::new(store.clone());
    let alice_id = manager.identity_public_key("alice").unwrap();
    let bob_id = manager.identity_public_key("bob").unwrap();

    let envelope = manager
        .encrypt_for("alice", bob_id, b"hello-manager")
        .unwrap();
    let plaintext = manager.decrypt_for("bob", &envelope).unwrap();

    assert_eq!(plaintext, b"hello-manager");
    assert_eq!(envelope.sender_public_key, alice_id);
}
