//! Interface abstraction for interactive agent execution.
//!
//! This module provides the `AgentInterface` trait that abstracts user interfaces
//! from agent logic. Both TUI and readline interfaces implement this trait,
//! allowing any agent to run interactively through either interface.

mod readline;
mod tui;

// Re-export interface implementations (currently used by tests)
#[allow(unused_imports)]
pub use readline::ReadlineInterface;
#[allow(unused_imports)]
pub use tui::TuiInterface;

use async_trait::async_trait;
use anyhow::Result;

use qq_core::Usage;

/// Output events from an agent during execution.
#[derive(Debug, Clone)]
pub enum AgentOutput {
    /// Content delta from the LLM response.
    ContentDelta(String),

    /// Thinking/reasoning delta (for models that support it).
    ThinkingDelta(String),

    /// A tool execution has started.
    ToolStarted {
        id: String,
        name: String,
    },

    /// A tool execution is in progress.
    ToolExecuting {
        name: String,
    },

    /// A tool execution has completed.
    ToolCompleted {
        id: String,
        name: String,
        result_len: usize,
        is_error: bool,
    },

    /// A new iteration has started (for multi-turn tool calls).
    IterationStart {
        iteration: u32,
    },

    /// Byte count for tracking I/O.
    ByteCount {
        input_bytes: usize,
        output_bytes: usize,
    },

    /// Stream started with model info.
    StreamStart {
        model: String,
    },

    /// The response is complete.
    Done {
        content: String,
        usage: Option<Usage>,
    },

    /// An error occurred.
    Error {
        message: String,
    },

    /// Status message for the user.
    Status(String),

    /// Clear status message.
    ClearStatus,
}

/// Commands that the interface can request.
#[derive(Debug, Clone, PartialEq)]
pub enum InterfaceCommand {
    /// Quit the application.
    Quit,

    /// Clear conversation history.
    ClearHistory,

    /// Reset the entire session (clear history and stats).
    Reset,

    /// List available tools.
    ListTools,

    /// List available agents.
    ListAgents,

    /// Show help.
    Help,

    /// Show conversation history count.
    History,

    /// Delegate to an agent manually.
    Delegate {
        agent: String,
        task: String,
    },

    /// Set or show system prompt.
    System(Option<String>),

    /// Debug command.
    Debug(String),
}

/// Input from the user via the interface.
#[derive(Debug, Clone)]
pub enum UserInput {
    /// A regular message to send to the agent.
    Message(String),

    /// A command from the user.
    Command(InterfaceCommand),

    /// Cancel current operation.
    Cancel,

    /// No input (e.g., empty line).
    Empty,
}

/// Result of checking for user input.
#[derive(Debug)]
pub enum InputResult {
    /// Input is available.
    Input(UserInput),

    /// No input available yet (non-blocking check).
    Pending,

    /// Interface should quit.
    Quit,
}

/// Trait for user interfaces that can run agents interactively.
///
/// Implementations handle all UI-specific concerns (rendering, input handling)
/// while the runner handles agent execution logic.
#[async_trait]
pub trait AgentInterface: Send + Sync {
    /// Get the next user input. Returns None if the user quits.
    ///
    /// This is a blocking call that waits for user input.
    async fn next_input(&mut self) -> Result<Option<UserInput>>;

    /// Check for input without blocking (for event loops).
    ///
    /// Returns InputResult::Pending if no input is available.
    async fn poll_input(&mut self) -> Result<InputResult>;

    /// Emit an output event to the user.
    async fn emit(&mut self, output: AgentOutput) -> Result<()>;

    /// Prepare for a new response (called when user submits input).
    fn start_response(&mut self, user_input: &str);

    /// Finalize the response (called when response is complete).
    fn finish_response(&mut self);

    /// Initialize the interface (e.g., set up terminal).
    async fn initialize(&mut self) -> Result<()>;

    /// Clean up the interface (e.g., restore terminal).
    async fn cleanup(&mut self) -> Result<()>;

    /// Check if the interface should quit.
    fn should_quit(&self) -> bool;

    /// Mark the interface as needing to quit.
    fn request_quit(&mut self);

    /// Check if a streaming operation is in progress.
    fn is_streaming(&self) -> bool;

    /// Set the streaming state.
    fn set_streaming(&mut self, streaming: bool);
}

