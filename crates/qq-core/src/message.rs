use base64::{engine::general_purpose::STANDARD as BASE64, Engine};
use serde::{Deserialize, Serialize};

use crate::error::Error;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    System,
    User,
    Assistant,
    Tool,
}

impl std::fmt::Display for Role {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Role::System => write!(f, "system"),
            Role::User => write!(f, "user"),
            Role::Assistant => write!(f, "assistant"),
            Role::Tool => write!(f, "tool"),
        }
    }
}

// ---------------------------------------------------------------------------
// ImageData
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ImageData {
    /// Base64-encoded image bytes.
    pub data: String,
    /// MIME type (e.g. "image/jpeg", "image/png", "image/gif", "image/webp").
    pub media_type: String,
    /// Image width in pixels.
    pub width: u32,
    /// Image height in pixels.
    pub height: u32,
}

impl ImageData {
    /// Create from raw image bytes.
    ///
    /// Uses `infer` for MIME detection and `imagesize` for dimension extraction.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, Error> {
        let kind = infer::get(bytes)
            .ok_or_else(|| Error::Unknown("Could not detect image format".into()))?;

        let media_type = kind.mime_type();
        if !media_type.starts_with("image/") {
            return Err(Error::Unknown(format!(
                "Not an image file (detected: {})",
                media_type
            )));
        }

        let size = imagesize::blob_size(bytes)
            .map_err(|e| Error::Unknown(format!("Could not determine image dimensions: {}", e)))?;

        Ok(Self {
            data: BASE64.encode(bytes),
            media_type: media_type.to_string(),
            width: size.width as u32,
            height: size.height as u32,
        })
    }

    /// Rough token estimate using Anthropic's formula: (width * height) / 750.
    pub fn estimated_tokens(&self) -> u32 {
        (self.width as u64 * self.height as u64 / 750) as u32
    }

    /// Approximate raw byte size of the decoded image data.
    pub fn decoded_size(&self) -> usize {
        self.data.len() * 3 / 4
    }
}

// ---------------------------------------------------------------------------
// TypedContent
// ---------------------------------------------------------------------------

/// A single piece of content with an associated type.
/// Used throughout the tool and message pipeline.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum TypedContent {
    /// text/* content
    Text { text: String },
    /// image/* content (base64-encoded with dimensions)
    Image { image: ImageData },
}

impl TypedContent {
    pub fn text(s: impl Into<String>) -> Self {
        TypedContent::Text { text: s.into() }
    }

    pub fn image(image: ImageData) -> Self {
        TypedContent::Image { image }
    }

    /// Byte count for budget/threshold calculations.
    ///
    /// For text: string length.
    /// For images: estimated_tokens * 4 (bytes-equivalent for compaction thresholds).
    pub fn byte_count(&self) -> usize {
        match self {
            TypedContent::Text { text } => text.len(),
            TypedContent::Image { image } => image.estimated_tokens() as usize * 4,
        }
    }
}

impl From<String> for TypedContent {
    fn from(s: String) -> Self {
        TypedContent::text(s)
    }
}

impl From<&str> for TypedContent {
    fn from(s: &str) -> Self {
        TypedContent::text(s)
    }
}

impl From<ImageData> for TypedContent {
    fn from(image: ImageData) -> Self {
        TypedContent::image(image)
    }
}

// ---------------------------------------------------------------------------
// IntoContent — ergonomic conversion for Message constructors
// ---------------------------------------------------------------------------

/// Trait for types that can be converted to message content.
pub trait IntoContent {
    fn into_content(self) -> Content;
}

impl IntoContent for &str {
    fn into_content(self) -> Content {
        Content::Text(self.to_string())
    }
}

impl IntoContent for &String {
    fn into_content(self) -> Content {
        Content::Text(self.clone())
    }
}

impl IntoContent for String {
    fn into_content(self) -> Content {
        Content::Text(self)
    }
}

impl IntoContent for Content {
    fn into_content(self) -> Content {
        self
    }
}

