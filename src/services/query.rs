use std::sync::Arc;

use async_stream::try_stream;
use futures::stream::BoxStream;
use futures::StreamExt;
use regex::Regex;

use crate::error::Result;
use crate::interfaces::guardrails::InputGuardrail;
use crate::interfaces::providers::{ImageInput, MemoryProvider};
use crate::interfaces::services::RoutingService as RoutingServiceTrait;
use crate::reminders::ReminderStore;
use crate::services::agent::AgentService;
use crate::services::routing::RoutingService;

#[derive(Debug, Clone)]
pub enum UserInput {
    Text(String),
    Audio {
        bytes: Vec<u8>,
        input_format: String,
    },
}

#[derive(Debug, Clone)]
pub enum OutputFormat {
    Text,
    Audio { voice: String, format: String },
}

#[derive(Clone)]
pub struct ProcessOptions {
    pub prompt: Option<String>,
    pub images: Vec<ImageInput>,
    pub output_format: OutputFormat,
    pub image_detail: String,
    pub json_schema: Option<serde_json::Value>,
    pub router: Option<Arc<dyn RoutingServiceTrait>>,
}

#[derive(Debug, Clone)]
pub enum ProcessResult {
    Text(String),
    Audio(Vec<u8>),
    Structured(serde_json::Value),
}

pub struct QueryService {
    agent_service: Arc<AgentService>,
    routing_service: Arc<RoutingService>,
    memory_provider: Option<Arc<dyn MemoryProvider>>,
    reminder_store: Option<Arc<ReminderStore>>,
    input_guardrails: Vec<Arc<dyn InputGuardrail>>,
}

impl QueryService {
    pub fn new(
        agent_service: Arc<AgentService>,
        routing_service: Arc<RoutingService>,
        memory_provider: Option<Arc<dyn MemoryProvider>>,
        reminder_store: Option<Arc<ReminderStore>>,
        input_guardrails: Vec<Arc<dyn InputGuardrail>>,
    ) -> Self {
        Self {
            agent_service,
            routing_service,
            memory_provider,
            reminder_store,
            input_guardrails,
        }
    }

    pub async fn process_text(
        &self,
        user_id: &str,
        query: &str,
        prompt: Option<&str>,
    ) -> Result<String> {
        let mut processed_query = query.to_string();
        for guardrail in &self.input_guardrails {
            processed_query = guardrail.process(&processed_query).await?;
        }

        if let Some(response) = self
            .try_handle_search_command(user_id, &processed_query)
            .await?
        {
            if let Some(provider) = &self.memory_provider {
                provider
                    .append_message(user_id, "user", &processed_query)
                    .await?;
                provider
                    .append_message(user_id, "assistant", &response)
                    .await?;
            }
            return Ok(response);
        }

        if let Some(response) = self
            .try_handle_reminder_command(user_id, &processed_query)
            .await?
        {
            if let Some(provider) = &self.memory_provider {
                provider
                    .append_message(user_id, "user", &processed_query)
                    .await?;
                provider
                    .append_message(user_id, "assistant", &response)
                    .await?;
            }
            return Ok(response);
        }

        let agent_name = self.routing_service.route_query(&processed_query).await?;
        let reminder_context = if let Some(store) = &self.reminder_store {
            build_reminder_context(store, user_id).await
        } else {
            None
        };
        let memory_context = if let Some(provider) = &self.memory_provider {
            let include_semantic = should_include_semantic_memory(&processed_query);
            let history_future = provider.get_history(user_id, 12);
            let semantic_future = async {
                if include_semantic {
                    provider.search(user_id, &processed_query, 5).await
                } else {
                    Ok(Vec::new())
                }
            };
            let (history, semantic) = tokio::try_join!(history_future, semantic_future)?;
            let history = history.join("\n");
            build_memory_context(history, semantic, reminder_context)
        } else {
            reminder_context.unwrap_or_default()
        };

        let response = self
            .agent_service
            .generate_response(
                &agent_name,
                user_id,
                &processed_query,
                &memory_context,
                prompt,
            )
            .await?;

        if let Some(provider) = &self.memory_provider {
            provider
                .append_message(user_id, "user", &processed_query)
                .await?;
            provider
                .append_message(user_id, "assistant", &response)
                .await?;
        }

        Ok(response)
    }

