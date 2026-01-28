use async_trait::async_trait;
use futures::stream::BoxStream;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::error::Result;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCall {
    pub name: String,
    pub arguments: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LlmResponse {
    pub text: String,
    pub tool_calls: Vec<ToolCall>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatEvent {
    pub event_type: String,
    pub delta: Option<String>,
    pub name: Option<String>,
    pub arguments_delta: Option<String>,
    pub finish_reason: Option<String>,
    pub error: Option<String>,
}

#[derive(Debug, Clone)]
pub struct ImageInput {
    pub data: ImageData,
}

#[derive(Debug, Clone)]
pub enum ImageData {
    Url(String),
    Bytes(Vec<u8>),
}

#[async_trait]
pub trait LlmProvider: Send + Sync {
    async fn generate_text(
        &self,
        prompt: &str,
        system_prompt: &str,
        tools: Option<Vec<Value>>,
    ) -> Result<String>;

    async fn generate_with_tools(
        &self,
        prompt: &str,
        system_prompt: &str,
        tools: Vec<Value>,
    ) -> Result<LlmResponse>;

    fn chat_stream(
        &self,
        messages: Vec<Value>,
        tools: Option<Vec<Value>>,
    ) -> BoxStream<'static, Result<ChatEvent>>;

    async fn parse_structured_output(
        &self,
        prompt: &str,
        system_prompt: &str,
        json_schema: Value,
        tools: Option<Vec<Value>>,
    ) -> Result<Value>;

    async fn tts(&self, text: &str, voice: &str, response_format: &str) -> Result<Vec<u8>>;

    async fn transcribe_audio(&self, audio_bytes: Vec<u8>, input_format: &str) -> Result<String>;

    async fn generate_text_with_images(
        &self,
        prompt: &str,
        images: Vec<ImageInput>,
        system_prompt: &str,
        detail: &str,
        tools: Option<Vec<Value>>,
    ) -> Result<String>;
}

#[async_trait]
pub trait MemoryProvider: Send + Sync {
    async fn append_message(&self, user_id: &str, role: &str, content: &str) -> Result<()>;
    async fn get_history(&self, user_id: &str, limit: usize) -> Result<Vec<String>>;
    async fn clear_history(&self, user_id: &str) -> Result<()>;

    async fn store(&self, user_id: &str, messages: Vec<Value>) -> Result<()> {
        for msg in messages {
            let role = msg.get("role").and_then(|v| v.as_str()).unwrap_or("user");
            let content = msg.get("content").and_then(|v| v.as_str()).unwrap_or("");
            self.append_message(user_id, role, content).await?;
        }
        Ok(())
    }

    async fn retrieve(&self, user_id: &str) -> Result<String> {
        Ok(self.get_history(user_id, 0).await?.join("\n"))
    }

    async fn delete(&self, user_id: &str) -> Result<()> {
        self.clear_history(user_id).await
    }

    fn find(
        &self,
        _collection: &str,
        _query: Value,
        _sort: Option<Vec<(String, i32)>>,
        _limit: Option<u64>,
        _skip: Option<u64>,
    ) -> Result<Vec<Value>> {
        Ok(Vec::new())
    }

    fn count_documents(&self, _collection: &str, _query: Value) -> Result<u64> {
        Ok(0)
    }

    async fn save_capture(
        &self,
        _user_id: &str,
        _capture_name: &str,
        _agent_name: Option<&str>,
        _data: Value,
        _schema: Option<Value>,
    ) -> Result<Option<String>> {
        Ok(None)
    }
}