impl IntoContent for TypedContent {
    fn into_content(self) -> Content {
        Content::Parts(vec![ContentPart::from(self)])
    }
}

impl IntoContent for Vec<TypedContent> {
    fn into_content(self) -> Content {
        Content::Parts(self.into_iter().map(ContentPart::from).collect())
    }
}

// ---------------------------------------------------------------------------
// Content
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Content {
    Text(String),
    Parts(Vec<ContentPart>),
}

impl Content {
    pub fn text(s: impl Into<String>) -> Self {
        Content::Text(s.into())
    }

    pub fn as_text(&self) -> Option<&str> {
        match self {
            Content::Text(s) => Some(s),
            Content::Parts(parts) => {
                if parts.len() == 1 {
                    if let ContentPart::Text { text } = &parts[0] {
                        return Some(text);
                    }
                }
                None
            }
        }
    }

    /// Concatenate all text parts, ignoring non-text content.
    /// Use only when you explicitly intend to discard non-text parts
    /// (e.g. for debug logging or compaction formatting).
    pub fn to_string_lossy(&self) -> String {
        match self {
            Content::Text(s) => s.clone(),
            Content::Parts(parts) => parts
                .iter()
                .filter_map(|p| match p {
                    ContentPart::Text { text } => Some(text.as_str()),
                    _ => None,
                })
                .collect::<Vec<_>>()
                .join(""),
        }
    }

    /// Count the number of bytes in this content.
    pub fn byte_count(&self) -> usize {
        match self {
            Content::Text(s) => s.len(),
            Content::Parts(parts) => parts
                .iter()
                .map(|p| match p {
                    ContentPart::Text { text } => text.len(),
                    ContentPart::Image { image } => image.estimated_tokens() as usize * 4,
                    ContentPart::ToolUse(tc) => tc.name.len() + tc.arguments.to_string().len(),
                    ContentPart::ToolResult(tr) => tr.byte_count(),
                })
                .sum(),
        }
    }
}

impl From<String> for Content {
    fn from(s: String) -> Self {
        Content::Text(s)
    }
}

impl From<&str> for Content {
    fn from(s: &str) -> Self {
        Content::Text(s.to_string())
    }
}

// ---------------------------------------------------------------------------
// ContentPart
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ContentPart {
    Text { text: String },
    Image { image: ImageData },
    ToolUse(ToolCall),
    ToolResult(ToolResult),
}

impl From<TypedContent> for ContentPart {
    fn from(tc: TypedContent) -> Self {
        match tc {
            TypedContent::Text { text } => ContentPart::Text { text },
            TypedContent::Image { image } => ContentPart::Image { image },
        }
    }
}

// ---------------------------------------------------------------------------
// ToolCall
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    pub arguments: serde_json::Value,
}

impl ToolCall {
    pub fn new(id: impl Into<String>, name: impl Into<String>, arguments: serde_json::Value) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            arguments,
        }
    }
}

// ---------------------------------------------------------------------------
// ToolResult
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolResult {
    pub tool_call_id: String,
    pub content: Vec<TypedContent>,
    #[serde(default)]
    pub is_error: bool,
}

impl ToolResult {
    pub fn success(tool_call_id: impl Into<String>, content: impl Into<TypedContent>) -> Self {
        Self {
            tool_call_id: tool_call_id.into(),
            content: vec![content.into()],
            is_error: false,
        }
    }

    pub fn error(tool_call_id: impl Into<String>, content: impl Into<TypedContent>) -> Self {
        Self {
            tool_call_id: tool_call_id.into(),
            content: vec![content.into()],
            is_error: true,
        }
    }

    pub fn with_content(
        tool_call_id: impl Into<String>,
        content: Vec<TypedContent>,
        is_error: bool,
    ) -> Self {
        Self {
            tool_call_id: tool_call_id.into(),
            content,
            is_error,
        }
    }

