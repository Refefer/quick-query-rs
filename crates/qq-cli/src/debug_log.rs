//! Debug logging module for tracing chat loop behavior.
//!
//! Writes JSON lines to a file for analyzing message flow, tool calls, and agent execution.

use std::fs::{File, OpenOptions};
use std::io::{BufWriter, Write};
use std::path::Path;
use std::sync::Mutex;

use chrono::Utc;
use serde::Serialize;

use qq_core::Message;

/// Debug logger that writes JSON lines to a file.
pub struct DebugLogger {
    writer: Mutex<BufWriter<File>>,
}

impl DebugLogger {
    /// Create a new debug logger writing to the specified file.
    pub fn new(path: &Path) -> std::io::Result<Self> {
        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)?;

        let writer = BufWriter::new(file);

        Ok(Self {
            writer: Mutex::new(writer),
        })
    }

    /// Log an event to the debug file.
    fn log<T: Serialize>(&self, event_type: &str, data: T) {
        let entry = LogEntry {
            timestamp: Utc::now().to_rfc3339(),
            event_type: event_type.to_string(),
            data: serde_json::to_value(data).unwrap_or_default(),
        };

        if let Ok(mut writer) = self.writer.lock() {
            if let Ok(json) = serde_json::to_string(&entry) {
                let _ = writeln!(writer, "{}", json);
                let _ = writer.flush();
            }
        }
    }

    /// Log messages being sent to the provider.
    pub fn log_messages_sent(&self, messages: &[Message], model: Option<&str>) {
        let summaries: Vec<MessageSummary> = messages.iter().map(|m| m.into()).collect();
        self.log("messages_sent", MessagesSentEvent {
            message_count: messages.len(),
            model: model.map(|s| s.to_string()),
            messages: summaries,
        });
    }

    /// Log a response received from the provider.
    pub fn log_response_received(
        &self,
        content_len: usize,
        thinking_len: Option<usize>,
        tool_call_count: usize,
        finish_reason: &str,
    ) {
        self.log("response_received", ResponseReceivedEvent {
            content_length: content_len,
            thinking_length: thinking_len,
            tool_call_count,
            finish_reason: finish_reason.to_string(),
        });
    }

    /// Log a tool call being made.
    pub fn log_tool_call(&self, name: &str, arguments_preview: &str) {
        self.log("tool_call", ToolCallEvent {
            tool_name: name.to_string(),
            arguments_preview: arguments_preview.to_string(),
        });
    }

    /// Log tool result being stored.
    pub fn log_tool_result(&self, tool_call_id: &str, result_len: usize, is_error: bool) {
        self.log("tool_result", ToolResultEvent {
            tool_call_id: tool_call_id.to_string(),
            result_length: result_len,
            is_error,
        });
    }

    /// Log message being stored in history.
    pub fn log_message_stored(&self, role: &str, content_len: usize, has_tool_calls: bool) {
        self.log("message_stored", MessageStoredEvent {
            role: role.to_string(),
            content_length: content_len,
            has_tool_calls,
        });
    }

    /// Log a warning or issue detected.
    pub fn log_warning(&self, message: &str) {
        self.log("warning", WarningEvent {
            message: message.to_string(),
        });
    }

    /// Log iteration in the chat/agent loop.
    pub fn log_iteration(&self, iteration: usize, context: &str) {
        self.log("iteration", IterationEvent {
            iteration,
            context: context.to_string(),
        });
    }

    /// Log a user message with full content.
    pub fn log_user_message(&self, content: &str) {
        self.log("user_message", UserMessageEvent {
            content: content.to_string(),
            content_length: content.len(),
        });
    }

    /// Log an assistant response with full content and optional thinking trace.
    pub fn log_assistant_response(&self, content: &str, thinking: Option<&str>, tool_call_count: usize) {
        self.log("assistant_response", AssistantResponseEvent {
            content: content.to_string(),
            content_length: content.len(),
            thinking: thinking.map(|s| s.to_string()),
            thinking_length: thinking.map(|s| s.len()),
            tool_call_count,
        });
    }

    /// Log a tool call with full un-truncated arguments.
    pub fn log_tool_call_full(&self, id: &str, name: &str, arguments: &serde_json::Value) {
        self.log("tool_call_full", ToolCallFullEvent {
            tool_call_id: id.to_string(),
            tool_name: name.to_string(),
            arguments: arguments.clone(),
        });
    }

    /// Log a tool result with full content.
    pub fn log_tool_result_full(&self, tool_call_id: &str, tool_name: &str, content: &str, is_error: bool) {
        self.log("tool_result_full", ToolResultFullEvent {
            tool_call_id: tool_call_id.to_string(),
            tool_name: tool_name.to_string(),
            content: content.to_string(),
            content_length: content.len(),
            is_error,
        });
    }

    /// Log the start of a conversation session.
    pub fn log_conversation_start(&self, system_prompt: Option<&str>, model: Option<&str>) {
        self.log("conversation_start", ConversationStartEvent {
            system_prompt: system_prompt.map(|s| s.to_string()),
            model: model.map(|s| s.to_string()),
        });
    }
}

