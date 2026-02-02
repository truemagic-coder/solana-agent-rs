use async_stream::try_stream;
use async_trait::async_trait;
use base64::{engine::general_purpose, Engine as _};
use futures::stream::BoxStream;
use futures::StreamExt;
use once_cell::sync::Lazy;
use serde_json::Value;
use std::sync::Arc;
use tokio::sync::Semaphore;

use async_openai::{
    config::OpenAIConfig,
    types::{
        audio::{
            AudioInput, AudioResponseFormat, CreateSpeechRequestArgs,
            CreateTranscriptionRequestArgs, SpeechModel, SpeechResponseFormat, Voice,
        },
        chat::{
            ChatCompletionMessageToolCalls, ChatCompletionRequestMessage,
            ChatCompletionRequestMessageContentPartImage,
            ChatCompletionRequestMessageContentPartText, ChatCompletionRequestSystemMessageArgs,
            ChatCompletionRequestUserMessageArgs, ChatCompletionRequestUserMessageContent,
            ChatCompletionRequestUserMessageContentPart, ChatCompletionTool, ChatCompletionTools,
            CreateChatCompletionRequestArgs, FunctionCall, FunctionObject, ImageDetail, ImageUrl,
            ResponseFormat, ResponseFormatJsonSchema,
        },
        embeddings::{CreateEmbeddingRequestArgs, EmbeddingInput},
        InputSource,
    },
    Client,
};

use crate::error::{ButterflyBotError, Result};
use crate::interfaces::providers::{
    ChatEvent, ImageData, ImageInput, LlmProvider, LlmResponse, ToolCall,
};

static LOCAL_LLM_GUARD: Lazy<Arc<Semaphore>> = Lazy::new(|| Arc::new(Semaphore::new(1)));

#[derive(Clone)]
pub struct OpenAiProvider {
    model: String,
    client: Client<OpenAIConfig>,
    is_local: bool,
}

impl OpenAiProvider {
    pub fn new(api_key: String, model: Option<String>, base_url: Option<String>) -> Self {
        let model = model.unwrap_or_else(|| "gpt-5.2".to_string());
        let base_url = base_url.unwrap_or_else(|| "https://api.openai.com/v1".to_string());
        let is_local = is_local_base_url(&base_url);
        let config = OpenAIConfig::new()
            .with_api_key(api_key)
            .with_api_base(base_url.clone());
        Self {
            model,
            client: Client::with_config(config),
            is_local,
        }
    }

    async fn local_guard(&self) -> Option<tokio::sync::OwnedSemaphorePermit> {
        if self.is_local {
            let permit = LOCAL_LLM_GUARD
                .clone()
                .acquire_owned()
                .await
                .expect("local llm guard closed");
            Some(permit)
        } else {
            None
        }
    }

    fn build_prompts_from_messages(messages: &[Value]) -> (String, String) {
        let mut system_prompt = String::new();
        let mut user_prompt = String::new();
        for message in messages {
            let role = message.get("role").and_then(|v| v.as_str()).unwrap_or("user");
            let content = message.get("content").and_then(|v| v.as_str()).unwrap_or("");
            if role == "system" {
                if !system_prompt.is_empty() {
                    system_prompt.push_str("\n\n");
                }
                system_prompt.push_str(content);
            } else {
                if !user_prompt.is_empty() {
                    user_prompt.push_str("\n\n");
                }
                user_prompt.push_str(content);
            }
        }
        (user_prompt, system_prompt)
    }

    fn build_system_message(system_prompt: &str) -> Result<Option<ChatCompletionRequestMessage>> {
        if system_prompt.is_empty() {
            return Ok(None);
        }
        let message = ChatCompletionRequestSystemMessageArgs::default()
            .content(system_prompt)
            .build()
            .map_err(|e| ButterflyBotError::Runtime(e.to_string()))?;
        Ok(Some(ChatCompletionRequestMessage::System(message)))
    }

