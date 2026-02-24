use std::time::Duration;

use async_trait::async_trait;
use futures::StreamExt;
use reqwest::Client;
use reqwest_eventsource::{Event, EventSource};
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;
use tracing::{debug, error, trace};

use qq_core::{
    CompletionRequest, CompletionResponse, Error, FinishReason, Message, Provider, Role,
    StreamChunk, StreamResult, ToolCall, ToolDefinition, Usage,
};

const DEFAULT_BASE_URL: &str = "https://api.anthropic.com/v1";
const ANTHROPIC_VERSION: &str = "2023-06-01";
const DEFAULT_MAX_TOKENS: u32 = 8192;

pub struct AnthropicProvider {
    client: Client,
    api_key: String,
    base_url: String,
    default_model: Option<String>,
    include_tool_reasoning: bool,
}

impl AnthropicProvider {
    pub fn new(api_key: impl Into<String>) -> Self {
        let client = Client::builder()
            .connect_timeout(Duration::from_secs(30))
            .build()
            .unwrap_or_else(|_| Client::new());
        Self {
            client,
            api_key: api_key.into(),
            base_url: DEFAULT_BASE_URL.to_string(),
            default_model: None,
            include_tool_reasoning: true,
        }
    }

    pub fn with_base_url(mut self, base_url: impl Into<String>) -> Self {
        self.base_url = base_url.into();
        self
    }

    pub fn with_default_model(mut self, model: impl Into<String>) -> Self {
        self.default_model = Some(model.into());
        self
    }

    pub fn with_include_tool_reasoning(mut self, include: bool) -> Self {
        self.include_tool_reasoning = include;
        self
    }

    fn build_request(&self, request: &CompletionRequest) -> AnthropicRequest {
        let model = request
            .model
            .clone()
            .or_else(|| self.default_model.clone());

        // Extract system messages into a separate field
        let mut system_parts: Vec<String> = Vec::new();
        let mut messages: Vec<AnthropicMessage> = Vec::new();

        for msg in &request.messages {
            match msg.role {
                Role::System => {
                    let text = msg.content.to_string_lossy();
                    if !text.is_empty() {
                        system_parts.push(text);
                    }
                }
                Role::User => {
                    let content = self.convert_user_content(msg);
                    messages.push(AnthropicMessage {
                        role: "user".to_string(),
                        content,
                    });
                }
                Role::Assistant => {
                    let content = self.convert_assistant_content(msg);
                    messages.push(AnthropicMessage {
                        role: "assistant".to_string(),
                        content,
                    });
                }
                Role::Tool => {
                    // Anthropic expects tool results as user messages with tool_result blocks
                    let tool_call_id = msg.tool_call_id.clone().unwrap_or_default();
                    let result_text = msg.content.to_string_lossy();
                    let block = AnthropicContentBlock::ToolResult {
                        tool_use_id: tool_call_id,
                        content: result_text,
                    };
                    messages.push(AnthropicMessage {
                        role: "user".to_string(),
                        content: vec![block],
                    });
                }
            }
        }

        // Merge adjacent same-role messages (Anthropic requires strict alternation)
        messages = merge_adjacent_messages(messages);

        let system = if system_parts.is_empty() {
            None
        } else {
            Some(system_parts.join("\n\n"))
        };

        let tools = if request.tools.is_empty() {
            None
        } else {
            Some(
                request
                    .tools
                    .iter()
                    .map(|t| self.convert_tool(t))
                    .collect(),
            )
        };

        // max_tokens is required by Anthropic
        let max_tokens = request.max_tokens.unwrap_or(DEFAULT_MAX_TOKENS);

        AnthropicRequest {
            model,
            messages,
            system,
            max_tokens,
            temperature: request.temperature,
            top_p: request.top_p,
            stream: Some(request.stream),
            tools,
        }
    }

    fn convert_user_content(&self, msg: &Message) -> Vec<AnthropicContentBlock> {
        let text = msg.content.to_string_lossy();
        if text.is_empty() {
            vec![]
        } else {
            vec![AnthropicContentBlock::Text { text }]
        }
    }

