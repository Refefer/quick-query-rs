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
        let client = Client::builder()
            .connect_timeout(Duration::from_secs(30))
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

        debug!(
            model = ?api_request.model,
            message_count = api_request.messages.len(),
            has_tools = api_request.tools.is_some(),
            "LLM request"
        );
        trace!(request = %serde_json::to_string(&api_request).unwrap_or_default(), "LLM request payload");

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
            error!(status = status.as_u16(), body = %error_text, "LLM request failed");
            return Err(self.parse_error(status.as_u16(), &error_text));
        }

        let response_text = response
            .text()
            .await
            .map_err(|e| Error::serialization(e.to_string()))?;

        trace!(response = %response_text, "LLM response payload");

        let api_response: OpenAIChatResponse = serde_json::from_str(&response_text)
            .map_err(|e| Error::serialization(e.to_string()))?;

        let parsed = self.parse_response(api_response)?;

        debug!(
            model = %parsed.model,
            finish_reason = ?parsed.finish_reason,
            content_len = parsed.message.content.to_string_lossy().len(),
            tool_calls = parsed.message.tool_calls.len(),
            prompt_tokens = parsed.usage.prompt_tokens,
            completion_tokens = parsed.usage.completion_tokens,
            "LLM response"
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
            "LLM stream request"
        );
        trace!(request = %serde_json::to_string(&api_request).unwrap_or_default(), "LLM stream request payload");

        let request_builder = self
            .client
            .post(format!("{}/chat/completions", self.base_url))
            .header("Authorization", format!("Bearer {}", self.api_key))
            .header("Content-Type", "application/json")
            .json(&api_request);

        let es = EventSource::new(request_builder).map_err(|e| Error::stream(e.to_string()))?;

        let (tx, rx) = mpsc::channel::<Result<StreamChunk, Error>>(100);

        tokio::spawn(async move {
            let mut es = es;

            while let Some(event) = es.next().await {
                match event {
                    Ok(Event::Open) => {
                        debug!("SSE connection opened");
                    }
                    Ok(Event::Message(msg)) => {
                        if msg.data == "[DONE]" {
                            debug!("SSE stream complete");
                            let _ = tx.send(Ok(StreamChunk::Done { usage: None })).await;
                            break;
                        }

                        trace!(chunk = %msg.data, "SSE chunk received");

                        match serde_json::from_str::<OpenAIStreamResponse>(&msg.data) {
                            Ok(response) => {
                                for choice in response.choices {
                                    // Handle reasoning/thinking content (o1 models)
                                    if let Some(reasoning) = choice.delta.reasoning_content {
                                        if !reasoning.is_empty()
                                            && tx
                                                .send(Ok(StreamChunk::ThinkingDelta { content: reasoning }))
                                                .await
                                                .is_err()
                                        {
                                            debug!("Stream receiver dropped, exiting");
                                            return;
                                        }
                                    }

                                    if let Some(content) = choice.delta.content {
                                        if !content.is_empty()
                                            && tx
                                                .send(Ok(StreamChunk::Delta { content }))
                                                .await
                                                .is_err()
                                        {
                                            debug!("Stream receiver dropped, exiting");
                                            return;
                                        }
                                    }

                                    if let Some(tool_calls) = choice.delta.tool_calls {
                                        for tc in tool_calls {
                                            if let Some(id) = tc.id {
                                                let name =
                                                    tc.function.as_ref().and_then(|f| f.name.clone()).unwrap_or_default();
                                                debug!(tool_id = %id, tool_name = %name, "Tool call started");
                                                if tx
                                                    .send(Ok(StreamChunk::ToolCallStart {
                                                        id,
                                                        name,
                                                    }))
                                                    .await
                                                    .is_err()
                                                {
                                                    debug!("Stream receiver dropped, exiting");
                                                    return;
                                                }
                                            }
                                            if let Some(args) = tc.function.and_then(|f| f.arguments) {
                                                if !args.is_empty()
                                                    && tx
                                                        .send(Ok(StreamChunk::ToolCallDelta {
                                                            arguments: args,
                                                        }))
                                                        .await
                                                        .is_err()
                                                {
                                                    debug!("Stream receiver dropped, exiting");
                                                    return;
                                                }
                                            }
                                        }
                                    }

                                    if let Some(ref reason) = choice.finish_reason {
                                        let usage = response.usage.as_ref().map(|u| {
                                            Usage::new(u.prompt_tokens, u.completion_tokens)
                                        });
                                        debug!(
                                            finish_reason = %reason,
                                            prompt_tokens = ?usage.as_ref().map(|u| u.prompt_tokens),
                                            completion_tokens = ?usage.as_ref().map(|u| u.completion_tokens),
                                            "LLM stream response complete"
                                        );
                                        // Final send - don't need to check error since we're done anyway
                                        let _ = tx.send(Ok(StreamChunk::Done { usage })).await;
                                    }
                                }
                            }
                            Err(e) => {
                                error!(error = %e, data = %msg.data, "Failed to parse SSE message");
                                let _ = tx
                                    .send(Err(Error::stream(format!(
                                        "Failed to parse SSE message: {}",
                                        e
                                    ))))
                                    .await;
                                break;
                            }
                        }
                    }
                    Err(e) => {
                        error!(error = ?e, "SSE error");
                        let _ = tx
                            .send(Err(Error::stream(format!("SSE error: {:?}", e))))
                            .await;
                        break;
                    }
                }
            }
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
    #[serde(default)]
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
