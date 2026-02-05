//! Event handling for the TUI.
//!
//! Defines stream events and keyboard input processing.

use qq_core::Usage;

use qq_core::Message;

/// Events sent from the LLM streaming task to the TUI.
#[derive(Debug, Clone)]
#[allow(dead_code)] // Some variant fields are used for debugging/logging only
pub enum StreamEvent {
    /// Stream has started
    Start { model: String },
    /// Delta content for thinking/reasoning
    ThinkingDelta(String),
    /// Delta content for the main response
    ContentDelta(String),
    /// A tool call has started
    ToolCallStart { id: String, name: String },
    /// Arguments fragment for current tool call
    ToolCallDelta { arguments: String },
    /// Stream is complete with final content to add to session
    Done { usage: Option<Usage>, content: String },
    /// An error occurred
    Error { message: String },
    /// Tool execution started
    ToolExecuting { name: String, arguments: String },
    /// Tool execution completed
    ToolComplete { id: String, name: String, result_len: usize, is_error: bool },
    /// Iteration started (for multi-turn tool calls)
    IterationStart { iteration: u32 },
    /// Messages to add to session (for tool calls and results)
    SessionUpdate { messages: Vec<Message> },
    /// Byte counts for input/output
    ByteCount { input_bytes: usize, output_bytes: usize },
}

/// Input action from keyboard events
#[derive(Debug, Clone, PartialEq)]
#[allow(dead_code)] // Variants for completeness, not all used yet
pub enum InputAction {
    /// Character input
    Char(char),
    /// Backspace
    Backspace,
    /// Delete
    Delete,
    /// Move cursor left
    Left,
    /// Move cursor right
    Right,
    /// Submit input
    Submit,
    /// Navigate history up
    HistoryUp,
    /// Navigate history down
    HistoryDown,
    /// Scroll content up
    ScrollUp,
    /// Scroll content down
    ScrollDown,
    /// Page up
    PageUp,
    /// Page down
    PageDown,
    /// Scroll to top
    ScrollToTop,
    /// Scroll to bottom
    ScrollToBottom,
    /// Toggle thinking panel
    ToggleThinking,
    /// Cancel current operation
    Cancel,
    /// Quit the application
    Quit,
    /// Show help
    Help,
    /// Clear conversation
    Clear,
    /// Move cursor to start of line
    Home,
    /// Move cursor to end of line
    End,
    /// Delete word before cursor
    DeleteWord,
    /// Hide/show thinking panel entirely
    HideThinking,
    /// Move cursor forward one word
    WordForward,
    /// Move cursor backward one word
    WordBackward,
}