    fn convert_assistant_content(&self, msg: &Message) -> Vec<AnthropicContentBlock> {
        let mut blocks = Vec::new();

        // Emit Thinking block before text/tool_use if reasoning is present
        if let Some(ref reasoning) = msg.reasoning_content {
            if !reasoning.is_empty() {
                blocks.push(AnthropicContentBlock::Thinking {
                    thinking: reasoning.clone(),
                });
            }
        }

        let text = msg.content.to_string_lossy();
        if !text.is_empty() {
            blocks.push(AnthropicContentBlock::Text { text });
        }

        for tc in &msg.tool_calls {
            blocks.push(AnthropicContentBlock::ToolUse {
                id: tc.id.clone(),
                name: tc.name.clone(),
                input: tc.arguments.clone(),
            });
        }

        blocks
    }

    fn convert_tool(&self, tool: &ToolDefinition) -> AnthropicTool {
        AnthropicTool {
            name: tool.name.clone(),
            description: tool.description.clone(),
            input_schema: serde_json::to_value(&tool.parameters).unwrap_or_default(),
        }
    }

    fn parse_response(&self, response: AnthropicResponse) -> Result<CompletionResponse, Error> {
        let mut content_text = String::new();
        let mut tool_calls = Vec::new();
        let mut thinking = None;

        for block in &response.content {
            match block {
                AnthropicContentBlock::Text { text } => {
                    if !content_text.is_empty() {
                        content_text.push('\n');
                    }
                    content_text.push_str(text);
                }
                AnthropicContentBlock::ToolUse { id, name, input } => {
                    tool_calls.push(ToolCall::new(
                        id.clone(),
                        name.clone(),
                        input.clone(),
                    ));
                }
                AnthropicContentBlock::Thinking { thinking: text } => {
                    thinking = Some(text.clone());
                }
                _ => {} // Ignore tool_result blocks in responses
            }
        }

        // Extract thinking from content tags as fallback
        if thinking.is_none() && !content_text.is_empty() {
            let (clean, extracted) = qq_core::strip_thinking_tags(&content_text);
            if extracted.is_some() {
                debug!("Extracted thinking from content tags");
                thinking = extracted;
                content_text = clean;
            }
        }

        let message = if tool_calls.is_empty() {
            Message::assistant(content_text)
        } else {
            Message::assistant_with_tool_calls(content_text, tool_calls)
        };

        let finish_reason = match response.stop_reason.as_deref() {
            Some("end_turn") | Some("stop") => FinishReason::Stop,
            Some("max_tokens") => FinishReason::Length,
            Some("tool_use") => FinishReason::ToolCalls,
            _ => FinishReason::Stop,
        };

        let usage = Usage::new(
            response.usage.input_tokens,
            response.usage.output_tokens,
        );

        Ok(CompletionResponse {
            message,
            thinking,
            usage,
            model: response.model,
            finish_reason,
        })
    }

    fn parse_error(&self, status: u16, body: &str) -> Error {
        #[derive(Deserialize)]
        struct ErrorResponse {
            error: ErrorDetail,
        }

        #[derive(Deserialize)]
        struct ErrorDetail {
            message: String,
            #[serde(rename = "type")]
            #[allow(dead_code)]
            error_type: Option<String>,
        }

        if let Ok(err) = serde_json::from_str::<ErrorResponse>(body) {
            match status {
                401 => Error::auth(err.error.message),
                429 => Error::rate_limit(err.error.message),
                400 => Error::invalid_request(err.error.message),
                _ => Error::api(status, err.error.message),
            }
        } else {
            Error::api(status, body.to_string())
        }
    }
}

/// Merge adjacent messages with the same role (Anthropic requires strict alternation)
fn merge_adjacent_messages(messages: Vec<AnthropicMessage>) -> Vec<AnthropicMessage> {
    let mut merged: Vec<AnthropicMessage> = Vec::new();

    for msg in messages {
        if let Some(last) = merged.last_mut() {
            if last.role == msg.role {
                last.content.extend(msg.content);
                continue;
            }
        }
        merged.push(msg);
    }

    merged
}

#[async_trait]
impl Provider for AnthropicProvider {
    fn name(&self) -> &str {
        "anthropic"
    }

    fn default_model(&self) -> Option<&str> {
        self.default_model.as_deref()
    }

    fn include_tool_reasoning(&self) -> bool {
        self.include_tool_reasoning
    }