    pub async fn process(
        &self,
        user_id: &str,
        input: UserInput,
        options: ProcessOptions,
    ) -> Result<ProcessResult> {
        let mut text = match input {
            UserInput::Text(value) => value,
            UserInput::Audio {
                bytes,
                input_format,
            } => {
                self.agent_service
                    .transcribe_audio(bytes, &input_format)
                    .await?
            }
        };

        for guardrail in &self.input_guardrails {
            text = guardrail.process(&text).await?;
        }

        if let Some(response) = self.try_handle_search_command(user_id, &text).await? {
            if let Some(provider) = &self.memory_provider {
                provider.append_message(user_id, "user", &text).await?;
                provider
                    .append_message(user_id, "assistant", &response)
                    .await?;
            }
            return Ok(ProcessResult::Text(response));
        }

        if let Some(response) = self.try_handle_reminder_command(user_id, &text).await? {
            if let Some(provider) = &self.memory_provider {
                provider.append_message(user_id, "user", &text).await?;
                provider
                    .append_message(user_id, "assistant", &response)
                    .await?;
            }
            return Ok(ProcessResult::Text(response));
        }

        let agent_name = if let Some(router) = &options.router {
            router.route_query(&text).await?
        } else {
            self.routing_service.route_query(&text).await?
        };
        let reminder_context = if let Some(store) = &self.reminder_store {
            build_reminder_context(store, user_id).await
        } else {
            None
        };
        let memory_context = if let Some(provider) = &self.memory_provider {
            let include_semantic = should_include_semantic_memory(&text);
            let history_future = provider.get_history(user_id, 12);
            let semantic_future = async {
                if include_semantic {
                    provider.search(user_id, &text, 5).await
                } else {
                    Ok(Vec::new())
                }
            };
            let (history, semantic) = tokio::try_join!(history_future, semantic_future)?;
            let history = history.join("\n");
            build_memory_context(history, semantic, reminder_context)
        } else {
            reminder_context.unwrap_or_default()
        };

        let result = if let Some(schema) = options.json_schema {
            let structured = self
                .agent_service
                .generate_structured_response(
                    &agent_name,
                    user_id,
                    &text,
                    &memory_context,
                    options.prompt.as_deref(),
                    schema,
                )
                .await?;
            ProcessResult::Structured(structured)
        } else if !options.images.is_empty() {
            let response = self
                .agent_service
                .generate_response_with_images(
                    &agent_name,
                    user_id,
                    &text,
                    options.images,
                    &memory_context,
                    options.prompt.as_deref(),
                    &options.image_detail,
                )
                .await?;
            ProcessResult::Text(response)
        } else {
            let response = self
                .agent_service
                .generate_response(
                    &agent_name,
                    user_id,
                    &text,
                    &memory_context,
                    options.prompt.as_deref(),
                )
                .await?;
            ProcessResult::Text(response)
        };

        let output = match (result, options.output_format) {
            (ProcessResult::Text(text), OutputFormat::Audio { voice, format }) => {
                let bytes = self
                    .agent_service
                    .synthesize_audio(&text, &voice, &format)
                    .await?;
                ProcessResult::Audio(bytes)
            }
            (other, _) => other,
        };

        if let Some(provider) = &self.memory_provider {
            provider.append_message(user_id, "user", &text).await?;
            if let ProcessResult::Text(ref message) = output {
                provider
                    .append_message(user_id, "assistant", message)
                    .await?;
            }
        }

        Ok(output)
    }

    pub fn process_text_stream<'a>(
        &'a self,
        user_id: &'a str,
        query: &'a str,
        prompt: Option<&'a str>,
    ) -> BoxStream<'a, Result<String>> {
        Box::pin(try_stream! {
            let mut processed_query = query.to_string();
            for guardrail in &self.input_guardrails {
                processed_query = guardrail.process(&processed_query).await?;
            }

            if let Some(response) = self.try_handle_search_command(user_id, &processed_query).await? {
                if let Some(provider) = &self.memory_provider {
                    provider.append_message(user_id, "user", &processed_query).await?;
                    provider.append_message(user_id, "assistant", &response).await?;
                }
                yield response;
                return;
            }

            if let Some(response) = self.try_handle_reminder_command(user_id, &processed_query).await? {
                if let Some(provider) = &self.memory_provider {
                    provider.append_message(user_id, "user", &processed_query).await?;
                    provider.append_message(user_id, "assistant", &response).await?;
                }
                yield response;
                return;
            }

            let agent_name = self.routing_service.route_query(&processed_query).await?;
            let reminder_context = if let Some(store) = &self.reminder_store {
                build_reminder_context(store, user_id).await
            } else {
                None
            };
            let memory_context = if let Some(provider) = &self.memory_provider {
                let include_semantic = should_include_semantic_memory(&processed_query);
                let history_future = provider.get_history(user_id, 12);
                let semantic_future = async {
                    if include_semantic {
                        provider.search(user_id, &processed_query, 5).await
                    } else {
                        Ok(Vec::new())
                    }
                };
                let (history, semantic) = tokio::try_join!(history_future, semantic_future)?;
                let history = history.join("\n");
                build_memory_context(history, semantic, reminder_context)
            } else {
                reminder_context.unwrap_or_default()
            };

            let mut response_text = String::new();
            let mut stream = self.agent_service.generate_response_stream(
                &agent_name,
                user_id,
                &processed_query,
                &memory_context,
                prompt,
            );

            while let Some(chunk) = stream.next().await {
                let chunk = chunk?;
                response_text.push_str(&chunk);
                yield chunk;
            }

            if let Some(provider) = &self.memory_provider {
                provider.append_message(user_id, "user", &processed_query).await?;
                if !response_text.is_empty() {
                    provider.append_message(user_id, "assistant", &response_text).await?;
                }
            }
        })
    }

    pub fn agent_service(&self) -> Arc<AgentService> {
        self.agent_service.clone()
    }

    pub async fn delete_user_history(&self, user_id: &str) -> Result<()> {
        if let Some(provider) = &self.memory_provider {
            provider.clear_history(user_id).await?;
        }
        Ok(())
    }

    pub async fn get_user_history(&self, user_id: &str, limit: usize) -> Result<Vec<String>> {
        if let Some(provider) = &self.memory_provider {
            return provider.get_history(user_id, limit).await;
        }
        Ok(Vec::new())
    }

    pub async fn search_memory(
        &self,
        user_id: &str,
        query: &str,
        limit: usize,
    ) -> Result<Vec<String>> {
        if let Some(provider) = &self.memory_provider {
            return provider.search(user_id, query, limit).await;
        }
        Ok(Vec::new())
    }
}

