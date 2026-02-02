use async_stream::try_stream;
use async_trait::async_trait;
use futures::stream::BoxStream;
use serde_json::Value;
use std::sync::Arc;

use candle_vllm::api::{Engine, EngineBuilder, ModelRepo};
use candle_vllm::get_dtype;
use candle_vllm::openai::requests::{
    ChatCompletionRequest, ChatMessage, EmbeddingInput, EmbeddingRequest, Messages,
};
use candle_vllm::openai::responses::EmbeddingOutput;
use candle_vllm::tools::{Tool as CandleTool, ToolChoice as CandleToolChoice};

use crate::candle_vllm::resolve_model_id;
use crate::config::CandleVllmConfig;
use crate::error::{ButterflyBotError, Result};
use crate::interfaces::providers::{ChatEvent, ImageInput, LlmProvider, LlmResponse, ToolCall};

#[derive(Clone)]
pub struct CandleVllmProvider {
    engine: Arc<Engine>,
    model: String,
}

impl CandleVllmProvider {
    pub async fn new(config: &CandleVllmConfig) -> Result<Self> {
        if let Some(token) = config.hf_token.as_ref().filter(|value| !value.trim().is_empty()) {
            std::env::set_var("HUGGINGFACE_TOKEN", token);
            std::env::set_var("HF_TOKEN", token);
        }
        let repo = resolve_model_repo(config)?;
        let mut builder = EngineBuilder::new(repo);

        if let Some(device_ids) = config.device_ids.clone() {
            if !device_ids.is_empty() {
                builder = builder.with_device_ids(device_ids);
            }
        }
        if let Some(kvcache_mem_gpu) = config.kvcache_mem_gpu {
            builder = builder.with_kvcache_mem_gpu(kvcache_mem_gpu);
        }
        if let Some(kvcache_mem_cpu) = config.kvcache_mem_cpu {
            builder = builder.with_kvcache_mem_cpu(kvcache_mem_cpu);
        }
        if let Some(temperature) = config.temperature {
            builder = builder.with_temperature(temperature);
        }
        if let Some(top_p) = config.top_p {
            builder = builder.with_top_p(top_p);
        }
        if let Some(dtype) = config.dtype.clone() {
            builder = builder.with_dtype(get_dtype(Some(dtype)));
        }
        if let Some(isq) = config.isq.clone() {
            builder = builder.with_isq(isq);
        }

        let engine = builder
            .build_async()
            .await
            .map_err(|e| ButterflyBotError::Runtime(e.to_string()))?;
        let model = resolve_model_id(config);

        Ok(Self {
            engine: Arc::new(engine),
            model,
        })
    }

    fn build_messages(&self, prompt: &str, system_prompt: &str) -> Vec<ChatMessage> {
        let mut messages = Vec::new();
        if !system_prompt.trim().is_empty() {
            messages.push(ChatMessage {
                role: "system".to_string(),
                content: Some(system_prompt.to_string()),
                tool_calls: None,
                tool_call_id: None,
            });
        }
        messages.push(ChatMessage {
            role: "user".to_string(),
            content: Some(prompt.to_string()),
            tool_calls: None,
            tool_call_id: None,
        });
        messages
    }

    fn convert_tools(tools: Vec<Value>) -> Vec<CandleTool> {
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
                    .unwrap_or("")
                    .to_string();
                let parameters = function_obj.get("parameters").cloned();

                let mut builder = CandleTool::function(name, description);
                if let Some(schema) = parameters {
                    builder = builder.parameters_schema(schema);
                }
                Some(builder.build())
            })
            .collect()
    }

    fn response_to_text(response: &candle_vllm::openai::responses::ChatCompletionResponse) -> String {
        response
            .choices
            .first()
            .and_then(|choice| choice.message.content.clone())
            .unwrap_or_default()
    }

    fn response_to_tool_calls(
        response: &candle_vllm::openai::responses::ChatCompletionResponse,
    ) -> Vec<ToolCall> {
        let mut calls = Vec::new();
        let Some(choice) = response.choices.first() else {
            return calls;
        };
        let Some(tool_calls) = &choice.message.tool_calls else {
            return calls;
        };

        for call in tool_calls {
            let args = serde_json::from_str::<Value>(&call.function.arguments)
                .unwrap_or(Value::String(call.function.arguments.clone()));
            calls.push(ToolCall {
                name: call.function.name.clone(),
                arguments: args,
            });
        }
        calls
    }
}

#[async_trait]
impl LlmProvider for CandleVllmProvider {
    async fn generate_text(
        &self,
        prompt: &str,
        system_prompt: &str,
        tools: Option<Vec<Value>>,
    ) -> Result<String> {
        let messages = self.build_messages(prompt, system_prompt);
        let mut request = ChatCompletionRequest {
            model: Some(self.model.clone()),
            messages: Messages::Chat(messages),
            ..Default::default()
        };
        if let Some(tools) = tools {
            let converted = Self::convert_tools(tools);
            if !converted.is_empty() {
                request.tools = Some(converted);
                request.tool_choice = Some(CandleToolChoice::auto());
            }
        }
        let engine = self.engine.clone();
        let response = tokio::task::spawn_blocking(move || {
            let handle = tokio::runtime::Handle::current();
            handle.block_on(engine.generate_request(request))
        })
        .await
        .map_err(|e| ButterflyBotError::Runtime(e.to_string()))?
        .map_err(|e| ButterflyBotError::Runtime(e.to_string()))?;
        Ok(Self::response_to_text(&response))
    }

