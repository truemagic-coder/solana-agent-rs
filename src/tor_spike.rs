use std::time::Instant;

use tokio::io::{AsyncReadExt, AsyncWriteExt};

use crate::error::{ButterflyBotError, Result};
use crate::interfaces::transport::Transport;
use crate::services::transport::TorTransport;

pub struct TorSpikeResult {
    pub bytes: usize,
    pub elapsed_ms: u128,
    pub response: String,
}

pub async fn tor_http_get(host: &str, port: u16) -> Result<TorSpikeResult> {
    let tor_client = TorTransport::new().await?;

    let start = Instant::now();
    let mut stream = tor_client.connect(host, port).await?;

    let request = format!(
        "GET / HTTP/1.1\r\nHost: {host}\r\nConnection: close\r\n\r\n"
    );
    stream
        .write_all(request.as_bytes())
        .await
        .map_err(|e| ButterflyBotError::Runtime(e.to_string()))?;
    stream
        .flush()
        .await
        .map_err(|e| ButterflyBotError::Runtime(e.to_string()))?;

    let mut buf = Vec::new();
    stream
        .read_to_end(&mut buf)
        .await
        .map_err(|e| ButterflyBotError::Runtime(e.to_string()))?;

    let elapsed_ms = start.elapsed().as_millis();
    let response = String::from_utf8_lossy(&buf).to_string();

    Ok(TorSpikeResult {
        bytes: buf.len(),
        elapsed_ms,
        response,
    })
}