    fn build_user_text_message(prompt: &str) -> Result<ChatCompletionRequestMessage> {
        let message = ChatCompletionRequestUserMessageArgs::default()
            .content(ChatCompletionRequestUserMessageContent::Text(
                prompt.to_string(),
            ))
            .build()
            .map_err(|e| ButterflyBotError::Runtime(e.to_string()))?;
        Ok(ChatCompletionRequestMessage::User(message))
    }

    fn build_user_image_message(
        prompt: &str,
        images: Vec<ImageInput>,
        detail: &str,
    ) -> Result<ChatCompletionRequestMessage> {
        let mut parts = Vec::new();
        parts.push(ChatCompletionRequestUserMessageContentPart::Text(
            ChatCompletionRequestMessageContentPartText {
                text: prompt.to_string(),
            },
        ));

        let detail = Self::image_detail(detail);
        for image in images {
            let image_url = match image.data {
                ImageData::Url(url) => url,
                ImageData::Bytes(bytes) => {
                    let encoded = general_purpose::STANDARD.encode(bytes);
                    format!("data:image/png;base64,{}", encoded)
                }
            };
            let image_part = ChatCompletionRequestMessageContentPartImage {
                image_url: ImageUrl {
                    url: image_url,
                    detail: Some(detail.clone()),
                },
            };
            parts.push(ChatCompletionRequestUserMessageContentPart::ImageUrl(
                image_part,
            ));
        }

        let message = ChatCompletionRequestUserMessageArgs::default()
            .content(ChatCompletionRequestUserMessageContent::Array(parts))
            .build()
            .map_err(|e| ButterflyBotError::Runtime(e.to_string()))?;
        Ok(ChatCompletionRequestMessage::User(message))
    }

    fn image_detail(detail: &str) -> ImageDetail {
        match detail.to_lowercase().as_str() {
            "low" => ImageDetail::Low,
            "high" => ImageDetail::High,
            _ => ImageDetail::Auto,
        }
    }

    fn convert_tools(tools: Vec<Value>) -> Vec<ChatCompletionTools> {
        tools
            .into_iter()
            .filter_map(|tool| {
                let tool_type = tool
                    .get("type")
                    .and_then(|v| v.as_str())
                    .unwrap_or("function");
                if tool_type != "function" {
                    return None;
                }
                let function_obj = tool.get("function").cloned().unwrap_or(tool);
                let name = function_obj.get("name")?.as_str()?.to_string();
                let description = function_obj
                    .get("description")
                    .and_then(|v| v.as_str())
                    .map(|v| v.to_string());
                let parameters = function_obj.get("parameters").cloned();
                let function = FunctionObject {
                    name,
                    description,
                    parameters,
                    strict: None,
                };
                Some(ChatCompletionTools::Function(ChatCompletionTool {
                    function,
                }))
            })
            .collect()
    }

    fn extract_text_from_response(
        response: &async_openai::types::chat::CreateChatCompletionResponse,
    ) -> Result<String> {
        let message = response
            .choices
            .first()
            .ok_or_else(|| ButterflyBotError::Runtime("No choices returned".to_string()))?
            .message
            .content
            .clone()
            .unwrap_or_default();
        Ok(message)
    }

    fn extract_tool_calls_from_response(
        response: &async_openai::types::chat::CreateChatCompletionResponse,
    ) -> Vec<ToolCall> {
        let mut calls = Vec::new();
        let Some(choice) = response.choices.first() else {
            return calls;
        };
        let message = &choice.message;
        if let Some(tool_calls) = &message.tool_calls {
            for call in tool_calls {
                match call {
                    ChatCompletionMessageToolCalls::Function(function_call) => {
                        let name = function_call.function.name.clone();
                        let args = function_call.function.arguments.clone();
                        let arguments = serde_json::from_str(&args).unwrap_or(Value::String(args));
                        calls.push(ToolCall { name, arguments });
                    }
                    ChatCompletionMessageToolCalls::Custom(custom_call) => {
                        let name = custom_call.custom_tool.name.clone();
                        let args = custom_call.custom_tool.input.clone();
                        let arguments = serde_json::from_str(&args).unwrap_or(Value::String(args));
                        calls.push(ToolCall { name, arguments });
                    }
                }
            }
        }

        if calls.is_empty() {
            #[allow(deprecated)]
            if let Some(FunctionCall { name, arguments }) = &message.function_call {
                let parsed =
                    serde_json::from_str(arguments).unwrap_or(Value::String(arguments.clone()));
                calls.push(ToolCall {
                    name: name.clone(),
                    arguments: parsed,
                });
            }
        }

        calls
    }