    /// Byte count of all content parts.
    pub fn byte_count(&self) -> usize {
        self.content.iter().map(|c| c.byte_count()).sum()
    }

    /// Extract text content, concatenating text parts only.
    pub fn text_content(&self) -> String {
        self.content
            .iter()
            .filter_map(|c| match c {
                TypedContent::Text { text } => Some(text.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("")
    }
}

// ---------------------------------------------------------------------------
// Message
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    pub role: Role,
    pub content: Content,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub tool_calls: Vec<ToolCall>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
    /// Reasoning/thinking content from reasoning models (e.g., o1, DeepSeek-R1, Qwen3).
    /// Preserved during tool-call exchanges so the model retains its reasoning context,
    /// then stripped after the final answer.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub reasoning_content: Option<String>,
}

impl Message {
    pub fn system(content: impl IntoContent) -> Self {
        Self {
            role: Role::System,
            content: content.into_content(),
            name: None,
            tool_calls: Vec::new(),
            tool_call_id: None,
            reasoning_content: None,
        }
    }

    pub fn user(content: impl IntoContent) -> Self {
        Self {
            role: Role::User,
            content: content.into_content(),
            name: None,
            tool_calls: Vec::new(),
            tool_call_id: None,
            reasoning_content: None,
        }
    }

    pub fn assistant(content: impl IntoContent) -> Self {
        Self {
            role: Role::Assistant,
            content: content.into_content(),
            name: None,
            tool_calls: Vec::new(),
            tool_call_id: None,
            reasoning_content: None,
        }
    }

    pub fn assistant_with_tool_calls(content: impl IntoContent, tool_calls: Vec<ToolCall>) -> Self {
        Self {
            role: Role::Assistant,
            content: content.into_content(),
            name: None,
            tool_calls,
            tool_call_id: None,
            reasoning_content: None,
        }
    }

    pub fn tool_result(tool_call_id: impl Into<String>, content: impl IntoContent) -> Self {
        Self {
            role: Role::Tool,
            content: content.into_content(),
            name: None,
            tool_calls: Vec::new(),
            tool_call_id: Some(tool_call_id.into()),
            reasoning_content: None,
        }
    }

    pub fn with_name(mut self, name: impl Into<String>) -> Self {
        self.name = Some(name.into());
        self
    }

    /// Attach reasoning/thinking content to this message.
    /// Normalizes `Some("")` to `None`.
    pub fn with_reasoning(mut self, reasoning: Option<String>) -> Self {
        self.reasoning_content = reasoning.filter(|r| !r.is_empty());
        self
    }

    /// Count the approximate number of bytes in this message.
    pub fn byte_count(&self) -> usize {
        self.content.byte_count()
            + self
                .tool_calls
                .iter()
                .map(|tc| tc.id.len() + tc.name.len() + tc.arguments.to_string().len())
                .sum::<usize>()
            + self
                .tool_call_id
                .as_ref()
                .map(|id| id.len())
                .unwrap_or(0)
            + self
                .reasoning_content
                .as_ref()
                .map(|r| r.len())
                .unwrap_or(0)
    }

    /// Count observable bytes (excludes ephemeral reasoning_content).
    /// Use for observation threshold checks and preserve_recent calculations.
    pub fn observable_byte_count(&self) -> usize {
        self.content.byte_count()
            + self
                .tool_calls
                .iter()
                .map(|tc| tc.id.len() + tc.name.len() + tc.arguments.to_string().len())
                .sum::<usize>()
            + self
                .tool_call_id
                .as_ref()
                .map(|id| id.len())
                .unwrap_or(0)
    }
}

/// Strip reasoning content from all messages in a history slice.
/// Called after the model delivers its final answer (no tool calls)
/// to avoid sending stale reasoning on future turns.
pub fn strip_reasoning_from_history(messages: &mut [Message]) {
    for msg in messages {
        msg.reasoning_content = None;
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Usage {
    pub prompt_tokens: u32,
    pub completion_tokens: u32,
    pub total_tokens: u32,
}

impl Usage {
    pub fn new(prompt_tokens: u32, completion_tokens: u32) -> Self {
        Self {
            prompt_tokens,
            completion_tokens,
            total_tokens: prompt_tokens + completion_tokens,
        }
    }
}

/// Why the model stopped generating. Mirrors the OpenAI / Anthropic / Gemini
/// `finish_reason` semantics so the agent loop can distinguish "model is done"
/// from "model was cut off mid-response by max-tokens".
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FinishReason {
    Stop,
    Length,
    ToolCalls,
    ContentFilter,
    Error,
}

#[derive(Debug, Clone)]
pub enum StreamChunk {
    Start {
        model: String,
    },
    Delta {
        content: String,
    },
    ThinkingDelta {
        content: String,
    },
    ToolCallStart {
        id: String,
        name: String,
    },
    ToolCallDelta {
        arguments: String,
    },
    Done {
        usage: Option<Usage>,
        /// Why the model stopped. `None` if the provider didn't surface a reason.
        finish_reason: Option<FinishReason>,
    },
    Error {
        message: String,
    },
}

/// Strip thinking tags from content for providers that embed thinking inline.
///
/// Handles common patterns:
/// - `<think>...</think>` (DeepSeek and others)
/// - `<reasoning>...</reasoning>` (some local models)
///
/// Returns (clean_content, extracted_thinking).
pub fn strip_thinking_tags(content: &str) -> (String, Option<String>) {
    // Try <think>...</think> pattern first (most common)
    if let Some((clean, thinking)) = extract_tagged_content(content, "think") {
        return (clean, Some(thinking));
    }

    // Try <reasoning>...</reasoning> pattern
    if let Some((clean, thinking)) = extract_tagged_content(content, "reasoning") {
        return (clean, Some(thinking));
    }

    // No thinking tags found
    (content.to_string(), None)
}

/// Extract content between XML-like tags.
fn extract_tagged_content(content: &str, tag: &str) -> Option<(String, String)> {
    let open_tag = format!("<{}>", tag);
    let close_tag = format!("</{}>", tag);

    let start_idx = content.find(&open_tag)?;
    let end_idx = content.find(&close_tag)?;

    if end_idx <= start_idx {
        return None;
    }

    let thinking_start = start_idx + open_tag.len();
    let thinking = content[thinking_start..end_idx].trim().to_string();

    // Build clean content by removing the tag section
    let before = content[..start_idx].trim_end();
    let after = content[end_idx + close_tag.len()..].trim_start();

    // Join with a space if both parts are non-empty
    let clean = if !before.is_empty() && !after.is_empty() {
        format!("{} {}", before, after)
    } else {
        format!("{}{}", before, after)
    };

    Some((clean, thinking))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_message_creation() {
        let msg = Message::user("Hello, world!");
        assert_eq!(msg.role, Role::User);
        assert_eq!(msg.content.as_text(), Some("Hello, world!"));
    }

    #[test]
    fn test_content_text() {
        let content = Content::text("test");
        assert_eq!(content.as_text(), Some("test"));
    }

    #[test]
    fn test_tool_call() {
        let tool_call = ToolCall::new("id-1", "read_file", serde_json::json!({"path": "/tmp/test"}));
        assert_eq!(tool_call.name, "read_file");
    }

    #[test]
    fn test_role_display() {
        assert_eq!(Role::User.to_string(), "user");
        assert_eq!(Role::Assistant.to_string(), "assistant");
    }

    #[test]
    fn test_strip_thinking_tags_with_think() {
        let content = "<think>Let me analyze this...</think>The answer is 42.";
        let (clean, thinking) = strip_thinking_tags(content);
        assert_eq!(clean, "The answer is 42.");
        assert_eq!(thinking, Some("Let me analyze this...".to_string()));
    }

    #[test]
    fn test_strip_thinking_tags_with_reasoning() {
        let content = "<reasoning>Step 1, step 2...</reasoning>Final answer: yes.";
        let (clean, thinking) = strip_thinking_tags(content);
        assert_eq!(clean, "Final answer: yes.");
        assert_eq!(thinking, Some("Step 1, step 2...".to_string()));
    }

    #[test]
    fn test_strip_thinking_tags_no_tags() {
        let content = "Just a normal response.";
        let (clean, thinking) = strip_thinking_tags(content);
        assert_eq!(clean, "Just a normal response.");
        assert_eq!(thinking, None);
    }

    #[test]
    fn test_message_byte_count_text() {
        let msg = Message::user("Hello, world!");
        assert_eq!(msg.byte_count(), 13);
    }

    #[test]
    fn test_message_byte_count_with_tool_calls() {
        let tool_call = ToolCall::new("tc-1", "read_file", serde_json::json!({"path": "/tmp"}));
        let msg = Message::assistant_with_tool_calls("thinking", vec![tool_call]);
        // content: "thinking" (8) + tool_call: id "tc-1" (4) + name "read_file" (9) + args (14)
        assert!(msg.byte_count() > 8);
    }

    #[test]
    fn test_message_byte_count_tool_result() {
        let msg = Message::tool_result("tc-1", "file contents here");
        // content: "file contents here" (18) + tool_call_id: "tc-1" (4)
        assert_eq!(msg.byte_count(), 22);
    }

    #[test]
    fn test_strip_thinking_tags_preserves_content_before_and_after() {
        let content = "Before <think>thinking here</think> After";
        let (clean, thinking) = strip_thinking_tags(content);
        // Content parts are joined with a space
        assert_eq!(clean, "Before After");
        assert_eq!(thinking, Some("thinking here".to_string()));
    }

    #[test]
    fn test_with_reasoning_some() {
        let msg = Message::assistant("hello")
            .with_reasoning(Some("I thought about it".to_string()));
        assert_eq!(msg.reasoning_content, Some("I thought about it".to_string()));
    }

    #[test]
    fn test_with_reasoning_none() {
        let msg = Message::assistant("hello")
            .with_reasoning(None);
        assert_eq!(msg.reasoning_content, None);
    }

    #[test]
    fn test_with_reasoning_empty_string_becomes_none() {
        let msg = Message::assistant("hello")
            .with_reasoning(Some(String::new()));
        assert_eq!(msg.reasoning_content, None);
    }

    #[test]
    fn test_strip_reasoning_from_history() {
        let mut messages = vec![
            Message::user("hello"),
            Message::assistant("thinking response")
                .with_reasoning(Some("my reasoning".to_string())),
            Message::assistant_with_tool_calls("", vec![
                ToolCall::new("tc-1", "read_file", serde_json::json!({"path": "/tmp"})),
            ]).with_reasoning(Some("tool reasoning".to_string())),
            Message::tool_result("tc-1", "file contents"),
        ];
        assert!(messages[1].reasoning_content.is_some());
        assert!(messages[2].reasoning_content.is_some());

        strip_reasoning_from_history(&mut messages);

        for msg in &messages {
            assert_eq!(msg.reasoning_content, None);
        }
    }

    #[test]
    fn test_byte_count_includes_reasoning() {
        let msg = Message::assistant("hello")
            .with_reasoning(Some("reasoning text".to_string()));
        // "hello" (5) + "reasoning text" (14) = 19
        assert_eq!(msg.byte_count(), 19);
    }

    #[test]
    fn test_byte_count_no_reasoning() {
        let msg = Message::assistant("hello");
        assert_eq!(msg.byte_count(), 5);
    }

    #[test]
    fn test_observable_byte_count_excludes_reasoning() {
        let msg = Message::assistant("hello")
            .with_reasoning(Some("reasoning text here".to_string()));
        // byte_count includes reasoning (19 bytes), observable does not
        assert_eq!(msg.byte_count(), 5 + 19);
        assert_eq!(msg.observable_byte_count(), 5);
        assert!(msg.observable_byte_count() < msg.byte_count());
    }

    #[test]
    fn test_observable_byte_count_no_reasoning_matches_byte_count() {
        let msg = Message::assistant("hello");
        assert_eq!(msg.observable_byte_count(), msg.byte_count());

        let msg2 = Message::tool_result("tc-1", "result data");
        assert_eq!(msg2.observable_byte_count(), msg2.byte_count());
    }

    #[test]
    fn test_observable_byte_count_with_tool_calls() {
        let tool_call = ToolCall::new("tc-1", "read_file", serde_json::json!({"path": "/tmp"}));
        let msg = Message::assistant_with_tool_calls("thinking", vec![tool_call])
            .with_reasoning(Some("long reasoning content".to_string()));
        // observable should include content + tool call but NOT reasoning
        let expected = 8 // "thinking"
            + 4 + 9 + serde_json::json!({"path": "/tmp"}).to_string().len(); // tool call
        assert_eq!(msg.observable_byte_count(), expected);
        assert!(msg.observable_byte_count() < msg.byte_count());
    }

    #[test]
    fn test_typed_content_text() {
        let tc = TypedContent::text("hello");
        assert_eq!(tc.byte_count(), 5);
    }

    #[test]
    fn test_typed_content_from_string() {
        let tc: TypedContent = "hello".into();
        match tc {
            TypedContent::Text { text } => assert_eq!(text, "hello"),
            _ => panic!("Expected Text variant"),
        }
    }

    #[test]
    fn test_typed_content_from_owned_string() {
        let tc: TypedContent = String::from("world").into();
        match tc {
            TypedContent::Text { text } => assert_eq!(text, "world"),
            _ => panic!("Expected Text variant"),
        }
    }

    #[test]
    fn test_tool_result_text_content() {
        let tr = ToolResult::success("tc-1", "result text");
        assert_eq!(tr.text_content(), "result text");
        assert_eq!(tr.byte_count(), 11);
    }

    #[test]
    fn test_tool_result_with_content() {
        let tr = ToolResult::with_content(
            "tc-1",
            vec![TypedContent::text("part1"), TypedContent::text("part2")],
            false,
        );
        assert_eq!(tr.text_content(), "part1part2");
        assert!(!tr.is_error);
    }

    #[test]
    fn test_into_content_str() {
        let content = "hello".into_content();
        assert_eq!(content.as_text(), Some("hello"));
    }

    #[test]
    fn test_into_content_string() {
        let content = String::from("hello").into_content();
        assert_eq!(content.as_text(), Some("hello"));
    }

    #[test]
    fn test_into_content_vec_typed() {
        let content = vec![TypedContent::text("a"), TypedContent::text("b")].into_content();
        match content {
            Content::Parts(parts) => assert_eq!(parts.len(), 2),
            _ => panic!("Expected Parts variant"),
        }
    }

    #[test]
    fn test_message_user_with_vec_typed_content() {
        let msg = Message::user(vec![TypedContent::text("describe this")]);
        match msg.content {
            Content::Parts(parts) => {
                assert_eq!(parts.len(), 1);
                match &parts[0] {
                    ContentPart::Text { text } => assert_eq!(text, "describe this"),
                    _ => panic!("Expected Text part"),
                }
            }
            _ => panic!("Expected Parts variant"),
        }
    }

    #[test]
    fn test_content_part_from_typed_content() {
        let tc = TypedContent::text("hello");
        let cp = ContentPart::from(tc);
        match cp {
            ContentPart::Text { text } => assert_eq!(text, "hello"),
            _ => panic!("Expected Text variant"),
        }
    }

    #[test]
    fn test_typed_content_serde_roundtrip() {
        let tc = TypedContent::text("hello");
        let json = serde_json::to_string(&tc).unwrap();
        let tc2: TypedContent = serde_json::from_str(&json).unwrap();
        match tc2 {
            TypedContent::Text { text } => assert_eq!(text, "hello"),
            _ => panic!("Expected Text variant"),
        }
    }
}