fn build_memory_context(
    history: String,
    semantic: Vec<String>,
    reminder_context: Option<String>,
) -> String {
    let mut context = String::new();
    if let Some(reminders) = reminder_context {
        if !reminders.is_empty() {
            context.push_str(&reminders);
            context.push_str("\n\n");
        }
    }
    if !history.is_empty() {
        context.push_str(&history);
    }
    if !semantic.is_empty() {
        if !context.is_empty() {
            context.push_str("\n\n");
        }
        context.push_str(
            "RELEVANT MEMORY (unverified; use only if clearly applicable to the user's request):\n",
        );
        for item in semantic {
            context.push_str("- ");
            context.push_str(&item);
            context.push('\n');
        }
    }
    context
}

async fn build_reminder_context(store: &ReminderStore, user_id: &str) -> Option<String> {
    let now_ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64;
    let items = store.due_reminders(user_id, now_ts, 5).await.ok()?;
    if items.is_empty() {
        return None;
    }
    let mut out = String::from("DUE REMINDERS:\n");
    for item in items {
        out.push_str(&format!(
            "- [{}] {} (due_at: {})\n",
            item.id, item.title, item.due_at
        ));
    }
    Some(out)
}

fn should_include_semantic_memory(query: &str) -> bool {
    let trimmed = query.trim();
    if trimmed.is_empty() {
        return false;
    }
    let lower = trimmed.to_lowercase();
    let tokens: Vec<&str> = lower.split_whitespace().collect();
    if tokens.len() < 3 || trimmed.len() < 12 {
        return false;
    }
    let greeting = matches!(
        tokens.as_slice(),
        ["hi"] | ["hello"] | ["hey"] | ["yo"] | ["sup"] | ["hey", "there"] | ["hi", "there"]
    );
    !greeting
}

impl QueryService {
    async fn try_handle_search_command(&self, user_id: &str, text: &str) -> Result<Option<String>> {
        let lower = text.to_lowercase();
        let looks_like_search = lower.contains("search")
            || lower.contains("latest")
            || lower.contains("current")
            || lower.contains("today")
            || lower.contains("breaking")
            || lower.contains("news")
            || lower.contains("headline")
            || lower.contains("up to date")
            || lower.contains("what's new")
            || lower.contains("whats new");
        if !looks_like_search {
            return Ok(None);
        }

        let tool = self
            .agent_service
            .tool_registry
            .get_tool("search_internet")
            .await;
        let Some(tool) = tool else {
            return Ok(None);
        };

        let query = if lower.contains("search tool") && lower.contains("error") {
            "check search tool status".to_string()
        } else {
            text.to_string()
        };

        let result = tool
            .execute(serde_json::json!({"query": query, "user_id": user_id}))
            .await?;
        let status = result.get("status").and_then(|v| v.as_str()).unwrap_or("");
        if status == "success" {
            let content = result
                .get("result")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            if content.is_empty() {
                return Ok(Some(
                    "Search completed, but no results were returned.".to_string(),
                ));
            }
            return Ok(Some(content));
        }

        let message = result
            .get("message")
            .and_then(|v| v.as_str())
            .unwrap_or("Search tool error");
        let details = result.get("details").and_then(|v| v.as_str()).unwrap_or("");
        let response = if details.is_empty() {
            format!("Search tool error: {}", message)
        } else {
            format!("Search tool error: {} ({})", message, details)
        };
        Ok(Some(response))
    }