    async fn complete(&self, request: CompletionRequest) -> Result<CompletionResponse, Error> {
        let mut req = request;
        req.stream = false;

        let api_request = self.build_request(&req);

        debug!(
            model = ?api_request.model,
            message_count = api_request.messages.len(),
            has_tools = api_request.tools.is_some(),
            "Anthropic request"
        );
        trace!(request = %serde_json::to_string(&api_request).unwrap_or_default(), "Anthropic request payload");

        let response = self
            .client
            .post(format!("{}/messages", self.base_url))
            .header("x-api-key", &self.api_key)
            .header("anthropic-version", ANTHROPIC_VERSION)
            .header("Content-Type", "application/json")
            .json(&api_request)
            .send()
            .await
            .map_err(|e| Error::network(e.to_string()))?;

        let status = response.status();
        if !status.is_success() {
            let error_text = response.text().await.unwrap_or_default();
            error!(status = status.as_u16(), body = %error_text, "Anthropic request failed");
            return Err(self.parse_error(status.as_u16(), &error_text));
        }

        let response_text = response
            .text()
            .await
            .map_err(|e| Error::serialization(e.to_string()))?;

        trace!(response = %response_text, "Anthropic response payload");

        let api_response: AnthropicResponse = serde_json::from_str(&response_text)
            .map_err(|e| Error::serialization(e.to_string()))?;

        let parsed = self.parse_response(api_response)?;

        debug!(
            model = %parsed.model,
            finish_reason = ?parsed.finish_reason,
            content_len = parsed.message.content.to_string_lossy().len(),
            tool_calls = parsed.message.tool_calls.len(),
            prompt_tokens = parsed.usage.prompt_tokens,
            completion_tokens = parsed.usage.completion_tokens,
            "Anthropic response"
        );

        Ok(parsed)
    }

    async fn stream(&self, request: CompletionRequest) -> Result<StreamResult, Error> {
        let mut req = request;
        req.stream = true;

        let api_request = self.build_request(&req);

        debug!(
            model = ?api_request.model,
            message_count = api_request.messages.len(),
            has_tools = api_request.tools.is_some(),
            "Anthropic stream request"
        );
        trace!(request = %serde_json::to_string(&api_request).unwrap_or_default(), "Anthropic stream request payload");

        let request_builder = self
            .client
            .post(format!("{}/messages", self.base_url))
            .header("x-api-key", &self.api_key)
            .header("anthropic-version", ANTHROPIC_VERSION)
            .header("Content-Type", "application/json")
            .json(&api_request);

        let es = EventSource::new(request_builder).map_err(|e| Error::stream(e.to_string()))?;

        let (tx, rx) = mpsc::channel::<Result<StreamChunk, Error>>(100);

        tokio::spawn(async move {
            let mut es = es;
            let mut current_block_type: Option<String> = None;

            while let Some(event) = es.next().await {
                match event {
                    Ok(Event::Open) => {
                        debug!("Anthropic SSE connection opened");
                    }
                    Ok(Event::Message(msg)) => {
                        trace!(event_type = %msg.event, data = %msg.data, "Anthropic SSE event");

                        match msg.event.as_str() {
                            "message_start" => {
                                // Contains model info; could extract usage later
                            }
                            "content_block_start" => {
                                if let Ok(event) = serde_json::from_str::<ContentBlockStartEvent>(&msg.data) {
                                    match event.content_block.block_type.as_str() {
                                        "tool_use" => {
                                            current_block_type = Some("tool_use".to_string());
                                            let id = event.content_block.id.unwrap_or_default();
                                            let name = event.content_block.name.unwrap_or_default();
                                            debug!(tool_id = %id, tool_name = %name, "Anthropic tool call started");
                                            let _ = tx.send(Ok(StreamChunk::ToolCallStart { id, name })).await;
                                        }
                                        "thinking" => {
                                            current_block_type = Some("thinking".to_string());
                                        }
                                        _ => {
                                            current_block_type = Some("text".to_string());
                                        }
                                    }
                                }
                            }
                            "content_block_delta" => {
                                if let Ok(event) = serde_json::from_str::<ContentBlockDeltaEvent>(&msg.data) {
                                    match event.delta.delta_type.as_str() {
                                        "text_delta" => {
                                            if let Some(text) = event.delta.text {
                                                if !text.is_empty() {
                                                    let _ = tx.send(Ok(StreamChunk::Delta { content: text })).await;
                                                }
                                            }
                                        }
                                        "input_json_delta" => {
                                            if let Some(json) = event.delta.partial_json {
                                                if !json.is_empty() {
                                                    let _ = tx.send(Ok(StreamChunk::ToolCallDelta { arguments: json })).await;
                                                }
                                            }
                                        }
                                        "thinking_delta" => {
                                            if let Some(thinking) = event.delta.thinking {
                                                if !thinking.is_empty() {
                                                    let _ = tx.send(Ok(StreamChunk::ThinkingDelta { content: thinking })).await;
                                                }
                                            }
                                        }
                                        _ => {}
                                    }
                                }
                            }
                            "content_block_stop" => {
                                current_block_type = None;
                            }
                            "message_delta" => {
                                if let Ok(event) = serde_json::from_str::<MessageDeltaEvent>(&msg.data) {
                                    let usage = event.usage.map(|u| Usage::new(
                                        u.input_tokens.unwrap_or(0),
                                        u.output_tokens.unwrap_or(0),
                                    ));
                                    if let Some(ref reason) = event.delta.stop_reason {
                                        debug!(stop_reason = %reason, "Anthropic stream message_delta");
                                    }
                                    let _ = tx.send(Ok(StreamChunk::Done { usage })).await;
                                }
                            }
                            "message_stop" => {
                                debug!("Anthropic SSE stream complete");
                                break;
                            }
                            "error" => {
                                error!(data = %msg.data, "Anthropic SSE error event");
                                let _ = tx.send(Err(Error::stream(format!(
                                    "Anthropic stream error: {}",
                                    msg.data
                                )))).await;
                                break;
                            }
                            "ping" => {} // keepalive, ignore
                            _ => {
                                trace!(event_type = %msg.event, "Unknown Anthropic SSE event");
                            }
                        }
                    }
                    Err(e) => {
                        error!(error = ?e, "Anthropic SSE error");
                        let _ = tx
                            .send(Err(Error::stream(format!("Anthropic SSE error: {:?}", e))))
                            .await;
                        break;
                    }
                }
            }

            drop(current_block_type);
        });

        let stream = ReceiverStream::new(rx);
        Ok(Box::pin(stream) as StreamResult)
    }

