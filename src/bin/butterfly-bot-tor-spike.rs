use std::env;
use butterfly_bot::tor_spike::tor_http_get;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    butterfly_bot::sqlcipher::configure_sqlcipher_logging();
    let host = env::args()
        .nth(1)
        .unwrap_or_else(|| "example.com".to_string());
    let port = env::args()
        .nth(2)
        .unwrap_or_else(|| "80".to_string())
        .parse::<u16>()?;

    println!("[tor-spike] connecting to {host}:{port} over Tor...");
    let result = tor_http_get(&host, port).await?;
    println!("[tor-spike] response bytes: {}", result.bytes);
    println!("[tor-spike] elapsed_ms: {}", result.elapsed_ms);
    println!("{}", result.response);

    Ok(())
}