    fn voice_from_str(voice: &str) -> Voice {
        match voice.to_lowercase().as_str() {
            "alloy" => Voice::Alloy,
            "ash" => Voice::Ash,
            "ballad" => Voice::Ballad,
            "coral" => Voice::Coral,
            "echo" => Voice::Echo,
            "fable" => Voice::Fable,
            "onyx" => Voice::Onyx,
            "nova" => Voice::Nova,
            "sage" => Voice::Sage,
            "shimmer" => Voice::Shimmer,
            "verse" => Voice::Verse,
            other => Voice::Other(other.to_string()),
        }
    }

    fn speech_format_from_str(format: &str) -> SpeechResponseFormat {
        match format.to_lowercase().as_str() {
            "opus" => SpeechResponseFormat::Opus,
            "aac" => SpeechResponseFormat::Aac,
            "flac" => SpeechResponseFormat::Flac,
            "wav" => SpeechResponseFormat::Wav,
            "pcm" | "pcm16" => SpeechResponseFormat::Pcm,
            _ => SpeechResponseFormat::Mp3,
        }
    }
}

fn is_local_base_url(base_url: &str) -> bool {
    let trimmed = base_url.trim();
    trimmed.starts_with("http://localhost:")
        || trimmed.starts_with("http://127.0.0.1:")
        || trimmed.starts_with("http://[::1]:")
        || trimmed.starts_with("https://localhost:")
        || trimmed.starts_with("https://127.0.0.1:")
        || trimmed.starts_with("https://[::1]:")
}

#[async_trait]
impl LlmProvider for OpenAiProvider {
    async fn generate_text(
        &self,
        prompt: &str,
        system_prompt: &str,
        tools: Option<Vec<Value>>,
    ) -> Result<String> {
        let _guard = self.local_guard().await;
        let mut messages = Vec::new();
        if let Some(system) = Self::build_system_message(system_prompt)? {
            messages.push(system);
        }
        messages.push(Self::build_user_text_message(prompt)?);

        let mut builder = CreateChatCompletionRequestArgs::default();
        builder.model(self.model.clone());
        builder.messages(messages);

        if let Some(tools) = tools {
            let tools = Self::convert_tools(tools);
            if !tools.is_empty() {
                builder.tools(tools);
            }
        }

        let request = builder
            .build()
            .map_err(|e| ButterflyBotError::Runtime(e.to_string()))?;

        let response = self
            .client
            .chat()
            .create(request)
            .await
            .map_err(|e| ButterflyBotError::Http(e.to_string()))?;

        Self::extract_text_from_response(&response)
    }