    fn available_models(&self) -> Vec<&str> {
        vec![
            "claude-sonnet-4-20250514",
            "claude-opus-4-20250514",
            "claude-haiku-3-5-20241022",
        ]
    }
}

// ── Anthropic API types ──────────────────────────────────────────────────────

#[derive(Debug, Serialize)]
struct AnthropicRequest {
    #[serde(skip_serializing_if = "Option::is_none")]
    model: Option<String>,
    messages: Vec<AnthropicMessage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    system: Option<String>,
    max_tokens: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    top_p: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    stream: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tools: Option<Vec<AnthropicTool>>,
}

#[derive(Debug, Serialize, Deserialize)]
struct AnthropicMessage {
    role: String,
    content: Vec<AnthropicContentBlock>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum AnthropicContentBlock {
    Text {
        text: String,
    },
    ToolUse {
        id: String,
        name: String,
        input: serde_json::Value,
    },
    ToolResult {
        tool_use_id: String,
        content: String,
    },
    Thinking {
        thinking: String,
    },
}

#[derive(Debug, Serialize)]
struct AnthropicTool {
    name: String,
    description: String,
    input_schema: serde_json::Value,
}

// ── Response types ───────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
struct AnthropicResponse {
    model: String,
    content: Vec<AnthropicContentBlock>,
    stop_reason: Option<String>,
    usage: AnthropicUsage,
}

#[derive(Debug, Deserialize)]
struct AnthropicUsage {
    input_tokens: u32,
    output_tokens: u32,
}

// ── Streaming event types ────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
struct ContentBlockStartEvent {
    content_block: ContentBlockInfo,
}

#[derive(Debug, Deserialize)]
struct ContentBlockInfo {
    #[serde(rename = "type")]
    block_type: String,
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    name: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ContentBlockDeltaEvent {
    delta: DeltaContent,
}

#[derive(Debug, Deserialize)]
struct DeltaContent {
    #[serde(rename = "type")]
    delta_type: String,
    #[serde(default)]
    text: Option<String>,
    #[serde(default)]
    partial_json: Option<String>,
    #[serde(default)]
    thinking: Option<String>,
}

#[derive(Debug, Deserialize)]
struct MessageDeltaEvent {
    delta: MessageDelta,
    #[serde(default)]
    usage: Option<MessageDeltaUsage>,
}

#[derive(Debug, Deserialize)]
struct MessageDelta {
    stop_reason: Option<String>,
}

#[derive(Debug, Deserialize)]
struct MessageDeltaUsage {
    input_tokens: Option<u32>,
    output_tokens: Option<u32>,
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_provider_creation() {
        let provider = AnthropicProvider::new("test-key");
        assert_eq!(provider.name(), "anthropic");
        assert_eq!(provider.default_model(), None);
    }

