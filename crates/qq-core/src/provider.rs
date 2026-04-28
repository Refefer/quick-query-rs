use std::pin::Pin;

use async_trait::async_trait;
use futures::Stream;
use serde::{Deserialize, Serialize};

use crate::error::Error;
use crate::message::{FinishReason, Message, StreamChunk, Usage};
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
    #[serde(skip_serializing_if = "Option::is_none")]
    pub top_k: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub min_p: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub presence_penalty: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub repetition_penalty: Option<f32>,
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
            top_k: None,
            min_p: None,
            presence_penalty: None,
            repetition_penalty: None,
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

    pub fn with_top_k(mut self, top_k: i32) -> Self {
        self.top_k = Some(top_k);
        self
    }

    pub fn with_min_p(mut self, min_p: f32) -> Self {
        self.min_p = Some(min_p);
        self
    }

    pub fn with_presence_penalty(mut self, presence_penalty: f32) -> Self {
        self.presence_penalty = Some(presence_penalty);
        self
    }

    pub fn with_repetition_penalty(mut self, repetition_penalty: f32) -> Self {
        self.repetition_penalty = Some(repetition_penalty);
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

    /// Whether to preserve reasoning/thinking content on assistant messages
    /// during multi-turn tool-call exchanges. Default: true.
    fn include_tool_reasoning(&self) -> bool {
        true
    }

    /// Context window size in tokens for this provider's active model.
    /// Returns None if unknown.
    fn context_window(&self) -> Option<u32> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_completion_request_builder() {
        // Test basic builder methods
        let request = CompletionRequest::new(vec![Message::user("Hello")])
            .with_model("gpt-4")
            .with_temperature(0.7)
            .with_max_tokens(1000);

        assert_eq!(request.model, Some("gpt-4".to_string()));
        assert_eq!(request.temperature, Some(0.7));
        assert_eq!(request.max_tokens, Some(1000));
    }

    #[test]
    fn test_completion_request_new_parameters() {
        // Test all 4 new generation parameter builder methods
        let request = CompletionRequest::new(vec![Message::user("Test")])
            .with_top_k(40)
            .with_min_p(0.05)
            .with_presence_penalty(1.0)
            .with_repetition_penalty(1.2);

        assert_eq!(request.top_k, Some(40));
        assert_eq!(request.min_p, Some(0.05));
        assert_eq!(request.presence_penalty, Some(1.0));
        assert_eq!(request.repetition_penalty, Some(1.2));
    }

    #[test]
    fn test_completion_request_builder_chaining() {
        // Test chaining in different order
        let request = CompletionRequest::new(vec![Message::user("Test")])
            .with_repetition_penalty(1.5)
            .with_model("claude-3")
            .with_min_p(0.1)
            .with_temperature(0.8)
            .with_top_k(50)
            .with_max_tokens(2048)
            .with_presence_penalty(-0.5);

        assert_eq!(request.model, Some("claude-3".to_string()));
        assert_eq!(request.temperature, Some(0.8));
        assert_eq!(request.max_tokens, Some(2048));
        assert_eq!(request.top_k, Some(50));
        assert_eq!(request.min_p, Some(0.1));
        assert_eq!(request.presence_penalty, Some(-0.5));
        assert_eq!(request.repetition_penalty, Some(1.5));
    }

    #[test]
    fn test_completion_request_default_values() {
        // Verify new params default to None for backward compatibility
        let request = CompletionRequest::new(vec![Message::user("Test")]);

        assert_eq!(request.top_k, None);
        assert_eq!(request.min_p, None);
        assert_eq!(request.presence_penalty, None);
        assert_eq!(request.repetition_penalty, None);
    }

    #[test]
    fn test_completion_request_serialization_with_params() {
        // Verify JSON serialization includes all 4 params when set
        let request = CompletionRequest::new(vec![Message::user("Test")])
            .with_top_k(30)
            .with_min_p(0.2)
            .with_presence_penalty(1.5)
            .with_repetition_penalty(0.8);

        let json = serde_json::to_string(&request).unwrap();
        assert!(json.contains("\"top_k\":30"));
        assert!(json.contains("\"min_p\":0.2"));
        assert!(json.contains("\"presence_penalty\":1.5"));
        assert!(json.contains("\"repetition_penalty\":0.8"));
    }

    #[test]
    fn test_completion_request_serialization_without_optional_params() {
        // Verify skip_serializing_if works (params omitted when None)
        let request = CompletionRequest::new(vec![Message::user("Test")])
            .with_model("test-model");

        let json = serde_json::to_string(&request).unwrap();
        assert!(json.contains("\"model\":\"test-model\""));
        assert!(!json.contains("\"top_k\""));
        assert!(!json.contains("\"min_p\""));
        assert!(!json.contains("\"presence_penalty\""));
        assert!(!json.contains("\"repetition_penalty\""));
    }

    #[test]
    fn test_completion_request_edge_cases() {
        // Test boundary values for new parameters
        let request = CompletionRequest::new(vec![Message::user("Test")])
            .with_top_k(0)        // Minimum valid value
            .with_min_p(0.0)      // Minimum probability
            .with_presence_penalty(-2.0)  // Minimum range
            .with_repetition_penalty(0.0); // Minimum range

        assert_eq!(request.top_k, Some(0));
        assert_eq!(request.min_p, Some(0.0));
        assert_eq!(request.presence_penalty, Some(-2.0));
        assert_eq!(request.repetition_penalty, Some(0.0));

        // Test maximum boundary values
        let request2 = CompletionRequest::new(vec![Message::user("Test")])
            .with_top_k(100)
            .with_min_p(1.0)
            .with_presence_penalty(2.0)
            .with_repetition_penalty(2.0);

        assert_eq!(request2.top_k, Some(100));
        assert_eq!(request2.min_p, Some(1.0));
        assert_eq!(request2.presence_penalty, Some(2.0));
        assert_eq!(request2.repetition_penalty, Some(2.0));
    }

}
