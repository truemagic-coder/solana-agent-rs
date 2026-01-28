use std::sync::Arc;

use async_stream::try_stream;
use futures::stream::BoxStream;

use crate::error::Result;
use crate::interfaces::guardrails::InputGuardrail;
use crate::interfaces::providers::{ImageInput, MemoryProvider};
use crate::interfaces::services::RoutingService as RoutingServiceTrait;
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
    input_guardrails: Vec<Arc<dyn InputGuardrail>>,
}

impl QueryService {
    pub fn new(
        agent_service: Arc<AgentService>,
        routing_service: Arc<RoutingService>,
        memory_provider: Option<Arc<dyn MemoryProvider>>,
        input_guardrails: Vec<Arc<dyn InputGuardrail>>,
    ) -> Self {
        Self {
            agent_service,
            routing_service,
            memory_provider,
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

        let agent_name = self.routing_service.route_query(&processed_query).await?;
        let memory_context = if let Some(provider) = &self.memory_provider {
            provider.get_history(user_id, 20).await?.join("\n")
        } else {
            String::new()
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

        let agent_name = if let Some(router) = &options.router {
            router.route_query(&text).await?
        } else {
            self.routing_service.route_query(&text).await?
        };
        let memory_context = if let Some(provider) = &self.memory_provider {
            provider.get_history(user_id, 20).await?.join("\n")
        } else {
            String::new()
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
            let response = self.process_text(user_id, query, prompt).await?;
            yield response;
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
}