#[derive(Serialize)]
struct LogEntry {
    timestamp: String,
    event_type: String,
    data: serde_json::Value,
}

#[derive(Serialize)]
struct MessagesSentEvent {
    message_count: usize,
    model: Option<String>,
    messages: Vec<MessageSummary>,
}

#[derive(Serialize)]
struct MessageSummary {
    role: String,
    content_length: usize,
    content_preview: String,
    has_tool_calls: bool,
    tool_call_count: usize,
}

impl From<&Message> for MessageSummary {
    fn from(msg: &Message) -> Self {
        let content = msg.content.to_string_lossy();
        let preview = if content.len() > 100 {
            format!("{}...", &content[..100])
        } else {
            content.clone()
        };

        Self {
            role: msg.role.to_string(),
            content_length: content.len(),
            content_preview: preview,
            has_tool_calls: !msg.tool_calls.is_empty(),
            tool_call_count: msg.tool_calls.len(),
        }
    }
}

#[derive(Serialize)]
struct ResponseReceivedEvent {
    content_length: usize,
    thinking_length: Option<usize>,
    tool_call_count: usize,
    finish_reason: String,
}

#[derive(Serialize)]
struct ToolCallEvent {
    tool_name: String,
    arguments_preview: String,
}

#[derive(Serialize)]
struct ToolResultEvent {
    tool_call_id: String,
    result_length: usize,
    is_error: bool,
}

#[derive(Serialize)]
struct MessageStoredEvent {
    role: String,
    content_length: usize,
    has_tool_calls: bool,
}

#[derive(Serialize)]
struct WarningEvent {
    message: String,
}

#[derive(Serialize)]
struct IterationEvent {
    iteration: usize,
    context: String,
}

#[derive(Serialize)]
struct UserMessageEvent {
    content: String,
    content_length: usize,
}

#[derive(Serialize)]
struct AssistantResponseEvent {
    content: String,
    content_length: usize,
    thinking: Option<String>,
    thinking_length: Option<usize>,
    tool_call_count: usize,
}

#[derive(Serialize)]
struct ToolCallFullEvent {
    tool_call_id: String,
    tool_name: String,
    arguments: serde_json::Value,
}

#[derive(Serialize)]
struct ToolResultFullEvent {
    tool_call_id: String,
    tool_name: String,
    content: String,
    content_length: usize,
    is_error: bool,
}

#[derive(Serialize)]
struct ConversationStartEvent {
    system_prompt: Option<String>,
    model: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;
    use tempfile::NamedTempFile;

    #[test]
    fn test_debug_logger_creates_file() {
        let temp = NamedTempFile::new().unwrap();
        let path = temp.path();

        let logger = DebugLogger::new(path).unwrap();
        logger.log_iteration(1, "test");

        drop(logger);

        let mut content = String::new();
        let mut file = File::open(path).unwrap();
        file.read_to_string(&mut content).unwrap();

        assert!(content.contains("iteration"));
        assert!(content.contains("\"iteration\":1"));
    }

