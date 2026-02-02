use bytes::Bytes;
use futures::{stream::BoxStream, StreamExt};
use http::header::{AUTHORIZATION, CONTENT_TYPE, HOST};
use http::{Method, StatusCode};
use http_body_util::{BodyExt, Full};
use hyper::client::conn::http1;
use hyper::Request;
use hyper_util::rt::TokioIo;
use serde::Serialize;
use std::time::Duration;

use crate::error::{ButterflyBotError, Result};
use crate::interfaces::transport::Transport;
use crate::services::transport::TorTransport;

pub struct DaemonStreamResponse {
    pub status: StatusCode,
    pub stream: BoxStream<'static, Result<Bytes>>,
}

impl DaemonStreamResponse {
    pub async fn collect_string(self) -> Result<String> {
        let mut stream = self.stream;
        let mut buffer = Vec::new();
        while let Some(chunk) = stream.next().await {
            let bytes = chunk?;
            buffer.extend_from_slice(&bytes);
        }
        Ok(String::from_utf8_lossy(&buffer).to_string())
    }
}

enum DaemonClientMode {
    Reqwest(reqwest::Client),
    Tor {
        transport: TorTransport,
        host: String,
        port: u16,
        base_path: String,
    },
}

pub struct DaemonClient {
    base_url: String,
    token: String,
    mode: DaemonClientMode,
}

impl DaemonClient {
    pub async fn new(base_url: String, token: String) -> Result<Self> {
        let parsed = ParsedBase::from_url(&base_url);
        if parsed.is_onion {
            let transport = TorTransport::new().await?;
            Ok(Self {
                base_url,
                token,
                mode: DaemonClientMode::Tor {
                    transport,
                    host: parsed.host,
                    port: parsed.port,
                    base_path: parsed.base_path,
                },
            })
        } else {
            let client = reqwest::Client::builder()
                .connect_timeout(Duration::from_secs(3))
                .timeout(Duration::from_secs(10))
                .build()
                .map_err(|e| ButterflyBotError::Runtime(e.to_string()))?;
            Ok(Self {
                base_url,
                token,
                mode: DaemonClientMode::Reqwest(client),
            })
        }
    }

    pub async fn post_json_stream<T: Serialize>(
        &self,
        path: &str,
        body: &T,
    ) -> Result<DaemonStreamResponse> {
        match &self.mode {
            DaemonClientMode::Reqwest(client) => {
                let url = join_url(&self.base_url, path);
                let mut request = client.post(url);
                if !self.token.trim().is_empty() {
                    request = request.header(AUTHORIZATION, format!("Bearer {}", self.token));
                }
                let response = request
                    .json(body)
                    .send()
                    .await
                    .map_err(|e: reqwest::Error| ButterflyBotError::Runtime(e.to_string()))?;
                let status = response.status();
                let stream = response
                    .bytes_stream()
                    .map(|item| {
                        item.map_err(|e: reqwest::Error| ButterflyBotError::Runtime(e.to_string()))
                    })
                    .boxed();
                Ok(DaemonStreamResponse { status, stream })
            }
            DaemonClientMode::Tor {
                transport,
                host,
                port,
                base_path,
            } => {
                let path = join_path(base_path, path);
                let payload = serde_json::to_vec(body)
                    .map_err(|e| ButterflyBotError::Serialization(e.to_string()))?;
                let req = Request::builder()
                    .method(Method::POST)
                    .uri(path)
                    .header(HOST, host.as_str())
                    .header(CONTENT_TYPE, "application/json")
                    .header(AUTHORIZATION, format!("Bearer {}", self.token))
                    .body(Full::new(Bytes::from(payload)))
                    .map_err(|e| ButterflyBotError::Runtime(e.to_string()))?;

                let stream = transport.connect(host, *port).await?;
                let io = TokioIo::new(stream);
                let (mut sender, conn) = http1::handshake::<_, Full<Bytes>>(io)
                    .await
                    .map_err(|e: hyper::Error| ButterflyBotError::Runtime(e.to_string()))?;
                tokio::spawn(async move {
                    let _ = conn.await;
                });
                let response = sender
                    .send_request(req)
                    .await
                    .map_err(|e: hyper::Error| ButterflyBotError::Runtime(e.to_string()))?;
                let status = response.status();
                let stream = response
                    .into_body()
                    .into_data_stream()
                    .map(|item| {
                        item.map_err(|e: hyper::Error| ButterflyBotError::Runtime(e.to_string()))
                    })
                    .boxed();
                Ok(DaemonStreamResponse { status, stream })
            }
        }
    }

