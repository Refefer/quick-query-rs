use serde::{Deserialize, Serialize};

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
                    ContentPart::Image { url } => url.len(),
                    ContentPart::ToolUse(tc) => tc.name.len() + tc.arguments.to_string().len(),
                    ContentPart::ToolResult(tr) => tr.content.len(),
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

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ContentPart {
    Text { text: String },
    Image { url: String },
    ToolUse(ToolCall),
    ToolResult(ToolResult),
}

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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolResult {
    pub tool_call_id: String,
    pub content: String,
    #[serde(default)]
    pub is_error: bool,
}

impl ToolResult {
    pub fn success(tool_call_id: impl Into<String>, content: impl Into<String>) -> Self {
        Self {
            tool_call_id: tool_call_id.into(),
            content: content.into(),
            is_error: false,
        }
    }

    pub fn error(tool_call_id: impl Into<String>, content: impl Into<String>) -> Self {
        Self {
            tool_call_id: tool_call_id.into(),
            content: content.into(),
            is_error: true,
        }
    }
}

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
    pub fn system(content: impl Into<Content>) -> Self {
        Self {
            role: Role::System,
            content: content.into(),
            name: None,
            tool_calls: Vec::new(),
            tool_call_id: None,
            reasoning_content: None,
        }
    }

    pub fn user(content: impl Into<Content>) -> Self {
        Self {
            role: Role::User,
            content: content.into(),
            name: None,
            tool_calls: Vec::new(),
            tool_call_id: None,
            reasoning_content: None,
        }
    }

    pub fn assistant(content: impl Into<Content>) -> Self {
        Self {
            role: Role::Assistant,
            content: content.into(),
            name: None,
            tool_calls: Vec::new(),
            tool_call_id: None,
            reasoning_content: None,
        }
    }

    pub fn assistant_with_tool_calls(content: impl Into<Content>, tool_calls: Vec<ToolCall>) -> Self {
        Self {
            role: Role::Assistant,
            content: content.into(),
            name: None,
            tool_calls,
            tool_call_id: None,
            reasoning_content: None,
        }
    }

    pub fn tool_result(tool_call_id: impl Into<String>, content: impl Into<String>) -> Self {
        Self {
            role: Role::Tool,
            content: Content::text(content),
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
}