    #[test]
    fn test_message_summary() {
        let msg = Message::user("Hello, world!");
        let summary: MessageSummary = (&msg).into();

        assert_eq!(summary.role, "user");
        assert_eq!(summary.content_length, 13);
        assert_eq!(summary.content_preview, "Hello, world!");
        assert!(!summary.has_tool_calls);
    }

    #[test]
    fn test_log_user_message() {
        let temp = NamedTempFile::new().unwrap();
        let logger = DebugLogger::new(temp.path()).unwrap();
        logger.log_user_message("What files are in /tmp?");
        drop(logger);

        let mut content = String::new();
        File::open(temp.path()).unwrap().read_to_string(&mut content).unwrap();

        assert!(content.contains("\"event_type\":\"user_message\""));
        assert!(content.contains("What files are in /tmp?"));
        assert!(content.contains("\"content_length\":23"));
    }

    #[test]
    fn test_log_assistant_response_with_thinking() {
        let temp = NamedTempFile::new().unwrap();
        let logger = DebugLogger::new(temp.path()).unwrap();
        logger.log_assistant_response("I'll check that.", Some("The user wants a listing"), 1);
        drop(logger);

        let mut content = String::new();
        File::open(temp.path()).unwrap().read_to_string(&mut content).unwrap();

        assert!(content.contains("\"event_type\":\"assistant_response\""));
        assert!(content.contains("I'll check that."));
        assert!(content.contains("The user wants a listing"));
        assert!(content.contains("\"tool_call_count\":1"));
        assert!(content.contains("\"thinking_length\":24"));
    }

    #[test]
    fn test_log_assistant_response_no_thinking() {
        let temp = NamedTempFile::new().unwrap();
        let logger = DebugLogger::new(temp.path()).unwrap();
        logger.log_assistant_response("Here are the files.", None, 0);
        drop(logger);

        let mut content = String::new();
        File::open(temp.path()).unwrap().read_to_string(&mut content).unwrap();

        assert!(content.contains("\"event_type\":\"assistant_response\""));
        assert!(content.contains("\"thinking\":null"));
        assert!(content.contains("\"thinking_length\":null"));
    }

    #[test]
    fn test_log_tool_call_full() {
        let temp = NamedTempFile::new().unwrap();
        let logger = DebugLogger::new(temp.path()).unwrap();
        let args = serde_json::json!({"path": "/tmp", "recursive": true});
        logger.log_tool_call_full("tc-1", "list_directory", &args);
        drop(logger);

        let mut content = String::new();
        File::open(temp.path()).unwrap().read_to_string(&mut content).unwrap();

        assert!(content.contains("\"event_type\":\"tool_call_full\""));
        assert!(content.contains("\"tool_call_id\":\"tc-1\""));
        assert!(content.contains("\"tool_name\":\"list_directory\""));
        assert!(content.contains("\"/tmp\""));
        assert!(content.contains("\"recursive\":true"));
    }

    #[test]
    fn test_log_tool_result_full() {
        let temp = NamedTempFile::new().unwrap();
        let logger = DebugLogger::new(temp.path()).unwrap();
        logger.log_tool_result_full("tc-1", "list_directory", "file1.txt\nfile2.txt", false);
        drop(logger);

        let mut content = String::new();
        File::open(temp.path()).unwrap().read_to_string(&mut content).unwrap();

        assert!(content.contains("\"event_type\":\"tool_result_full\""));
        assert!(content.contains("\"tool_call_id\":\"tc-1\""));
        assert!(content.contains("\"tool_name\":\"list_directory\""));
        assert!(content.contains("file1.txt"));
        assert!(content.contains("\"content_length\":19"));
        assert!(content.contains("\"is_error\":false"));
    }

    #[test]
    fn test_log_conversation_start() {
        let temp = NamedTempFile::new().unwrap();
        let logger = DebugLogger::new(temp.path()).unwrap();
        logger.log_conversation_start(Some("You are helpful"), Some("gpt-4o"));
        drop(logger);

        let mut content = String::new();
        File::open(temp.path()).unwrap().read_to_string(&mut content).unwrap();

        assert!(content.contains("\"event_type\":\"conversation_start\""));
        assert!(content.contains("You are helpful"));
        assert!(content.contains("gpt-4o"));
    }
}
