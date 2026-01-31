#![allow(clippy::clone_on_copy, clippy::collapsible_match, clippy::collapsible_else_if)]

use dioxus::document::eval;
use dioxus::launch;
use dioxus::prelude::*;
use futures::StreamExt;
use notify_rust::Notification;
use pulldown_cmark::{html, Options, Parser};
use serde::Serialize;
use serde_json::{json, Value};
use std::env;
use std::thread;
use tokio::time::{sleep, timeout, Duration};

const AVAILABLE_TOOLS: [&str; 2] = ["search_internet", "reminders"];

#[derive(Clone, Serialize)]
struct ProcessTextRequest {
    user_id: String,
    text: String,
    prompt: Option<String>,
}

#[derive(Clone)]
struct ChatMessage {
    id: u64,
    role: MessageRole,
    text: String,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum MessageRole {
    User,
    Bot,
}

#[derive(Clone)]
struct ToolToggle {
    name: String,
    enabled: bool,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum UiTab {
    Chat,
    Settings,
}

fn markdown_to_html(input: &str) -> String {
    let mut options = Options::empty();
    options.insert(Options::ENABLE_TABLES);
    options.insert(Options::ENABLE_STRIKETHROUGH);
    options.insert(Options::ENABLE_TASKLISTS);
    options.insert(Options::ENABLE_FOOTNOTES);
    options.insert(Options::ENABLE_SMART_PUNCTUATION);
    let parser = Parser::new_ext(input, options);
    let mut output = String::new();
    html::push_html(&mut output, parser);
    output
}

async fn scroll_chat_to_bottom() {
    let _ = eval(
        "const el = document.getElementById('chat-scroll'); if (el) { el.scrollTop = el.scrollHeight; }",
    )
    .await;
}

async fn scroll_chat_after_render() {
    scroll_chat_to_bottom().await;
    sleep(Duration::from_millis(16)).await;
    scroll_chat_to_bottom().await;
}

pub fn launch_ui() {
    start_local_daemon();
    launch(app_view);
}

fn start_local_daemon() {
    if env::var("BUTTERFLY_BOT_DISABLE_DAEMON").is_ok() {
        return;
    }

    let daemon_url =
        env::var("BUTTERFLY_BOT_DAEMON").unwrap_or_else(|_| "http://127.0.0.1:7878".to_string());
    let (host, port) = parse_daemon_address(&daemon_url);
    let db_path =
        env::var("BUTTERFLY_BOT_DB").unwrap_or_else(|_| "./data/butterfly-bot.db".to_string());
    let token = env::var("BUTTERFLY_BOT_TOKEN").unwrap_or_default();

    thread::spawn(move || {
        if let Ok(runtime) = tokio::runtime::Runtime::new() {
            runtime.block_on(async move {
                let _ = crate::daemon::run(&host, port, &db_path, &token).await;
            });
        }
    });
}

fn parse_daemon_address(daemon: &str) -> (String, u16) {
    let trimmed = daemon.trim();
    let without_scheme = trimmed
        .strip_prefix("http://")
        .or_else(|| trimmed.strip_prefix("https://"))
        .unwrap_or(trimmed);
    let host_port = without_scheme.split('/').next().unwrap_or("127.0.0.1:7878");
    let mut parts = host_port.splitn(2, ':');
    let host = parts.next().unwrap_or("127.0.0.1");
    let port = parts
        .next()
        .and_then(|value| value.parse::<u16>().ok())
        .unwrap_or(7878);
    (host.to_string(), port)
}

fn app_view() -> Element {
    let db_path =
        env::var("BUTTERFLY_BOT_DB").unwrap_or_else(|_| "./data/butterfly-bot.db".to_string());
    let daemon_url = use_signal(|| {
        env::var("BUTTERFLY_BOT_DAEMON").unwrap_or_else(|_| "http://127.0.0.1:7878".to_string())
    });
    let token = use_signal(|| env::var("BUTTERFLY_BOT_TOKEN").unwrap_or_default());
    let user_id =
        use_signal(|| env::var("BUTTERFLY_BOT_USER_ID").unwrap_or_else(|_| "cli_user".to_string()));
    let prompt = use_signal(String::new);
    let input = use_signal(String::new);
    let busy = use_signal(|| false);
    let error = use_signal(String::new);
    let messages = use_signal(Vec::<ChatMessage>::new);
    let next_id = use_signal(|| 1u64);
    let active_tab = use_signal(|| UiTab::Chat);
    let reminders_listening = use_signal(|| false);
    let ui_events_listening = use_signal(|| false);

    let tools_loaded = use_signal(|| false);
    let tool_toggles = use_signal(Vec::<ToolToggle>::new);
    let tools_safe_mode = use_signal(|| false);
    let settings_error = use_signal(String::new);
    let settings_status = use_signal(String::new);

    let search_provider = use_signal(|| "openai".to_string());
    let search_model = use_signal(String::new);
    let search_citations = use_signal(|| true);
    let search_grok_web = use_signal(|| true);
    let search_grok_x = use_signal(|| true);
    let search_grok_timeout = use_signal(|| "90".to_string());
    let search_network_allow = use_signal(String::new);
    let search_default_deny = use_signal(|| false);
    let search_api_key = use_signal(String::new);
    let search_api_key_status = use_signal(String::new);

    let reminders_sqlite_path = use_signal(String::new);
    let memory_enabled = use_signal(|| true);

    let on_send = {
        let daemon_url = daemon_url.clone();
        let token = token.clone();
        let user_id = user_id.clone();
        let prompt = prompt.clone();
        let input = input.clone();
        let busy = busy.clone();
        let error = error.clone();
        let messages = messages.clone();
        let next_id = next_id.clone();

        use_callback(move |_| {
            let daemon_url = daemon_url();
            let token = token();
            let user_id = user_id();
            let prompt = prompt();
            let text = input();
            let busy = busy.clone();
            let error = error.clone();
            let messages = messages.clone();
            let next_id = next_id.clone();

            spawn(async move {
                let mut busy = busy;
                let mut error = error;
                let mut messages = messages;
                let mut next_id = next_id;
                let mut input = input;

                if *busy.read() {
                    error.set("A request is already in progress. Please wait.".to_string());
                    return;
                }

                if text.trim().is_empty() {
                    error.set("Enter a message to send.".to_string());
                    return;
                }

                busy.set(true);
                error.set(String::new());

                let user_message_id = {
                    let id = next_id();
                    next_id.set(id + 1);
                    id
                };
                let bot_message_id = {
                    let id = next_id();
                    next_id.set(id + 1);
                    id
                };

                messages.write().push(ChatMessage {
                    id: user_message_id,
                    role: MessageRole::User,
                    text: text.clone(),
                });
                messages.write().push(ChatMessage {
                    id: bot_message_id,
                    role: MessageRole::Bot,
                    text: String::new(),
                });

                input.set(String::new());
                scroll_chat_after_render().await;

                let client = reqwest::Client::new();
                let url = format!("{}/process_text_stream", daemon_url.trim_end_matches('/'));
                let body = ProcessTextRequest {
                    user_id,
                    text,
                    prompt: if prompt.trim().is_empty() {
                        None
                    } else {
                        Some(prompt)
                    },
                };

                let make_request = |client: &reqwest::Client,
                                    url: &str,
                                    token: &str,
                                    body: &ProcessTextRequest| {
                    let mut request = client.post(url);
                    if !token.trim().is_empty() {
                        request = request.header("authorization", format!("Bearer {token}"));
                    }
                    request.json(body)
                };

                match make_request(&client, &url, &token, &body).send().await {
                    Ok(response) => {
                        let mut messages = messages.clone();
                        let mut error = error.clone();
                        if response.status().is_success() {
                            let mut stream = response.bytes_stream();
                            loop {
                                let next_chunk =
                                    match timeout(Duration::from_secs(45), stream.next()).await {
                                        Ok(value) => value,
                                        Err(_) => {
                                            error.set(
                                                "Stream timed out waiting for response."
                                                    .to_string(),
                                            );
                                            break;
                                        }
                                    };
                                let Some(chunk) = next_chunk else {
                                    break;
                                };
                                match chunk {
                                    Ok(bytes) => {
                                        if let Ok(text_chunk) = std::str::from_utf8(&bytes) {
                                            if !text_chunk.is_empty() {
                                                let mut list = messages.write();
                                                if let Some(last) = list
                                                    .iter_mut()
                                                    .rev()
                                                    .find(|msg| msg.id == bot_message_id)
                                                {
                                                    last.text.push_str(text_chunk);
                                                }
                                            }
                                        }
                                        scroll_chat_to_bottom().await;
                                    }
                                    Err(err) => {
                                        error.set(format!("Stream error: {err}"));
                                        break;
                                    }
                                }
                            }
                        } else {
                            let status = response.status();
                            let text = response
                                .text()
                                .await
                                .unwrap_or_else(|_| "Unable to read error body".to_string());
                            error.set(format!("Request failed ({status}): {text}"));
                        }
                    }
                    Err(_err) => {
                        start_local_daemon();
                        sleep(Duration::from_millis(400)).await;
                        match make_request(&client, &url, &token, &body).send().await {
                            Ok(response) => {
                                let mut messages = messages.clone();
                                let mut error = error.clone();
                                if response.status().is_success() {
                                    let mut stream = response.bytes_stream();
                                    loop {
                                        let next_chunk =
                                            match timeout(Duration::from_secs(45), stream.next())
                                                .await
                                            {
                                                Ok(value) => value,
                                                Err(_) => {
                                                    error.set(
                                                        "Stream timed out waiting for response."
                                                            .to_string(),
                                                    );
                                                    break;
                                                }
                                            };
                                        let Some(chunk) = next_chunk else {
                                            break;
                                        };
                                        match chunk {
                                            Ok(bytes) => {
                                                if let Ok(text_chunk) = std::str::from_utf8(&bytes)
                                                {
                                                    if !text_chunk.is_empty() {
                                                        let mut list = messages.write();
                                                        if let Some(last) = list
                                                            .iter_mut()
                                                            .rev()
                                                            .find(|msg| msg.id == bot_message_id)
                                                        {
                                                            last.text.push_str(text_chunk);
                                                        }
                                                    }
                                                }
                                                scroll_chat_to_bottom().await;
                                            }
                                            Err(err) => {
                                                error.set(format!("Stream error: {err}"));
                                                break;
                                            }
                                        }
                                    }
                                } else {
                                    let status = response.status();
                                    let text = response.text().await.unwrap_or_else(|_| {
                                        "Unable to read error body".to_string()
                                    });
                                    error.set(format!("Request failed ({status}): {text}"));
                                }
                            }
                            Err(err) => {
                                error.set(format!(
                                    "Request failed: {err}. Daemon unreachable at {daemon_url}."
                                ));
                            }
                        }
                    }
                }

                busy.set(false);
            });
        })
    };
    let on_send_key = on_send.clone();

    if !*reminders_listening.read() {
        let reminders_listening = reminders_listening.clone();
        let daemon_url = daemon_url.clone();
        let token = token.clone();
        let user_id = user_id.clone();
        let messages = messages.clone();
        let next_id = next_id.clone();

        spawn(async move {
            let mut reminders_listening = reminders_listening;
            let daemon_url = daemon_url;
            let token = token;
            let user_id = user_id;
            let mut messages = messages;
            let mut next_id = next_id;

            reminders_listening.set(true);
            let client = reqwest::Client::new();
            loop {
                let url = format!(
                    "{}/reminder_stream?user_id={}",
                    daemon_url().trim_end_matches('/'),
                    user_id()
                );
                let mut request = client.get(&url);
                let token_value = token();
                if !token_value.trim().is_empty() {
                    request = request.header("authorization", format!("Bearer {token_value}"));
                }

                let response = match request.send().await {
                    Ok(resp) => resp,
                    Err(_) => {
                        sleep(Duration::from_secs(2)).await;
                        continue;
                    }
                };
                if !response.status().is_success() {
                    sleep(Duration::from_secs(2)).await;
                    continue;
                }

                let mut stream = response.bytes_stream();
                let mut buffer = String::new();
                while let Some(chunk) = stream.next().await {
                    let Ok(chunk) = chunk else {
                        break;
                    };
                    if let Ok(text) = std::str::from_utf8(&chunk) {
                        buffer.push_str(text);
                        while let Some(idx) = buffer.find('\n') {
                            let mut line = buffer[..idx].to_string();
                            buffer = buffer[idx + 1..].to_string();
                            if line.starts_with("data:") {
                                line = line.trim_start_matches("data:").trim().to_string();
                                if let Ok(value) = serde_json::from_str::<Value>(&line) {
                                    let title = value
                                        .get("title")
                                        .and_then(|v| v.as_str())
                                        .unwrap_or("Reminder");
                                    let id = next_id();
                                    next_id.set(id + 1);
                                    messages.write().push(ChatMessage {
                                        id,
                                        role: MessageRole::Bot,
                                        text: format!("⏰ {title}"),
                                    });
                                    scroll_chat_to_bottom().await;
                                    let _ = Notification::new()
                                        .summary("Butterfly Bot")
                                        .body(title)
                                        .show();
                                }
                            }
                        }
                    }
                }
                sleep(Duration::from_secs(2)).await;
            }
        });
    }

    if !*ui_events_listening.read() {
        let ui_events_listening = ui_events_listening.clone();
        let daemon_url = daemon_url.clone();
        let token = token.clone();
        let user_id = user_id.clone();
        let messages = messages.clone();
        let next_id = next_id.clone();

        spawn(async move {
            let mut ui_events_listening = ui_events_listening;
            let daemon_url = daemon_url;
            let token = token;
            let user_id = user_id;
            let mut messages = messages;
            let mut next_id = next_id;

            ui_events_listening.set(true);
            let client = reqwest::Client::new();
            loop {
                let url = format!(
                    "{}/ui_events?user_id={}",
                    daemon_url().trim_end_matches('/'),
                    user_id()
                );
                let mut request = client.get(&url);
                let token_value = token();
                if !token_value.trim().is_empty() {
                    request = request.header("authorization", format!("Bearer {token_value}"));
                }

                let response = match request.send().await {
                    Ok(resp) => resp,
                    Err(_) => {
                        sleep(Duration::from_secs(2)).await;
                        continue;
                    }
                };
                if !response.status().is_success() {
                    sleep(Duration::from_secs(2)).await;
                    continue;
                }

                let mut stream = response.bytes_stream();
                let mut buffer = String::new();
                while let Some(chunk) = stream.next().await {
                    let Ok(chunk) = chunk else {
                        break;
                    };
                    if let Ok(text) = std::str::from_utf8(&chunk) {
                        buffer.push_str(text);
                        while let Some(idx) = buffer.find('\n') {
                            let mut line = buffer[..idx].to_string();
                            buffer = buffer[idx + 1..].to_string();
                            if line.starts_with("data:") {
                                line = line.trim_start_matches("data:").trim().to_string();
                                if let Ok(value) = serde_json::from_str::<Value>(&line) {
                                    let tool = value
                                        .get("tool")
                                        .and_then(|v| v.as_str())
                                        .unwrap_or("tool");
                                    let status = value
                                        .get("status")
                                        .and_then(|v| v.as_str())
                                        .unwrap_or("ok");
                                    let mut text = format!("🔧 {tool}: {status}");
                                    if let Some(payload) = value.get("payload") {
                                        if let Some(error) =
                                            payload.get("error").and_then(|v| v.as_str())
                                        {
                                            text.push_str(&format!(" — {error}"));
                                        }
                                    }
                                    let id = next_id();
                                    next_id.set(id + 1);
                                    messages.write().push(ChatMessage {
                                        id,
                                        role: MessageRole::Bot,
                                        text,
                                    });
                                    scroll_chat_to_bottom().await;
                                }
                            }
                        }
                    }
                }
                sleep(Duration::from_secs(2)).await;
            }
        });
    }

    if !*tools_loaded.read() {
        let tool_toggles = tool_toggles.clone();
        let tools_safe_mode = tools_safe_mode.clone();
        let settings_error = settings_error.clone();
        let tools_loaded = tools_loaded.clone();
        let search_provider = search_provider.clone();
        let search_model = search_model.clone();
        let search_citations = search_citations.clone();
        let search_grok_web = search_grok_web.clone();
        let search_grok_x = search_grok_x.clone();
        let search_grok_timeout = search_grok_timeout.clone();
        let search_network_allow = search_network_allow.clone();
        let search_default_deny = search_default_deny.clone();
        let search_api_key_status = search_api_key_status.clone();
        let reminders_sqlite_path = reminders_sqlite_path.clone();
        let memory_enabled = memory_enabled.clone();
        let db_path = db_path.clone();

        spawn(async move {
            let mut tool_toggles = tool_toggles;
            let mut tools_safe_mode = tools_safe_mode;
            let mut settings_error = settings_error;
            let mut tools_loaded = tools_loaded;
            let mut search_provider = search_provider;
            let mut search_model = search_model;
            let mut search_citations = search_citations;
            let mut search_grok_web = search_grok_web;
            let mut search_grok_x = search_grok_x;
            let mut search_grok_timeout = search_grok_timeout;
            let mut search_network_allow = search_network_allow;
            let mut search_default_deny = search_default_deny;
            let mut search_api_key_status = search_api_key_status;
            let mut reminders_sqlite_path = reminders_sqlite_path;
            let mut memory_enabled = memory_enabled;

            let config = match crate::config::Config::from_store(&db_path) {
                Ok(value) => value,
                Err(err) => {
                    settings_error.set(format!("Failed to load config: {err}"));
                    tools_loaded.set(true);
                    return;
                }
            };

            let mut tool_names: Vec<String> = AVAILABLE_TOOLS
                .iter()
                .map(|name| name.to_string())
                .collect();
            for agent in &config.agents {
                if let Some(tools) = &agent.tools {
                    for tool in tools {
                        if !tool_names.contains(tool) {
                            tool_names.push(tool.to_string());
                        }
                    }
                }
            }

            let mut enabled_list: Vec<String> = Vec::new();
            let mut disabled_list: Vec<String> = Vec::new();
            let mut safe_mode = false;
            let mut allowlist: Vec<String> = Vec::new();
            let mut default_deny = false;

            if let Some(tools_value) = &config.tools {
                if let Value::Object(map) = tools_value {
                    for (key, value) in map {
                        if key == "settings" {
                            if let Some(settings) = value.as_object() {
                                safe_mode = settings
                                    .get("safe_mode")
                                    .and_then(|v| v.as_bool())
                                    .unwrap_or(false);
                                enabled_list = settings
                                    .get("enabled")
                                    .and_then(|v| v.as_array())
                                    .map(|items| {
                                        items
                                            .iter()
                                            .filter_map(|v| v.as_str().map(|s| s.to_string()))
                                            .collect()
                                    })
                                    .unwrap_or_default();
                                disabled_list = settings
                                    .get("disabled")
                                    .and_then(|v| v.as_array())
                                    .map(|items| {
                                        items
                                            .iter()
                                            .filter_map(|v| v.as_str().map(|s| s.to_string()))
                                            .collect()
                                    })
                                    .unwrap_or_default();
                                if let Some(perms) = settings.get("permissions") {
                                    if let Some(items) =
                                        perms.get("network_allow").and_then(|v| v.as_array())
                                    {
                                        allowlist = items
                                            .iter()
                                            .filter_map(|v| v.as_str().map(|s| s.to_string()))
                                            .collect();
                                    }
                                    if let Some(value) =
                                        perms.get("default_deny").and_then(|v| v.as_bool())
                                    {
                                        default_deny = value;
                                    }
                                }
                            }
                        } else {
                            if !tool_names.contains(key) {
                                tool_names.push(key.to_string());
                            }
                        }
                    }
                }
            }

            let enabled = config
                .memory
                .as_ref()
                .and_then(|memory| memory.enabled)
                .unwrap_or(true);
            memory_enabled.set(enabled);

            if let Some(tools_value) = &config.tools {
                if let Some(search_cfg) = tools_value.get("search_internet") {
                    if let Some(provider) = search_cfg.get("provider").and_then(|v| v.as_str()) {
                        search_provider.set(provider.to_string());
                    }
                    if let Some(model) = search_cfg.get("model").and_then(|v| v.as_str()) {
                        search_model.set(model.to_string());
                    }
                    if let Some(citations) = search_cfg.get("citations").and_then(|v| v.as_bool()) {
                        search_citations.set(citations);
                    }
                    if let Some(web) = search_cfg.get("grok_web_search").and_then(|v| v.as_bool()) {
                        search_grok_web.set(web);
                    }
                    if let Some(x_search) =
                        search_cfg.get("grok_x_search").and_then(|v| v.as_bool())
                    {
                        search_grok_x.set(x_search);
                    }
                    if let Some(timeout) = search_cfg.get("grok_timeout").and_then(|v| v.as_u64()) {
                        search_grok_timeout.set(timeout.to_string());
                    }
                    if let Some(perms) = search_cfg.get("permissions") {
                        if allowlist.is_empty() {
                            if let Some(items) =
                                perms.get("network_allow").and_then(|v| v.as_array())
                            {
                                allowlist = items
                                    .iter()
                                    .filter_map(|v| v.as_str().map(|s| s.to_string()))
                                    .collect();
                            }
                        }
                    }
                }

                if let Some(reminders_cfg) = tools_value.get("reminders") {
                    if let Some(path) = reminders_cfg.get("sqlite_path").and_then(|v| v.as_str()) {
                        reminders_sqlite_path.set(path.to_string());
                    }
                }
            }

            search_default_deny.set(default_deny);
            if !allowlist.is_empty() {
                search_network_allow.set(allowlist.join(", "));
            }

            let provider_name = search_provider();
            let secret_name = match provider_name.as_str() {
                "perplexity" => "search_internet_perplexity_api_key",
                "grok" => "search_internet_grok_api_key",
                _ => "search_internet_openai_api_key",
            };
            match crate::vault::get_secret(secret_name) {
                Ok(Some(secret)) if !secret.trim().is_empty() => {
                    search_api_key_status.set("Stored in vault".to_string());
                }
                Ok(_) => {
                    search_api_key_status.set("Not set".to_string());
                }
                Err(err) => {
                    search_api_key_status.set(format!("Vault error: {err}"));
                }
            }

            tool_names.sort();
            let enabled_set: std::collections::HashSet<String> = enabled_list.into_iter().collect();
            let disabled_set: std::collections::HashSet<String> =
                disabled_list.into_iter().collect();
            let mut toggles = Vec::new();
            for name in tool_names {
                let enabled = if safe_mode && enabled_set.is_empty() {
                    false
                } else if !enabled_set.is_empty() {
                    enabled_set.contains(&name)
                } else if !disabled_set.is_empty() {
                    !disabled_set.contains(&name)
                } else {
                    true
                };
                toggles.push(ToolToggle { name, enabled });
            }

            tools_safe_mode.set(safe_mode);
            tool_toggles.set(toggles);
            tools_loaded.set(true);
        });
    }

    let on_save_settings = {
        let tool_toggles = tool_toggles.clone();
        let tools_safe_mode = tools_safe_mode.clone();
        let settings_error = settings_error.clone();
        let settings_status = settings_status.clone();
        let search_provider = search_provider.clone();
        let search_model = search_model.clone();
        let search_citations = search_citations.clone();
        let search_grok_web = search_grok_web.clone();
        let search_grok_x = search_grok_x.clone();
        let search_grok_timeout = search_grok_timeout.clone();
        let search_network_allow = search_network_allow.clone();
        let search_default_deny = search_default_deny.clone();
        let search_api_key = search_api_key.clone();
        let search_api_key_status = search_api_key_status.clone();
        let reminders_sqlite_path = reminders_sqlite_path.clone();
        let memory_enabled = memory_enabled.clone();
        let db_path = db_path.clone();

        use_callback(move |_| {
            let tool_toggles = tool_toggles.clone();
            let tools_safe_mode = tools_safe_mode.clone();
            let settings_error = settings_error.clone();
            let settings_status = settings_status.clone();
            let search_provider = search_provider.clone();
            let search_model = search_model.clone();
            let search_citations = search_citations.clone();
            let search_grok_web = search_grok_web.clone();
            let search_grok_x = search_grok_x.clone();
            let search_grok_timeout = search_grok_timeout.clone();
            let search_network_allow = search_network_allow.clone();
            let search_default_deny = search_default_deny.clone();
            let search_api_key = search_api_key.clone();
            let search_api_key_status = search_api_key_status.clone();
            let reminders_sqlite_path = reminders_sqlite_path.clone();
            let memory_enabled = memory_enabled.clone();
            let db_path = db_path.clone();

            spawn(async move {
                let tool_toggles = tool_toggles;
                let tools_safe_mode = tools_safe_mode;
                let mut settings_error = settings_error;
                let mut settings_status = settings_status;
                let search_provider = search_provider;
                let search_model = search_model;
                let search_citations = search_citations;
                let search_grok_web = search_grok_web;
                let search_grok_x = search_grok_x;
                let search_grok_timeout = search_grok_timeout;
                let search_network_allow = search_network_allow;
                let search_default_deny = search_default_deny;
                let mut search_api_key = search_api_key;
                let mut search_api_key_status = search_api_key_status;
                let reminders_sqlite_path = reminders_sqlite_path;
                let memory_enabled = memory_enabled;

                settings_error.set(String::new());
                settings_status.set(String::new());

                let enabled: Vec<String> = tool_toggles()
                    .iter()
                    .filter(|tool| tool.enabled)
                    .map(|tool| tool.name.clone())
                    .collect();
                let disabled: Vec<String> = tool_toggles()
                    .iter()
                    .filter(|tool| !tool.enabled)
                    .map(|tool| tool.name.clone())
                    .collect();

                let mut settings = serde_json::Map::new();
                if tools_safe_mode() {
                    settings.insert("safe_mode".to_string(), Value::Bool(true));
                    settings.insert(
                        "enabled".to_string(),
                        Value::Array(enabled.into_iter().map(Value::String).collect()),
                    );
                } else {
                    settings.insert("safe_mode".to_string(), Value::Bool(false));
                    if !disabled.is_empty() {
                        settings.insert(
                            "disabled".to_string(),
                            Value::Array(disabled.into_iter().map(Value::String).collect()),
                        );
                    }
                }

                let mut tools_object = serde_json::Map::new();

                let network_allow: Vec<String> = search_network_allow()
                    .split([',', '\n'])
                    .map(|item| item.trim())
                    .filter(|item| !item.is_empty())
                    .map(|item| item.to_string())
                    .collect();
                settings.insert(
                    "permissions".to_string(),
                    json!({
                        "default_deny": search_default_deny(),
                        "network_allow": network_allow,
                    }),
                );
                tools_object.insert("settings".to_string(), Value::Object(settings));

                let timeout = search_grok_timeout()
                    .trim()
                    .parse::<u64>()
                    .map_err(|_| "grok_timeout must be a number".to_string());
                let timeout = match timeout {
                    Ok(value) => value,
                    Err(err) => {
                        settings_error.set(err);
                        return;
                    }
                };

                let mut search_cfg = serde_json::Map::new();
                search_cfg.insert("provider".to_string(), Value::String(search_provider()));
                if !search_model().trim().is_empty() {
                    search_cfg.insert("model".to_string(), Value::String(search_model()));
                }
                search_cfg.insert("citations".to_string(), Value::Bool(search_citations()));
                search_cfg.insert(
                    "grok_web_search".to_string(),
                    Value::Bool(search_grok_web()),
                );
                search_cfg.insert("grok_x_search".to_string(), Value::Bool(search_grok_x()));
                search_cfg.insert("grok_timeout".to_string(), Value::Number(timeout.into()));
                search_cfg.insert(
                    "permissions".to_string(),
                    json!({ "network_allow": search_network_allow()
                        .split([',', '\n'])
                        .map(|item| item.trim())
                        .filter(|item| !item.is_empty())
                        .map(|item| item.to_string())
                        .collect::<Vec<_>>()
                    }),
                );
                tools_object.insert("search_internet".to_string(), Value::Object(search_cfg));

                if !reminders_sqlite_path().trim().is_empty() {
                    tools_object.insert(
                        "reminders".to_string(),
                        json!({ "sqlite_path": reminders_sqlite_path() }),
                    );
                }

                if !search_api_key().trim().is_empty() {
                    let secret_name = match search_provider().as_str() {
                        "perplexity" => "search_internet_perplexity_api_key",
                        "grok" => "search_internet_grok_api_key",
                        _ => "search_internet_openai_api_key",
                    };
                    match crate::vault::set_secret(secret_name, &search_api_key()) {
                        Ok(()) => {
                            search_api_key.set(String::new());
                            search_api_key_status.set("Stored in vault".to_string());
                        }
                        Err(err) => {
                            settings_error.set(format!("Failed to store API key: {err}"));
                            return;
                        }
                    }
                }

                let mut config = match crate::config::Config::from_store(&db_path) {
                    Ok(value) => value,
                    Err(err) => {
                        settings_error.set(format!("Failed to load config: {err}"));
                        return;
                    }
                };
                config.tools = Some(Value::Object(tools_object));
                if let Some(memory) = &mut config.memory {
                    memory.enabled = Some(memory_enabled());
                }

                let result = tokio::task::spawn_blocking(move || {
                    crate::config_store::save_config(&db_path, &config)
                })
                .await;

                match result {
                    Ok(Ok(())) => settings_status.set("Settings saved.".to_string()),
                    Ok(Err(err)) => settings_error.set(format!("Save failed: {err}")),
                    Err(err) => settings_error.set(format!("Save failed: {err}")),
                }
            });
        })
    };

    let active_tab_chat = active_tab.clone();
    let active_tab_settings = active_tab.clone();
    let tools_safe_mode_toggle = tools_safe_mode.clone();
    let prompt_input = prompt.clone();
    let message_input = input.clone();
    let search_provider_input = search_provider.clone();
    let search_model_input = search_model.clone();
    let search_citations_toggle = search_citations.clone();
    let search_grok_web_toggle = search_grok_web.clone();
    let search_grok_x_toggle = search_grok_x.clone();
    let search_grok_timeout_input = search_grok_timeout.clone();
    let search_network_allow_input = search_network_allow.clone();
    let search_default_deny_toggle = search_default_deny.clone();
    let search_api_key_input = search_api_key.clone();
    let reminders_sqlite_input = reminders_sqlite_path.clone();
    let memory_enabled_toggle = memory_enabled.clone();

    rsx! {
        style { r#"
            body {{
                font-family: system-ui, -apple-system, BlinkMacSystemFont, "SF Pro Text", "SF Pro Display", sans-serif;
                background: radial-gradient(1200px 800px at 20% -10%, rgba(120,119,198,0.35), transparent 60%),
                            radial-gradient(1000px 700px at 110% 10%, rgba(56,189,248,0.25), transparent 60%),
                            #0b1020;
                color: #e5e7eb;
            }}
            .container {{ max-width: 980px; margin: 0 auto; padding: 0; height: 100vh; display: flex; flex-direction: column; }}
            .header {{
                padding: 16px 20px;
                background: rgba(17,24,39,0.55);
                color: #e5e7eb;
                display: flex; align-items: center; justify-content: space-between;
                border-bottom: 1px solid rgba(255,255,255,0.08);
                backdrop-filter: blur(18px) saturate(180%);
                box-shadow: 0 8px 30px rgba(0,0,0,0.25);
            }}
            .nav {{ display: flex; gap: 8px; }}
            .nav button {{ background: rgba(255,255,255,0.08); }}
            .nav button.active {{ background: rgba(99,102,241,0.6); }}
            .title {{ font-size: 18px; font-weight: 700; letter-spacing: 0.2px; }}
            .chat {{ flex: 1; min-height: 0; overflow-y: auto; padding: 20px; background: transparent; }}
            .bubble {{
                max-width: 72%;
                padding: 12px 14px;
                border-radius: 18px;
                margin-bottom: 10px;
                white-space: pre-wrap;
                overflow-wrap: anywhere;
                word-break: break-word;
                line-height: 1.45;
                background: rgba(255,255,255,0.10);
                border: 1px solid rgba(255,255,255,0.12);
                backdrop-filter: blur(14px) saturate(180%);
                box-shadow: inset 0 1px 0 rgba(255,255,255,0.08), 0 10px 30px rgba(0,0,0,0.18);
            }}
            .bubble.user {{ margin-left: auto; background: rgba(99,102,241,0.55); color: white; border-bottom-right-radius: 6px; }}
            .bubble.bot {{ margin-right: auto; background: rgba(124,58,237,0.45); color: white; border-bottom-left-radius: 6px; }}
            .composer {{
                padding: 16px 20px;
                background: rgba(17,24,39,0.55);
                border-top: 1px solid rgba(255,255,255,0.08);
                display: flex; flex-direction: column; gap: 12px;
                position: sticky; bottom: 0;
                backdrop-filter: blur(18px) saturate(180%);
            }}
            .composer-row {{ display: flex; flex-direction: column; gap: 8px; }}
            .composer-input {{ position: relative; display: flex; align-items: stretch; }}
            textarea {{
                flex: 1;
                min-height: 52px;
                max-height: 200px;
                resize: vertical;
                padding-right: 60px;
                white-space: pre-wrap;
                overflow-wrap: anywhere;
                word-break: break-word;
            }}
            label {{ display: block; font-size: 11px; text-transform: uppercase; letter-spacing: 0.08em; color: rgba(229,231,235,0.7); margin-bottom: 6px; }}
            input, textarea {{
                width: 100%; padding: 10px 12px; border-radius: 12px;
                border: 1px solid rgba(255,255,255,0.12);
                background: rgba(15,23,42,0.55);
                color: #e5e7eb;
                backdrop-filter: blur(12px) saturate(180%);
                box-shadow: inset 0 1px 0 rgba(255,255,255,0.06);
            }}
            button {{
                padding: 10px 18px; border-radius: 16px; border: 1px solid rgba(255,255,255,0.12);
                background: rgba(99,102,241,0.55);
                color: white; font-weight: 600; cursor: pointer;
                backdrop-filter: blur(14px) saturate(180%);
                box-shadow: inset 0 1px 0 rgba(255,255,255,0.08), 0 10px 24px rgba(0,0,0,0.18);
                transition: transform 0.08s ease, box-shadow 0.2s ease, background 0.2s ease;
            }}
            button:hover {{ background: rgba(99,102,241,0.7); }}
            button:active {{ transform: translateY(1px); }}
            button:disabled {{ opacity: 0.6; cursor: not-allowed; }}
            .send {{
                position: absolute;
                right: 6px;
                bottom: 6px;
                height: 40px;
                width: 40px;
                min-width: 40px;
                padding: 0;
                border-radius: 10px;
                display: flex; align-items: center; justify-content: center;
            }}
            .error {{ color: #fca5a5; font-weight: 600; padding: 8px 20px; background: rgba(17,24,39,0.55); backdrop-filter: blur(12px); }}
            .hint {{ color: rgba(229,231,235,0.7); font-size: 12px; }}
            .bubble pre {{ background: rgba(0,0,0,0.2); padding: 10px; border-radius: 10px; overflow-x: auto; }}
            .bubble code {{ font-family: ui-monospace, SFMono-Regular, Menlo, Monaco, Consolas, "Liberation Mono", "Courier New", monospace; }}
            .bubble a {{ color: #e0e7ff; text-decoration: underline; }}
            .bubble blockquote {{ border-left: 3px solid rgba(255,255,255,0.5); margin: 6px 0; padding-left: 10px; color: rgba(255,255,255,0.9); }}
            .bubble ul, .bubble ol {{ padding-left: 20px; margin: 6px 0; }}
            .bubble h1, .bubble h2, .bubble h3 {{ margin: 6px 0; font-weight: 700; }}
            .settings {{ flex: 1; overflow-y: auto; padding: 20px; display: flex; flex-direction: column; gap: 16px; }}
            .settings-card {{
                background: rgba(17,24,39,0.55);
                border: 1px solid rgba(255,255,255,0.12);
                border-radius: 16px;
                padding: 16px;
                backdrop-filter: blur(14px) saturate(180%);
                box-shadow: inset 0 1px 0 rgba(255,255,255,0.06), 0 12px 28px rgba(0,0,0,0.18);
            }}
            .tool-list {{ display: grid; grid-template-columns: repeat(auto-fit, minmax(220px, 1fr)); gap: 10px; }}
            .tool-item {{ display: flex; align-items: center; gap: 10px; }}
            .status {{ color: #34d399; font-weight: 600; }}
        "# }
        div { class: "container",
            div { class: "header",
                div { class: "title", "ButterFly Bot" }
                div { class: "nav",
                    button {
                        class: if *active_tab.read() == UiTab::Chat { "active" } else { "" },
                        onclick: move |_| {
                            let mut active_tab_chat = active_tab_chat.clone();
                            active_tab_chat.set(UiTab::Chat);
                        },
                        "Chat"
                    }
                    button {
                        class: if *active_tab.read() == UiTab::Settings { "active" } else { "" },
                        onclick: move |_| {
                            let mut active_tab_settings = active_tab_settings.clone();
                            active_tab_settings.set(UiTab::Settings);
                        },
                        "Settings"
                    }
                }
            }
            if !error.read().is_empty() {
                div { class: "error", "{error}" }
            }
            if *active_tab.read() == UiTab::Chat {
                div { class: "chat", id: "chat-scroll",
                    for message in messages
                        .read()
                        .iter()
                        .filter(|msg| msg.role == MessageRole::User || !msg.text.is_empty())
                    {
                        div {
                            class: if message.role == MessageRole::User {
                                "bubble user"
                            } else {
                                "bubble bot"
                            },
                            dangerous_inner_html: markdown_to_html(&message.text),
                        }
                    }
                    if *busy.read() {
                        div { class: "hint", "Bot is typing…" }
                    }
                }
                div { class: "composer",
                    div {
                        label { "System Prompt (optional)" }
                        input {
                            value: "{prompt}",
                            oninput: move |evt| {
                                let mut prompt_input = prompt_input.clone();
                                prompt_input.set(evt.value());
                            },
                        }
                    }
                    div { class: "composer-row",
                        label { "Message" }
                        div { class: "composer-input",
                            textarea {
                                value: "{input}",
                                oninput: move |evt| {
                                    let mut message_input = message_input.clone();
                                    message_input.set(evt.value());
                                },
                                onkeydown: move |evt| {
                                    if evt.key() == Key::Enter && !evt.modifiers().shift() {
                                        evt.prevent_default();
                                        on_send_key.call(());
                                    }
                                },
                            }
                            button {
                                class: "send",
                                disabled: *busy.read(),
                                onclick: move |_| on_send.call(()),
                                "Send"
                            }
                        }
                    }
                }
            }
            if *active_tab.read() == UiTab::Settings {
                div { class: "settings",
                    if !*tools_loaded.read() {
                        div { class: "hint", "Loading settings…" }
                    }
                    if *tools_loaded.read() {
                        div { class: "settings-card",
                            label { "Tool Mode" }
                            div { class: "tool-item",
                                input {
                                    r#type: "checkbox",
                                    checked: *tools_safe_mode.read(),
                                    onclick: move |_| {
                                        let mut tools_safe_mode_toggle = tools_safe_mode_toggle.clone();
                                        let current = *tools_safe_mode_toggle.read();
                                        tools_safe_mode_toggle.set(!current);
                                    },
                                }
                                span { "Safe mode (allowlist)" }
                            }
                            p { class: "hint", "When enabled, only tools explicitly enabled below can run." }
                        }
                        div { class: "settings-card",
                            label { "Tools" }
                            div { class: "tool-list",
                                for (idx, tool) in tool_toggles.read().iter().enumerate() {
                                    div { class: "tool-item",
                                        input {
                                            r#type: "checkbox",
                                            checked: tool.enabled,
                                            onclick: move |_| {
                                                let mut tool_toggles = tool_toggles.clone();
                                                let mut list = tool_toggles.write();
                                                if let Some(item) = list.get_mut(idx) {
                                                    item.enabled = !item.enabled;
                                                }
                                            },
                                        }
                                        span { "{tool.name}" }
                                    }
                                }
                            }
                        }
                        div { class: "settings-card",
                            label { "Search Internet" }
                            div { class: "tool-list",
                                div { class: "tool-item",
                                    span { "Provider" }
                                    input {
                                        value: "{search_provider}",
                                        oninput: move |evt| {
                                            let mut search_provider_input = search_provider_input.clone();
                                            search_provider_input.set(evt.value());
                                        },
                                    }
                                }
                                div { class: "tool-item",
                                    span { "Model" }
                                    input {
                                        value: "{search_model}",
                                        oninput: move |evt| {
                                            let mut search_model_input = search_model_input.clone();
                                            search_model_input.set(evt.value());
                                        },
                                    }
                                }
                                div { class: "tool-item",
                                    input {
                                        r#type: "checkbox",
                                        checked: *search_citations.read(),
                                        onclick: move |_| {
                                            let mut search_citations_toggle = search_citations_toggle.clone();
                                            let current = *search_citations_toggle.read();
                                            search_citations_toggle.set(!current);
                                        },
                                    }
                                    span { "Citations" }
                                }
                                div { class: "tool-item",
                                    input {
                                        r#type: "checkbox",
                                        checked: *search_grok_web.read(),
                                        onclick: move |_| {
                                            let mut search_grok_web_toggle = search_grok_web_toggle.clone();
                                            let current = *search_grok_web_toggle.read();
                                            search_grok_web_toggle.set(!current);
                                        },
                                    }
                                    span { "Grok web search" }
                                }
                                div { class: "tool-item",
                                    input {
                                        r#type: "checkbox",
                                        checked: *search_grok_x.read(),
                                        onclick: move |_| {
                                            let mut search_grok_x_toggle = search_grok_x_toggle.clone();
                                            let current = *search_grok_x_toggle.read();
                                            search_grok_x_toggle.set(!current);
                                        },
                                    }
                                    span { "Grok X search" }
                                }
                                div { class: "tool-item",
                                    span { "Grok timeout (seconds)" }
                                    input {
                                        value: "{search_grok_timeout}",
                                        oninput: move |evt| {
                                            let mut search_grok_timeout_input = search_grok_timeout_input.clone();
                                            search_grok_timeout_input.set(evt.value());
                                        },
                                    }
                                }
                                div { class: "tool-item",
                                    span { "Network allowlist" }
                                    input {
                                        placeholder: "example.com, *.trusted.com",
                                        value: "{search_network_allow}",
                                        oninput: move |evt| {
                                            let mut search_network_allow_input = search_network_allow_input.clone();
                                            search_network_allow_input.set(evt.value());
                                        },
                                    }
                                }
                                div { class: "tool-item",
                                    input {
                                        r#type: "checkbox",
                                        checked: *search_default_deny.read(),
                                        onclick: move |_| {
                                            let mut search_default_deny_toggle = search_default_deny_toggle.clone();
                                            let current = *search_default_deny_toggle.read();
                                            search_default_deny_toggle.set(!current);
                                        },
                                    }
                                    span { "Default deny network" }
                                }
                                div { class: "tool-item",
                                    span { "API key (stored in vault)" }
                                    input {
                                        r#type: "password",
                                        placeholder: "Enter new key",
                                        value: "{search_api_key}",
                                        oninput: move |evt| {
                                            let mut search_api_key_input = search_api_key_input.clone();
                                            search_api_key_input.set(evt.value());
                                        },
                                    }
                                }
                                if !search_api_key_status.read().is_empty() {
                                    div { class: "hint", "{search_api_key_status}" }
                                }
                            }
                        }
                        div { class: "settings-card",
                            label { "Reminders" }
                            div { class: "tool-item",
                                span { "SQLite path" }
                                input {
                                    value: "{reminders_sqlite_path}",
                                    oninput: move |evt| {
                                        let mut reminders_sqlite_input = reminders_sqlite_input.clone();
                                        reminders_sqlite_input.set(evt.value());
                                    },
                                }
                            }
                        }
                        div { class: "settings-card",
                            label { "Memory" }
                            div { class: "tool-item",
                                input {
                                    r#type: "checkbox",
                                    checked: *memory_enabled.read(),
                                    onclick: move |_| {
                                        let mut memory_enabled_toggle = memory_enabled_toggle.clone();
                                        let current = *memory_enabled_toggle.read();
                                        memory_enabled_toggle.set(!current);
                                    },
                                }
                                span { "Enable memory" }
                            }
                        }
                        if !settings_error.read().is_empty() {
                            div { class: "error", "{settings_error}" }
                        }
                        if !settings_status.read().is_empty() {
                            div { class: "status", "{settings_status}" }
                        }
                        button { onclick: move |_| on_save_settings.call(()), "Save Settings" }
                    }
                }
            }
        }
    }
}