    #[test]
    fn test_provider_with_custom_url() {
        let provider = AnthropicProvider::new("test-key")
            .with_base_url("https://custom.proxy.com/v1");
        assert_eq!(provider.base_url, "https://custom.proxy.com/v1");
    }

    #[test]
    fn test_provider_with_custom_model() {
        let provider = AnthropicProvider::new("test-key")
            .with_default_model("claude-sonnet-4-20250514");
        assert_eq!(provider.default_model(), Some("claude-sonnet-4-20250514"));
    }

    #[test]
    fn test_build_request_basic() {
        let provider = AnthropicProvider::new("test-key")
            .with_default_model("claude-sonnet-4-20250514");
        let request = CompletionRequest::new(vec![Message::user("Hello")]);
        let api_request = provider.build_request(&request);

        assert_eq!(api_request.model, Some("claude-sonnet-4-20250514".to_string()));
        assert_eq!(api_request.messages.len(), 1);
        assert_eq!(api_request.messages[0].role, "user");
        assert_eq!(api_request.max_tokens, DEFAULT_MAX_TOKENS);
        assert!(api_request.system.is_none());
    }

    #[test]
    fn test_build_request_system_extraction() {
        let provider = AnthropicProvider::new("test-key");
        let request = CompletionRequest::new(vec![
            Message::system("You are helpful."),
            Message::user("Hello"),
        ]);
        let api_request = provider.build_request(&request);

        assert_eq!(api_request.system, Some("You are helpful.".to_string()));
        // System message should not appear in messages array
        assert_eq!(api_request.messages.len(), 1);
        assert_eq!(api_request.messages[0].role, "user");
    }

    #[test]
    fn test_build_request_tool_conversion() {
        let provider = AnthropicProvider::new("test-key");
        let tool = ToolDefinition::new("test_tool", "A test tool");
        let request = CompletionRequest::new(vec![Message::user("Use tool")])
            .with_tools(vec![tool]);
        let api_request = provider.build_request(&request);

        assert!(api_request.tools.is_some());
        let tools = api_request.tools.unwrap();
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0].name, "test_tool");
        assert_eq!(tools[0].description, "A test tool");
    }

    #[test]
    fn test_build_request_max_tokens_override() {
        let provider = AnthropicProvider::new("test-key");
        let request = CompletionRequest::new(vec![Message::user("Hello")])
            .with_max_tokens(4096);
        let api_request = provider.build_request(&request);

        assert_eq!(api_request.max_tokens, 4096);
    }

    #[test]
    fn test_merge_adjacent_messages() {
        let messages = vec![
            AnthropicMessage {
                role: "user".to_string(),
                content: vec![AnthropicContentBlock::Text { text: "Hello".to_string() }],
            },
            AnthropicMessage {
                role: "user".to_string(),
                content: vec![AnthropicContentBlock::ToolResult {
                    tool_use_id: "tc_1".to_string(),
                    content: "result".to_string(),
                }],
            },
            AnthropicMessage {
                role: "assistant".to_string(),
                content: vec![AnthropicContentBlock::Text { text: "Hi".to_string() }],
            },
        ];

        let merged = merge_adjacent_messages(messages);
        assert_eq!(merged.len(), 2);
        assert_eq!(merged[0].role, "user");
        assert_eq!(merged[0].content.len(), 2); // Text + ToolResult merged
        assert_eq!(merged[1].role, "assistant");
    }

