#![allow(
    clippy::clone_on_copy,
    clippy::collapsible_match,
    clippy::collapsible_else_if
)]

use dioxus::document::eval;
use dioxus::launch;
use dioxus::prelude::*;
use futures::StreamExt;
use notify_rust::Notification;
use pulldown_cmark::{html, Options, Parser};
use serde::Serialize;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::env;
use std::thread;
use tokio::time::{sleep, Duration};
use time::{format_description::parse, OffsetDateTime};

use crate::services::daemon_client::DaemonClient;

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
    timestamp: String,
    message_id: Option<u64>,
    delivery: Option<MessageDeliveryStatus>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum MessageDeliveryStatus {
    Sending,
    Sent,
    Delivered,
    Read,
}

fn delivery_marker(status: MessageDeliveryStatus) -> &'static str {
    match status {
        MessageDeliveryStatus::Sending => "⏳",
        MessageDeliveryStatus::Sent => "✓",
        MessageDeliveryStatus::Delivered => "✓✓",
        MessageDeliveryStatus::Read => "✓✓",
    }
}

fn message_meta(message: &ChatMessage) -> String {
    if let Some(status) = message.delivery {
        format!("{} {}", message.timestamp, delivery_marker(status))
    } else {
        message.timestamp.clone()
    }
}