    async fn generate_with_tools(
        &self,
        prompt: &str,
        system_prompt: &str,
        tools: Vec<Value>,
    ) -> Result<LlmResponse> {
        let messages = self.build_messages(prompt, system_prompt);
        let mut request = ChatCompletionRequest {
            model: Some(self.model.clone()),
            messages: Messages::Chat(messages),
            ..Default::default()
        };
        let converted = Self::convert_tools(tools);
        if !converted.is_empty() {
            request.tools = Some(converted);
            request.tool_choice = Some(CandleToolChoice::auto());
        }

        let engine = self.engine.clone();
        let response = tokio::task::spawn_blocking(move || {
            let handle = tokio::runtime::Handle::current();
            handle.block_on(engine.generate_request(request))
        })
        .await
        .map_err(|e| ButterflyBotError::Runtime(e.to_string()))?
        .map_err(|e| ButterflyBotError::Runtime(e.to_string()))?;

        let text = Self::response_to_text(&response);
        let tool_calls = Self::response_to_tool_calls(&response);
        Ok(LlmResponse { text, tool_calls })
    }

    fn chat_stream(
        &self,
        messages: Vec<Value>,
        tools: Option<Vec<Value>>,
    ) -> BoxStream<'static, Result<ChatEvent>> {
        let provider = self.clone();
        Box::pin(try_stream! {
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

            let text = provider.generate_text(&user_prompt, &system_prompt, tools).await?;
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
        })
    }

    async fn parse_structured_output(
        &self,
        prompt: &str,
        system_prompt: &str,
        _json_schema: Value,
        tools: Option<Vec<Value>>,
    ) -> Result<Value> {
        let text = self.generate_text(prompt, system_prompt, tools).await?;
        serde_json::from_str(&text)
            .map_err(|e| ButterflyBotError::Serialization(e.to_string()))
    }

    async fn tts(&self, _text: &str, _voice: &str, _response_format: &str) -> Result<Vec<u8>> {
        Err(ButterflyBotError::Runtime(
            "TTS is not supported by candle-vllm".to_string(),
        ))
    }

    async fn transcribe_audio(&self, _audio_bytes: Vec<u8>, _input_format: &str) -> Result<String> {
        Err(ButterflyBotError::Runtime(
            "Transcription is not supported by candle-vllm".to_string(),
        ))
    }

    async fn generate_text_with_images(
        &self,
        _prompt: &str,
        _images: Vec<ImageInput>,
        _system_prompt: &str,
        _detail: &str,
        _tools: Option<Vec<Value>>,
    ) -> Result<String> {
        Err(ButterflyBotError::Runtime(
            "Image input is not supported by candle-vllm".to_string(),
        ))
    }

    async fn embed(&self, inputs: Vec<String>, model: Option<&str>) -> Result<Vec<Vec<f32>>> {
        let mut outputs = Vec::new();
        let model_name = model
            .map(|value| value.to_string())
            .unwrap_or_else(|| self.model.clone());
        for input in inputs {
            let request = EmbeddingRequest {
                model: Some(model_name.clone()),
                input: EmbeddingInput::String(input),
                encoding_format: Default::default(),
                embedding_type: Default::default(),
            };
            let engine = self.engine.clone();
            let response = tokio::task::spawn_blocking(move || {
                let handle = tokio::runtime::Handle::current();
                handle.block_on(engine.embed_async(request))
            })
            .await
            .map_err(|e| ButterflyBotError::Runtime(e.to_string()))?
            .map_err(|e| ButterflyBotError::Runtime(e.to_string()))?;
            let data = response
                .data
                .first()
                .ok_or_else(|| ButterflyBotError::Runtime("No embedding data".to_string()))?;
            match &data.embedding {
                EmbeddingOutput::Vector(vec) => outputs.push(vec.clone()),
                _ => {
                    return Err(ButterflyBotError::Runtime(
                        "Embedding output was not a vector".to_string(),
                    ))
                }
            }
        }
        Ok(outputs)
    }
}

fn resolve_model_repo(config: &CandleVllmConfig) -> Result<ModelRepo> {
    if let Some(file) = config.gguf_file.as_ref().filter(|value| !value.trim().is_empty()) {
        return Ok(ModelRepo::ModelFile(vec![leak_str(file)]));
    }
    if let Some(path) = config.weight_path.as_ref().filter(|value| !value.trim().is_empty()) {
        return Ok(ModelRepo::ModelPath(leak_str(path)));
    }
    if let Some(model_id) = config.model_id.as_ref().filter(|value| !value.trim().is_empty()) {
        return Ok(ModelRepo::ModelID((leak_str(model_id), None)));
    }
    Err(ButterflyBotError::Config(
        "candle-vllm requires model_id, weight_path, or gguf_file".to_string(),
    ))
}

fn leak_str(value: &str) -> &'static str {
    Box::leak(value.to_string().into_boxed_str())
}