    #[test]
    fn test_parse_response_text() {
        let provider = AnthropicProvider::new("test-key");
        let response = AnthropicResponse {
            model: "claude-sonnet-4-20250514".to_string(),
            content: vec![AnthropicContentBlock::Text {
                text: "Hello!".to_string(),
            }],
            stop_reason: Some("end_turn".to_string()),
            usage: AnthropicUsage {
                input_tokens: 10,
                output_tokens: 5,
            },
        };

        let parsed = provider.parse_response(response).unwrap();
        assert_eq!(parsed.message.content.to_string_lossy(), "Hello!");
        assert_eq!(parsed.finish_reason, FinishReason::Stop);
        assert_eq!(parsed.usage.prompt_tokens, 10);
        assert_eq!(parsed.usage.completion_tokens, 5);
    }

    #[test]
    fn test_parse_response_tool_calls() {
        let provider = AnthropicProvider::new("test-key");
        let response = AnthropicResponse {
            model: "claude-sonnet-4-20250514".to_string(),
            content: vec![
                AnthropicContentBlock::Text {
                    text: "Let me search.".to_string(),
                },
                AnthropicContentBlock::ToolUse {
                    id: "toolu_123".to_string(),
                    name: "search".to_string(),
                    input: serde_json::json!({"query": "rust"}),
                },
            ],
            stop_reason: Some("tool_use".to_string()),
            usage: AnthropicUsage {
                input_tokens: 20,
                output_tokens: 15,
            },
        };

        let parsed = provider.parse_response(response).unwrap();
        assert_eq!(parsed.message.tool_calls.len(), 1);
        assert_eq!(parsed.message.tool_calls[0].name, "search");
        assert_eq!(parsed.finish_reason, FinishReason::ToolCalls);
    }

    #[test]
    fn test_parse_response_thinking() {
        let provider = AnthropicProvider::new("test-key");
        let response = AnthropicResponse {
            model: "claude-sonnet-4-20250514".to_string(),
            content: vec![
                AnthropicContentBlock::Thinking {
                    thinking: "Let me think about this...".to_string(),
                },
                AnthropicContentBlock::Text {
                    text: "Here's my answer.".to_string(),
                },
            ],
            stop_reason: Some("end_turn".to_string()),
            usage: AnthropicUsage {
                input_tokens: 10,
                output_tokens: 20,
            },
        };

        let parsed = provider.parse_response(response).unwrap();
        assert_eq!(parsed.thinking, Some("Let me think about this...".to_string()));
        assert_eq!(parsed.message.content.to_string_lossy(), "Here's my answer.");
    }

    #[test]
    fn test_parse_error_auth() {
        let provider = AnthropicProvider::new("test-key");
        let body = r#"{"error": {"type": "authentication_error", "message": "Invalid API key"}}"#;
        let err = provider.parse_error(401, body);
        assert!(err.is_auth_error());
    }

    #[test]
    fn test_parse_error_rate_limit() {
        let provider = AnthropicProvider::new("test-key");
        let body = r#"{"error": {"type": "rate_limit_error", "message": "Too many requests"}}"#;
        let err = provider.parse_error(429, body);
        assert!(err.is_retryable());
    }

    #[test]
    fn test_tool_result_as_user_message() {
        let provider = AnthropicProvider::new("test-key");
        let messages = vec![
            Message::user("Use a tool"),
            Message::assistant_with_tool_calls(
                "Sure",
                vec![ToolCall::new("tc_1", "search", serde_json::json!({"q": "test"}))],
            ),
            Message::tool_result("tc_1", "search result here"),
        ];
        let request = CompletionRequest::new(messages);
        let api_request = provider.build_request(&request);

        // Tool result should become a user message
        assert_eq!(api_request.messages.len(), 3);
        assert_eq!(api_request.messages[0].role, "user");
        assert_eq!(api_request.messages[1].role, "assistant");
        assert_eq!(api_request.messages[2].role, "user"); // tool result
    }

    #[test]
    fn test_available_models() {
        let provider = AnthropicProvider::new("test-key");
        let models = provider.available_models();
        assert!(models.contains(&"claude-sonnet-4-20250514"));
        assert!(models.contains(&"claude-opus-4-20250514"));
    }
}
