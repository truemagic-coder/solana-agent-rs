use async_trait::async_trait;
use tokio::io::{AsyncRead, AsyncWrite};

use crate::error::Result;

pub trait AsyncReadWrite: AsyncRead + AsyncWrite {}

impl<T> AsyncReadWrite for T where T: AsyncRead + AsyncWrite {}

pub type BoxedStream = Box<dyn AsyncReadWrite + Unpin + Send>;

#[async_trait]
pub trait Transport: Send + Sync {
    async fn connect(&self, host: &str, port: u16) -> Result<BoxedStream>;
}