    async fn embed(&self, inputs: Vec<String>, model: Option<&str>) -> Result<Vec<Vec<f32>>> {
        let _guard = self.local_guard().await;
        let model = model.unwrap_or(&self.model).to_string();
        let request = CreateEmbeddingRequestArgs::default()
            .model(model)
            .input(EmbeddingInput::StringArray(inputs))
            .build()
            .map_err(|e| ButterflyBotError::Runtime(e.to_string()))?;
        let response = self
            .client
            .embeddings()
            .create(request)
            .await
            .map_err(|e| ButterflyBotError::Runtime(e.to_string()))?;
        let mut data = response.data;
        data.sort_by_key(|item| item.index);
        Ok(data.into_iter().map(|item| item.embedding).collect())
    }
    async fn generate_with_tools(
        &self,
        prompt: &str,
        system_prompt: &str,
        tools: Vec<Value>,
    ) -> Result<LlmResponse> {
        let _guard = self.local_guard().await;
        let mut messages = Vec::new();
        if let Some(system) = Self::build_system_message(system_prompt)? {
            messages.push(system);
        }
        messages.push(Self::build_user_text_message(prompt)?);

        let tools = Self::convert_tools(tools);
        let mut builder = CreateChatCompletionRequestArgs::default();
        builder.model(self.model.clone());
        builder.messages(messages);
        if !tools.is_empty() {
            builder.tools(tools);
        }

        let request = builder
            .build()
            .map_err(|e| ButterflyBotError::Runtime(e.to_string()))?;

        let response = self
            .client
            .chat()
            .create(request)
            .await
            .map_err(|e| ButterflyBotError::Http(e.to_string()))?;

        let text = Self::extract_text_from_response(&response).unwrap_or_default();
        let tool_calls = Self::extract_tool_calls_from_response(&response);

        Ok(LlmResponse { text, tool_calls })
    }

    fn chat_stream(
        &self,
        messages: Vec<Value>,
        tools: Option<Vec<Value>>,
    ) -> BoxStream<'static, Result<ChatEvent>> {
        let provider = self.clone();

        if provider.is_local
            || std::env::var("BUTTERFLY_BOT_DISABLE_STREAM")
                .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
                .unwrap_or(false)
        {
            return Box::pin(try_stream! {
                let (prompt, system_prompt) = OpenAiProvider::build_prompts_from_messages(&messages);
                let text = provider
                    .generate_text(&prompt, &system_prompt, tools)
                    .await?;
                if !text.is_empty() {
                    yield ChatEvent {
                        event_type: "content".to_string(),
                        delta: Some(text),
                        name: None,
                        arguments_delta: None,
                        finish_reason: Some("stop".to_string()),
                        error: None,
                    };
                }
            });
        }

