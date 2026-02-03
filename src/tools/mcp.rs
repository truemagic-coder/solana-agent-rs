use std::collections::HashMap;

use async_trait::async_trait;
use serde_json::{json, Value};
use tokio::sync::RwLock;

use crate::error::{ButterflyBotError, Result};
use crate::interfaces::plugins::Tool;

use rust_mcp_sdk::mcp_client::{client_runtime, ClientHandler, McpClientOptions, ToMcpClientHandler};
use rust_mcp_sdk::schema::{
    CallToolRequestParams, ClientCapabilities, Implementation, InitializeRequestParams,
    LATEST_PROTOCOL_VERSION,
};
use rust_mcp_sdk::{ClientSseTransport, ClientSseTransportOptions, McpClient};
use std::sync::Arc;
use std::time::Duration;
use rust_mcp_transport::{RequestOptions, StreamableTransportOptions};

#[derive(Clone, Debug)]
struct McpServerConfig {
    name: String,
    transport: McpTransport,
    url: String,
    headers: HashMap<String, String>,
}

#[derive(Clone, Debug)]
enum McpTransport {
    Sse,
    Http,
}

#[derive(Default)]
struct NoopClientHandler;

#[async_trait]
impl ClientHandler for NoopClientHandler {}

pub struct McpTool {
    servers: RwLock<Vec<McpServerConfig>>,
}

impl Default for McpTool {
    fn default() -> Self {
        Self::new()
    }
}

impl McpTool {
    pub fn new() -> Self {
        Self {
            servers: RwLock::new(Vec::new()),
        }
    }

    fn parse_servers(config: &Value) -> Result<Vec<McpServerConfig>> {
        let Some(servers) = config
            .get("tools")
            .and_then(|tools| tools.get("mcp"))
            .and_then(|mcp| mcp.get("servers"))
            .and_then(|servers| servers.as_array())
        else {
            return Ok(Vec::new());
        };

        let mut parsed = Vec::new();
        for server in servers {
            let name = server
                .get("name")
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .trim()
                .to_string();
            let url = server
                .get("url")
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .trim()
                .to_string();
            let transport = match server
                .get("type")
                .and_then(|v| v.as_str())
                .unwrap_or("sse")
                .to_lowercase()
                .as_str()
            {
                "http" | "streamable-http" => McpTransport::Http,
                _ => McpTransport::Sse,
            };
            if name.is_empty() || url.is_empty() {
                return Err(ButterflyBotError::Config(
                    "MCP server entry requires name and url".to_string(),
                ));
            }
            let headers = server
                .get("headers")
                .and_then(|v| v.as_object())
                .map(|map| {
                    map.iter()
                        .filter_map(|(k, v)| v.as_str().map(|s| (k.clone(), s.to_string())))
                        .collect::<HashMap<_, _>>()
                })
                .unwrap_or_default();
            parsed.push(McpServerConfig {
                name,
                transport,
                url,
                headers,
            });
        }
        Ok(parsed)
    }

    async fn find_server(&self, name: Option<&str>) -> Result<McpServerConfig> {
        let servers = self.servers.read().await;
        if servers.is_empty() {
            return Err(ButterflyBotError::Runtime(
                "No MCP servers configured".to_string(),
            ));
        }
        if let Some(name) = name {
            let trimmed = name.trim();
            if !trimmed.is_empty() {
                if let Some(server) = servers.iter().find(|s| s.name == trimmed) {
                    return Ok(server.clone());
                }
                return Err(ButterflyBotError::Runtime(format!(
                    "Unknown MCP server '{trimmed}'"
                )));
            }
        }
        if servers.len() == 1 {
            Ok(servers[0].clone())
        } else {
            Err(ButterflyBotError::Runtime(
                "Multiple MCP servers configured; specify server name".to_string(),
            ))
        }
    }

