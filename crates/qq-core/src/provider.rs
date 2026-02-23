use std::pin::Pin;

use async_trait::async_trait;
use futures::Stream;
use serde::{Deserialize, Serialize};

use crate::error::Error;
use crate::message::{Message, StreamChunk, Usage};
use crate::tool::ToolDefinition;

pub type StreamResult = Pin<Box<dyn Stream<Item = Result<StreamChunk, Error>> + Send>>;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompletionRequest {
    pub messages: Vec<Message>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub top_p: Option<f32>,
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub tools: Vec<ToolDefinition>,
    #[serde(default)]
    pub stream: bool,
    /// Extra parameters to pass through to the API (e.g., reasoning_effort, chat_template_kwargs)
    #[serde(default, skip_serializing_if = "std::collections::HashMap::is_empty")]
    pub extra: std::collections::HashMap<String, serde_json::Value>,
}

impl CompletionRequest {
    pub fn new(messages: Vec<Message>) -> Self {
        Self {
            messages,
            model: None,
            temperature: None,
            max_tokens: None,
            top_p: None,
            tools: Vec::new(),
            stream: true,
            extra: std::collections::HashMap::new(),
        }
    }

    pub fn with_model(mut self, model: impl Into<String>) -> Self {
        self.model = Some(model.into());
        self
    }

    pub fn with_temperature(mut self, temperature: f32) -> Self {
        self.temperature = Some(temperature);
        self
    }

    pub fn with_max_tokens(mut self, max_tokens: u32) -> Self {
        self.max_tokens = Some(max_tokens);
        self
    }

    pub fn with_tools(mut self, tools: Vec<ToolDefinition>) -> Self {
        self.tools = tools;
        self
    }

    pub fn with_stream(mut self, stream: bool) -> Self {
        self.stream = stream;
        self
    }

    pub fn with_extra(mut self, extra: std::collections::HashMap<String, serde_json::Value>) -> Self {
        self.extra = extra;
        self
    }

}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompletionResponse {
    /// The assistant's response message (content only, no thinking).
    pub message: Message,
    /// Extracted thinking/reasoning content (displayed but never stored in history).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thinking: Option<String>,
    pub usage: Usage,
    pub model: String,
    pub finish_reason: FinishReason,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FinishReason {
    Stop,
    Length,
    ToolCalls,
    ContentFilter,
    Error,
}

#[async_trait]
pub trait Provider: Send + Sync {
    fn name(&self) -> &str;

    /// Get the default model, if one is configured.
    /// Returns None if no default model is set (API will use its own default).
    fn default_model(&self) -> Option<&str>;

    async fn complete(&self, request: CompletionRequest) -> Result<CompletionResponse, Error>;

    async fn stream(&self, request: CompletionRequest) -> Result<StreamResult, Error>;

    fn available_models(&self) -> Vec<&str> {
        self.default_model().into_iter().collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_completion_request_builder() {
        let request = CompletionRequest::new(vec![Message::user("Hello")])
            .with_model("gpt-4")
            .with_temperature(0.7)
            .with_max_tokens(1000);

        assert_eq!(request.model, Some("gpt-4".to_string()));
        assert_eq!(request.temperature, Some(0.7));
        assert_eq!(request.max_tokens, Some(1000));
    }

}