        Box::pin(try_stream! {
            let mut request_messages = Vec::new();
            for message in messages {
                let role = message.get("role").and_then(|v| v.as_str()).unwrap_or("user");
                let content = message.get("content").and_then(|v| v.as_str()).unwrap_or("");
                match role {
                    "system" => {
                        if let Some(msg) = OpenAiProvider::build_system_message(content)? {
                            request_messages.push(msg);
                        }
                    }
                    "user" => {
                        request_messages.push(OpenAiProvider::build_user_text_message(content)?);
                    }
                    _ => {
                        request_messages.push(OpenAiProvider::build_user_text_message(content)?);
                    }
                }
            }

            let mut builder = CreateChatCompletionRequestArgs::default();
            builder.model(provider.model.clone());
            builder.messages(request_messages);

            if let Some(tools) = tools {
                let tools = OpenAiProvider::convert_tools(tools);
                if !tools.is_empty() {
                    builder.tools(tools);
                }
            }

            let request = builder
                .build()
                .map_err(|e| ButterflyBotError::Runtime(e.to_string()))?;

            let mut stream = provider
                .client
                .chat()
                .create_stream(request)
                .await
                .map_err(|e| ButterflyBotError::Http(e.to_string()))?;

            while let Some(item) = stream.next().await {
                let response = item.map_err(|e| ButterflyBotError::Http(e.to_string()))?;
                for choice in response.choices {
                    if let Some(delta) = choice.delta.content {
                        if !delta.is_empty() {
                            yield ChatEvent {
                                event_type: "content".to_string(),
                                delta: Some(delta),
                                name: None,
                                arguments_delta: None,
                                finish_reason: None,
                                error: None,
                            };
                        }
                    }
                    if let Some(reason) = choice.finish_reason {
                        yield ChatEvent {
                            event_type: "message_end".to_string(),
                            delta: None,
                            name: None,
                            arguments_delta: None,
                            finish_reason: Some(format!("{reason:?}")),
                            error: None,
                        };
                    }
                }
            }
        })
    }

    async fn parse_structured_output(
        &self,
        prompt: &str,
        system_prompt: &str,
        json_schema: Value,
        tools: Option<Vec<Value>>,
    ) -> Result<Value> {
        let _guard = self.local_guard().await;
        let mut messages = Vec::new();
        if let Some(system) = Self::build_system_message(system_prompt)? {
            messages.push(system);
        }
        messages.push(Self::build_user_text_message(prompt)?);

        let name = json_schema
            .get("title")
            .and_then(|v| v.as_str())
            .unwrap_or("structured_output")
            .to_string();
        let response_format = ResponseFormat::JsonSchema {
            json_schema: ResponseFormatJsonSchema {
                name,
                description: None,
                schema: Some(json_schema),
                strict: Some(true),
            },
        };

        let mut builder = CreateChatCompletionRequestArgs::default();
        builder.model(self.model.clone());
        builder.messages(messages);
        builder.response_format(response_format);

        if let Some(tools) = tools {
            let tools = Self::convert_tools(tools);
            if !tools.is_empty() {
                builder.tools(tools);
            }
        }

        let request = builder
            .build()
            .map_err(|e| ButterflyBotError::Runtime(e.to_string()))?;

        let response = self
            .client
            .chat()
            .create(request)
            .await
            .map_err(|e| ButterflyBotError::Http(e.to_string()))?;

        let content = Self::extract_text_from_response(&response)?;
        let parsed = serde_json::from_str(&content)
            .map_err(|e| ButterflyBotError::Serialization(e.to_string()))?;
        Ok(parsed)
    }

    async fn tts(&self, text: &str, voice: &str, response_format: &str) -> Result<Vec<u8>> {
        let _guard = self.local_guard().await;
        let request = CreateSpeechRequestArgs::default()
            .model(SpeechModel::Tts1)
            .input(text)
            .voice(Self::voice_from_str(voice))
            .response_format(Self::speech_format_from_str(response_format))
            .build()
            .map_err(|e| ButterflyBotError::Runtime(e.to_string()))?;

        let response = self
            .client
            .audio()
            .speech()
            .create(request)
            .await
            .map_err(|e| ButterflyBotError::Http(e.to_string()))?;

        Ok(response.bytes.to_vec())
    }

    async fn transcribe_audio(&self, audio_bytes: Vec<u8>, input_format: &str) -> Result<String> {
        let _guard = self.local_guard().await;
        let file = AudioInput {
            source: InputSource::VecU8 {
                filename: format!("audio.{}", input_format),
                vec: audio_bytes,
            },
        };

        let request = CreateTranscriptionRequestArgs::default()
            .file(file)
            .model("gpt-4o-mini-transcribe")
            .response_format(AudioResponseFormat::Json)
            .build()
            .map_err(|e| ButterflyBotError::Runtime(e.to_string()))?;

        let response = self
            .client
            .audio()
            .transcription()
            .create(request)
            .await
            .map_err(|e| ButterflyBotError::Http(e.to_string()))?;

        Ok(response.text)
    }

    async fn generate_text_with_images(
        &self,
        prompt: &str,
        images: Vec<ImageInput>,
        system_prompt: &str,
        detail: &str,
        tools: Option<Vec<Value>>,
    ) -> Result<String> {
        let _guard = self.local_guard().await;
        let mut messages = Vec::new();
        if let Some(system) = Self::build_system_message(system_prompt)? {
            messages.push(system);
        }
        messages.push(Self::build_user_image_message(prompt, images, detail)?);

        let mut builder = CreateChatCompletionRequestArgs::default();
        builder.model(self.model.clone());
        builder.messages(messages);

        if let Some(tools) = tools {
            let tools = Self::convert_tools(tools);
            if !tools.is_empty() {
                builder.tools(tools);
            }
        }

        let request = builder
            .build()
            .map_err(|e| ButterflyBotError::Runtime(e.to_string()))?;

        let response = self
            .client
            .chat()
            .create(request)
            .await
            .map_err(|e| ButterflyBotError::Http(e.to_string()))?;

        Self::extract_text_from_response(&response)
    }
}
