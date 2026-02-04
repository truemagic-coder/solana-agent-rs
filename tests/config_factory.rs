mod common;

use serde_json::json;

use butterfly_bot::config::{Config, OpenAiConfig};
use butterfly_bot::error::ButterflyBotError;
use butterfly_bot::factories::agent_factory::ButterflyBotFactory;

#[tokio::test]
async fn config_from_file_and_factory_errors() {
    let tmp = tempfile::NamedTempFile::new().unwrap();
    std::fs::write(
        tmp.path(),
        json!({
            "openai": {"api_key":"key","model":null,"base_url":null},
            "skill_file": null,
            "heartbeat_file": null
        })
        .to_string(),
    )
    .unwrap();
    let config = Config::from_file(tmp.path()).unwrap();
    let _ = ButterflyBotFactory::create_from_config(config)
        .await
        .unwrap();

    let no_key_with_base_url = Config {
        openai: Some(OpenAiConfig {
            api_key: None,
            model: None,
            base_url: Some("http://localhost:11434/v1".to_string()),
        }),
        skill_file: None,
        heartbeat_file: None,
        memory: None,
        tools: None,
        brains: None,
    };
    let _ = ButterflyBotFactory::create_from_config(no_key_with_base_url)
        .await
        .unwrap();

    let missing_key = Config {
        openai: Some(OpenAiConfig {
            api_key: None,
            model: None,
            base_url: None,
        }),
        skill_file: None,
        heartbeat_file: None,
        memory: None,
        tools: None,
        brains: None,
    };
    let err = ButterflyBotFactory::create_from_config(missing_key)
        .await
        .err()
        .unwrap();
    assert!(matches!(err, ButterflyBotError::Config(_)));

    let bad = tempfile::NamedTempFile::new().unwrap();
    std::fs::write(bad.path(), "{bad}").unwrap();
    let err = Config::from_file(bad.path()).unwrap_err();
    assert!(matches!(err, ButterflyBotError::Config(_)));

    let err = Config::from_file("/nope/not-found.json").unwrap_err();
    assert!(matches!(err, ButterflyBotError::Config(_)));

    let missing = Config {
        openai: None,
        skill_file: None,
        heartbeat_file: None,
        memory: None,
        tools: None,
        brains: None,
    };
    let err = ButterflyBotFactory::create_from_config(missing)
        .await
        .err()
        .unwrap();
    assert!(matches!(err, ButterflyBotError::Config(_)));

    let _ok: butterfly_bot::error::Result<()> = Ok(());
    let err = ButterflyBotError::Runtime("boom".to_string());
    assert!(format!("{err}").contains("boom"));
}
