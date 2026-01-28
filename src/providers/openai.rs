use async_stream::try_stream;
use async_trait::async_trait;
use base64::{engine::general_purpose, Engine as _};
use futures::stream::BoxStream;
use serde_json::Value;

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
        InputSource,
    },
    Client,
};

use crate::error::{Result, SolanaAgentError};
use crate::interfaces::providers::{
    ChatEvent, ImageData, ImageInput, LlmProvider, LlmResponse, ToolCall,
};

#[derive(Clone)]
pub struct OpenAiProvider {
    model: String,
    client: Client<OpenAIConfig>,
}

impl OpenAiProvider {
    pub fn new(api_key: String, model: Option<String>, base_url: Option<String>) -> Self {
        let model = model.unwrap_or_else(|| "gpt-5.2".to_string());
        let base_url = base_url.unwrap_or_else(|| "https://api.openai.com/v1".to_string());
        let config = OpenAIConfig::new()
            .with_api_key(api_key)
            .with_api_base(base_url);
        Self {
            model,
            client: Client::with_config(config),
        }
    }

    fn build_system_message(system_prompt: &str) -> Result<Option<ChatCompletionRequestMessage>> {
        if system_prompt.is_empty() {
            return Ok(None);
        }
        let message = ChatCompletionRequestSystemMessageArgs::default()
            .content(system_prompt)
            .build()
            .map_err(|e| SolanaAgentError::Runtime(e.to_string()))?;
        Ok(Some(ChatCompletionRequestMessage::System(message)))
    }

    fn build_user_text_message(prompt: &str) -> Result<ChatCompletionRequestMessage> {
        let message = ChatCompletionRequestUserMessageArgs::default()
            .content(ChatCompletionRequestUserMessageContent::Text(
                prompt.to_string(),
            ))
            .build()
            .map_err(|e| SolanaAgentError::Runtime(e.to_string()))?;
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
            .map_err(|e| SolanaAgentError::Runtime(e.to_string()))?;
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
            .ok_or_else(|| SolanaAgentError::Runtime("No choices returned".to_string()))?
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

#[async_trait]
impl LlmProvider for OpenAiProvider {
    async fn generate_text(
        &self,
        prompt: &str,
        system_prompt: &str,
        tools: Option<Vec<Value>>,
    ) -> Result<String> {
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
            .map_err(|e| SolanaAgentError::Runtime(e.to_string()))?;

        let response = self
            .client
            .chat()
            .create(request)
            .await
            .map_err(|e| SolanaAgentError::Http(e.to_string()))?;

        Self::extract_text_from_response(&response)
    }

    async fn generate_with_tools(
        &self,
        prompt: &str,
        system_prompt: &str,
        tools: Vec<Value>,
    ) -> Result<LlmResponse> {
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
            .map_err(|e| SolanaAgentError::Runtime(e.to_string()))?;

        let response = self
            .client
            .chat()
            .create(request)
            .await
            .map_err(|e| SolanaAgentError::Http(e.to_string()))?;

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
        let prompt = messages
            .iter()
            .map(|m| m.to_string())
            .collect::<Vec<_>>()
            .join("\n");
        let system_prompt = "".to_string();
        let tools_clone = tools.clone();

        Box::pin(try_stream! {
            let response = provider.generate_text(&prompt, &system_prompt, tools_clone).await?;
            if !response.is_empty() {
                yield ChatEvent {
                    event_type: "content".to_string(),
                    delta: Some(response),
                    name: None,
                    arguments_delta: None,
                    finish_reason: None,
                    error: None,
                };
            }
            yield ChatEvent {
                event_type: "message_end".to_string(),
                delta: None,
                name: None,
                arguments_delta: None,
                finish_reason: Some("stop".to_string()),
                error: None,
            };
        })
    }

    async fn parse_structured_output(
        &self,
        prompt: &str,
        system_prompt: &str,
        json_schema: Value,
        tools: Option<Vec<Value>>,
    ) -> Result<Value> {
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
            .map_err(|e| SolanaAgentError::Runtime(e.to_string()))?;

        let response = self
            .client
            .chat()
            .create(request)
            .await
            .map_err(|e| SolanaAgentError::Http(e.to_string()))?;

        let content = Self::extract_text_from_response(&response)?;
        let parsed = serde_json::from_str(&content)
            .map_err(|e| SolanaAgentError::Serialization(e.to_string()))?;
        Ok(parsed)
    }

    async fn tts(&self, text: &str, voice: &str, response_format: &str) -> Result<Vec<u8>> {
        let request = CreateSpeechRequestArgs::default()
            .model(SpeechModel::Tts1)
            .input(text)
            .voice(Self::voice_from_str(voice))
            .response_format(Self::speech_format_from_str(response_format))
            .build()
            .map_err(|e| SolanaAgentError::Runtime(e.to_string()))?;

        let response = self
            .client
            .audio()
            .speech()
            .create(request)
            .await
            .map_err(|e| SolanaAgentError::Http(e.to_string()))?;

        Ok(response.bytes.to_vec())
    }

    async fn transcribe_audio(&self, audio_bytes: Vec<u8>, input_format: &str) -> Result<String> {
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
            .map_err(|e| SolanaAgentError::Runtime(e.to_string()))?;

        let response = self
            .client
            .audio()
            .transcription()
            .create(request)
            .await
            .map_err(|e| SolanaAgentError::Http(e.to_string()))?;

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
            .map_err(|e| SolanaAgentError::Runtime(e.to_string()))?;

        let response = self
            .client
            .chat()
            .create(request)
            .await
            .map_err(|e| SolanaAgentError::Http(e.to_string()))?;

        Self::extract_text_from_response(&response)
    }
}