    async fn try_handle_reminder_command(
        &self,
        user_id: &str,
        text: &str,
    ) -> Result<Option<String>> {
        let lower = text.to_lowercase();
        let clear_re = Regex::new(r"(?i)^\s*clear\s+(all\s+)?reminders\b").unwrap();
        let list_re = Regex::new(r"(?i)^\s*(list|show)\s+reminders\b").unwrap();
        let remind_re = Regex::new(
            r"(?i)\bremind me to (.+?)\s+in\s+(\d+)\s*(seconds?|minutes?|hours?|secs?|mins?|hrs?)\b",
        )
        .unwrap();
        let set_re = Regex::new(
            r"(?i)\bset a reminder\s+in\s+(\d+)\s*(seconds?|minutes?|hours?|secs?|mins?|hrs?)\s+to\s+(.+)$",
        )
        .unwrap();
        let remind_in_re = Regex::new(
            r"(?i)\bremind me in\s+(\d+)\s*(seconds?|minutes?|hours?|secs?|mins?|hrs?)\s+to\s+(.+)$",
        )
        .unwrap();

        let tool = self.agent_service.tool_registry.get_tool("reminders").await;
        let Some(tool) = tool else {
            return Ok(None);
        };

        if clear_re.is_match(&lower) {
            let include_completed = lower.contains("clear all");
            let params = serde_json::json!({
                "action": "clear",
                "user_id": user_id,
                "status": if include_completed { "all" } else { "open" }
            });
            let result = tool.execute(params).await?;
            let deleted = result.get("deleted").and_then(|v| v.as_u64()).unwrap_or(0);
            let response = if deleted == 0 {
                "No reminders to clear.".to_string()
            } else {
                format!("Cleared {} reminders.", deleted)
            };
            return Ok(Some(response));
        }

        if list_re.is_match(&lower) {
            let params = serde_json::json!({
                "action": "list",
                "user_id": user_id,
                "status": "open",
                "limit": 20
            });
            let result = tool.execute(params).await?;
            let reminders = result
                .get("reminders")
                .and_then(|v| v.as_array())
                .cloned()
                .unwrap_or_default();
            if reminders.is_empty() {
                return Ok(Some("No open reminders.".to_string()));
            }
            let mut out = String::from("Open reminders:\n");
            for item in reminders {
                let id = item.get("id").and_then(|v| v.as_i64()).unwrap_or(0);
                let title = item
                    .get("title")
                    .and_then(|v| v.as_str())
                    .unwrap_or("Reminder");
                let due = item.get("due_at").and_then(|v| v.as_i64()).unwrap_or(0);
                out.push_str(&format!("- #{}: {} (due_at: {})\n", id, title, due));
            }
            return Ok(Some(out.trim_end().to_string()));
        }

        let capture = remind_re
            .captures(text)
            .or_else(|| set_re.captures(text))
            .or_else(|| remind_in_re.captures(text));
        if let Some(caps) = capture {
            let (title, amount_idx, unit_idx) = if caps.len() >= 4 {
                // set_re or remind_in_re
                (caps.get(3).map(|m| m.as_str()), 1, 2)
            } else {
                // remind_re
                (caps.get(1).map(|m| m.as_str()), 2, 3)
            };
            let title = title.unwrap_or("reminder").trim();
            let amount: i64 = caps
                .get(amount_idx)
                .and_then(|m| m.as_str().parse::<i64>().ok())
                .unwrap_or(0);
            let unit = caps.get(unit_idx).map(|m| m.as_str()).unwrap_or("seconds");
            let multiplier = if unit.starts_with('h') {
                3600
            } else if unit.starts_with('m') {
                60
            } else {
                1
            };
            let delay_seconds = amount.max(0) * multiplier;
            if delay_seconds <= 0 {
                return Ok(None);
            }
            let params = serde_json::json!({
                "action": "create",
                "user_id": user_id,
                "title": title,
                "delay_seconds": delay_seconds
            });
            let result = tool.execute(params).await?;
            let reminder = result.get("reminder");
            let id = reminder.and_then(|v| v.get("id")).and_then(|v| v.as_i64());
            let response = match id {
                Some(id) => format!(
                    "Reminder set (#{}): {} in {} seconds.",
                    id, title, delay_seconds
                ),
                None => format!("Reminder set: {} in {} seconds.", title, delay_seconds),
            };
            return Ok(Some(response));
        }

        Ok(None)
    }
}