    pub async fn get_stream(&self, path: &str, query: &[(&str, String)]) -> Result<DaemonStreamResponse> {
        match &self.mode {
            DaemonClientMode::Reqwest(client) => {
                let url = join_url_with_query(&self.base_url, path, query);
                let mut request = client.get(url);
                if !self.token.trim().is_empty() {
                    request = request.header(AUTHORIZATION, format!("Bearer {}", self.token));
                }
                let response = request
                    .send()
                    .await
                    .map_err(|e: reqwest::Error| ButterflyBotError::Runtime(e.to_string()))?;
                let status = response.status();
                let stream = response
                    .bytes_stream()
                    .map(|item| {
                        item.map_err(|e: reqwest::Error| ButterflyBotError::Runtime(e.to_string()))
                    })
                    .boxed();
                Ok(DaemonStreamResponse { status, stream })
            }
            DaemonClientMode::Tor {
                transport,
                host,
                port,
                base_path,
            } => {
                let path = join_path(base_path, path);
                let path = join_query(&path, query);
                let req = Request::builder()
                    .method(Method::GET)
                    .uri(path)
                    .header(HOST, host.as_str())
                    .header(AUTHORIZATION, format!("Bearer {}", self.token))
                    .body(Full::new(Bytes::new()))
                    .map_err(|e| ButterflyBotError::Runtime(e.to_string()))?;

                let stream = transport.connect(host, *port).await?;
                let io = TokioIo::new(stream);
                let (mut sender, conn) = http1::handshake::<_, Full<Bytes>>(io)
                    .await
                    .map_err(|e: hyper::Error| ButterflyBotError::Runtime(e.to_string()))?;
                tokio::spawn(async move {
                    let _ = conn.await;
                });
                let response = sender
                    .send_request(req)
                    .await
                    .map_err(|e: hyper::Error| ButterflyBotError::Runtime(e.to_string()))?;
                let status = response.status();
                let stream = response
                    .into_body()
                    .into_data_stream()
                    .map(|item| {
                        item.map_err(|e: hyper::Error| ButterflyBotError::Runtime(e.to_string()))
                    })
                    .boxed();
                Ok(DaemonStreamResponse { status, stream })
            }
        }
    }
}

struct ParsedBase {
    host: String,
    port: u16,
    base_path: String,
    is_onion: bool,
}

impl ParsedBase {
    fn from_url(url: &str) -> Self {
        let trimmed = url.trim();
        let without_scheme = trimmed
            .strip_prefix("http://")
            .or_else(|| trimmed.strip_prefix("https://"))
            .unwrap_or(trimmed);
        let mut parts = without_scheme.splitn(2, '/');
        let host_port = parts.next().unwrap_or("127.0.0.1:7878");
        let base_path = parts.next().map(|rest| format!("/{}", rest)).unwrap_or_default();
        let mut host_parts = host_port.splitn(2, ':');
        let host = host_parts.next().unwrap_or("127.0.0.1").to_string();
        let port = host_parts
            .next()
            .and_then(|value| value.parse::<u16>().ok())
            .unwrap_or(7878);
        let is_onion = host.ends_with(".onion");
        Self {
            host,
            port,
            base_path,
            is_onion,
        }
    }
}

fn join_url(base_url: &str, path: &str) -> String {
    let base = base_url.trim_end_matches('/');
    let path = path.trim_start_matches('/');
    format!("{base}/{path}")
}

fn join_url_with_query(base_url: &str, path: &str, query: &[(&str, String)]) -> String {
    let mut url = join_url(base_url, path);
    if !query.is_empty() {
        let query_string = query
            .iter()
            .map(|(key, value)| format!("{key}={}", urlencoding::encode(value)))
            .collect::<Vec<_>>()
            .join("&");
        url.push('?');
        url.push_str(&query_string);
    }
    url
}

fn join_path(base_path: &str, path: &str) -> String {
    let base = base_path.trim_end_matches('/');
    let path = path.trim_start_matches('/');
    if base.is_empty() {
        format!("/{path}")
    } else {
        format!("{base}/{path}")
    }
}

fn join_query(path: &str, query: &[(&str, String)]) -> String {
    if query.is_empty() {
        return path.to_string();
    }
    let query_string = query
        .iter()
        .map(|(key, value)| format!("{key}={}", urlencoding::encode(value)))
        .collect::<Vec<_>>()
        .join("&");
    format!("{path}?{query_string}")
}
