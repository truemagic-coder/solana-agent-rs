use async_trait::async_trait;
use tokio::net::TcpStream;

use arti_client::{TorClient, TorClientConfig};
use tor_rtcompat::PreferredRuntime;

use crate::error::{ButterflyBotError, Result};
use crate::interfaces::transport::{BoxedStream, Transport};

pub struct TorTransport {
    client: TorClient<PreferredRuntime>,
}

impl TorTransport {
    pub async fn new() -> Result<Self> {
        let config = TorClientConfig::default();
        let client = TorClient::create_bootstrapped(config)
            .await
            .map_err(|e| ButterflyBotError::Runtime(e.to_string()))?;
        Ok(Self { client })
    }
}

#[async_trait]
impl Transport for TorTransport {
    async fn connect(&self, host: &str, port: u16) -> Result<BoxedStream> {
        let stream = self
            .client
            .connect((host, port))
            .await
            .map_err(|e| ButterflyBotError::Runtime(e.to_string()))?;
        Ok(Box::new(stream))
    }
}

pub struct LocalTransport;

#[async_trait]
impl Transport for LocalTransport {
    async fn connect(&self, host: &str, port: u16) -> Result<BoxedStream> {
        let addr = format!("{host}:{port}");
        let stream = TcpStream::connect(addr)
            .await
            .map_err(|e| ButterflyBotError::Runtime(e.to_string()))?;
        Ok(Box::new(stream))
    }
}
