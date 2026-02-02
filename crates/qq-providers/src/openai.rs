use async_trait::async_trait;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::io::Write;
use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;
use tracing::{debug, error};

/// Log diagnostic message to file for streaming debug
fn diag_log(msg: &str) {
    let path = std::env::var("QQ_DIAG_LOG").unwrap_or_else(|_| "/tmp/qq-stream-diag.log".to_string());
    if let Ok(mut file) = std::fs::OpenOptions::new().create(true).append(true).open(&path) {
        let _ = writeln!(file, "{}", msg);
    }
}

use qq_core::{
    CompletionRequest, CompletionResponse, Content, Error, FinishReason, Message, Provider, Role,
    StreamChunk, StreamResult, ToolCall, ToolDefinition, Usage,
};

const DEFAULT_BASE_URL: &str = "https://api.openai.com/v1";

pub struct OpenAIProvider {
    client: Client,
    api_key: String,
    base_url: String,
    default_model: Option<String>,
}

impl OpenAIProvider {
    pub fn new(api_key: impl Into<String>) -> Self {
        // Configure client for proper SSE streaming:
        // - Use HTTP/1.1 to avoid HTTP/2 framing issues
        // - Disable automatic decompression which can buffer entire response
        let client = Client::builder()
            .http1_only()
            .no_gzip()
            .no_brotli()
            .no_deflate()
            .build()
            .unwrap_or_else(|_| Client::new());

        Self {
            client,
            api_key: api_key.into(),
            base_url: DEFAULT_BASE_URL.to_string(),
            default_model: None,
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

    fn build_request(&self, request: &CompletionRequest) -> OpenAIChatRequest {
        // Model priority: request > provider default
        // If neither is set, don't send model field (let API use its default)
        let model = request
            .model
            .clone()
            .or_else(|| self.default_model.clone());

        let messages: Vec<OpenAIMessage> = request
            .messages
            .iter()
            .map(|m| self.convert_message(m))
            .collect();

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

        OpenAIChatRequest {
            model,
            messages,
            temperature: request.temperature,
            max_tokens: request.max_tokens,
            top_p: request.top_p,
            stream: Some(request.stream),
            tools,
            stream_options: if request.stream {
                Some(StreamOptions {
                    include_usage: true,
                })
            } else {
                None
            },
            extra: request.extra.clone(),
        }
    }

    fn convert_message(&self, message: &Message) -> OpenAIMessage {
        let role = match message.role {
            Role::System => "system",
            Role::User => "user",
            Role::Assistant => "assistant",
            Role::Tool => "tool",
        };

        let content = match &message.content {
            Content::Text(s) => Some(s.clone()),
            Content::Parts(parts) => {
                let text: String = parts
                    .iter()
                    .filter_map(|p| match p {
                        qq_core::ContentPart::Text { text } => Some(text.as_str()),
                        _ => None,
                    })
                    .collect::<Vec<_>>()
                    .join("");
                if text.is_empty() {
                    None
                } else {
                    Some(text)
                }
            }
        };

        let tool_calls = if message.tool_calls.is_empty() {
            None
        } else {
            Some(
                message
                    .tool_calls
                    .iter()
                    .map(|tc| OpenAIToolCall {
                        id: tc.id.clone(),
                        r#type: "function".to_string(),
                        function: OpenAIFunctionCall {
                            name: tc.name.clone(),
                            arguments: tc.arguments.to_string(),
                        },
                    })
                    .collect(),
            )
        };

        OpenAIMessage {
            role: role.to_string(),
            content,
            reasoning_content: None, // We never send reasoning content, only receive it
            name: message.name.clone(),
            tool_calls,
            tool_call_id: message.tool_call_id.clone(),
        }
    }

    fn convert_tool(&self, tool: &ToolDefinition) -> OpenAITool {
        OpenAITool {
            r#type: "function".to_string(),
            function: OpenAIFunction {
                name: tool.name.clone(),
                description: tool.description.clone(),
                parameters: serde_json::to_value(&tool.parameters).unwrap_or_default(),
            },
        }
    }

    fn parse_response(&self, response: OpenAIChatResponse) -> Result<CompletionResponse, Error> {
        let choice = response
            .choices
            .into_iter()
            .next()
            .ok_or_else(|| Error::api(500, "No choices in response"))?;

        let tool_calls: Vec<ToolCall> = choice
            .message
            .tool_calls
            .unwrap_or_default()
            .into_iter()
            .map(|tc| {
                ToolCall::new(
                    tc.id,
                    tc.function.name,
                    serde_json::from_str(&tc.function.arguments).unwrap_or_default(),
                )
            })
            .collect();

        // Extract thinking/reasoning content (for display only, never stored in message)
        let mut thinking = choice.message.reasoning_content.clone();
        if let Some(ref t) = thinking {
            debug!("Extracted {} chars of reasoning_content from response", t.len());
        }

        // Get the actual content
        let mut content = choice.message.content.unwrap_or_default();

        // Some providers embed thinking in content with tags - extract it
        if thinking.is_none() && !content.is_empty() {
            let (clean, extracted) = qq_core::strip_thinking_tags(&content);
            if extracted.is_some() {
                debug!("Extracted thinking from content tags");
                thinking = extracted;
                content = clean;
            }
        }

        let message = if tool_calls.is_empty() {
            Message::assistant(content)
        } else {
            Message::assistant_with_tool_calls(content, tool_calls)
        };

        let finish_reason = match choice.finish_reason.as_deref() {
            Some("stop") => FinishReason::Stop,
            Some("length") => FinishReason::Length,
            Some("tool_calls") => FinishReason::ToolCalls,
            Some("content_filter") => FinishReason::ContentFilter,
            _ => FinishReason::Stop,
        };

        let usage = response.usage.map(|u| Usage::new(u.prompt_tokens, u.completion_tokens));

        Ok(CompletionResponse {
            message,
            thinking,
            usage: usage.unwrap_or_default(),
            model: response.model,
            finish_reason,
        })
    }
}

#[async_trait]
impl Provider for OpenAIProvider {
    fn name(&self) -> &str {
        "openai"
    }

    fn default_model(&self) -> Option<&str> {
        self.default_model.as_deref()
    }

    async fn complete(&self, request: CompletionRequest) -> Result<CompletionResponse, Error> {
        let mut req = request;
        req.stream = false;

        let api_request = self.build_request(&req);
        debug!("OpenAI request: {:?}", api_request);

        let response = self
            .client
            .post(format!("{}/chat/completions", self.base_url))
            .header("Authorization", format!("Bearer {}", self.api_key))
            .header("Content-Type", "application/json")
            .json(&api_request)
            .send()
            .await
            .map_err(|e| Error::network(e.to_string()))?;

        let status = response.status();
        if !status.is_success() {
            let error_text = response.text().await.unwrap_or_default();
            return Err(self.parse_error(status.as_u16(), &error_text));
        }

        let api_response: OpenAIChatResponse = response
            .json()
            .await
            .map_err(|e| Error::serialization(e.to_string()))?;

        self.parse_response(api_response)
    }

    async fn stream(&self, request: CompletionRequest) -> Result<StreamResult, Error> {
        let mut req = request;
        req.stream = true;

        let api_request = self.build_request(&req);
        debug!("OpenAI stream request: {:?}", api_request);

        // Make the request directly with reqwest for true streaming
        // Disable compression and request SSE content type to prevent buffering
        let response = self
            .client
            .post(format!("{}/chat/completions", self.base_url))
            .header("Authorization", format!("Bearer {}", self.api_key))
            .header("Content-Type", "application/json")
            .header("Accept", "text/event-stream")
            .header("Accept-Encoding", "identity") // Disable compression
            .header("Cache-Control", "no-cache")
            .json(&api_request)
            .send()
            .await
            .map_err(|e| Error::network(e.to_string()))?;

        let status = response.status();
        if !status.is_success() {
            let error_text = response.text().await.unwrap_or_default();
            return Err(self.parse_error(status.as_u16(), &error_text));
        }

        // Log response headers for debugging
        diag_log(&format!("[SSE] Response status: {}", status));
        diag_log(&format!("[SSE] Content-Type: {:?}", response.headers().get("content-type")));
        diag_log(&format!("[SSE] Transfer-Encoding: {:?}", response.headers().get("transfer-encoding")));
        diag_log(&format!("[SSE] Content-Length: {:?}", response.headers().get("content-length")));

        let (tx, rx) = mpsc::channel::<Result<StreamChunk, Error>>(100);

        // Use response.chunk() directly instead of bytes_stream() for more direct access
        tokio::spawn(async move {
            let stream_start = std::time::Instant::now();
            let mut message_count = 0u32;
            let mut buffer = String::new();
            let mut byte_count = 0usize;
            let mut response = response;

            diag_log(&format!("[{:?}] SSE: Starting to read chunks", stream_start.elapsed()));

            // Use chunk() which reads directly from the connection
            while let Ok(Some(chunk)) = response.chunk().await {
                let elapsed = stream_start.elapsed();
                byte_count += chunk.len();
                diag_log(&format!("[{:?}] SSE: Received {} bytes (total: {})", elapsed, chunk.len(), byte_count));

                // Append chunk to buffer
                if let Ok(text) = std::str::from_utf8(&chunk) {
                    buffer.push_str(text);
                } else {
                    error!("Invalid UTF-8 in SSE stream");
                    continue;
                }

                // Process complete SSE events (separated by \n\n)
                while let Some(event_end) = buffer.find("\n\n") {
                    let event_data = buffer[..event_end].to_string();
                    buffer = buffer[event_end + 2..].to_string();

                    // Parse SSE event - look for "data: " lines
                    for line in event_data.lines() {
                        if let Some(data) = line.strip_prefix("data: ") {
                            message_count += 1;
                            diag_log(&format!("[{:?}] SSE: Message #{} parsed, {} bytes", elapsed, message_count, data.len()));

                            if data == "[DONE]" {
                                diag_log(&format!("[{:?}] SSE: Stream complete, {} total messages", elapsed, message_count));
                                let _ = tx.send(Ok(StreamChunk::Done { usage: None })).await;
                                return;
                            }

                            match serde_json::from_str::<OpenAIStreamResponse>(data) {
                                Ok(response) => {
                                    for choice in response.choices {
                                        // Handle reasoning/thinking content (o1 models)
                                        if let Some(reasoning) = choice.delta.reasoning_content {
                                            if !reasoning.is_empty() {
                                                let _ = tx
                                                    .send(Ok(StreamChunk::ThinkingDelta { content: reasoning }))
                                                    .await;
                                            }
                                        }

                                        if let Some(content) = choice.delta.content {
                                            if !content.is_empty() {
                                                let _ = tx
                                                    .send(Ok(StreamChunk::Delta { content }))
                                                    .await;
                                            }
                                        }

                                        if let Some(tool_calls) = choice.delta.tool_calls {
                                            for tc in tool_calls {
                                                if let Some(id) = tc.id {
                                                    let name =
                                                        tc.function.as_ref().and_then(|f| f.name.clone()).unwrap_or_default();
                                                    let _ = tx
                                                        .send(Ok(StreamChunk::ToolCallStart {
                                                            id,
                                                            name,
                                                        }))
                                                        .await;
                                                }
                                                if let Some(args) = tc.function.and_then(|f| f.arguments) {
                                                    if !args.is_empty() {
                                                        let _ = tx
                                                            .send(Ok(StreamChunk::ToolCallDelta {
                                                                arguments: args,
                                                            }))
                                                            .await;
                                                    }
                                                }
                                            }
                                        }

                                        if choice.finish_reason.is_some() {
                                            let usage = response.usage.as_ref().map(|u| {
                                                Usage::new(u.prompt_tokens, u.completion_tokens)
                                            });
                                            let _ = tx.send(Ok(StreamChunk::Done { usage })).await;
                                        }
                                    }
                                }
                                Err(e) => {
                                    error!("Failed to parse SSE message: {} - data: {}", e, data);
                                }
                            }
                        }
                    }
                }
            }

            // Stream ended
            let elapsed = stream_start.elapsed();
            diag_log(&format!("[{:?}] SSE: Byte stream ended, {} total bytes, {} messages", elapsed, byte_count, message_count));
            let _ = tx.send(Ok(StreamChunk::Done { usage: None })).await;
        });

        let stream = ReceiverStream::new(rx);
        Ok(Box::pin(stream) as StreamResult)
    }

    fn supports_tools(&self) -> bool {
        true
    }

    fn supports_vision(&self) -> bool {
        true
    }

    fn available_models(&self) -> Vec<&str> {
        vec![
            "gpt-4o",
            "gpt-4o-mini",
            "gpt-4-turbo",
            "gpt-4",
            "gpt-3.5-turbo",
            "o1",
            "o1-mini",
            "o1-preview",
        ]
    }
}

impl OpenAIProvider {
    fn parse_error(&self, status: u16, body: &str) -> Error {
        #[derive(Deserialize)]
        struct ErrorResponse {
            error: ErrorDetail,
        }

        #[derive(Deserialize)]
        #[allow(dead_code)]
        struct ErrorDetail {
            message: String,
            #[serde(rename = "type")]
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

// OpenAI API types

#[derive(Debug, Serialize)]
struct OpenAIChatRequest {
    /// Model to use. Optional for servers that have a default model.
    #[serde(skip_serializing_if = "Option::is_none")]
    model: Option<String>,
    messages: Vec<OpenAIMessage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_tokens: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    top_p: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    stream: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tools: Option<Vec<OpenAITool>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    stream_options: Option<StreamOptions>,
    /// Extra parameters (reasoning_effort, chat_template_kwargs, etc.)
    #[serde(flatten)]
    extra: std::collections::HashMap<String, serde_json::Value>,
}

#[derive(Debug, Serialize)]
struct StreamOptions {
    include_usage: bool,
}

#[derive(Debug, Serialize, Deserialize)]
struct OpenAIMessage {
    role: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    content: Option<String>,
    /// Reasoning/thinking content from o1/reasoning models (non-streaming response).
    #[serde(skip_serializing_if = "Option::is_none")]
    reasoning_content: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_calls: Option<Vec<OpenAIToolCall>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_call_id: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
struct OpenAIToolCall {
    id: String,
    r#type: String,
    function: OpenAIFunctionCall,
}

#[derive(Debug, Serialize, Deserialize)]
struct OpenAIFunctionCall {
    name: String,
    arguments: String,
}

#[derive(Debug, Serialize)]
struct OpenAITool {
    r#type: String,
    function: OpenAIFunction,
}

#[derive(Debug, Serialize)]
struct OpenAIFunction {
    name: String,
    description: String,
    parameters: serde_json::Value,
}

#[derive(Debug, Deserialize)]
struct OpenAIChatResponse {
    model: String,
    choices: Vec<OpenAIChoice>,
    usage: Option<OpenAIUsage>,
}

#[derive(Debug, Deserialize)]
struct OpenAIChoice {
    message: OpenAIMessage,
    finish_reason: Option<String>,
}

#[derive(Debug, Deserialize)]
struct OpenAIUsage {
    prompt_tokens: u32,
    completion_tokens: u32,
}

#[allow(dead_code)]
#[derive(Debug, Deserialize)]
struct OpenAIStreamResponse {
    model: String,
    choices: Vec<OpenAIStreamChoice>,
    usage: Option<OpenAIUsage>,
}

#[derive(Debug, Deserialize)]
struct OpenAIStreamChoice {
    delta: OpenAIStreamDelta,
    finish_reason: Option<String>,
}

#[derive(Debug, Deserialize)]
struct OpenAIStreamDelta {
    content: Option<String>,
    /// Reasoning/thinking content from o1/reasoning models
    reasoning_content: Option<String>,
    tool_calls: Option<Vec<OpenAIStreamToolCall>>,
}

#[allow(dead_code)]
#[derive(Debug, Deserialize)]
struct OpenAIStreamToolCall {
    #[serde(default)]
    index: usize,
    id: Option<String>,
    function: Option<OpenAIStreamFunction>,
}

#[derive(Debug, Deserialize)]
struct OpenAIStreamFunction {
    name: Option<String>,
    arguments: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_provider_creation() {
        let provider = OpenAIProvider::new("test-key");
        assert_eq!(provider.name(), "openai");
        assert_eq!(provider.default_model(), None);
    }

    #[test]
    fn test_provider_with_custom_model() {
        let provider = OpenAIProvider::new("test-key").with_default_model("gpt-4-turbo");
        assert_eq!(provider.default_model(), Some("gpt-4-turbo"));
    }

    #[test]
    fn test_build_request() {
        let provider = OpenAIProvider::new("test-key").with_default_model("test-model");
        let request = CompletionRequest::new(vec![Message::user("Hello")]);
        let api_request = provider.build_request(&request);

        assert_eq!(api_request.model, Some("test-model".to_string()));
        assert_eq!(api_request.messages.len(), 1);
        assert_eq!(api_request.messages[0].role, "user");
    }

    #[test]
    fn test_build_request_no_model() {
        let provider = OpenAIProvider::new("test-key");
        let request = CompletionRequest::new(vec![Message::user("Hello")]);
        let api_request = provider.build_request(&request);

        // No model configured - field should be None (skipped in serialization)
        assert_eq!(api_request.model, None);
    }
}