/// Parse a command from user input.
///
/// Returns Some(UserInput::Command) if the input is a command,
/// or None if it's a regular message.
pub fn parse_command(input: &str) -> Option<InterfaceCommand> {
    let trimmed = input.trim();

    if trimmed.is_empty() {
        return None;
    }

    // Check for @agent syntax: @agent_name <task>
    if trimmed.starts_with('@') {
        let parts: Vec<&str> = trimmed[1..].splitn(2, char::is_whitespace).collect();
        let agent = parts[0].to_string();
        let task = parts.get(1).map(|s| s.trim().to_string()).unwrap_or_default();

        if task.is_empty() {
            return None; // Invalid - missing task
        }

        return Some(InterfaceCommand::Delegate { agent, task });
    }

    if !trimmed.starts_with('/') {
        return None;
    }

    let parts: Vec<&str> = trimmed.splitn(2, ' ').collect();
    let cmd = parts[0].to_lowercase();
    let arg = parts.get(1).map(|s| s.to_string()).unwrap_or_default();

    match cmd.as_str() {
        "/quit" | "/exit" | "/q" => Some(InterfaceCommand::Quit),
        "/clear" | "/c" => Some(InterfaceCommand::ClearHistory),
        "/reset" => Some(InterfaceCommand::Reset),
        "/history" | "/h" => Some(InterfaceCommand::History),
        "/help" | "/?" => Some(InterfaceCommand::Help),
        "/tools" | "/t" => Some(InterfaceCommand::ListTools),
        "/agents" | "/a" => Some(InterfaceCommand::ListAgents),
        "/delegate" | "/d" => {
            // Parse: /delegate agent_name <task>
            let delegate_parts: Vec<&str> = arg.splitn(2, char::is_whitespace).collect();
            if delegate_parts.is_empty() || delegate_parts[0].is_empty() {
                None // Invalid
            } else {
                let agent = delegate_parts[0].to_string();
                let task = delegate_parts.get(1).map(|s| s.trim().to_string()).unwrap_or_default();

                if task.is_empty() {
                    None // Invalid
                } else {
                    Some(InterfaceCommand::Delegate { agent, task })
                }
            }
        }
        "/system" | "/sys" => {
            if arg.is_empty() {
                Some(InterfaceCommand::System(None))
            } else {
                Some(InterfaceCommand::System(Some(arg)))
            }
        }
        "/debug" => Some(InterfaceCommand::Debug(arg)),
        _ => None,
    }
}

/// Parse user input into a UserInput enum.
pub fn parse_user_input(input: &str) -> UserInput {
    let trimmed = input.trim();

    if trimmed.is_empty() {
        return UserInput::Empty;
    }

    if let Some(cmd) = parse_command(trimmed) {
        UserInput::Command(cmd)
    } else {
        UserInput::Message(trimmed.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_empty() {
        assert!(matches!(parse_user_input(""), UserInput::Empty));
        assert!(matches!(parse_user_input("   "), UserInput::Empty));
    }

    #[test]
    fn test_parse_message() {
        match parse_user_input("hello world") {
            UserInput::Message(msg) => assert_eq!(msg, "hello world"),
            _ => panic!("Expected Message"),
        }
    }

    #[test]
    fn test_parse_quit_command() {
        assert!(matches!(
            parse_user_input("/quit"),
            UserInput::Command(InterfaceCommand::Quit)
        ));
        assert!(matches!(
            parse_user_input("/exit"),
            UserInput::Command(InterfaceCommand::Quit)
        ));
        assert!(matches!(
            parse_user_input("/q"),
            UserInput::Command(InterfaceCommand::Quit)
        ));
    }

    #[test]
    fn test_parse_clear_command() {
        assert!(matches!(
            parse_user_input("/clear"),
            UserInput::Command(InterfaceCommand::ClearHistory)
        ));
        assert!(matches!(
            parse_user_input("/c"),
            UserInput::Command(InterfaceCommand::ClearHistory)
        ));
    }

    #[test]
    fn test_parse_agent_syntax() {
        match parse_user_input("@explore Find all rust files") {
            UserInput::Command(InterfaceCommand::Delegate { agent, task }) => {
                assert_eq!(agent, "explore");
                assert_eq!(task, "Find all rust files");
            }
            _ => panic!("Expected Delegate command"),
        }
    }

    #[test]
    fn test_parse_delegate_command() {
        match parse_user_input("/delegate researcher Look up rust async") {
            UserInput::Command(InterfaceCommand::Delegate { agent, task }) => {
                assert_eq!(agent, "researcher");
                assert_eq!(task, "Look up rust async");
            }
            _ => panic!("Expected Delegate command"),
        }
    }

    #[test]
    fn test_parse_system_command() {
        match parse_user_input("/system You are a helpful assistant") {
            UserInput::Command(InterfaceCommand::System(Some(prompt))) => {
                assert_eq!(prompt, "You are a helpful assistant");
            }
            _ => panic!("Expected System command with prompt"),
        }

        match parse_user_input("/system") {
            UserInput::Command(InterfaceCommand::System(None)) => {}
            _ => panic!("Expected System command without prompt"),
        }
    }

    #[test]
    fn test_parse_unknown_command() {
        // Unknown commands are treated as regular messages
        match parse_user_input("/unknown") {
            UserInput::Message(_) => {}
            _ => panic!("Expected Message for unknown command"),
        }
    }
}
