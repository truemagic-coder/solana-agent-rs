use butterfly_bot::tor_spike::tor_http_get;

fn tor_host() -> String {
    std::env::var("TOR_SPIKE_HOST").unwrap_or_else(|_| "example.com".to_string())
}

fn tor_port() -> u16 {
    std::env::var("TOR_SPIKE_PORT")
        .ok()
        .and_then(|v| v.parse::<u16>().ok())
        .unwrap_or(80)
}

#[tokio::test]
#[ignore = "requires Tor network access; run with TOR_SPIKE_HOST/PORT if desired"]
async fn tor_spike_can_fetch_http() {
    let host = tor_host();
    let port = tor_port();
    let result = tor_http_get(&host, port).await.unwrap();

    assert!(result.bytes > 0);
    assert!(result.elapsed_ms > 0);
    assert!(!result.response.is_empty());
}
