use butterfly_bot::sqlcipher::get_or_create_db_key;

#[test]
fn db_key_env_override() {
    std::env::set_var("BUTTERFLY_BOT_DB_KEY", "test-key");
    let key = get_or_create_db_key("/tmp/butterfly-bot-test.db").unwrap();
    assert_eq!(key, "test-key");
    std::env::remove_var("BUTTERFLY_BOT_DB_KEY");
}
