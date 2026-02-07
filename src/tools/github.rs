use std::collections::HashMap;
use async_trait::async_trait;
use serde_json::{json, Value};
use tokio::sync::RwLock;

use crate::error::{ButterflyBotError, Result};
use crate::interfaces::plugins::{Tool, ToolSecret};
use crate::tools::mcp::McpTool;
use crate::vault;

#[derive(Clone, Debug)]
struct GitHubConfig {
    url: String,
    transport: String,
    headers: HashMap<String, String>,
    pat: Option<String>,
}

impl Default for GitHubConfig {
    fn default() -> Self {
        Self {
            url: "https://api.githubcopilot.com/mcp/".to_string(),
            transport: "http".to_string(),
            headers: HashMap::new(),
            pat: None,
        }
    }
}

pub struct GitHubTool {
    config: RwLock<GitHubConfig>,
}

impl Default for GitHubTool {
    fn default() -> Self {
        Self::new()
    }
}

impl GitHubTool {
    pub fn new() -> Self {
        Self {
            config: RwLock::new(GitHubConfig::default()),
        }
    }

    fn get_tool_config<'a>(config: &'a Value) -> Option<&'a Value> {
        config.get("tools").and_then(|tools| tools.get("github"))
    }

    fn parse_transport(value: Option<&str>) -> String {
        match value.unwrap_or("") {
            "sse" => "sse".to_string(),
            "http" | "streamable-http" => "http".to_string(),
            _ => "http".to_string(),
        }
    }

    fn insert_pat_header(headers: &mut HashMap<String, String>, pat: &str) {
        if !headers.contains_key("Authorization") {
            headers.insert("Authorization".to_string(), format!("Bearer {pat}"));
        }
    }

    fn parse_headers(value: &Value) -> HashMap<String, String> {
        value
            .as_object()
            .map(|map| {
                map.iter()
                    .filter_map(|(key, value)| value.as_str().map(|v| (key.clone(), v.to_string())))
                    .collect::<HashMap<String, String>>()
            })
            .unwrap_or_default()
    }

    fn build_mcp_config(&self, config: &GitHubConfig) -> Value {
        json!({
            "tools": {
                "mcp": {
                    "servers": [
                        {
                            "name": "github",
                            "type": config.transport.clone(),
                            "url": config.url.clone(),
                            "headers": config.headers.clone()
                        }
                    ]
                }
            }
        })
    }
}

#[async_trait]
impl Tool for GitHubTool {
    fn name(&self) -> &str {
        "github"
    }

    fn description(&self) -> &str {
        "Access GitHub MCP tools with a single GitHub PAT."
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "action": {
                    "type": "string",
                    "enum": ["list_tools", "call_tool"]
                },
                "tool": { "type": "string", "description": "GitHub MCP tool name" },
                "arguments": { "type": "object", "description": "Arguments for the GitHub MCP tool" }
            },
            "required": ["action"],
            "additionalProperties": false
        })
    }

    fn required_secrets_for_config(&self, config: &Value) -> Vec<ToolSecret> {
        let tool_cfg = match Self::get_tool_config(config) {
            Some(cfg) => cfg,
            None => return Vec::new(),
        };
        let has_pat = tool_cfg.get("pat").and_then(|v| v.as_str()).is_some();
        let has_auth_header = tool_cfg
            .get("headers")
            .and_then(|v| v.as_object())
            .and_then(|headers| headers.get("Authorization"))
            .and_then(|v| v.as_str())
            .is_some();
        if has_pat || has_auth_header {
            Vec::new()
        } else {
            vec![ToolSecret::new("github_pat", "GitHub PAT (for MCP GitHub tool)")]
        }
    }

    fn configure(&self, config: &Value) -> Result<()> {
        let mut next = GitHubConfig::default();

        if let Some(tool_cfg) = Self::get_tool_config(config) {
            if let Some(url) = tool_cfg.get("url").and_then(|v| v.as_str()) {
                if !url.trim().is_empty() {
                    next.url = url.to_string();
                }
            }
            if let Some(transport) = tool_cfg.get("type").and_then(|v| v.as_str()) {
                next.transport = Self::parse_transport(Some(transport));
            }
            if let Some(pat) = tool_cfg.get("pat").and_then(|v| v.as_str()) {
                if !pat.trim().is_empty() {
                    next.pat = Some(pat.to_string());
                }
            }
            if let Some(headers) = tool_cfg.get("headers") {
                next.headers = Self::parse_headers(headers);
            }
        }

        if next.pat.is_none() {
            if let Some(secret) = vault::get_secret("github_pat")? {
                if !secret.trim().is_empty() {
                    next.pat = Some(secret);
                }
            }
        }

        if let Some(pat) = next.pat.clone() {
            Self::insert_pat_header(&mut next.headers, &pat);
        }

        let mut guard = self
            .config
            .try_write()
            .map_err(|_| ButterflyBotError::Runtime("GitHub tool lock busy".to_string()))?;
        *guard = next;
        Ok(())
    }

    async fn execute(&self, params: Value) -> Result<Value> {
        let action = params
            .get("action")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();

        let config = self
            .config
            .read()
            .await
            .clone();

        if config.headers.get("Authorization").is_none() {
            return Err(ButterflyBotError::Runtime(
                "Missing GitHub PAT (set tools.github.pat or vault github_pat)".to_string(),
            ));
        }

        let mcp_config = self.build_mcp_config(&config);
        let mcp_tool = McpTool::new();
        mcp_tool.configure(&mcp_config)?;

        match action.as_str() {
            "list_tools" => {
                let result = mcp_tool
                    .execute(json!({"action": "list_tools", "server": "github"}))
                    .await?;
                Ok(result)
            }
            "call_tool" => {
                let tool_name = params
                    .get("tool")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| ButterflyBotError::Runtime("Missing tool name".to_string()))?;
                let args = params.get("arguments").cloned();
                let result = mcp_tool
                    .execute(json!({
                        "action": "call_tool",
                        "server": "github",
                        "tool": tool_name,
                        "arguments": args
                    }))
                    .await?;
                Ok(result)
            }
            _ => Err(ButterflyBotError::Runtime("Unsupported action".to_string())),
        }
    }
}
