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

const DEFAULT_BASE_URL: &str = "https://generativelanguage.googleapis.com/v1beta";

pub struct GeminiProvider {
    client: Client,
    api_key: String,
    base_url: String,
    default_model: Option<String>,
}

impl GeminiProvider {
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

    fn resolve_model(&self, request: &CompletionRequest) -> String {
        request
            .model
            .clone()
            .or_else(|| self.default_model.clone())
            .unwrap_or_else(|| "gemini-2.5-flash".to_string())
    }

    fn build_request(&self, request: &CompletionRequest) -> GeminiRequest {
        let mut system_instruction: Option<GeminiContent> = None;
        let mut contents: Vec<GeminiContent> = Vec::new();

        for msg in &request.messages {
            match msg.role {
                Role::System => {
                    let text = msg.content.to_string_lossy();
                    if !text.is_empty() {
                        // Gemini uses system_instruction field
                        if let Some(ref mut existing) = system_instruction {
                            existing.parts.push(GeminiPart::Text { text });
                        } else {
                            system_instruction = Some(GeminiContent {
                                role: None, // system_instruction has no role
                                parts: vec![GeminiPart::Text { text }],
                            });
                        }
                    }
                }
                Role::User => {
                    let text = msg.content.to_string_lossy();
                    let mut parts = Vec::new();
                    if !text.is_empty() {
                        parts.push(GeminiPart::Text { text });
                    }
                    contents.push(GeminiContent {
                        role: Some("user".to_string()),
                        parts,
                    });
                }
                Role::Assistant => {
                    let mut parts = Vec::new();
                    let text = msg.content.to_string_lossy();
                    if !text.is_empty() {
                        parts.push(GeminiPart::Text { text });
                    }
                    for tc in &msg.tool_calls {
                        parts.push(GeminiPart::FunctionCall {
                            function_call: GeminiFunctionCall {
                                name: tc.name.clone(),
                                args: tc.arguments.clone(),
                            },
                        });
                    }
                    contents.push(GeminiContent {
                        role: Some("model".to_string()),
                        parts,
                    });
                }
                Role::Tool => {
                    // Gemini expects function responses in user-role messages
                    // We need to find the function name from the tool_call_id
                    let tool_call_id = msg.tool_call_id.clone().unwrap_or_default();
                    let result_text = msg.content.to_string_lossy();

                    // Look up the function name from previous assistant messages
                    let fn_name = find_function_name_by_id(&contents, &request.messages, &tool_call_id)
                        .unwrap_or_else(|| format!("unknown_{}", tool_call_id));

                    let response_value = serde_json::json!({ "result": result_text });

                    contents.push(GeminiContent {
                        role: Some("user".to_string()),
                        parts: vec![GeminiPart::FunctionResponse {
                            function_response: GeminiFunctionResponse {
                                name: fn_name,
                                response: response_value,
                            },
                        }],
                    });
                }
            }
        }

        // Merge adjacent same-role contents
        contents = merge_adjacent_contents(contents);

        let tools = if request.tools.is_empty() {
            None
        } else {
            let declarations: Vec<GeminiFunctionDeclaration> = request
                .tools
                .iter()
                .map(|t| self.convert_tool(t))
                .collect();
            Some(vec![GeminiToolsEntry {
                function_declarations: declarations,
            }])
        };

        let generation_config = GeminiGenerationConfig {
            temperature: request.temperature,
            top_p: request.top_p,
            max_output_tokens: request.max_tokens,
        };

        GeminiRequest {
            contents,
            system_instruction,
            tools,
            generation_config: Some(generation_config),
        }
    }

    fn convert_tool(&self, tool: &ToolDefinition) -> GeminiFunctionDeclaration {
        // Convert ToolParameters to a clean JSON schema for Gemini
        let mut parameters = serde_json::to_value(&tool.parameters).unwrap_or_default();

        // Gemini doesn't support additionalProperties — remove it
        if let Some(obj) = parameters.as_object_mut() {
            obj.remove("additionalProperties");
        }

        GeminiFunctionDeclaration {
            name: tool.name.clone(),
            description: tool.description.clone(),
            parameters,
        }
    }

