use tempfile::NamedTempFile;

use butterfly_bot::e2e::trust_store::{get_peer_key, set_trust_state, upsert_peer_key, TrustState};

#[test]
fn trust_store_roundtrip() {
    std::env::set_var("BUTTERFLY_BOT_DB_KEY", "test-key");
    let db = NamedTempFile::new().unwrap();
    let path = db.path().to_str().unwrap();

    upsert_peer_key(path, "alice", "bob", "pubkey", TrustState::Unverified).unwrap();
    let record = get_peer_key(path, "alice", "bob").unwrap().unwrap();
    assert_eq!(record.public_key, "pubkey");
    assert_eq!(record.trust_state, TrustState::Unverified);

    set_trust_state(path, "alice", "bob", TrustState::Verified).unwrap();
    let record = get_peer_key(path, "alice", "bob").unwrap().unwrap();
    assert_eq!(record.trust_state, TrustState::Verified);

    std::env::remove_var("BUTTERFLY_BOT_DB_KEY");
}