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

    /// Log agent start.
    pub fn log_agent_start(&self, agent_name: &str, task_preview: &str, tool_count: usize) {
        self.log("agent_start", AgentStartEvent {
            agent_name: agent_name.to_string(),
            task_preview: task_preview.to_string(),
            tool_count,
        });
    }

    /// Log agent completion.
    pub fn log_agent_end(&self, agent_name: &str, success: bool, iterations: usize, response_len: usize) {
        self.log("agent_end", AgentEndEvent {
            agent_name: agent_name.to_string(),
            success,
            iterations,
            response_length: response_len,
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
struct AgentStartEvent {
    agent_name: String,
    task_preview: String,
    tool_count: usize,
}

#[derive(Serialize)]
struct AgentEndEvent {
    agent_name: String,
    success: bool,
    iterations: usize,
    response_length: usize,
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
}