    fn parse_response(
        &self,
        response: GeminiResponse,
        model: &str,
    ) -> Result<CompletionResponse, Error> {
        let candidate = response
            .candidates
            .and_then(|c| c.into_iter().next())
            .ok_or_else(|| {
                // Check if this was a safety filter
                if let Some(ref feedback) = response.prompt_feedback {
                    if let Some(ref reason) = feedback.block_reason {
                        return Error::api(400, format!("Blocked by safety filter: {}", reason));
                    }
                }
                Error::api(500, "No candidates in Gemini response")
            })?;

        let mut content_text = String::new();
        let mut tool_calls = Vec::new();
        let mut thinking = None;
        let mut tc_counter: usize = 0;

        if let Some(content) = candidate.content {
            for part in content.parts {
                match part {
                    GeminiPart::Text { text } => {
                        if !content_text.is_empty() {
                            content_text.push('\n');
                        }
                        content_text.push_str(&text);
                    }
                    GeminiPart::FunctionCall { function_call } => {
                        let id = format!("gemini_tc_{}", tc_counter);
                        tc_counter += 1;
                        tool_calls.push(ToolCall::new(
                            id,
                            function_call.name,
                            function_call.args,
                        ));
                    }
                    _ => {} // FunctionResponse shouldn't appear in model output
                }
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

        let finish_reason = match candidate.finish_reason.as_deref() {
            Some("STOP") => FinishReason::Stop,
            Some("MAX_TOKENS") => FinishReason::Length,
            Some("SAFETY") => FinishReason::ContentFilter,
            Some("RECITATION") => FinishReason::ContentFilter,
            _ => FinishReason::Stop,
        };

        let usage = response
            .usage_metadata
            .map(|u| {
                Usage::new(
                    u.prompt_token_count.unwrap_or(0),
                    u.candidates_token_count.unwrap_or(0),
                )
            })
            .unwrap_or_default();

        Ok(CompletionResponse {
            message,
            thinking,
            usage,
            model: model.to_string(),
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
            #[allow(dead_code)]
            status: Option<String>,
        }

        if let Ok(err) = serde_json::from_str::<ErrorResponse>(body) {
            match status {
                401 | 403 => Error::auth(err.error.message),
                429 => Error::rate_limit(err.error.message),
                400 => Error::invalid_request(err.error.message),
                _ => Error::api(status, err.error.message),
            }
        } else {
            Error::api(status, body.to_string())
        }
    }
}

/// Find the function name for a given tool_call_id by searching previous messages
fn find_function_name_by_id(
    _contents: &[GeminiContent],
    messages: &[Message],
    tool_call_id: &str,
) -> Option<String> {
    for msg in messages {
        for tc in &msg.tool_calls {
            if tc.id == tool_call_id {
                return Some(tc.name.clone());
            }
        }
    }
    None
}

/// Merge adjacent contents with the same role
fn merge_adjacent_contents(contents: Vec<GeminiContent>) -> Vec<GeminiContent> {
    let mut merged: Vec<GeminiContent> = Vec::new();

    for content in contents {
        if let Some(last) = merged.last_mut() {
            if last.role == content.role {
                last.parts.extend(content.parts);
                continue;
            }
        }
        merged.push(content);
    }

    merged
}

#[async_trait]
impl Provider for GeminiProvider {
    fn name(&self) -> &str {
        "gemini"
    }

    fn default_model(&self) -> Option<&str> {
        self.default_model.as_deref()
    }

    async fn complete(&self, request: CompletionRequest) -> Result<CompletionResponse, Error> {
        let mut req = request;
        req.stream = false;

        let model = self.resolve_model(&req);
        let api_request = self.build_request(&req);

        debug!(
            model = %model,
            content_count = api_request.contents.len(),
            has_tools = api_request.tools.is_some(),
            "Gemini request"
        );
        trace!(request = %serde_json::to_string(&api_request).unwrap_or_default(), "Gemini request payload");

        let url = format!(
            "{}/models/{}:generateContent?key={}",
            self.base_url, model, self.api_key
        );

        let response = self
            .client
            .post(&url)
            .header("Content-Type", "application/json")
            .json(&api_request)
            .send()
            .await
            .map_err(|e| Error::network(e.to_string()))?;

        let status = response.status();
        if !status.is_success() {
            let error_text = response.text().await.unwrap_or_default();
            error!(status = status.as_u16(), body = %error_text, "Gemini request failed");
            return Err(self.parse_error(status.as_u16(), &error_text));
        }

        let response_text = response
            .text()
            .await
            .map_err(|e| Error::serialization(e.to_string()))?;

        trace!(response = %response_text, "Gemini response payload");

        let api_response: GeminiResponse = serde_json::from_str(&response_text)
            .map_err(|e| Error::serialization(e.to_string()))?;

        let parsed = self.parse_response(api_response, &model)?;

        debug!(
            model = %parsed.model,
            finish_reason = ?parsed.finish_reason,
            content_len = parsed.message.content.to_string_lossy().len(),
            tool_calls = parsed.message.tool_calls.len(),
            prompt_tokens = parsed.usage.prompt_tokens,
            completion_tokens = parsed.usage.completion_tokens,
            "Gemini response"
        );

        Ok(parsed)
    }

    async fn stream(&self, request: CompletionRequest) -> Result<StreamResult, Error> {
        let mut req = request;
        req.stream = true;

        let model = self.resolve_model(&req);
        let api_request = self.build_request(&req);

        debug!(
            model = %model,
            content_count = api_request.contents.len(),
            has_tools = api_request.tools.is_some(),
            "Gemini stream request"
        );
        trace!(request = %serde_json::to_string(&api_request).unwrap_or_default(), "Gemini stream request payload");

        let url = format!(
            "{}/models/{}:streamGenerateContent?alt=sse&key={}",
            self.base_url, model, self.api_key
        );

        let request_builder = self
            .client
            .post(&url)
            .header("Content-Type", "application/json")
            .json(&api_request);

        let es = EventSource::new(request_builder).map_err(|e| Error::stream(e.to_string()))?;

        let (tx, rx) = mpsc::channel::<Result<StreamChunk, Error>>(100);

        tokio::spawn(async move {
            let mut es = es;
            let mut tc_counter: usize = 0;

            while let Some(event) = es.next().await {
                match event {
                    Ok(Event::Open) => {
                        debug!("Gemini SSE connection opened");
                    }
                    Ok(Event::Message(msg)) => {
                        trace!(data = %msg.data, "Gemini SSE chunk");

                        match serde_json::from_str::<GeminiResponse>(&msg.data) {
                            Ok(response) => {
                                // Extract usage if present
                                let usage = response.usage_metadata.as_ref().map(|u| {
                                    Usage::new(
                                        u.prompt_token_count.unwrap_or(0),
                                        u.candidates_token_count.unwrap_or(0),
                                    )
                                });

                                if let Some(candidates) = &response.candidates {
                                    for candidate in candidates {
                                        if let Some(ref content) = candidate.content {
                                            for part in &content.parts {
                                                match part {
                                                    GeminiPart::Text { text } => {
                                                        if !text.is_empty() {
                                                            let _ = tx
                                                                .send(Ok(StreamChunk::Delta {
                                                                    content: text.clone(),
                                                                }))
                                                                .await;
                                                        }
                                                    }
                                                    GeminiPart::FunctionCall { function_call } => {
                                                        let id = format!("gemini_tc_{}", tc_counter);
                                                        tc_counter += 1;
                                                        let _ = tx
                                                            .send(Ok(StreamChunk::ToolCallStart {
                                                                id,
                                                                name: function_call.name.clone(),
                                                            }))
                                                            .await;
                                                        let args = serde_json::to_string(&function_call.args)
                                                            .unwrap_or_default();
                                                        if !args.is_empty() {
                                                            let _ = tx
                                                                .send(Ok(StreamChunk::ToolCallDelta {
                                                                    arguments: args,
                                                                }))
                                                                .await;
                                                        }
                                                    }
                                                    _ => {}
                                                }
                                            }
                                        }

                                        // Check for finish reason
                                        if let Some(ref reason) = candidate.finish_reason {
                                            debug!(finish_reason = %reason, "Gemini stream complete");
                                            let _ = tx
                                                .send(Ok(StreamChunk::Done { usage: usage.clone() }))
                                                .await;
                                        }
                                    }
                                }
                            }
                            Err(e) => {
                                error!(error = %e, data = %msg.data, "Failed to parse Gemini SSE chunk");
                                let _ = tx
                                    .send(Err(Error::stream(format!(
                                        "Failed to parse Gemini SSE: {}",
                                        e
                                    ))))
                                    .await;
                                break;
                            }
                        }
                    }
                    Err(reqwest_eventsource::Error::StreamEnded) => {
                        debug!("Gemini SSE stream ended");
                        break;
                    }
                    Err(e) => {
                        error!(error = ?e, "Gemini SSE error");
                        let _ = tx
                            .send(Err(Error::stream(format!("Gemini SSE error: {:?}", e))))
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
            "gemini-2.5-pro",
            "gemini-2.5-flash",
            "gemini-2.0-flash",
        ]
    }
}

// ── Gemini API types ─────────────────────────────────────────────────────────

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct GeminiRequest {
    contents: Vec<GeminiContent>,
    #[serde(skip_serializing_if = "Option::is_none")]
    system_instruction: Option<GeminiContent>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tools: Option<Vec<GeminiToolsEntry>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    generation_config: Option<GeminiGenerationConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct GeminiContent {
    #[serde(skip_serializing_if = "Option::is_none")]
    role: Option<String>,
    parts: Vec<GeminiPart>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
enum GeminiPart {
    FunctionCall {
        #[serde(rename = "functionCall")]
        function_call: GeminiFunctionCall,
    },
    FunctionResponse {
        #[serde(rename = "functionResponse")]
        function_response: GeminiFunctionResponse,
    },
    Text {
        text: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct GeminiFunctionCall {
    name: String,
    args: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct GeminiFunctionResponse {
    name: String,
    response: serde_json::Value,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct GeminiToolsEntry {
    function_declarations: Vec<GeminiFunctionDeclaration>,
}

#[derive(Debug, Serialize)]
struct GeminiFunctionDeclaration {
    name: String,
    description: String,
    parameters: serde_json::Value,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct GeminiGenerationConfig {
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    top_p: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_output_tokens: Option<u32>,
}

// ── Response types ───────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GeminiResponse {
    #[serde(default)]
    candidates: Option<Vec<GeminiCandidate>>,
    #[serde(default)]
    usage_metadata: Option<GeminiUsageMetadata>,
    #[serde(default)]
    prompt_feedback: Option<GeminiPromptFeedback>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GeminiCandidate {
    #[serde(default)]
    content: Option<GeminiContent>,
    #[serde(default)]
    finish_reason: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GeminiUsageMetadata {
    #[serde(default)]
    prompt_token_count: Option<u32>,
    #[serde(default)]
    candidates_token_count: Option<u32>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GeminiPromptFeedback {
    #[serde(default)]
    block_reason: Option<String>,
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_provider_creation() {
        let provider = GeminiProvider::new("test-key");
        assert_eq!(provider.name(), "gemini");
        assert_eq!(provider.default_model(), None);
    }

    #[test]
    fn test_provider_with_custom_url() {
        let provider = GeminiProvider::new("test-key")
            .with_base_url("https://custom.proxy.com/v1beta");
        assert_eq!(provider.base_url, "https://custom.proxy.com/v1beta");
    }

    #[test]
    fn test_provider_with_custom_model() {
        let provider = GeminiProvider::new("test-key")
            .with_default_model("gemini-2.5-pro");
        assert_eq!(provider.default_model(), Some("gemini-2.5-pro"));
    }

    #[test]
    fn test_build_request_basic() {
        let provider = GeminiProvider::new("test-key")
            .with_default_model("gemini-2.5-flash");
        let request = CompletionRequest::new(vec![Message::user("Hello")]);
        let api_request = provider.build_request(&request);

        assert_eq!(api_request.contents.len(), 1);
        assert_eq!(api_request.contents[0].role, Some("user".to_string()));
        assert!(api_request.system_instruction.is_none());
        assert!(api_request.tools.is_none());
    }

    #[test]
    fn test_build_request_system_instruction() {
        let provider = GeminiProvider::new("test-key");
        let request = CompletionRequest::new(vec![
            Message::system("You are helpful."),
            Message::user("Hello"),
        ]);
        let api_request = provider.build_request(&request);

        assert!(api_request.system_instruction.is_some());
        let sys = api_request.system_instruction.unwrap();
        assert!(sys.role.is_none()); // system_instruction has no role
        assert_eq!(sys.parts.len(), 1);

        // System message should not appear in contents
        assert_eq!(api_request.contents.len(), 1);
        assert_eq!(api_request.contents[0].role, Some("user".to_string()));
    }

    #[test]
    fn test_build_request_tool_conversion() {
        let provider = GeminiProvider::new("test-key");
        let tool = ToolDefinition::new("test_tool", "A test tool");
        let request = CompletionRequest::new(vec![Message::user("Use tool")])
            .with_tools(vec![tool]);
        let api_request = provider.build_request(&request);

        assert!(api_request.tools.is_some());
        let tools = api_request.tools.unwrap();
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0].function_declarations.len(), 1);
        assert_eq!(tools[0].function_declarations[0].name, "test_tool");
    }

    #[test]
    fn test_build_request_assistant_role_mapping() {
        let provider = GeminiProvider::new("test-key");
        let request = CompletionRequest::new(vec![
            Message::user("Hello"),
            Message::assistant("Hi there"),
            Message::user("How are you?"),
        ]);
        let api_request = provider.build_request(&request);

        assert_eq!(api_request.contents.len(), 3);
        assert_eq!(api_request.contents[0].role, Some("user".to_string()));
        assert_eq!(api_request.contents[1].role, Some("model".to_string())); // assistant → model
        assert_eq!(api_request.contents[2].role, Some("user".to_string()));
    }

    #[test]
    fn test_build_request_tool_result_as_user() {
        let provider = GeminiProvider::new("test-key");
        let messages = vec![
            Message::user("Search for rust"),
            Message::assistant_with_tool_calls(
                "Sure",
                vec![ToolCall::new("tc_1", "search", serde_json::json!({"q": "rust"}))],
            ),
            Message::tool_result("tc_1", "search results here"),
        ];
        let request = CompletionRequest::new(messages);
        let api_request = provider.build_request(&request);

        // Should have 3 contents: user, model (assistant), user (tool result)
        assert_eq!(api_request.contents.len(), 3);
        assert_eq!(api_request.contents[2].role, Some("user".to_string()));
    }

    #[test]
    fn test_parse_response_text() {
        let provider = GeminiProvider::new("test-key");
        let response = GeminiResponse {
            candidates: Some(vec![GeminiCandidate {
                content: Some(GeminiContent {
                    role: Some("model".to_string()),
                    parts: vec![GeminiPart::Text {
                        text: "Hello!".to_string(),
                    }],
                }),
                finish_reason: Some("STOP".to_string()),
            }]),
            usage_metadata: Some(GeminiUsageMetadata {
                prompt_token_count: Some(10),
                candidates_token_count: Some(5),
            }),
            prompt_feedback: None,
        };

        let parsed = provider.parse_response(response, "gemini-2.5-flash").unwrap();
        assert_eq!(parsed.message.content.to_string_lossy(), "Hello!");
        assert_eq!(parsed.finish_reason, FinishReason::Stop);
        assert_eq!(parsed.usage.prompt_tokens, 10);
        assert_eq!(parsed.usage.completion_tokens, 5);
        assert_eq!(parsed.model, "gemini-2.5-flash");
    }

    #[test]
    fn test_parse_response_tool_calls() {
        let provider = GeminiProvider::new("test-key");
        let response = GeminiResponse {
            candidates: Some(vec![GeminiCandidate {
                content: Some(GeminiContent {
                    role: Some("model".to_string()),
                    parts: vec![
                        GeminiPart::Text {
                            text: "Let me search.".to_string(),
                        },
                        GeminiPart::FunctionCall {
                            function_call: GeminiFunctionCall {
                                name: "search".to_string(),
                                args: serde_json::json!({"query": "rust"}),
                            },
                        },
                    ],
                }),
                finish_reason: Some("STOP".to_string()),
            }]),
            usage_metadata: None,
            prompt_feedback: None,
        };

        let parsed = provider.parse_response(response, "gemini-2.5-flash").unwrap();
        assert_eq!(parsed.message.tool_calls.len(), 1);
        assert_eq!(parsed.message.tool_calls[0].name, "search");
        assert_eq!(parsed.message.tool_calls[0].id, "gemini_tc_0");
    }

    #[test]
    fn test_parse_response_empty_candidates() {
        let provider = GeminiProvider::new("test-key");
        let response = GeminiResponse {
            candidates: None,
            usage_metadata: None,
            prompt_feedback: Some(GeminiPromptFeedback {
                block_reason: Some("SAFETY".to_string()),
            }),
        };

        let err = provider.parse_response(response, "gemini-2.5-flash").unwrap_err();
        let msg = format!("{}", err);
        assert!(msg.contains("safety filter"));
    }

    #[test]
    fn test_parse_error_auth() {
        let provider = GeminiProvider::new("test-key");
        let body = r#"{"error": {"message": "API key not valid", "status": "PERMISSION_DENIED"}}"#;
        let err = provider.parse_error(403, body);
        assert!(err.is_auth_error());
    }

    #[test]
    fn test_parse_error_rate_limit() {
        let provider = GeminiProvider::new("test-key");
        let body = r#"{"error": {"message": "Quota exceeded", "status": "RESOURCE_EXHAUSTED"}}"#;
        let err = provider.parse_error(429, body);
        assert!(err.is_retryable());
    }

    #[test]
    fn test_merge_adjacent_contents() {
        let contents = vec![
            GeminiContent {
                role: Some("user".to_string()),
                parts: vec![GeminiPart::Text { text: "Hello".to_string() }],
            },
            GeminiContent {
                role: Some("user".to_string()),
                parts: vec![GeminiPart::FunctionResponse {
                    function_response: GeminiFunctionResponse {
                        name: "search".to_string(),
                        response: serde_json::json!({"result": "found"}),
                    },
                }],
            },
            GeminiContent {
                role: Some("model".to_string()),
                parts: vec![GeminiPart::Text { text: "Here you go".to_string() }],
            },
        ];

        let merged = merge_adjacent_contents(contents);
        assert_eq!(merged.len(), 2);
        assert_eq!(merged[0].role, Some("user".to_string()));
        assert_eq!(merged[0].parts.len(), 2);
        assert_eq!(merged[1].role, Some("model".to_string()));
    }

    #[test]
    fn test_available_models() {
        let provider = GeminiProvider::new("test-key");
        let models = provider.available_models();
        assert!(models.contains(&"gemini-2.5-pro"));
        assert!(models.contains(&"gemini-2.5-flash"));
        assert!(models.contains(&"gemini-2.0-flash"));
    }

    #[test]
    fn test_resolve_model_default() {
        let provider = GeminiProvider::new("test-key");
        let request = CompletionRequest::new(vec![Message::user("Hello")]);
        assert_eq!(provider.resolve_model(&request), "gemini-2.5-flash");
    }

    #[test]
    fn test_resolve_model_from_provider() {
        let provider = GeminiProvider::new("test-key")
            .with_default_model("gemini-2.5-pro");
        let request = CompletionRequest::new(vec![Message::user("Hello")]);
        assert_eq!(provider.resolve_model(&request), "gemini-2.5-pro");
    }

    #[test]
    fn test_resolve_model_from_request() {
        let provider = GeminiProvider::new("test-key")
            .with_default_model("gemini-2.5-pro");
        let request = CompletionRequest::new(vec![Message::user("Hello")])
            .with_model("gemini-2.0-flash");
        assert_eq!(provider.resolve_model(&request), "gemini-2.0-flash");
    }
}