#[derive(Clone)]
struct ChatSummary {
    id: String,
    title: String,
    status: String,
    last_message: String,
    last_time: String,
    unread_count: u32,
    peer_id: String,
    onion_address: String,
    trust_state: String,
    public_key: Option<String>,
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

fn now_timestamp() -> String {
    let format = parse("[hour repr:24]:[minute]").unwrap();
    let now = OffsetDateTime::now_utc();
    now.format(&format).unwrap_or_else(|_| "00:00".to_string())
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
    let messages_by_chat = use_signal(HashMap::<String, Vec<ChatMessage>>::new);
    let next_id = use_signal(|| 1u64);
    let active_tab = use_signal(|| UiTab::Chat);
    let reminders_listening = use_signal(|| false);
    let ui_events_listening = use_signal(|| false);
    let chat_search = use_signal(String::new);
    let active_chat_id = use_signal(|| "bot".to_string());
    let username_lookup_inflight = use_signal(|| false);
    let chats = use_signal(|| {
        vec![
            ChatSummary {
                id: "bot".to_string(),
                title: "Butterfly Bot".to_string(),
                status: "online".to_string(),
                last_message: "Ready to help.".to_string(),
                last_time: now_timestamp(),
                unread_count: 0,
                peer_id: "bot".to_string(),
                onion_address: "".to_string(),
                trust_state: "verified".to_string(),
                public_key: None,
            },
            ChatSummary {
                id: "peer".to_string(),
                title: "Peer".to_string(),
                status: "last seen recently".to_string(),
                last_message: "No messages yet".to_string(),
                last_time: "".to_string(),
                unread_count: 0,
                peer_id: "peer".to_string(),
                onion_address: "".to_string(),
                trust_state: "unverified".to_string(),
                public_key: None,
            },
        ]
    });
    let peer_id = use_signal(|| "peer".to_string());
    let trust_state = use_signal(|| "unknown".to_string());
    let trust_error = use_signal(String::new);
    let contacts_loaded = use_signal(|| false);
    let contacts_error = use_signal(String::new);
    let contacts_lookup_inflight = use_signal(|| false);
    let contact_label = use_signal(String::new);
    let contact_onion = use_signal(String::new);
    let p2p_info_loaded = use_signal(|| false);
    let p2p_info_inflight = use_signal(|| false);
    let p2p_peer_id = use_signal(String::new);
    let p2p_listen_addrs = use_signal(String::new);
    let p2p_error = use_signal(String::new);
    let username_claim = use_signal(String::new);
    let username_status = use_signal(String::new);
    let username_lookup = use_signal(String::new);
    let username_lookup_error = use_signal(String::new);
    let username_ready = use_signal(|| false);
    let local_public_key = use_signal(String::new);
    let e2e_public_key = use_signal(String::new);
    let e2e_peer_public_key = use_signal(String::new);
    let e2e_plaintext = use_signal(String::new);
    let e2e_ciphertext = use_signal(String::new);
    let e2e_error = use_signal(String::new);
    let daemon_ready = use_signal(|| false);
    let daemon_probe_inflight = use_signal(|| false);

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
        let e2e_peer_public_key = e2e_peer_public_key.clone();
        let busy = busy.clone();
        let error = error.clone();
        let messages_by_chat = messages_by_chat.clone();
        let active_chat_id = active_chat_id.clone();
        let chats = chats.clone();
        let next_id = next_id.clone();

        use_callback(move |_| {
            let daemon_url = daemon_url();
            let token = token();
            let user_id = user_id();
            let prompt = prompt();
            let text = input();
            let e2e_peer_public_key = e2e_peer_public_key.clone();
            let busy = busy.clone();
            let error = error.clone();
            let messages_by_chat = messages_by_chat.clone();
            let active_chat_id = active_chat_id.clone();
            let chats = chats.clone();
            let next_id = next_id.clone();

            spawn(async move {
                let mut busy = busy;
                let mut error = error;
                let mut messages_by_chat = messages_by_chat;
                let mut chats = chats;
                let mut next_id = next_id;
                let mut input = input;

                if *busy.read() {
                    error.set("A request is already in progress. Please wait.".to_string());
                    return;
                }

                if active_chat_id() != "bot" {
                    let chat_id = active_chat_id();
                    let chat_info = {
                        let list = chats.read();
                        list.iter().find(|chat| chat.id == chat_id).cloned()
                    };
                    let Some(chat) = chat_info else {
                        error.set("Unknown contact selected.".to_string());
                        return;
                    };
                    if chat.trust_state != "verified" {
                        error.set("Verify the contact before sending.".to_string());
                        return;
                    }
                    let peer_public_key = chat
                        .public_key
                        .clone()
                        .filter(|key| !key.trim().is_empty())
                        .or_else(|| {
                            let key = e2e_peer_public_key();
                            if key.trim().is_empty() {
                                None
                            } else {
                                Some(key)
                            }
                        });
                    let Some(peer_public_key) = peer_public_key else {
                        error.set("Peer public key is required.".to_string());
                        return;
                    };

                    busy.set(true);
                    error.set(String::new());

                    let user_message_id = {
                        let id = next_id();
                        next_id.set(id + 1);
                        id
                    };
                    let timestamp = now_timestamp();
                    {
                        let mut map = messages_by_chat.write();
                        let entry = map.entry(chat_id.clone()).or_default();
                        entry.push(ChatMessage {
                            id: user_message_id,
                            role: MessageRole::User,
                            text: text.clone(),
                            timestamp: timestamp.clone(),
                            message_id: Some(user_message_id),
                            delivery: Some(MessageDeliveryStatus::Sending),
                        });
                    }
                    {
                        let mut list = chats.write();
                        if let Some(idx) = list.iter().position(|item| item.id == chat_id) {
                            let mut item = list.remove(idx);
                            item.last_message = text.clone();
                            item.last_time = timestamp.clone();
                            item.unread_count = 0;
                            list.insert(0, item);
                        }
                    }
                    input.set(String::new());
                    scroll_chat_after_render().await;

                    let local_client = match DaemonClient::new(daemon_url.clone(), token.clone()).await {
                        Ok(client) => client,
                        Err(err) => {
                            error.set(format!("Failed to connect to daemon: {err}"));
                            busy.set(false);
                            return;
                        }
                    };
                    let encrypt_body = json!({
                        "user_id": user_id,
                        "peer_id": chat.peer_id,
                        "peer_public_key": peer_public_key,
                        "plaintext": text,
                    });
                    let envelope = match local_client.post_json_stream("e2e/encrypt", &encrypt_body).await {
                        Ok(resp) if resp.status.is_success() => {
                            let text = resp.collect_string().await.unwrap_or_default();
                            let value = serde_json::from_str::<Value>(&text).ok();
                            value.and_then(|v| v.get("envelope").cloned())
                        }
                        Ok(resp) => {
                            let text = resp.collect_string().await.unwrap_or_default();
                            error.set(text);
                            busy.set(false);
                            return;
                        }
                        Err(err) => {
                            error.set(format!("Encrypt failed: {err}"));
                            busy.set(false);
                            return;
                        }
                    };
                    let Some(envelope) = envelope else {
                        error.set("Encrypt failed: missing envelope.".to_string());
                        busy.set(false);
                        return;
                    };

                    let send_body = json!({
                        "user_id": user_id,
                        "peer_id": chat.peer_id,
                        "message_id": user_message_id,
                        "envelope": envelope,
                    });
                    let mut attempts = 0;
                    loop {
                        attempts += 1;
                        let peer_client = match DaemonClient::new(daemon_url.clone(), token.clone()).await {
                            Ok(client) => client,
                            Err(err) => {
                                error.set(format!("Failed to connect to daemon: {err}"));
                                busy.set(false);
                                break;
                            }
                        };
                        match peer_client.post_json_stream("p2p/message", &send_body).await {
                            Ok(resp) if resp.status.is_success() => {
                                let mut map = messages_by_chat.write();
                                if let Some(list) = map.get_mut(&chat_id) {
                                    if let Some(item) = list
                                        .iter_mut()
                                        .rev()
                                        .find(|msg| msg.message_id == Some(user_message_id))
                                    {
                                        item.delivery = Some(MessageDeliveryStatus::Sent);
                                    }
                                }
                                busy.set(false);
                                break;
                            }
                            Ok(resp) => {
                                let text = resp.collect_string().await.unwrap_or_default();
                                if attempts >= 3 {
                                    error.set(text);
                                    busy.set(false);
                                    break;
                                }
                            }
                            Err(err) => {
                                if attempts >= 3 {
                                    error.set(format!("Send failed: {err}"));
                                    busy.set(false);
                                    break;
                                }
                            }
                        }
                        sleep(Duration::from_millis(600)).await;
                    }
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
                let chat_id = active_chat_id();
                let timestamp = now_timestamp();
                {
                    let mut map = messages_by_chat.write();
                    let entry = map.entry(chat_id.clone()).or_default();
                    entry.push(ChatMessage {
                        id: user_message_id,
                        role: MessageRole::User,
                        text: text.clone(),
                        timestamp: timestamp.clone(),
                        message_id: Some(user_message_id),
                        delivery: None,
                    });
                    entry.push(ChatMessage {
                        id: bot_message_id,
                        role: MessageRole::Bot,
                        text: String::new(),
                        timestamp: timestamp.clone(),
                        message_id: None,
                        delivery: None,
                    });
                }
                {
                    let mut list = chats.write();
                    if let Some(idx) = list.iter().position(|chat| chat.id == chat_id) {
                        let mut chat = list.remove(idx);
                        chat.last_message = text.clone();
                        chat.last_time = timestamp.clone();
                        chat.unread_count = 0;
                        list.insert(0, chat);
                    }
                }

                input.set(String::new());
                scroll_chat_after_render().await;

                let body = ProcessTextRequest {
                    user_id,
                    text,
                    prompt: if prompt.trim().is_empty() {
                        None
                    } else {
                        Some(prompt)
                    },
                };

                let mut attempt = 0;
                loop {
                    let client = match DaemonClient::new(daemon_url.clone(), token.clone()).await {
                        Ok(client) => client,
                        Err(err) => {
                            error.set(format!("Failed to connect to daemon: {err}"));
                            break;
                        }
                    };

                    match client.post_json_stream("process_text", &body).await {
                        Ok(response) => {
                            let mut messages_by_chat = messages_by_chat.clone();
                            let mut error = error.clone();
                            if response.status.is_success() {
                                let text = response
                                    .collect_string()
                                    .await
                                    .unwrap_or_default();
                                if let Ok(value) = serde_json::from_str::<Value>(&text) {
                                    if let Some(reply) = value.get("text").and_then(|v| v.as_str()) {
                                        let mut map = messages_by_chat.write();
                                        if let Some(list) = map.get_mut(&chat_id) {
                                            if let Some(last) = list
                                                .iter_mut()
                                                .rev()
                                                .find(|msg| msg.id == bot_message_id)
                                            {
                                                last.text = reply.to_string();
                                            }
                                        }
                                        scroll_chat_to_bottom().await;
                                    } else {
                                        error.set("Invalid response payload.".to_string());
                                    }
                                } else {
                                    error.set("Invalid response payload.".to_string());
                                }
                            } else {
                                let status = response.status;
                                let text = response
                                    .collect_string()
                                    .await
                                    .unwrap_or_else(|_| "Unable to read error body".to_string());
                                error.set(format!("Request failed ({status}): {text}"));
                            }
                            break;
                        }
                        Err(err) => {
                            if attempt == 0 {
                                attempt += 1;
                                start_local_daemon();
                                sleep(Duration::from_millis(400)).await;
                                continue;
                            }
                            error.set(format!(
                                "Request failed: {err}. Daemon unreachable at {daemon_url}."
                            ));
                            break;
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
        let messages_by_chat = messages_by_chat.clone();
        let chats = chats.clone();
        let active_chat_id = active_chat_id.clone();
        let next_id = next_id.clone();

        spawn(async move {
            let mut reminders_listening = reminders_listening;
            let daemon_url = daemon_url;
            let token = token;
            let user_id = user_id;
            let mut messages_by_chat = messages_by_chat;
            let mut chats = chats;
            let active_chat_id = active_chat_id;
            let mut next_id = next_id;

            reminders_listening.set(true);
            loop {
                let token_value = token();
                let client = match DaemonClient::new(daemon_url().to_string(), token_value.clone()).await {
                    Ok(client) => client,
                    Err(_) => {
                        sleep(Duration::from_secs(2)).await;
                        continue;
                    }
                };
                let response = match client
                    .get_stream("reminder_stream", &[("user_id", user_id())])
                    .await
                {
                    Ok(resp) => resp,
                    Err(_) => {
                        sleep(Duration::from_secs(2)).await;
                        continue;
                    }
                };
                if !response.status.is_success() {
                    sleep(Duration::from_secs(2)).await;
                    continue;
                }

                let mut stream = response.stream;
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
                                    let timestamp = now_timestamp();
                                    let chat_id = "bot".to_string();
                                    {
                                        let mut map = messages_by_chat.write();
                                        let entry = map.entry(chat_id.clone()).or_default();
                                        entry.push(ChatMessage {
                                            id,
                                            role: MessageRole::Bot,
                                            text: format!("⏰ {title}"),
                                            timestamp: timestamp.clone(),
                                            message_id: None,
                                            delivery: None,
                                        });
                                    }
                                    {
                                        let mut list = chats.write();
                                        if let Some(idx) = list.iter().position(|chat| chat.id == chat_id) {
                                            let mut chat = list.remove(idx);
                                            chat.last_message = format!("⏰ {title}");
                                            chat.last_time = timestamp.clone();
                                            if active_chat_id() != chat_id {
                                                chat.unread_count += 1;
                                            } else {
                                                chat.unread_count = 0;
                                            }
                                            list.insert(0, chat);
                                        }
                                    }
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
        let messages_by_chat = messages_by_chat.clone();
        let chats = chats.clone();
        let active_chat_id = active_chat_id.clone();
        let next_id = next_id.clone();

        spawn(async move {
            let mut ui_events_listening = ui_events_listening;
            let daemon_url = daemon_url;
            let token = token;
            let user_id = user_id;
            let mut messages_by_chat = messages_by_chat;
            let mut chats = chats;
            let active_chat_id = active_chat_id;
            let mut next_id = next_id;

            ui_events_listening.set(true);
            loop {
                let token_value = token();
                let client = match DaemonClient::new(daemon_url().to_string(), token_value.clone()).await {
                    Ok(client) => client,
                    Err(_) => {
                        sleep(Duration::from_secs(2)).await;
                        continue;
                    }
                };
                let response = match client
                    .get_stream("ui_events", &[("user_id", user_id())])
                    .await
                {
                    Ok(resp) => resp,
                    Err(_) => {
                        sleep(Duration::from_secs(2)).await;
                        continue;
                    }
                };
                if !response.status.is_success() {
                    sleep(Duration::from_secs(2)).await;
                    continue;
                }

                let mut stream = response.stream;
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
                                    let event_type = value
                                        .get("event_type")
                                        .and_then(|v| v.as_str())
                                        .unwrap_or("tool");
                                    if event_type == "p2p_message" {
                                        let payload = value.get("payload").cloned().unwrap_or(Value::Null);
                                        let peer = payload
                                            .get("peer_id")
                                            .and_then(|v| v.as_str())
                                            .unwrap_or("peer");
                                        let message_id = payload.get("message_id").and_then(|v| v.as_u64());
                                        let text = payload
                                            .get("text")
                                            .and_then(|v| v.as_str())
                                            .unwrap_or("");
                                        let id = next_id();
                                        next_id.set(id + 1);
                                        let timestamp = now_timestamp();
                                        let chat_id = peer.to_string();
                                        {
                                            let mut map = messages_by_chat.write();
                                            let entry = map.entry(chat_id.clone()).or_default();
                                            entry.push(ChatMessage {
                                                id,
                                                role: MessageRole::Bot,
                                                text: text.to_string(),
                                                timestamp: timestamp.clone(),
                                                message_id,
                                                delivery: None,
                                            });
                                        }
                                        {
                                            let mut list = chats.write();
                                            if let Some(idx) = list.iter().position(|chat| chat.id == chat_id) {
                                                let mut chat = list.remove(idx);
                                                chat.last_message = text.to_string();
                                                chat.last_time = timestamp.clone();
                                                if active_chat_id() != chat_id {
                                                    chat.unread_count += 1;
                                                } else {
                                                    chat.unread_count = 0;
                                                    if let Some(message_id) = message_id {
                                                        let peer_id = chat.peer_id.clone();
                                                        let user_value = user_id();
                                                        let daemon_url = daemon_url.clone();
                                                        let token = token.clone();
                                                        spawn(async move {
                                                            let client = match DaemonClient::new(
                                                                daemon_url().to_string(),
                                                                token().to_string(),
                                                            )
                                                            .await
                                                            {
                                                                Ok(client) => client,
                                                                Err(_) => return,
                                                            };
                                                            let body = json!({
                                                                "user_id": user_value,
                                                                "peer_id": peer_id,
                                                                "message_id": message_id,
                                                                "status": "read",
                                                            });
                                                            let _ = client.post_json_stream("p2p/receipt", &body).await;
                                                        });
                                                    }
                                                }
                                                list.insert(0, chat);
                                            } else {
                                                list.insert(0, ChatSummary {
                                                    id: chat_id.clone(),
                                                    title: peer.to_string(),
                                                    status: "trust: unverified".to_string(),
                                                    last_message: text.to_string(),
                                                    last_time: timestamp.clone(),
                                                    unread_count: 1,
                                                    peer_id: peer.to_string(),
                                                    onion_address: String::new(),
                                                    trust_state: "unverified".to_string(),
                                                    public_key: None,
                                                });
                                            }
                                        }
                                        scroll_chat_to_bottom().await;
                                    } else if event_type == "p2p_receipt" {
                                        let payload = value.get("payload").cloned().unwrap_or(Value::Null);
                                        let peer = payload
                                            .get("peer_id")
                                            .and_then(|v| v.as_str())
                                            .unwrap_or("peer");
                                        let message_id = payload.get("message_id").and_then(|v| v.as_u64());
                                        let status = payload
                                            .get("status")
                                            .and_then(|v| v.as_str())
                                            .unwrap_or("sent");
                                        if let Some(message_id) = message_id {
                                            let mut map = messages_by_chat.write();
                                            if let Some(list) = map.get_mut(peer) {
                                                if let Some(item) = list
                                                    .iter_mut()
                                                    .rev()
                                                    .find(|msg| msg.message_id == Some(message_id))
                                                {
                                                    item.delivery = Some(match status {
                                                        "read" => MessageDeliveryStatus::Read,
                                                        "delivered" => MessageDeliveryStatus::Delivered,
                                                        "sent" => MessageDeliveryStatus::Sent,
                                                        _ => MessageDeliveryStatus::Sent,
                                                    });
                                                }
                                            }
                                        }
                                    } else {
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
                                        let timestamp = now_timestamp();
                                        let chat_id = "bot".to_string();
                                        {
                                            let mut map = messages_by_chat.write();
                                            let entry = map.entry(chat_id.clone()).or_default();
                                            entry.push(ChatMessage {
                                                id,
                                                role: MessageRole::Bot,
                                                text: text.clone(),
                                                timestamp: timestamp.clone(),
                                                message_id: None,
                                                delivery: None,
                                            });
                                        }
                                        {
                                            let mut list = chats.write();
                                            if let Some(idx) = list.iter().position(|chat| chat.id == chat_id) {
                                                let mut chat = list.remove(idx);
                                                chat.last_message = text;
                                                chat.last_time = timestamp.clone();
                                                if active_chat_id() != chat_id {
                                                    chat.unread_count += 1;
                                                } else {
                                                    chat.unread_count = 0;
                                                }
                                                list.insert(0, chat);
                                            }
                                        }
                                        scroll_chat_to_bottom().await;
                                    }
                                }
                            }
                        }
                    }
                }
                sleep(Duration::from_secs(2)).await;
            }
        });
    }

    if !*daemon_ready.read() && !*daemon_probe_inflight.read() {
        let daemon_ready = daemon_ready.clone();
        let daemon_probe_inflight = daemon_probe_inflight.clone();
        let daemon_url = daemon_url.clone();
        let token = token.clone();

        spawn(async move {
            let mut daemon_ready = daemon_ready;
            let mut daemon_probe_inflight = daemon_probe_inflight;
            daemon_probe_inflight.set(true);
            for attempt in 0..8 {
                let client = match DaemonClient::new(daemon_url().to_string(), token().to_string()).await {
                    Ok(client) => client,
                    Err(_) => {
                        sleep(Duration::from_millis(400 * (attempt + 1) as u64)).await;
                        continue;
                    }
                };
                match client.get_stream("health", &[]).await {
                    Ok(resp) if resp.status.is_success() => {
                        daemon_ready.set(true);
                        break;
                    }
                    _ => {
                        sleep(Duration::from_millis(400 * (attempt + 1) as u64)).await;
                    }
                }
            }
            daemon_probe_inflight.set(false);
        });
    }

    if *daemon_ready.read() && !*contacts_loaded.read() && !*contacts_lookup_inflight.read() {
        let contacts_loaded = contacts_loaded.clone();
        let contacts_error = contacts_error.clone();
        let contacts_lookup_inflight = contacts_lookup_inflight.clone();
        let chats = chats.clone();
        let active_chat_id = active_chat_id.clone();
        let daemon_url = daemon_url.clone();
        let token = token.clone();
        let user_id = user_id.clone();

        spawn(async move {
            let mut contacts_loaded = contacts_loaded;
            let mut contacts_error = contacts_error;
            let mut contacts_lookup_inflight = contacts_lookup_inflight;
            let mut chats = chats;
            let mut active_chat_id = active_chat_id;

            contacts_lookup_inflight.set(true);
            for attempt in 0..5 {
                let client = match DaemonClient::new(daemon_url().to_string(), token().to_string()).await {
                    Ok(client) => client,
                    Err(err) => {
                        contacts_error.set(format!("{err}"));
                        sleep(Duration::from_millis(400 * (attempt + 1) as u64)).await;
                        continue;
                    }
                };
                let response = client
                    .get_stream("contacts", &[("user_id", user_id())])
                    .await;
                match response {
                    Ok(resp) if resp.status.is_success() => {
                        if let Ok(text) = resp.collect_string().await {
                            if let Ok(value) = serde_json::from_str::<Value>(&text) {
                                let mut list = chats.write();
                                let existing_bot = list.iter().find(|chat| chat.id == "bot").cloned();
                                let bot_chat = existing_bot.unwrap_or(ChatSummary {
                                    id: "bot".to_string(),
                                    title: "Butterfly Bot".to_string(),
                                    status: "online".to_string(),
                                    last_message: "Ready to help.".to_string(),
                                    last_time: now_timestamp(),
                                    unread_count: 0,
                                    peer_id: "bot".to_string(),
                                    onion_address: "".to_string(),
                                    trust_state: "verified".to_string(),
                                    public_key: None,
                                });
                                let mut next_list = vec![bot_chat];
                                if let Some(items) = value.get("contacts").and_then(|v| v.as_array()) {
                                    for item in items {
                                        let peer_id = item.get("peer_id").and_then(|v| v.as_str()).unwrap_or("peer");
                                        let label = item.get("label").and_then(|v| v.as_str()).unwrap_or(peer_id);
                                        let onion = item
                                            .get("onion_address")
                                            .and_then(|v| v.as_str())
                                            .unwrap_or_default();
                                        let trust = item
                                            .get("trust_state")
                                            .and_then(|v| v.as_str())
                                            .unwrap_or("unverified");
                                        let public_key = item
                                            .get("public_key")
                                            .and_then(|v| v.as_str())
                                            .map(|value| value.to_string());
                                        let status = if onion.is_empty() {
                                            format!("trust: {trust}")
                                        } else {
                                            format!("trust: {trust} • {onion}")
                                        };
                                        let existing = list.iter().find(|chat| chat.id == peer_id).cloned();
                                        let mut chat = existing.unwrap_or(ChatSummary {
                                            id: peer_id.to_string(),
                                            title: label.to_string(),
                                            status: status.clone(),
                                            last_message: "No messages yet".to_string(),
                                            last_time: "".to_string(),
                                            unread_count: 0,
                                            peer_id: peer_id.to_string(),
                                            onion_address: onion.to_string(),
                                            trust_state: trust.to_string(),
                                            public_key: public_key.clone(),
                                        });
                                        chat.title = label.to_string();
                                        chat.status = status;
                                        chat.onion_address = onion.to_string();
                                        chat.peer_id = peer_id.to_string();
                                        chat.trust_state = trust.to_string();
                                        chat.public_key = public_key.clone();
                                        next_list.push(chat);
                                    }
                                }
                                let active = active_chat_id();
                                if !next_list.iter().any(|chat| chat.id == active) {
                                    active_chat_id.set("bot".to_string());
                                }
                                *list = next_list;
                                contacts_loaded.set(true);
                                break;
                            }
                        }
                    }
                    Ok(resp) => {
                        let text = resp.collect_string().await.unwrap_or_default();
                        contacts_error.set(text);
                    }
                    Err(err) => contacts_error.set(format!("{err}")),
                }

                sleep(Duration::from_millis(400 * (attempt + 1) as u64)).await;
            }
            contacts_lookup_inflight.set(false);
        });
    }

    if *daemon_ready.read() && !*p2p_info_loaded.read() && !*p2p_info_inflight.read() {
        let p2p_info_loaded = p2p_info_loaded.clone();
        let p2p_peer_id = p2p_peer_id.clone();
        let p2p_listen_addrs = p2p_listen_addrs.clone();
        let p2p_error = p2p_error.clone();
        let p2p_info_inflight = p2p_info_inflight.clone();
        let daemon_url = daemon_url.clone();
        let token = token.clone();

        spawn(async move {
            let mut p2p_info_loaded = p2p_info_loaded;
            let mut p2p_peer_id = p2p_peer_id;
            let mut p2p_listen_addrs = p2p_listen_addrs;
            let mut p2p_error = p2p_error;
            let mut p2p_info_inflight = p2p_info_inflight;

            p2p_info_inflight.set(true);
            for attempt in 0..5 {
                let client = match DaemonClient::new(daemon_url().to_string(), token().to_string()).await {
                    Ok(client) => client,
                    Err(err) => {
                        p2p_error.set(format!("{err}"));
                        sleep(Duration::from_millis(400 * (attempt + 1) as u64)).await;
                        continue;
                    }
                };
                let response = client.get_stream("p2p/info", &[]).await;
                match response {
                    Ok(resp) if resp.status.is_success() => {
                        if let Ok(text) = resp.collect_string().await {
                            if let Ok(value) = serde_json::from_str::<Value>(&text) {
                                if let Some(peer) = value.get("peer_id").and_then(|v| v.as_str()) {
                                    p2p_peer_id.set(peer.to_string());
                                }
                                if let Some(addrs) = value.get("listen_addrs").and_then(|v| v.as_array()) {
                                    let list = addrs
                                        .iter()
                                        .filter_map(|v| v.as_str().map(|s| s.to_string()))
                                        .collect::<Vec<_>>()
                                        .join("\n");
                                    p2p_listen_addrs.set(list);
                                }
                                p2p_info_loaded.set(true);
                                break;
                            }
                        }
                    }
                    Ok(resp) => {
                        let text = resp.collect_string().await.unwrap_or_default();
                        p2p_error.set(text);
                    }
                    Err(err) => p2p_error.set(format!("{err}")),
                }

                sleep(Duration::from_millis(400 * (attempt + 1) as u64)).await;
            }
            p2p_info_inflight.set(false);
        });
    }

    if *daemon_ready.read()
        && (local_public_key().is_empty() || !*username_ready.read())
        && !*username_lookup_inflight.read()
    {
        let mut local_public_key = local_public_key.clone();
        let mut username_ready = username_ready.clone();
        let mut username_claim = username_claim.clone();
        let daemon_url = daemon_url.clone();
        let token = token.clone();
        let user_id = user_id.clone();
        let mut username_lookup_error = username_lookup_error.clone();
        let mut username_lookup_inflight = username_lookup_inflight.clone();

        spawn(async move {
            username_lookup_inflight.set(true);
            let client = match DaemonClient::new(daemon_url().to_string(), token().to_string()).await {
                Ok(client) => client,
                Err(err) => {
                    username_lookup_error.set(format!("{err}"));
                    username_lookup_inflight.set(false);
                    return;
                }
            };
            for attempt in 0..5 {
                if local_public_key().is_empty() {
                    let response = client
                        .get_stream("e2e/identity", &[("user_id", user_id())])
                        .await;
                    if let Ok(resp) = response {
                        if resp.status.is_success() {
                            if let Ok(text) = resp.collect_string().await {
                                if let Ok(value) = serde_json::from_str::<Value>(&text) {
                                    if let Some(key) = value.get("public_key").and_then(|v| v.as_str()) {
                                        local_public_key.set(key.to_string());
                                    }
                                }
                            }
                        }
                    }
                }
                let response = client
                    .get_stream("username/me", &[("user_id", user_id())])
                    .await;
                match response {
                    Ok(resp) if resp.status.is_success() => {
                        if let Ok(text) = resp.collect_string().await {
                            if let Ok(value) = serde_json::from_str::<Value>(&text) {
                                if let Some(name) = value.get("username").and_then(|v| v.as_str()) {
                                    username_claim.set(name.to_string());
                                    username_ready.set(true);
                                    break;
                                }
                            }
                        }
                    }
                    Ok(resp) => {
                        if resp.status == reqwest::StatusCode::NOT_FOUND {
                            username_ready.set(false);
                            break;
                        } else {
                            let text = resp.collect_string().await.unwrap_or_default();
                            username_lookup_error.set(text);
                        }
                    }
                    Err(err) => username_lookup_error.set(format!("{err}")),
                }

                sleep(Duration::from_millis(400 * (attempt + 1) as u64)).await;
            }
            username_lookup_inflight.set(false);
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
    let peer_id_input = peer_id.clone();
    let contact_label_input = contact_label.clone();
    let contact_onion_input = contact_onion.clone();
    let contacts_error_value = contacts_error.clone();
    let p2p_peer_id_value = p2p_peer_id.clone();
    let p2p_listen_addrs_value = p2p_listen_addrs.clone();
    let p2p_error_value = p2p_error.clone();
    let username_claim_input = username_claim.clone();
    let username_status_value = username_status.clone();
    let username_lookup_input = username_lookup.clone();
    let username_lookup_error_value = username_lookup_error.clone();
    let e2e_public_key_value = e2e_public_key.clone();
    let e2e_peer_public_key_input = e2e_peer_public_key.clone();
    let e2e_plaintext_input = e2e_plaintext.clone();
    let e2e_ciphertext_input = e2e_ciphertext.clone();
    let trust_state_value = trust_state.clone();
    let trust_error_value = trust_error.clone();
    let chat_search_input = chat_search.clone();
    let active_chat_id_value = active_chat_id.clone();
    let chats_value = chats.clone();
    let chat_search_value = chat_search();
    let active_chat_summary = {
        let active_id = active_chat_id();
        chats
            .read()
            .iter()
            .find(|chat| chat.id == active_id)
            .cloned()
    };
    let chat_list = chats.read().clone();
    let active_chat_title = active_chat_summary
        .as_ref()
        .map(|chat| chat.title.clone())
        .unwrap_or_else(|| "Chat".to_string());
    let active_chat_status = active_chat_summary
        .as_ref()
        .map(|chat| chat.status.clone())
        .unwrap_or_else(|| "offline".to_string());

    rsx! {
        style { r#"
            body {{
                font-family: system-ui, -apple-system, BlinkMacSystemFont, "SF Pro Text", "SF Pro Display", sans-serif;
                background: radial-gradient(1200px 800px at 20% -10%, rgba(120,119,198,0.35), transparent 60%),
                            radial-gradient(1000px 700px at 110% 10%, rgba(56,189,248,0.25), transparent 60%),
                            #0b1020;
                color: #e5e7eb;
            }}
            .container {{ width: 100%; margin: 0 auto; padding: 0; height: 100vh; display: flex; flex-direction: column; }}
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
            .layout {{ flex: 1; min-height: 0; display: flex; overflow: hidden; }}
            .sidebar {{
                width: 320px; min-width: 280px; max-width: 360px;
                background: rgba(12,18,34,0.85);
                border-right: 1px solid rgba(255,255,255,0.08);
                display: flex; flex-direction: column;
            }}
            .sidebar-header {{ padding: 16px; display: flex; flex-direction: column; gap: 10px; }}
            .chat-search input {{ padding: 10px 12px; border-radius: 12px; }}
            .chat-list {{ flex: 1; overflow-y: auto; padding: 6px 8px 12px; display: flex; flex-direction: column; gap: 6px; }}
            .chat-item {{
                display: grid; grid-template-columns: 1fr auto; gap: 8px;
                padding: 12px; border-radius: 14px; cursor: pointer;
                border: 1px solid transparent;
                background: rgba(15,23,42,0.4);
                transition: background 0.2s ease, border 0.2s ease;
            }}
            .chat-item.active {{ background: rgba(99,102,241,0.2); border-color: rgba(99,102,241,0.4); }}
            .chat-item-title {{ font-weight: 700; }}
            .chat-item-preview {{ color: rgba(229,231,235,0.7); font-size: 12px; }}
            .chat-meta {{ display: flex; flex-direction: column; align-items: flex-end; gap: 6px; font-size: 11px; color: rgba(229,231,235,0.6); }}
            .chat-badge {{ background: rgba(99,102,241,0.7); color: white; font-size: 11px; padding: 2px 8px; border-radius: 999px; }}
            .chat-view {{ flex: 1; min-width: 0; display: flex; flex-direction: column; }}
            .chat-topbar {{
                padding: 14px 20px;
                display: flex; align-items: center; justify-content: space-between;
                background: rgba(17,24,39,0.55);
                border-bottom: 1px solid rgba(255,255,255,0.08);
                backdrop-filter: blur(18px) saturate(180%);
            }}
            .chat-title {{ font-size: 16px; font-weight: 700; }}
            .chat-status {{ font-size: 12px; color: rgba(229,231,235,0.7); }}
            .chat-scroll {{ flex: 1; min-height: 0; overflow-y: auto; padding: 20px; background: transparent; }}
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
            .timestamp {{ margin-top: 6px; font-size: 11px; color: rgba(229,231,235,0.6); text-align: right; }}
            .composer {{
                padding: 16px 20px;
                background: rgba(17,24,39,0.55);
                border-top: 1px solid rgba(255,255,255,0.08);
                display: flex; flex-direction: column; gap: 12px;
                position: sticky; bottom: 0;
                backdrop-filter: blur(18px) saturate(180%);
            }}
            .composer-toolbar {{ display: flex; gap: 8px; align-items: center; }}
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
            .attach {{
                background: rgba(255,255,255,0.12);
                border: 1px solid rgba(255,255,255,0.12);
                height: 36px; width: 36px; min-width: 36px;
                border-radius: 10px; padding: 0;
            }}
            .error {{ color: #fca5a5; font-weight: 600; padding: 8px 20px; background: rgba(17,24,39,0.55); backdrop-filter: blur(12px); }}
            .hint {{ color: rgba(229,231,235,0.7); font-size: 12px; }}
            .gate {{ position: fixed; inset: 0; background: rgba(7,10,20,0.86); display: flex; align-items: center; justify-content: center; z-index: 20; }}
            .gate-card {{ max-width: 420px; width: 100%; background: rgba(17,24,39,0.85); border: 1px solid rgba(255,255,255,0.12); border-radius: 18px; padding: 24px; display: flex; flex-direction: column; gap: 14px; box-shadow: 0 12px 30px rgba(0,0,0,0.35); }}
            .gate-title {{ font-size: 18px; font-weight: 700; }}
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
            if !*username_ready.read() {
                div { class: "gate",
                    div { class: "gate-card",
                        div { class: "gate-title", "Claim your username" }
                        div { class: "hint", "Pick a unique name (3-32 chars, a-z, 0-9, underscore)." }
                        if !*daemon_ready.read() {
                            div { class: "hint", "Daemon starting…" }
                        }
                        input {
                            value: "{username_claim}",
                            placeholder: "yourname",
                            oninput: move |evt| {
                                let mut username_claim_input = username_claim_input.clone();
                                username_claim_input.set(evt.value());
                            },
                        }
                        button {
                            disabled: !*daemon_ready.read(),
                            onclick: move |_| {
                                let daemon_url = daemon_url.clone();
                                let token = token.clone();
                                let user_id = user_id.clone();
                                let username_claim = username_claim.clone();
                                let local_public_key = local_public_key.clone();
                                let mut username_ready = username_ready.clone();
                                let mut username_status_value = username_status_value.clone();
                                let mut username_lookup_error_value = username_lookup_error_value.clone();
                                spawn(async move {
                                    username_status_value.set(String::new());
                                    username_lookup_error_value.set(String::new());
                                    let client = match DaemonClient::new(
                                        daemon_url().to_string(),
                                        token().to_string(),
                                    )
                                    .await
                                    {
                                        Ok(client) => client,
                                        Err(err) => {
                                            username_lookup_error_value.set(format!("{err}"));
                                            return;
                                        }
                                    };
                                    let lookup = client
                                        .get_stream(
                                            "username/lookup",
                                            &[("username", username_claim())],
                                        )
                                        .await;
                                    match lookup {
                                        Ok(resp) if resp.status.is_success() => {
                                            if let Ok(text) = resp.collect_string().await {
                                                if let Ok(value) = serde_json::from_str::<Value>(&text) {
                                                    let public_key = value
                                                        .get("public_key")
                                                        .and_then(|v| v.as_str())
                                                        .unwrap_or("");
                                                    if !public_key.is_empty()
                                                        && public_key != local_public_key()
                                                    {
                                                        username_lookup_error_value
                                                            .set("Username already taken.".to_string());
                                                        return;
                                                    }
                                                }
                                            }
                                        }
                                        Ok(resp) if resp.status == reqwest::StatusCode::NOT_FOUND => {}
                                        Ok(resp) => {
                                            let text = resp.collect_string().await.unwrap_or_default();
                                            username_lookup_error_value.set(text);
                                            return;
                                        }
                                        Err(err) => {
                                            username_lookup_error_value.set(format!("{err}"));
                                            return;
                                        }
                                    }
                                    let body = json!({
                                        "user_id": user_id(),
                                        "username": username_claim(),
                                    });
                                    match client.post_json_stream("username/claim", &body).await {
                                        Ok(resp) if resp.status.is_success() => {
                                            username_status_value.set("Username claimed".to_string());
                                            username_ready.set(true);
                                        }
                                        Ok(resp) => {
                                            let text = resp.collect_string().await.unwrap_or_default();
                                            username_lookup_error_value.set(text);
                                        }
                                        Err(err) => username_lookup_error_value.set(format!("{err}")),
                                    }
                                });
                            },
                            "Claim"
                        }
                        if !username_lookup_error.read().is_empty() {
                            div { class: "error", "{username_lookup_error}" }
                        }
                    }
                }
            }
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
                div { class: "layout",
                    div { class: "sidebar",
                        div { class: "sidebar-header",
                            div { class: "chat-search",
                                input {
                                    placeholder: "Search",
                                    value: "{chat_search}",
                                    oninput: move |evt| {
                                        let mut chat_search_input = chat_search_input.clone();
                                        chat_search_input.set(evt.value());
                                    },
                                }
                            }
                        }
                        div { class: "chat-list",
                            for chat in chat_list
                                .iter()
                                .cloned()
                                .filter(|chat| {
                                    if chat_search_value.trim().is_empty() {
                                        true
                                    } else {
                                        chat.title
                                            .to_lowercase()
                                            .contains(&chat_search_value.to_lowercase())
                                    }
                                })
                            {
                                div {
                                    class: if chat.id == *active_chat_id.read() {
                                        "chat-item active"
                                    } else {
                                        "chat-item"
                                    },
                                    onclick: {
                                        let chat_id = chat.id.clone();
                                        let peer_value = chat.peer_id.clone();
                                        let label_value = chat.title.clone();
                                        let onion_value = chat.onion_address.clone();
                                        let trust_value = chat.trust_state.clone();
                                        let public_key_value = chat.public_key.clone();
                                        let mut active_chat_id_value = active_chat_id_value.clone();
                                        let mut chats_value = chats_value.clone();
                                        let mut peer_id_input = peer_id_input.clone();
                                        let mut contact_label_input = contact_label_input.clone();
                                        let mut contact_onion_input = contact_onion_input.clone();
                                        let mut trust_state_value = trust_state_value.clone();
                                        let mut e2e_peer_public_key_input = e2e_peer_public_key_input.clone();
                                        let messages_by_chat = messages_by_chat.clone();
                                        let user_id = user_id.clone();
                                        move |_| {
                                            active_chat_id_value.set(chat_id.clone());
                                            peer_id_input.set(peer_value.clone());
                                            contact_label_input.set(label_value.clone());
                                            contact_onion_input.set(onion_value.clone());
                                            trust_state_value.set(trust_value.clone());
                                            if let Some(value) = public_key_value.clone() {
                                                e2e_peer_public_key_input.set(value);
                                            }
                                            if chat.unread_count > 0 {
                                                let chat_id = chat_id.clone();
                                                let peer_value = peer_value.clone();
                                                let messages_by_chat = messages_by_chat.clone();
                                                let user_value = user_id();
                                                let daemon_url = daemon_url.clone();
                                                let token = token.clone();
                                                spawn(async move {
                                                    let message_id = messages_by_chat
                                                        .read()
                                                        .get(&chat_id)
                                                        .and_then(|list| list.iter().rev().find_map(|msg| msg.message_id));
                                                    let Some(message_id) = message_id else {
                                                        return;
                                                    };
                                                    let client = match DaemonClient::new(
                                                        daemon_url().to_string(),
                                                        token().to_string(),
                                                    )
                                                    .await
                                                    {
                                                        Ok(client) => client,
                                                        Err(_) => return,
                                                    };
                                                    let body = json!({
                                                        "user_id": user_value,
                                                        "peer_id": peer_value,
                                                        "message_id": message_id,
                                                        "status": "read",
                                                    });
                                                    let _ = client.post_json_stream("p2p/receipt", &body).await;
                                                });
                                            }
                                            let mut list = chats_value.write();
                                            if let Some(idx) = list.iter().position(|item| item.id == chat_id) {
                                                let mut selected = list.remove(idx);
                                                selected.unread_count = 0;
                                                list.insert(0, selected);
                                            }
                                        }
                                    },
                                    div {
                                        div { class: "chat-item-title", "{chat.title}" }
                                        div { class: "chat-item-preview", "{chat.last_message}" }
                                    }
                                    div { class: "chat-meta",
                                        if !chat.last_time.is_empty() {
                                            div { class: "chat-time", "{chat.last_time}" }
                                        }
                                        if chat.unread_count > 0 {
                                            div { class: "chat-badge", "{chat.unread_count}" }
                                        }
                                    }
                                }
                            }
                        }
                    }
                    div { class: "chat-view",
                        div { class: "chat-topbar",
                            div {
                                div { class: "chat-title", "{active_chat_title}" }
                                div { class: "chat-status", "{active_chat_status}" }
                            }
                        }
                        div { class: "chat-scroll", id: "chat-scroll",
                            if let Some(list) = messages_by_chat.read().get(&active_chat_id()) {
                                for message in list
                                    .iter()
                                    .filter(|msg| msg.role == MessageRole::User || !msg.text.is_empty())
                                {
                                    div {
                                        class: if message.role == MessageRole::User {
                                            "bubble user"
                                        } else {
                                            "bubble bot"
                                        },
                                        div { dangerous_inner_html: markdown_to_html(&message.text) }
                                        div { class: "timestamp", "{message_meta(message)}" }
                                    }
                                }
                            }
                            if *busy.read() {
                                div { class: "hint", "Bot is typing…" }
                            }
                        }
                        div { class: "composer",
                            div { class: "composer-toolbar",
                                button { class: "attach", disabled: true, "📎" }
                                div { class: "hint", "Attachments coming soon" }
                            }
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
                        div { class: "settings-card",
                            label { "P2P" }
                            div { class: "tool-item",
                                span { "Peer ID" }
                                input { value: "{p2p_peer_id}", readonly: true }
                            }
                            div { class: "tool-item",
                                span { "Listen addresses" }
                                textarea { value: "{p2p_listen_addrs}", rows: 2, readonly: true }
                            }
                            div { class: "tool-item",
                                button {
                                    onclick: move |_| {
                                        let daemon_url = daemon_url.clone();
                                        let token = token.clone();
                                        let mut p2p_peer_id_value = p2p_peer_id_value.clone();
                                        let mut p2p_listen_addrs_value = p2p_listen_addrs_value.clone();
                                        let mut p2p_error_value = p2p_error_value.clone();
                                        spawn(async move {
                                            p2p_error_value.set(String::new());
                                            let client = match DaemonClient::new(
                                                daemon_url().to_string(),
                                                token().to_string(),
                                            )
                                            .await
                                            {
                                                Ok(client) => client,
                                                Err(err) => {
                                                    p2p_error_value.set(format!("{err}"));
                                                    return;
                                                }
                                            };
                                            let response = client.get_stream("p2p/info", &[]).await;
                                            match response {
                                                Ok(resp) if resp.status.is_success() => {
                                                    if let Ok(text) = resp.collect_string().await {
                                                        if let Ok(value) = serde_json::from_str::<Value>(&text) {
                                                            if let Some(peer) = value
                                                                .get("peer_id")
                                                                .and_then(|v| v.as_str())
                                                            {
                                                                p2p_peer_id_value.set(peer.to_string());
                                                            }
                                                            if let Some(addrs) = value
                                                                .get("listen_addrs")
                                                                .and_then(|v| v.as_array())
                                                            {
                                                                let list = addrs
                                                                    .iter()
                                                                    .filter_map(|v| {
                                                                        v.as_str().map(|s| s.to_string())
                                                                    })
                                                                    .collect::<Vec<_>>()
                                                                    .join("\n");
                                                                p2p_listen_addrs_value.set(list);
                                                            }
                                                        }
                                                    }
                                                }
                                                Ok(resp) => {
                                                    let text =
                                                        resp.collect_string().await.unwrap_or_default();
                                                    p2p_error_value.set(text);
                                                }
                                                Err(err) => p2p_error_value.set(format!("{err}")),
                                            }
                                        });
                                    },
                                    "Refresh"
                                }
                            }
                            if !p2p_error.read().is_empty() {
                                div { class: "error", "{p2p_error}" }
                            }
                        }
                        div { class: "settings-card",
                            label { "Username" }
                            div { class: "tool-item",
                                span { "Claim username" }
                                input {
                                    value: "{username_claim}",
                                    placeholder: "alphanumeric / underscore",
                                    oninput: move |evt| {
                                        let mut username_claim_input = username_claim_input.clone();
                                        username_claim_input.set(evt.value());
                                    },
                                }
                                button {
                                    onclick: move |_| {
                                        let daemon_url = daemon_url.clone();
                                        let token = token.clone();
                                        let user_id = user_id.clone();
                                        let username_claim = username_claim.clone();
                                        let local_public_key = local_public_key.clone();
                                        let mut username_ready = username_ready.clone();
                                        let mut username_status_value = username_status_value.clone();
                                        let mut username_lookup_error_value = username_lookup_error_value.clone();
                                        spawn(async move {
                                            username_status_value.set(String::new());
                                            username_lookup_error_value.set(String::new());
                                            let client = match DaemonClient::new(
                                                daemon_url().to_string(),
                                                token().to_string(),
                                            )
                                            .await
                                            {
                                                Ok(client) => client,
                                                Err(err) => {
                                                    username_lookup_error_value.set(format!("{err}"));
                                                    return;
                                                }
                                            };
                                            let lookup = client
                                                .get_stream(
                                                    "username/lookup",
                                                    &[("username", username_claim())],
                                                )
                                                .await;
                                            match lookup {
                                                Ok(resp) if resp.status.is_success() => {
                                                    if let Ok(text) = resp.collect_string().await {
                                                        if let Ok(value) =
                                                            serde_json::from_str::<Value>(&text)
                                                        {
                                                            let public_key = value
                                                                .get("public_key")
                                                                .and_then(|v| v.as_str())
                                                                .unwrap_or("");
                                                            if !public_key.is_empty()
                                                                && public_key != local_public_key()
                                                            {
                                                                username_lookup_error_value
                                                                    .set("Username already taken.".to_string());
                                                                return;
                                                            }
                                                        }
                                                    }
                                                }
                                                Ok(resp)
                                                    if resp.status == reqwest::StatusCode::NOT_FOUND => {}
                                                Ok(resp) => {
                                                    let text =
                                                        resp.collect_string().await.unwrap_or_default();
                                                    username_lookup_error_value.set(text);
                                                    return;
                                                }
                                                Err(err) => {
                                                    username_lookup_error_value.set(format!("{err}"));
                                                    return;
                                                }
                                            }
                                            let body = json!({
                                                "user_id": user_id(),
                                                "username": username_claim(),
                                            });
                                            match client.post_json_stream("username/claim", &body).await {
                                                Ok(resp) if resp.status.is_success() => {
                                                    username_status_value.set("Username claimed".to_string());
                                                    username_ready.set(true);
                                                }
                                                Ok(resp) => {
                                                    let text = resp.collect_string().await.unwrap_or_default();
                                                    username_lookup_error_value.set(text);
                                                }
                                                Err(err) => username_lookup_error_value.set(format!("{err}")),
                                            }
                                        });
                                    },
                                    "Claim"
                                }
                            }
                            if !username_status.read().is_empty() {
                                div { class: "status", "{username_status}" }
                            }
                            div { class: "tool-item",
                                span { "Lookup username" }
                                input {
                                    value: "{username_lookup}",
                                    placeholder: "friendname",
                                    oninput: move |evt| {
                                        let mut username_lookup_input = username_lookup_input.clone();
                                        username_lookup_input.set(evt.value());
                                    },
                                }
                                button {
                                    onclick: move |_| {
                                        let daemon_url = daemon_url.clone();
                                        let token = token.clone();
                                        let username_lookup = username_lookup.clone();
                                        let mut username_lookup_error_value = username_lookup_error_value.clone();
                                        let mut peer_id_input = peer_id_input.clone();
                                        let mut contact_label_input = contact_label_input.clone();
                                        let mut contact_onion_input = contact_onion_input.clone();
                                        let mut e2e_peer_public_key_input = e2e_peer_public_key_input.clone();
                                        let mut chats_value = chats_value.clone();
                                        spawn(async move {
                                            username_lookup_error_value.set(String::new());
                                            let client = match DaemonClient::new(
                                                daemon_url().to_string(),
                                                token().to_string(),
                                            )
                                            .await
                                            {
                                                Ok(client) => client,
                                                Err(err) => {
                                                    username_lookup_error_value.set(format!("{err}"));
                                                    return;
                                                }
                                            };
                                            let response = client
                                                .get_stream(
                                                    "username/lookup",
                                                    &[("username", username_lookup())],
                                                )
                                                .await;
                                            match response {
                                                Ok(resp) if resp.status.is_success() => {
                                                    if let Ok(text) = resp.collect_string().await {
                                                        if let Ok(value) = serde_json::from_str::<Value>(&text) {
                                                            let peer = value
                                                                .get("peer_id")
                                                                .and_then(|v| v.as_str())
                                                                .unwrap_or_default();
                                                            let public_key = value
                                                                .get("public_key")
                                                                .and_then(|v| v.as_str())
                                                                .unwrap_or_default();
                                                            let p2p_addr = value
                                                                .get("p2p_addr")
                                                                .and_then(|v| v.as_str())
                                                                .unwrap_or_default();
                                                            peer_id_input.set(peer.to_string());
                                                            contact_label_input.set(username_lookup());
                                                            contact_onion_input.set(p2p_addr.to_string());
                                                            e2e_peer_public_key_input
                                                                .set(public_key.to_string());

                                                            let mut list = chats_value.write();
                                                            if let Some(idx) =
                                                                list.iter().position(|chat| chat.id == peer)
                                                            {
                                                                let mut chat = list.remove(idx);
                                                                chat.title = username_lookup();
                                                                chat.peer_id = peer.to_string();
                                                                chat.onion_address = p2p_addr.to_string();
                                                                chat.public_key = Some(public_key.to_string());
                                                                chat.trust_state = "unverified".to_string();
                                                                list.insert(0, chat);
                                                            } else {
                                                                list.insert(0, ChatSummary {
                                                                    id: peer.to_string(),
                                                                    title: username_lookup(),
                                                                    status: "trust: unverified".to_string(),
                                                                    last_message: "No messages yet".to_string(),
                                                                    last_time: "".to_string(),
                                                                    unread_count: 0,
                                                                    peer_id: peer.to_string(),
                                                                    onion_address: p2p_addr.to_string(),
                                                                    trust_state: "unverified".to_string(),
                                                                    public_key: Some(public_key.to_string()),
                                                                });
                                                            }
                                                        }
                                                    }
                                                }
                                                Ok(resp) => {
                                                    let text = resp.collect_string().await.unwrap_or_default();
                                                    username_lookup_error_value.set(text);
                                                }
                                                Err(err) => {
                                                    username_lookup_error_value.set(format!("{err}"));
                                                }
                                            }
                                        });
                                    },
                                    "Lookup"
                                }
                            }
                            if !username_lookup_error.read().is_empty() {
                                div { class: "error", "{username_lookup_error}" }
                            }
                        }
                        div { class: "settings-card",
                            label { "Contacts" }
                            div { class: "tool-item",
                                span { "Label" }
                                input {
                                    value: "{contact_label}",
                                    oninput: move |evt| {
                                        let mut contact_label_input = contact_label_input.clone();
                                        contact_label_input.set(evt.value());
                                    },
                                }
                            }
                            div { class: "tool-item",
                                span { "Peer ID" }
                                input {
                                    value: "{peer_id}",
                                    oninput: move |evt| {
                                        let mut peer_id_input = peer_id_input.clone();
                                        peer_id_input.set(evt.value());
                                    },
                                }
                            }
                            div { class: "tool-item",
                                span { "P2P address (multiaddr)" }
                                input {
                                    value: "{contact_onion}",
                                    placeholder: "/ip4/1.2.3.4/tcp/9000/p2p/12D3KooW...",
                                    oninput: move |evt| {
                                        let mut contact_onion_input = contact_onion_input.clone();
                                        contact_onion_input.set(evt.value());
                                    },
                                }
                            }
                            div { class: "tool-item",
                                button {
                                    onclick: move |_| {
                                        let daemon_url = daemon_url.clone();
                                        let token = token.clone();
                                        let user_id = user_id.clone();
                                        let peer_id = peer_id.clone();
                                        let contact_label = contact_label.clone();
                                        let contact_onion = contact_onion.clone();
                                        let mut contacts_error_value = contacts_error_value.clone();
                                        let mut chats_value = chats_value.clone();
                                        spawn(async move {
                                            contacts_error_value.set(String::new());
                                            if peer_id().trim().is_empty()
                                                || contact_label().trim().is_empty()
                                            {
                                                contacts_error_value.set(
                                                    "Label and peer ID are required.".to_string(),
                                                );
                                                return;
                                            }
                                            let client = match DaemonClient::new(
                                                daemon_url().to_string(),
                                                token().to_string(),
                                            )
                                            .await
                                            {
                                                Ok(client) => client,
                                                Err(err) => {
                                                    contacts_error_value.set(format!("{err}"));
                                                    return;
                                                }
                                            };
                                            let body = json!({
                                                "user_id": user_id(),
                                                "peer_id": peer_id(),
                                                "label": contact_label(),
                                                "onion_address": contact_onion(),
                                            });
                                            match client.post_json_stream("contacts", &body).await {
                                                Ok(resp) if resp.status.is_success() => {
                                                    let mut list = chats_value.write();
                                                    let peer_value = peer_id();
                                                    let label_value = contact_label();
                                                    let onion_value = contact_onion();
                                                    let status = if onion_value.trim().is_empty() {
                                                        "trust: unverified".to_string()
                                                    } else {
                                                        format!("trust: unverified • {onion_value}")
                                                    };
                                                    if let Some(idx) = list.iter().position(|chat| chat.id == peer_value) {
                                                        let mut chat = list.remove(idx);
                                                        chat.title = label_value.clone();
                                                        chat.status = status;
                                                        chat.onion_address = onion_value.clone();
                                                        chat.peer_id = peer_value.clone();
                                                        chat.trust_state = "unverified".to_string();
                                                        if chat.public_key.is_none() {
                                                            chat.public_key = None;
                                                        }
                                                        list.insert(0, chat);
                                                    } else {
                                                        list.insert(0, ChatSummary {
                                                            id: peer_value.clone(),
                                                            title: label_value,
                                                            status,
                                                            last_message: "No messages yet".to_string(),
                                                            last_time: "".to_string(),
                                                            unread_count: 0,
                                                            peer_id: peer_value,
                                                            onion_address: onion_value,
                                                            trust_state: "unverified".to_string(),
                                                            public_key: None,
                                                        });
                                                    }
                                                }
                                                Ok(resp) => {
                                                    let text = resp.collect_string().await.unwrap_or_default();
                                                    contacts_error_value.set(text);
                                                }
                                                Err(err) => contacts_error_value.set(format!("{err}")),
                                            }
                                        });
                                    },
                                    "Save Contact"
                                }
                            }
                            if !contacts_error.read().is_empty() {
                                div { class: "error", "{contacts_error}" }
                            }
                        }
                        div { class: "settings-card",
                            label { "E2E Trust" }
                            div { class: "tool-item",
                                span { "Peer ID" }
                                input {
                                    value: "{peer_id}",
                                    oninput: move |evt| {
                                        let mut peer_id_input = peer_id_input.clone();
                                        peer_id_input.set(evt.value());
                                    },
                                }
                            }
                            div { class: "tool-item",
                                span { "Current" }
                                span { class: "hint", "{trust_state}" }
                                button {
                                    onclick: move |_| {
                                        let daemon_url = daemon_url.clone();
                                        let token = token.clone();
                                        let user_id = user_id.clone();
                                        let peer_id = peer_id.clone();
                                        let mut trust_state_value = trust_state_value.clone();
                                        let mut trust_error_value = trust_error_value.clone();
                                        let mut chats_value = chats_value.clone();
                                        spawn(async move {
                                            trust_error_value.set(String::new());
                                            let client = match DaemonClient::new(
                                                daemon_url().to_string(),
                                                token().to_string(),
                                            )
                                            .await
                                            {
                                                Ok(client) => client,
                                                Err(err) => {
                                                    trust_error_value.set(format!("{err}"));
                                                    return;
                                                }
                                            };
                                            let response = client
                                                .get_stream(
                                                    "e2e/trust_status",
                                                    &[("user_id", user_id()), ("peer_id", peer_id())],
                                                )
                                                .await;
                                            match response {
                                                Ok(resp) if resp.status.is_success() => {
                                                    if let Ok(text) = resp.collect_string().await {
                                                        if let Ok(value) = serde_json::from_str::<Value>(&text) {
                                                            if let Some(state) = value
                                                                .get("trust_state")
                                                                .and_then(|v| v.as_str())
                                                            {
                                                                trust_state_value.set(state.to_string());
                                                                let peer_value = peer_id();
                                                                let mut list = chats_value.write();
                                                                if let Some(idx) =
                                                                    list.iter().position(|chat| chat.id == peer_value)
                                                                {
                                                                    let mut chat = list.remove(idx);
                                                                    chat.trust_state = state.to_string();
                                                                    chat.status = if chat.onion_address.is_empty() {
                                                                        format!("trust: {state}")
                                                                    } else {
                                                                        format!(
                                                                            "trust: {state} • {}",
                                                                            chat.onion_address
                                                                        )
                                                                    };
                                                                    list.insert(0, chat);
                                                                }
                                                            }
                                                        }
                                                    }
                                                }
                                                Ok(resp) => {
                                                    let text = resp
                                                        .collect_string()
                                                        .await
                                                        .unwrap_or_else(|_| "".to_string());
                                                    trust_error_value.set(format!("{text}"));
                                                }
                                                Err(err) => {
                                                    trust_error_value.set(format!("{err}"));
                                                }
                                            }
                                        });
                                    },
                                    "Refresh"
                                }
                            }
                            div { class: "tool-item",
                                button {
                                    onclick: move |_| {
                                        let daemon_url = daemon_url.clone();
                                        let token = token.clone();
                                        let user_id = user_id.clone();
                                        let peer_id = peer_id.clone();
                                        let mut trust_state_value = trust_state_value.clone();
                                        let mut trust_error_value = trust_error_value.clone();
                                        let mut chats_value = chats_value.clone();
                                        spawn(async move {
                                            trust_error_value.set(String::new());
                                            let client = match DaemonClient::new(
                                                daemon_url().to_string(),
                                                token().to_string(),
                                            )
                                            .await
                                            {
                                                Ok(client) => client,
                                                Err(err) => {
                                                    trust_error_value.set(format!("{err}"));
                                                    return;
                                                }
                                            };
                                            let body = json!({
                                                "user_id": user_id(),
                                                "peer_id": peer_id(),
                                                "trust_state": "verified",
                                            });
                                            match client.post_json_stream("e2e/trust", &body).await {
                                                Ok(resp) if resp.status.is_success() => {
                                                    trust_state_value.set("verified".to_string());
                                                    let peer_value = peer_id();
                                                    let mut list = chats_value.write();
                                                    if let Some(idx) =
                                                        list.iter().position(|chat| chat.id == peer_value)
                                                    {
                                                        let mut chat = list.remove(idx);
                                                        chat.trust_state = "verified".to_string();
                                                        chat.status = if chat.onion_address.is_empty() {
                                                            "trust: verified".to_string()
                                                        } else {
                                                            format!(
                                                                "trust: verified • {}",
                                                                chat.onion_address
                                                            )
                                                        };
                                                        list.insert(0, chat);
                                                    }
                                                }
                                                Ok(resp) => {
                                                    let text = resp.collect_string().await.unwrap_or_default();
                                                    trust_error_value.set(text);
                                                }
                                                Err(err) => trust_error_value.set(format!("{err}")),
                                            }
                                        });
                                    },
                                    "Verify"
                                }
                                button {
                                    onclick: move |_| {
                                        let daemon_url = daemon_url.clone();
                                        let token = token.clone();
                                        let user_id = user_id.clone();
                                        let peer_id = peer_id.clone();
                                        let mut trust_state_value = trust_state_value.clone();
                                        let mut trust_error_value = trust_error_value.clone();
                                        let mut chats_value = chats_value.clone();
                                        spawn(async move {
                                            trust_error_value.set(String::new());
                                            let client = match DaemonClient::new(
                                                daemon_url().to_string(),
                                                token().to_string(),
                                            )
                                            .await
                                            {
                                                Ok(client) => client,
                                                Err(err) => {
                                                    trust_error_value.set(format!("{err}"));
                                                    return;
                                                }
                                            };
                                            let body = json!({
                                                "user_id": user_id(),
                                                "peer_id": peer_id(),
                                                "trust_state": "blocked",
                                            });
                                            match client.post_json_stream("e2e/trust", &body).await {
                                                Ok(resp) if resp.status.is_success() => {
                                                    trust_state_value.set("blocked".to_string());
                                                    let peer_value = peer_id();
                                                    let mut list = chats_value.write();
                                                    if let Some(idx) =
                                                        list.iter().position(|chat| chat.id == peer_value)
                                                    {
                                                        let mut chat = list.remove(idx);
                                                        chat.trust_state = "blocked".to_string();
                                                        chat.status = if chat.onion_address.is_empty() {
                                                            "trust: blocked".to_string()
                                                        } else {
                                                            format!(
                                                                "trust: blocked • {}",
                                                                chat.onion_address
                                                            )
                                                        };
                                                        list.insert(0, chat);
                                                    }
                                                }
                                                Ok(resp) => {
                                                    let text = resp.collect_string().await.unwrap_or_default();
                                                    trust_error_value.set(text);
                                                }
                                                Err(err) => trust_error_value.set(format!("{err}")),
                                            }
                                        });
                                    },
                                    "Block"
                                }
                                button {
                                    onclick: move |_| {
                                        let daemon_url = daemon_url.clone();
                                        let token = token.clone();
                                        let user_id = user_id.clone();
                                        let peer_id = peer_id.clone();
                                        let mut trust_state_value = trust_state_value.clone();
                                        let mut trust_error_value = trust_error_value.clone();
                                        let mut chats_value = chats_value.clone();
                                        spawn(async move {
                                            trust_error_value.set(String::new());
                                            let client = match DaemonClient::new(
                                                daemon_url().to_string(),
                                                token().to_string(),
                                            )
                                            .await
                                            {
                                                Ok(client) => client,
                                                Err(err) => {
                                                    trust_error_value.set(format!("{err}"));
                                                    return;
                                                }
                                            };
                                            let body = json!({
                                                "user_id": user_id(),
                                                "peer_id": peer_id(),
                                                "trust_state": "unverified",
                                            });
                                            match client.post_json_stream("e2e/trust", &body).await {
                                                Ok(resp) if resp.status.is_success() => {
                                                    trust_state_value.set("unverified".to_string());
                                                    let peer_value = peer_id();
                                                    let mut list = chats_value.write();
                                                    if let Some(idx) =
                                                        list.iter().position(|chat| chat.id == peer_value)
                                                    {
                                                        let mut chat = list.remove(idx);
                                                        chat.trust_state = "unverified".to_string();
                                                        chat.status = if chat.onion_address.is_empty() {
                                                            "trust: unverified".to_string()
                                                        } else {
                                                            format!(
                                                                "trust: unverified • {}",
                                                                chat.onion_address
                                                            )
                                                        };
                                                        list.insert(0, chat);
                                                    }
                                                }
                                                Ok(resp) => {
                                                    let text = resp.collect_string().await.unwrap_or_default();
                                                    trust_error_value.set(text);
                                                }
                                                Err(err) => trust_error_value.set(format!("{err}")),
                                            }
                                        });
                                    },
                                    "Unverify"
                                }
                            }
                            if !trust_error.read().is_empty() {
                                div { class: "error", "{trust_error}" }
                            }
                        }
                        div { class: "settings-card",
                            label { "E2E Tools" }
                            div { class: "tool-item",
                                span { "My Public Key" }
                                input {
                                    value: "{e2e_public_key}",
                                    readonly: true,
                                }
                                button {
                                    onclick: move |_| {
                                        let daemon_url = daemon_url.clone();
                                        let token = token.clone();
                                        let user_id = user_id.clone();
                                        let mut e2e_public_key_value = e2e_public_key_value.clone();
                                        let mut e2e_error_value = e2e_error.clone();
                                        spawn(async move {
                                            e2e_error_value.set(String::new());
                                            let client = match DaemonClient::new(
                                                daemon_url().to_string(),
                                                token().to_string(),
                                            )
                                            .await
                                            {
                                                Ok(client) => client,
                                                Err(err) => {
                                                    e2e_error_value.set(format!("{err}"));
                                                    return;
                                                }
                                            };
                                            let response = client
                                                .get_stream("e2e/identity", &[("user_id", user_id())])
                                                .await;
                                            match response {
                                                Ok(resp) if resp.status.is_success() => {
                                                    if let Ok(text) = resp.collect_string().await {
                                                        if let Ok(value) = serde_json::from_str::<Value>(&text) {
                                                            if let Some(key) = value
                                                                .get("public_key")
                                                                .and_then(|v| v.as_str())
                                                            {
                                                                e2e_public_key_value.set(key.to_string());
                                                            }
                                                        }
                                                    }
                                                }
                                                Ok(resp) => {
                                                    let text = resp
                                                        .collect_string()
                                                        .await
                                                        .unwrap_or_default();
                                                    e2e_error_value.set(text);
                                                }
                                                Err(err) => e2e_error_value.set(format!("{err}")),
                                            }
                                        });
                                    },
                                    "Fetch"
                                }
                            }
                            div { class: "tool-item",
                                span { "Peer Public Key" }
                                textarea {
                                    value: "{e2e_peer_public_key}",
                                    rows: 2,
                                    oninput: move |evt| {
                                        let mut e2e_peer_public_key_input = e2e_peer_public_key_input.clone();
                                        e2e_peer_public_key_input.set(evt.value());
                                    },
                                }
                            }
                            div { class: "tool-item",
                                span { "Plaintext" }
                                textarea {
                                    value: "{e2e_plaintext}",
                                    rows: 3,
                                    oninput: move |evt| {
                                        let mut e2e_plaintext_input = e2e_plaintext_input.clone();
                                        e2e_plaintext_input.set(evt.value());
                                    },
                                }
                            }
                            div { class: "tool-item",
                                span { "Ciphertext Envelope (JSON)" }
                                textarea {
                                    value: "{e2e_ciphertext}",
                                    rows: 4,
                                    oninput: move |evt| {
                                        let mut e2e_ciphertext_input = e2e_ciphertext_input.clone();
                                        e2e_ciphertext_input.set(evt.value());
                                    },
                                }
                            }
                            div { class: "tool-item",
                                button {
                                    onclick: move |_| {
                                        let daemon_url = daemon_url.clone();
                                        let token = token.clone();
                                        let user_id = user_id.clone();
                                        let peer_id = peer_id.clone();
                                        let trust_state = trust_state.clone();
                                        let e2e_peer_public_key = e2e_peer_public_key.clone();
                                        let e2e_plaintext = e2e_plaintext.clone();
                                        let mut e2e_ciphertext_value = e2e_ciphertext.clone();
                                        let mut e2e_error_value = e2e_error.clone();
                                        spawn(async move {
                                            e2e_error_value.set(String::new());
                                            if trust_state().as_str() != "verified" {
                                                e2e_error_value.set(
                                                    "Verify the peer before encrypting.".to_string(),
                                                );
                                                return;
                                            }
                                            if e2e_peer_public_key().trim().is_empty()
                                                || e2e_plaintext().trim().is_empty()
                                            {
                                                e2e_error_value.set(
                                                    "Peer public key and plaintext are required."
                                                        .to_string(),
                                                );
                                                return;
                                            }
                                            let client = match DaemonClient::new(
                                                daemon_url().to_string(),
                                                token().to_string(),
                                            )
                                            .await
                                            {
                                                Ok(client) => client,
                                                Err(err) => {
                                                    e2e_error_value.set(format!("{err}"));
                                                    return;
                                                }
                                            };
                                            let body = json!({
                                                "user_id": user_id(),
                                                "peer_id": peer_id(),
                                                "peer_public_key": e2e_peer_public_key(),
                                                "plaintext": e2e_plaintext(),
                                            });
                                            match client.post_json_stream("e2e/encrypt", &body).await {
                                                Ok(resp) if resp.status.is_success() => {
                                                    if let Ok(text) = resp.collect_string().await {
                                                        if let Ok(value) = serde_json::from_str::<Value>(&text) {
                                                            if let Some(envelope) = value.get("envelope") {
                                                                if let Ok(pretty) = serde_json::to_string_pretty(envelope) {
                                                                    e2e_ciphertext_value.set(pretty);
                                                                }
                                                            }
                                                        }
                                                    }
                                                }
                                                Ok(resp) => {
                                                    let text = resp.collect_string().await.unwrap_or_default();
                                                    e2e_error_value.set(text);
                                                }
                                                Err(err) => e2e_error_value.set(format!("{err}")),
                                            }
                                        });
                                    },
                                    "Encrypt"
                                }
                                button {
                                    onclick: move |_| {
                                        let daemon_url = daemon_url.clone();
                                        let token = token.clone();
                                        let user_id = user_id.clone();
                                        let e2e_ciphertext = e2e_ciphertext.clone();
                                        let mut e2e_plaintext_value = e2e_plaintext.clone();
                                        let mut e2e_error_value = e2e_error.clone();
                                        spawn(async move {
                                            e2e_error_value.set(String::new());
                                            let ciphertext_value = e2e_ciphertext();
                                            let envelope_value = match serde_json::from_str::<Value>(&ciphertext_value) {
                                                Ok(value) => value,
                                                Err(_) => {
                                                    e2e_error_value.set(
                                                        "Ciphertext must be a JSON envelope.".to_string(),
                                                    );
                                                    return;
                                                }
                                            };
                                            let client = match DaemonClient::new(
                                                daemon_url().to_string(),
                                                token().to_string(),
                                            )
                                            .await
                                            {
                                                Ok(client) => client,
                                                Err(err) => {
                                                    e2e_error_value.set(format!("{err}"));
                                                    return;
                                                }
                                            };
                                            let body = json!({
                                                "user_id": user_id(),
                                                "envelope": envelope_value,
                                            });
                                            match client.post_json_stream("e2e/decrypt", &body).await {
                                                Ok(resp) if resp.status.is_success() => {
                                                    if let Ok(text) = resp.collect_string().await {
                                                        if let Ok(value) = serde_json::from_str::<Value>(&text) {
                                                            if let Some(plaintext) = value
                                                                .get("plaintext")
                                                                .and_then(|v| v.as_str())
                                                            {
                                                                e2e_plaintext_value
                                                                    .set(plaintext.to_string());
                                                            }
                                                        }
                                                    }
                                                }
                                                Ok(resp) => {
                                                    let text = resp.collect_string().await.unwrap_or_default();
                                                    e2e_error_value.set(text);
                                                }
                                                Err(err) => e2e_error_value.set(format!("{err}")),
                                            }
                                        });
                                    },
                                    "Decrypt"
                                }
                            }
                            if !e2e_error.read().is_empty() {
                                div { class: "error", "{e2e_error}" }
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
