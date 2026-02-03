use std::collections::HashMap;
use std::time::Duration;

use async_trait::async_trait;
use reqwest::header::HeaderMap;
use serde_json::{json, Value};
use tokio::sync::RwLock;

use crate::error::{ButterflyBotError, Result};
use crate::interfaces::plugins::Tool;

#[derive(Clone, Debug, Default)]
struct HttpCallConfig {
    base_url: Option<String>,
    default_headers: HashMap<String, String>,
    timeout_seconds: Option<u64>,
}

pub struct HttpCallTool {
    config: RwLock<HttpCallConfig>,
}

impl Default for HttpCallTool {
    fn default() -> Self {
        Self::new()
    }
}

impl HttpCallTool {
    pub fn new() -> Self {
        Self {
            config: RwLock::new(HttpCallConfig::default()),
        }
    }

    fn build_headers(
        default_headers: &HashMap<String, String>,
        headers: Option<&Value>,
    ) -> Result<HeaderMap> {
        let mut out = HeaderMap::new();
        for (key, value) in default_headers {
            let header_name = key
                .parse::<reqwest::header::HeaderName>()
                .map_err(|e| ButterflyBotError::Runtime(e.to_string()))?;
            let header_value = value
                .parse::<reqwest::header::HeaderValue>()
                .map_err(|e| ButterflyBotError::Runtime(e.to_string()))?;
            out.insert(header_name, header_value);
        }
        if let Some(headers) = headers.and_then(|v| v.as_object()) {
            for (key, value) in headers {
                if let Some(value) = value.as_str() {
                    let header_name = key
                        .parse::<reqwest::header::HeaderName>()
                        .map_err(|e| ButterflyBotError::Runtime(e.to_string()))?;
                    let header_value = value
                        .parse::<reqwest::header::HeaderValue>()
                        .map_err(|e| ButterflyBotError::Runtime(e.to_string()))?;
                    out.insert(header_name, header_value);
                }
            }
        }
        Ok(out)
    }

    fn build_url(base_url: &Option<String>, url: Option<&str>, endpoint: Option<&str>) -> Result<String> {
        if let Some(url) = url {
            if !url.trim().is_empty() {
                return Ok(url.trim().to_string());
            }
        }
        let endpoint = endpoint.unwrap_or("").trim();
        if endpoint.is_empty() {
            return Err(ButterflyBotError::Runtime("Missing url or endpoint".to_string()));
        }
        let base = base_url
            .as_ref()
            .map(|v| v.trim().trim_end_matches('/').to_string())
            .filter(|v| !v.is_empty())
            .ok_or_else(|| ButterflyBotError::Runtime("Missing base_url for endpoint".to_string()))?;
        let endpoint = endpoint.trim_start_matches('/');
        Ok(format!("{base}/{endpoint}"))
    }

    fn apply_query(req: reqwest::RequestBuilder, query: Option<&Value>) -> reqwest::RequestBuilder {
        if let Some(map) = query.and_then(|v| v.as_object()) {
            let pairs: Vec<(String, String)> = map
                .iter()
                .map(|(k, v)| (k.clone(), v.as_str().unwrap_or(&v.to_string()).to_string()))
                .collect();
            return req.query(&pairs);
        }
        req
    }
}

#[async_trait]
impl Tool for HttpCallTool {
    fn name(&self) -> &str {
        "http_call"
    }

    fn description(&self) -> &str {
        "Perform arbitrary HTTP requests with custom headers and optional JSON/body payloads."
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "method": { "type": "string" },
                "url": { "type": "string" },
                "endpoint": { "type": "string" },
                "headers": { "type": "object" },
                "query": { "type": "object" },
                "body": { "type": "string" },
                "json": { "type": "object" },
                "timeout_seconds": { "type": "integer" }
            },
            "required": ["method"]
        })
    }

    fn configure(&self, config: &Value) -> Result<()> {
        let tool_cfg = config
            .get("tools")
            .and_then(|v| v.get("http_call"));
        let mut next = HttpCallConfig::default();
        if let Some(cfg) = tool_cfg {
            if let Some(base_url) = cfg.get("base_url").and_then(|v| v.as_str()) {
                let trimmed = base_url.trim();
                if !trimmed.is_empty() {
                    next.base_url = Some(trimmed.to_string());
                }
            }
            if let Some(headers) = cfg.get("default_headers").and_then(|v| v.as_object()) {
                next.default_headers = headers
                    .iter()
                    .filter_map(|(k, v)| v.as_str().map(|s| (k.clone(), s.to_string())))
                    .collect();
            }
            if let Some(timeout) = cfg.get("timeout_seconds").and_then(|v| v.as_u64()) {
                next.timeout_seconds = Some(timeout);
            }
        }

        let mut guard = self
            .config
            .try_write()
            .map_err(|_| ButterflyBotError::Runtime("HTTP call tool lock busy".to_string()))?;
        *guard = next;
        Ok(())
    }

    async fn execute(&self, params: Value) -> Result<Value> {
        let method = params
            .get("method")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_uppercase();
        if method.is_empty() {
            return Err(ButterflyBotError::Runtime("Missing method".to_string()));
        }

        let url = params.get("url").and_then(|v| v.as_str());
        let endpoint = params.get("endpoint").and_then(|v| v.as_str());
        let headers = params.get("headers");
        let query = params.get("query");
        let body = params.get("body").and_then(|v| v.as_str()).map(|s| s.to_string());
        let json_body = params.get("json").cloned();
        let timeout_override = params.get("timeout_seconds").and_then(|v| v.as_u64());

        let cfg = self.config.read().await.clone();
        let url = Self::build_url(&cfg.base_url, url, endpoint)?;
        let headers = Self::build_headers(&cfg.default_headers, headers)?;

        let client = reqwest::Client::new();
        let mut req = client.request(method.parse().map_err(|_| {
            ButterflyBotError::Runtime("Invalid method".to_string())
        })?, &url);

        if !headers.is_empty() {
            req = req.headers(headers);
        }
        req = Self::apply_query(req, query);

        if let Some(json_body) = json_body.and_then(|v| v.as_object().cloned()) {
            req = req.json(&json_body);
        } else if let Some(body) = body {
            req = req.body(body);
        }

        let timeout = timeout_override.or(cfg.timeout_seconds).unwrap_or(60);
        req = req.timeout(Duration::from_secs(timeout));

        let response = req
            .send()
            .await
            .map_err(|e| ButterflyBotError::Http(e.to_string()))?;

        let status = response.status().as_u16();
        let headers = response
            .headers()
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_str().unwrap_or("").to_string()))
            .collect::<HashMap<_, _>>();

        let text = response
            .text()
            .await
            .map_err(|e| ButterflyBotError::Http(e.to_string()))?;
        let json_value = serde_json::from_str::<Value>(&text).ok();

        Ok(json!({
            "status": "ok",
            "http_status": status,
            "headers": headers,
            "text": text,
            "json": json_value
        }))
    }
}