    async fn create_client(
        &self,
        server: &McpServerConfig,
    ) -> Result<Arc<rust_mcp_sdk::mcp_client::ClientRuntime>> {
        let client_details = InitializeRequestParams {
            capabilities: ClientCapabilities::default(),
            client_info: Implementation {
                name: "butterfly-bot-mcp".into(),
                version: "0.1.0".into(),
                title: Some("Butterfly Bot MCP Client".into()),
                description: Some("MCP client used by Butterfly Bot tools".into()),
                icons: Vec::new(),
                website_url: None,
            },
            protocol_version: LATEST_PROTOCOL_VERSION.into(),
            meta: None,
        };

        match server.transport {
            McpTransport::Sse => {
                let transport_options = if server.headers.is_empty() {
                    ClientSseTransportOptions::default()
                } else {
                    ClientSseTransportOptions {
                        custom_headers: Some(server.headers.clone()),
                        ..ClientSseTransportOptions::default()
                    }
                };

                let transport = ClientSseTransport::new(&server.url, transport_options)
                    .map_err(|e| ButterflyBotError::Runtime(e.to_string()))?;

                let client = client_runtime::create_client(McpClientOptions {
                    client_details,
                    transport,
                    handler: NoopClientHandler.to_mcp_client_handler(),
                    task_store: None,
                    server_task_store: None,
                });

                client
                    .clone()
                    .start()
                    .await
                    .map_err(|e| ButterflyBotError::Runtime(e.to_string()))?;

                Ok(client)
            }
            McpTransport::Http => {
                let request_options = RequestOptions {
                    request_timeout: Duration::from_secs(60),
                    retry_delay: None,
                    max_retries: None,
                    custom_headers: if server.headers.is_empty() {
                        None
                    } else {
                        Some(server.headers.clone())
                    },
                };
                let transport_options = StreamableTransportOptions {
                    mcp_url: server.url.clone(),
                    request_options,
                };
                let client = rust_mcp_sdk::mcp_client::client_runtime::with_transport_options(
                    client_details,
                    transport_options,
                    NoopClientHandler,
                    None,
                    None,
                );
                client
                    .clone()
                    .start()
                    .await
                    .map_err(|e| ButterflyBotError::Runtime(e.to_string()))?;
                Ok(client)
            }
        }
    }

    async fn list_tools(&self, server: &McpServerConfig) -> Result<Value> {
        let client = self.create_client(server).await?;
        let list = client
            .request_tool_list(None)
            .await
            .map_err(|e| ButterflyBotError::Runtime(e.to_string()))?;
        client
            .shut_down()
            .await
            .map_err(|e| ButterflyBotError::Runtime(e.to_string()))?;
        serde_json::to_value(&list).map_err(|e| ButterflyBotError::Serialization(e.to_string()))
    }

    async fn call_tool(&self, server: &McpServerConfig, tool_name: &str, args: Option<Value>) -> Result<Value> {
        let client = self.create_client(server).await?;
        let args_map = args
            .and_then(|value| value.as_object().cloned());
        let result = client
            .request_tool_call(CallToolRequestParams {
                name: tool_name.to_string(),
                arguments: args_map,
                meta: None,
                task: None,
            })
            .await
            .map_err(|e| ButterflyBotError::Runtime(e.to_string()))?;
        client
            .shut_down()
            .await
            .map_err(|e| ButterflyBotError::Runtime(e.to_string()))?;
        serde_json::to_value(&result).map_err(|e| ButterflyBotError::Serialization(e.to_string()))
    }
}

#[async_trait]
impl Tool for McpTool {
    fn name(&self) -> &str {
        "mcp"
    }

    fn description(&self) -> &str {
        "Call tools on configured MCP servers (SSE)."
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "action": {
                    "type": "string",
                    "enum": ["list_tools", "call_tool"]
                },
                "server": { "type": "string", "description": "MCP server name from config" },
                "tool": { "type": "string", "description": "Tool name to invoke on MCP server" },
                "arguments": { "type": "object", "description": "Arguments for the MCP tool" }
            },
            "required": ["action"]
        })
    }

    fn configure(&self, config: &Value) -> Result<()> {
        let servers = Self::parse_servers(config)?;
        let mut guard = self
            .servers
            .try_write()
            .map_err(|_| ButterflyBotError::Runtime("MCP tool lock busy".to_string()))?;
        *guard = servers;
        Ok(())
    }

    async fn execute(&self, params: Value) -> Result<Value> {
        let action = params
            .get("action")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let server_name = params.get("server").and_then(|v| v.as_str());
        let server = self.find_server(server_name).await?;

        match action.as_str() {
            "list_tools" => {
                let list = self.list_tools(&server).await?;
                Ok(json!({"status": "ok", "server": server.name, "tools": list}))
            }
            "call_tool" => {
                let tool_name = params
                    .get("tool")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| ButterflyBotError::Runtime("Missing tool name".to_string()))?;
                let args = params.get("arguments").cloned();
                let result = self.call_tool(&server, tool_name, args).await?;
                Ok(json!({"status": "ok", "server": server.name, "result": result}))
            }
            _ => Err(ButterflyBotError::Runtime(
                "Unsupported action".to_string(),
            )),
        }
    }
}
