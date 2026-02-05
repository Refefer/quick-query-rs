//! TUI Application state and main event loop.

use std::collections::VecDeque;
use std::io;
use std::panic;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use crossterm::{
    event::{
        self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEvent, KeyModifiers,
        MouseEventKind,
    },
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use futures::{stream::FuturesUnordered, StreamExt};
use ratatui::{backend::CrosstermBackend, Terminal};
use tokio::sync::{mpsc, RwLock};
use tokio_util::sync::CancellationToken;
use tui_input::Input;

use qq_core::{
    ChunkProcessor, ChunkerConfig, CompletionRequest, Message, Provider, StreamChunk, ToolCall,
    ToolExecutionResult, ToolRegistry,
};

use crate::agents::AgentExecutor;
use crate::chat::ChatSession;
use crate::config::Config as AppConfig;
use crate::debug_log::DebugLogger;
use crate::event_bus::{AgentEvent, AgentEventBus};
use crate::execution_context::ExecutionContext;
use crate::Cli;

use super::events::{InputAction, StreamEvent};
use super::layout::{LayoutConfig, PaneId};
use super::markdown::markdown_to_text;
use super::scroll::ScrollState;
use super::ui;
use super::widgets::{InputHistory, ToolNotification, ToolNotificationStatus};

/// Cached rendered content to avoid re-parsing markdown every frame
#[derive(Debug)]
struct ContentCache {
    /// Width the content was rendered at
    width: u16,
    /// Pre-rendered text
    text: ratatui::text::Text<'static>,
    /// Number of lines in the rendered content
    line_count: u16,
}

/// Ring buffer for thinking content to prevent unbounded memory growth.
///
/// Stores lines in a fixed-capacity deque, discarding oldest lines when full.
#[derive(Debug, Default)]
pub struct ThinkingBuffer {
    lines: VecDeque<String>,
    /// Partial line being accumulated (content after last newline)
    partial: String,
}

impl ThinkingBuffer {
    /// Maximum number of lines to retain
    const MAX_LINES: usize = 100;

    /// Create a new empty buffer.
    pub fn new() -> Self {
        Self {
            lines: VecDeque::with_capacity(Self::MAX_LINES),
            partial: String::new(),
        }
    }

    /// Append content to the buffer.
    pub fn push_str(&mut self, s: &str) {
        // Split input by newlines
        let mut parts = s.split('\n');

        // First part appends to partial line
        if let Some(first) = parts.next() {
            self.partial.push_str(first);
        }

        // Each subsequent part means we hit a newline
        for part in parts {
            // Complete the partial line and push it
            let complete_line = std::mem::take(&mut self.partial);
            self.lines.push_back(complete_line);

            // Evict oldest if over capacity
            if self.lines.len() > Self::MAX_LINES {
                self.lines.pop_front();
            }

            // Start new partial line
            self.partial.push_str(part);
        }
    }

    /// Clear the buffer.
    pub fn clear(&mut self) {
        self.lines.clear();
        self.partial.clear();
    }

    /// Check if the buffer is empty.
    pub fn is_empty(&self) -> bool {
        self.lines.is_empty() && self.partial.is_empty()
    }

    /// Count the number of lines (including partial).
    pub fn line_count(&self) -> usize {
        self.lines.len() + if self.partial.is_empty() { 0 } else { 1 }
    }

    /// Get the content as a string (for rendering).
    pub fn as_str(&self) -> String {
        let mut result = String::new();
        for line in &self.lines {
            result.push_str(line);
            result.push('\n');
        }
        result.push_str(&self.partial);
        result
    }
}

/// State of the LLM request/response cycle
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum StreamingState {
    /// Not currently streaming
    #[default]
    Idle,
    /// Request is being sent to the server
    Asking,
    /// Waiting for the LLM to start generating (time to first token)
    Thinking,
    /// Actively receiving streamed content
    Listening,
}

/// TUI Application state
pub struct TuiApp {
    // Display state
    pub profile: String,
    /// Primary agent name for this session (e.g., "chat", "explore")
    pub primary_agent: String,
    pub content: String,
    pub thinking_content: ThinkingBuffer,
    pub show_thinking: bool,
    pub thinking_expanded: bool,

    // Token counts
    pub prompt_tokens: u32,
    pub completion_tokens: u32,

    // Tool-call iteration tracking (for multi-turn tool calls)
    pub tool_iteration: u32,

    // Streaming state
    pub is_streaming: bool,
    pub status_message: Option<String>,

    // Scroll state (replaces individual scroll fields)
    pub scroll: ScrollState,

    // Tool notifications (displayed in thinking panel)
    pub tool_notifications: Vec<ToolNotification>,

    // Input
    pub input: Input,
    pub input_history: InputHistory,

    // UI state
    pub show_help: bool,
    pub should_quit: bool,

    // Execution context (for displaying call stack)
    pub execution_context: ExecutionContext,

    // Agent progress tracking (agent_name, iteration, max_turns)
    pub agent_progress: Option<(String, u32, u32)>,

    // Agent byte counts (cumulative input/output bytes for current agent)
    pub agent_input_bytes: usize,
    pub agent_output_bytes: usize,

    // Session-level byte counts (total across all LLM calls)
    pub session_input_bytes: usize,
    pub session_output_bytes: usize,

    // Current streaming state
    pub streaming_state: StreamingState,

    // Markdown rendering cache (avoids re-parsing every frame)
    content_cache: Option<ContentCache>,
    content_dirty: bool,
}

impl Default for TuiApp {
    fn default() -> Self {
        Self::new("auto", "chat", ExecutionContext::new())
    }
}

impl TuiApp {
    pub fn new(profile: &str, primary_agent: &str, execution_context: ExecutionContext) -> Self {
        Self {
            profile: profile.to_string(),
            primary_agent: primary_agent.to_string(),
            content: String::new(),
            thinking_content: ThinkingBuffer::new(),
            show_thinking: true,
            thinking_expanded: false,
            prompt_tokens: 0,
            completion_tokens: 0,
            tool_iteration: 0,
            is_streaming: false,
            status_message: None,
            scroll: ScrollState::default(),
            tool_notifications: Vec::new(),
            input: Input::default(),
            input_history: InputHistory::load(),
            show_help: false,
            should_quit: false,
            execution_context,
            agent_progress: None,
            agent_input_bytes: 0,
            agent_output_bytes: 0,
            session_input_bytes: 0,
            session_output_bytes: 0,
            streaming_state: StreamingState::Idle,
            content_cache: None,
            content_dirty: true,
        }
    }

    /// Reset for a new response (preserves conversation history)
    pub fn start_response(&mut self, user_input: &str) {
        // Add separator and user message to existing content
        if !self.content.is_empty() {
            self.content.push('\n');
        }
        // Wrap user input with visual separators so it stands out
        self.content.push_str("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━\n");
        self.content.push_str("**You:** ");
        self.content.push_str(user_input);
        self.content.push_str("\n━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━\n\n");
        self.content.push_str("**Assistant:** ");

        // Invalidate content cache since content changed
        self.content_dirty = true;

        // Clear thinking for new response
        self.thinking_content.clear();
        self.thinking_expanded = false;
        self.is_streaming = true;
        self.streaming_state = StreamingState::Asking;
        self.tool_notifications.clear();
        self.scroll.enable_auto_scroll();
        self.status_message = None;
        self.agent_progress = None;
        self.agent_input_bytes = 0;
        self.agent_output_bytes = 0;
    }

    /// Handle a stream event
    pub fn handle_stream_event(&mut self, event: StreamEvent) {
        match event {
            StreamEvent::Start { model: _ } => {
                // Connection established, waiting for first token
                self.streaming_state = StreamingState::Thinking;
            }
            StreamEvent::ThinkingDelta(delta) => {
                self.streaming_state = StreamingState::Listening;
                self.thinking_content.push_str(&delta);
            }
            StreamEvent::ContentDelta(delta) => {
                self.streaming_state = StreamingState::Listening;
                self.content.push_str(&delta);
                // Invalidate content cache since content changed
                self.content_dirty = true;
                // Auto-scroll if enabled (handled by ScrollState)
            }
            StreamEvent::ToolCallStart { id: _, name } => {
                self.tool_notifications.push(ToolNotification::new(
                    name.clone(),
                    ToolNotificationStatus::Started,
                ));
                self.status_message = Some(format!("Tool: {}", name));
            }
            StreamEvent::ToolCallDelta { arguments: _ } => {
                // We don't update args preview in real-time to avoid noise
            }
            StreamEvent::Done { usage, content: _ } => {
                self.is_streaming = false;
                self.streaming_state = StreamingState::Idle;
                if let Some(u) = usage {
                    self.prompt_tokens = u.prompt_tokens;
                    self.completion_tokens = u.completion_tokens;
                }
                self.status_message = None;
            }
            StreamEvent::SessionUpdate { messages: _ } => {
                // Session updates are handled in the main loop
            }
            StreamEvent::Error { message } => {
                self.is_streaming = false;
                self.streaming_state = StreamingState::Idle;
                self.status_message = Some(format!("Error: {}", message));
            }
            StreamEvent::ToolExecuting { name } => {
                // Mark tool as executing
                if let Some(notif) = self
                    .tool_notifications
                    .iter_mut()
                    .find(|n| n.tool_name == name)
                {
                    notif.status = ToolNotificationStatus::Executing;
                }
                self.status_message = Some(format!("Running: {}", name));
            }
            StreamEvent::ToolComplete {
                id: _,
                name,
                result_len,
                is_error,
            } => {
                if let Some(notif) = self
                    .tool_notifications
                    .iter_mut()
                    .find(|n| n.tool_name == name)
                {
                    notif.status = if is_error {
                        ToolNotificationStatus::Error
                    } else {
                        ToolNotificationStatus::Completed
                    };
                    notif.preview = format!("{} bytes", result_len);
                }
            }
            StreamEvent::IterationStart { iteration } => {
                self.tool_iteration = iteration;
                self.streaming_state = StreamingState::Asking;
            }
            StreamEvent::ByteCount {
                input_bytes,
                output_bytes,
            } => {
                // Accumulate byte counts from main streaming
                self.session_input_bytes += input_bytes;
                self.session_output_bytes += output_bytes;
            }
        }
    }

    /// Handle an agent event from the event bus
    pub fn handle_agent_event(&mut self, event: AgentEvent) {
        match event {
            AgentEvent::IterationStart {
                agent_name,
                iteration,
                max_turns,
            } => {
                self.agent_progress = Some((agent_name, iteration, max_turns));
                self.streaming_state = StreamingState::Asking;
            }
            AgentEvent::ThinkingDelta {
                agent_name: _,
                content,
            } => {
                self.streaming_state = StreamingState::Listening;
                self.thinking_content.push_str(&content);
            }
            AgentEvent::ToolStart {
                agent_name: _,
                tool_name,
            } => {
                self.streaming_state = StreamingState::Listening;
                self.tool_notifications.push(ToolNotification::new(
                    tool_name.clone(),
                    ToolNotificationStatus::Executing,
                ));
                self.status_message = Some(format!("Agent tool: {}", tool_name));
            }
            AgentEvent::ToolComplete {
                agent_name: _,
                tool_name,
                is_error,
            } => {
                if let Some(notif) = self
                    .tool_notifications
                    .iter_mut()
                    .find(|n| n.tool_name == tool_name)
                {
                    notif.status = if is_error {
                        ToolNotificationStatus::Error
                    } else {
                        ToolNotificationStatus::Completed
                    };
                }
            }
            AgentEvent::UsageUpdate { agent_name: _, usage } => {
                // Accumulate tokens from agent calls
                self.prompt_tokens += usage.prompt_tokens;
                self.completion_tokens += usage.completion_tokens;
            }
            AgentEvent::ByteCount {
                agent_name: _,
                input_bytes,
                output_bytes,
            } => {
                // Accumulate byte counts from agent calls (both per-agent and session)
                self.agent_input_bytes += input_bytes;
                self.agent_output_bytes += output_bytes;
                self.session_input_bytes += input_bytes;
                self.session_output_bytes += output_bytes;
            }
            AgentEvent::UserNotification {
                agent_name,
                message,
            } => {
                // Append notification to content area with visual distinction
                self.content.push_str(&format!("\n**{}**: {}\n", agent_name, message));
                self.content_dirty = true;
            }
            AgentEvent::ContinuationStarted {
                agent_name,
                continuation_number,
                max_continuations,
            } => {
                // Append continuation notice to content area
                self.content.push_str(&format!(
                    "\n**{}**: Continuing execution ({}/{})\n",
                    agent_name, continuation_number, max_continuations
                ));
                self.content_dirty = true;
            }
        }
    }

    /// Clear agent progress when agent completes
    pub fn clear_agent_progress(&mut self) {
        self.agent_progress = None;
    }

    /// Handle input action
    pub fn handle_input_action(&mut self, action: InputAction) {
        match action {
            InputAction::Char(c) => {
                // Insert character at cursor position
                let cursor = self.input.visual_cursor();
                let value = self.input.value().to_string();
                let mut chars: Vec<char> = value.chars().collect();
                chars.insert(cursor, c);
                let new_value: String = chars.into_iter().collect();
                self.input = Input::new(new_value).with_cursor(cursor + 1);
                self.input_history.reset();
            }
            InputAction::Backspace => {
                let cursor = self.input.visual_cursor();
                if cursor > 0 {
                    let value = self.input.value().to_string();
                    let mut chars: Vec<char> = value.chars().collect();
                    chars.remove(cursor - 1);
                    let new_value: String = chars.into_iter().collect();
                    self.input = Input::new(new_value).with_cursor(cursor - 1);
                }
            }
            InputAction::Delete => {
                let cursor = self.input.visual_cursor();
                let value = self.input.value().to_string();
                let chars: Vec<char> = value.chars().collect();
                if cursor < chars.len() {
                    let mut chars = chars;
                    chars.remove(cursor);
                    let new_value: String = chars.into_iter().collect();
                    self.input = Input::new(new_value).with_cursor(cursor);
                }
            }
            InputAction::Left => {
                let cursor = self.input.visual_cursor();
                if cursor > 0 {
                    let value = self.input.value().to_string();
                    self.input = Input::new(value).with_cursor(cursor - 1);
                }
            }
            InputAction::Right => {
                let cursor = self.input.visual_cursor();
                let value = self.input.value().to_string();
                let len = value.chars().count();
                if cursor < len {
                    self.input = Input::new(value).with_cursor(cursor + 1);
                }
            }
            InputAction::Home => {
                let value = self.input.value().to_string();
                self.input = Input::new(value).with_cursor(0);
            }
            InputAction::End => {
                let value = self.input.value().to_string();
                let len = value.chars().count();
                self.input = Input::new(value).with_cursor(len);
            }
            InputAction::WordForward => {
                let value = self.input.value().to_string();
                let cursor = self.input.visual_cursor();
                let chars: Vec<char> = value.chars().collect();
                let len = chars.len();

                if cursor < len {
                    let mut new_cursor = cursor;
                    // Skip whitespace
                    while new_cursor < len && chars[new_cursor].is_whitespace() {
                        new_cursor += 1;
                    }
                    // Skip word
                    while new_cursor < len && !chars[new_cursor].is_whitespace() {
                        new_cursor += 1;
                    }
                    self.input = Input::new(value).with_cursor(new_cursor);
                }
            }
            InputAction::WordBackward => {
                let value = self.input.value().to_string();
                let cursor = self.input.visual_cursor();
                let chars: Vec<char> = value.chars().collect();

                if cursor > 0 {
                    let mut new_cursor = cursor;
                    // Skip whitespace going backward
                    while new_cursor > 0 && chars[new_cursor - 1].is_whitespace() {
                        new_cursor -= 1;
                    }
                    // Skip word going backward
                    while new_cursor > 0 && !chars[new_cursor - 1].is_whitespace() {
                        new_cursor -= 1;
                    }
                    self.input = Input::new(value).with_cursor(new_cursor);
                }
            }
            InputAction::HistoryUp => {
                if let Some(entry) = self.input_history.navigate_up(self.input.value()) {
                    self.input = Input::new(entry.to_string());
                }
            }
            InputAction::HistoryDown => {
                if let Some(entry) = self.input_history.navigate_down(self.input.value()) {
                    self.input = Input::new(entry.to_string());
                }
            }
            InputAction::ScrollUp => {
                self.scroll.scroll_up(3);
            }
            InputAction::ScrollDown => {
                self.scroll.scroll_down(3);
            }
            InputAction::PageUp => {
                self.scroll.page_up();
            }
            InputAction::PageDown => {
                self.scroll.page_down();
            }
            InputAction::ScrollToTop => {
                self.scroll.scroll_to_top();
            }
            InputAction::ScrollToBottom => {
                self.scroll.scroll_to_bottom();
            }
            InputAction::ToggleThinking => {
                if self.show_thinking && !self.thinking_content.is_empty() {
                    self.thinking_expanded = !self.thinking_expanded;
                }
            }
            InputAction::HideThinking => {
                self.show_thinking = !self.show_thinking;
            }
            InputAction::Help => {
                self.show_help = !self.show_help;
            }
            InputAction::Quit => {
                self.should_quit = true;
            }
            InputAction::DeleteWord => {
                // Delete word before cursor
                let value = self.input.value().to_string();
                let cursor = self.input.visual_cursor();
                if cursor > 0 {
                    // Find start of word
                    let before: String = value.chars().take(cursor).collect();
                    let trimmed = before.trim_end();
                    let new_cursor = if trimmed.is_empty() {
                        0
                    } else {
                        trimmed
                            .rfind(|c: char| c.is_whitespace())
                            .map(|i| i + 1)
                            .unwrap_or(0)
                    };
                    let after: String = value.chars().skip(cursor).collect();
                    let new_value: String =
                        value.chars().take(new_cursor).chain(after.chars()).collect();
                    self.input = Input::new(new_value).with_cursor(new_cursor);
                }
            }
            _ => {}
        }
    }

    /// Update scroll state with current content and viewport dimensions.
    /// Call this BEFORE rendering to ensure scroll state is accurate.
    /// Returns a reference to the cached rendered text for use in rendering.
    pub fn update_scroll_dimensions(&mut self, viewport_height: u16, content_width: u16) {
        // Update viewport height first (this clamps offset if needed)
        self.scroll.set_viewport_height(viewport_height);

        // Re-render only if content changed or width changed
        let needs_rerender = self.content_dirty
            || self
                .content_cache
                .as_ref()
                .map(|c| c.width != content_width)
                .unwrap_or(true);

        if needs_rerender {
            let text = markdown_to_text(&self.content, Some(content_width as usize));
            let line_count = text.lines.len() as u16;
            self.content_cache = Some(ContentCache {
                width: content_width,
                text,
                line_count,
            });
            self.content_dirty = false;
        }

        // Use cached line count
        let content_height = self
            .content_cache
            .as_ref()
            .map(|c| c.line_count)
            .unwrap_or(0);

        self.scroll.set_content_height(content_height);
    }

    /// Get the cached rendered content text.
    /// Returns None if not yet rendered.
    pub fn get_cached_content(&self) -> Option<&ratatui::text::Text<'static>> {
        self.content_cache.as_ref().map(|c| &c.text)
    }

    /// Get current input value and clear
    pub fn take_input(&mut self) -> String {
        let value = self.input.value().to_string();
        self.input_history.add(value.clone());
        self.input = Input::default();
        value
    }
}

/// Set up panic hook to restore terminal on panic
fn setup_panic_hook() {
    let original_hook = panic::take_hook();
    panic::set_hook(Box::new(move |panic_info| {
        // Restore terminal
        let _ = disable_raw_mode();
        let _ = execute!(
            io::stdout(),
            LeaveAlternateScreen,
            DisableMouseCapture,
            crossterm::cursor::Show
        );
        // Call original hook
        original_hook(panic_info);
    }));
}

/// Run the TUI chat interface
pub async fn run_tui(
    cli: &Cli,
    _config: &AppConfig,
    provider: Arc<dyn Provider>,
    system_prompt: Option<String>,
    tools_registry: ToolRegistry,
    extra_params: std::collections::HashMap<String, serde_json::Value>,
    profile_name: String,
    primary_agent: String,
    agent_executor: Option<Arc<RwLock<AgentExecutor>>>,
    execution_context: ExecutionContext,
    chunker_config: ChunkerConfig,
    event_bus: Option<AgentEventBus>,
) -> Result<()> {
    // Set up panic hook
    setup_panic_hook();

    // Set up debug logger if requested (log_file takes precedence over deprecated debug_file)
    let log_path = cli.log_file.as_ref().or(cli.debug_file.as_ref());
    let debug_logger: Option<Arc<DebugLogger>> = if let Some(path) = log_path {
        match DebugLogger::new(path) {
            Ok(logger) => Some(Arc::new(logger)),
            Err(_) => None,
        }
    } else {
        None
    };

    // Initialize terminal with mouse capture enabled
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(
        stdout,
        EnterAlternateScreen,
        EnableMouseCapture,
        crossterm::cursor::Hide
    )?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    // Create chat session
    let mut session = ChatSession::new(system_prompt);

    // Create TUI app
    let mut app = TuiApp::new(&profile_name, &primary_agent, execution_context.clone());

    // Channel for stream events
    let (stream_tx, mut stream_rx) = mpsc::channel::<StreamEvent>(100);

    // Subscribe to agent event bus if available
    let mut agent_event_rx = event_bus.as_ref().map(|bus| bus.subscribe());

    // Cancellation token for stopping streams (recreated after each cancel)
    let mut cancel_token = CancellationToken::new();

    // Main event loop
    let tick_rate = Duration::from_millis(33); // ~30fps

    loop {
        // Render
        terminal.draw(|f| {
            let area = f.area();

            // Build layout config - must be identical to what ui::render expects
            let mut layout_config = LayoutConfig::new();
            let has_thinking = app.show_thinking && !app.thinking_content.is_empty();
            let thinking_lines = app.thinking_content.line_count() as u16;
            layout_config.set_thinking(has_thinking, app.thinking_expanded, thinking_lines);

            // Configure input pane based on text wrapping
            let input_lines = ui::calculate_input_lines(app.input.value(), area.width);
            layout_config.set_input_lines(input_lines);

            // Compute layout ONCE and use for both scroll dimensions and rendering
            let layout = layout_config.compute(area);
            if let Some(&content_rect) = layout.get(&PaneId::Content) {
                // Update scroll dimensions BEFORE rendering
                // Subtract 2 for borders
                let viewport_height = content_rect.height.saturating_sub(2);
                let content_width = content_rect.width.saturating_sub(2);
                app.update_scroll_dimensions(viewport_height, content_width);
            }

            ui::render(&app, f, &layout);
        })?;

        // Handle events with timeout for render ticks
        let timeout = if app.is_streaming {
            Duration::from_millis(16) // Faster updates during streaming
        } else {
            tick_rate
        };

        // Check for stream events first (non-blocking)
        while let Ok(event) = stream_rx.try_recv() {
            match &event {
                StreamEvent::Done { usage: _, content } => {
                    // Add assistant response to session
                    if !content.is_empty() {
                        session.add_assistant_message(content);
                    }
                    // Clear agent progress when main completion is done
                    app.clear_agent_progress();
                }
                StreamEvent::SessionUpdate { messages } => {
                    // Add messages to session (tool calls and results)
                    for msg in messages {
                        if msg.role == qq_core::Role::Assistant && !msg.tool_calls.is_empty() {
                            session.add_assistant_with_tools(msg.clone());
                        } else if msg.role == qq_core::Role::Tool && msg.tool_call_id.is_some() {
                            session.add_tool_result(
                                msg.tool_call_id.as_ref().unwrap(),
                                &msg.content.to_string_lossy(),
                            );
                        }
                    }
                }
                _ => {}
            }
            app.handle_stream_event(event);
        }

        // Check for agent events (non-blocking)
        if let Some(ref mut rx) = agent_event_rx {
            while let Ok(event) = rx.try_recv() {
                app.handle_agent_event(event);
            }
        }

        // Poll for events (keyboard and mouse)
        if event::poll(timeout)? {
            match event::read()? {
                Event::Key(key) => {
                    // Handle help overlay dismissal
                    if app.show_help {
                        app.show_help = false;
                        continue;
                    }

                    let action = key_to_action(key, app.is_streaming);

                    match action {
                        Some(InputAction::Quit) => {
                            app.should_quit = true;
                        }
                        Some(InputAction::Cancel) => {
                            if app.is_streaming {
                                cancel_token.cancel();
                                // Create a fresh token for future requests
                                cancel_token = CancellationToken::new();
                                app.is_streaming = false;
                                app.streaming_state = StreamingState::Idle;
                                app.status_message = Some("Cancelled".to_string());
                            }
                        }
                        Some(InputAction::Submit) => {
                            if !app.is_streaming {
                                let input = app.take_input();
                                if !input.is_empty() {
                                    // Handle commands
                                    if let Some(cmd) = parse_tui_command(&input) {
                                        match cmd {
                                            TuiCommand::Quit => {
                                                app.should_quit = true;
                                            }
                                            TuiCommand::Clear => {
                                                session.clear();
                                                app.content.clear();
                                                app.thinking_content.clear();
                                                app.tool_notifications.clear();
                                                app.content_dirty = true;
                                                app.content_cache = None;
                                                app.status_message = Some("Cleared".to_string());
                                            }
                                            TuiCommand::Reset => {
                                                session.clear();
                                                app.content.clear();
                                                app.thinking_content.clear();
                                                app.tool_notifications.clear();
                                                app.content_dirty = true;
                                                app.content_cache = None;
                                                app.prompt_tokens = 0;
                                                app.completion_tokens = 0;
                                                app.status_message = Some("Session reset".to_string());
                                            }
                                            TuiCommand::Help => {
                                                app.show_help = true;
                                            }
                                            TuiCommand::Tools => {
                                                app.content = format_tools_list(&tools_registry);
                                                app.content_dirty = true;
                                            }
                                            TuiCommand::Agents => {
                                                if let Some(ref exec) = agent_executor {
                                                    let exec = exec.read().await;
                                                    app.content = format_agents_list(&exec);
                                                } else {
                                                    app.content = "Agents not configured.".to_string();
                                                }
                                                app.content_dirty = true;
                                            }
                                            TuiCommand::History => {
                                                app.content = format!(
                                                    "Messages in conversation: {}",
                                                    session.message_count()
                                                );
                                                app.content_dirty = true;
                                            }
                                        }
                                    } else {
                                        // Regular message - start completion
                                        session.add_user_message(&input);
                                        app.start_response(&input);

                                        // Clone things for the spawned task
                                        let provider = Arc::clone(&provider);
                                        let tools = tools_registry.clone();
                                        let params = extra_params.clone();
                                        let model = provider.default_model().map(|s| s.to_string());
                                        let tx = stream_tx.clone();
                                        let messages = session.build_messages();
                                        let debug = debug_logger.clone();
                                        let temp = cli.temperature;
                                        let max_tok = cli.max_tokens;
                                        let exec_ctx = execution_context.clone();
                                        let chunker_cfg = chunker_config.clone();
                                        let original_query = input.clone();
                                        let no_stream = cli.no_stream;

                                        // Clone cancel token for the spawned task
                                        let cancel = cancel_token.clone();

                                        // Spawn streaming task
                                        tokio::spawn(async move {
                                            run_streaming_completion(
                                                provider,
                                                tools,
                                                params,
                                                model,
                                                messages,
                                                tx,
                                                debug,
                                                temp,
                                                max_tok,
                                                exec_ctx,
                                                chunker_cfg,
                                                original_query,
                                                no_stream,
                                                cancel,
                                            )
                                            .await;
                                        });
                                    }
                                }
                            }
                        }
                        Some(action) => {
                            app.handle_input_action(action);
                        }
                        None => {}
                    }
                }
                Event::Mouse(mouse) => {
                    // Handle mouse scroll events
                    match mouse.kind {
                        MouseEventKind::ScrollUp => {
                            app.scroll.scroll_up(3);
                        }
                        MouseEventKind::ScrollDown => {
                            app.scroll.scroll_down(3);
                        }
                        _ => {
                            // Ignore other mouse events
                        }
                    }
                }
                Event::Resize(_, _) => {
                    // Terminal resized - next render will update dimensions
                }
                _ => {}
            }
        }

        if app.should_quit {
            break;
        }
    }

    // Save input history before exiting
    app.input_history.save();

    // Restore terminal
    disable_raw_mode()?;
    execute!(
        terminal.backend_mut(),
        LeaveAlternateScreen,
        DisableMouseCapture,
        crossterm::cursor::Show
    )?;
    terminal.show_cursor()?;

    // Print the conversation to stdout so it's preserved after exit
    if !app.content.is_empty() {
        print_conversation(&app.content);
    }

    Ok(())
}

/// Convert key event to input action
fn key_to_action(key: KeyEvent, is_streaming: bool) -> Option<InputAction> {
    match (key.code, key.modifiers) {
        // Quit
        (KeyCode::Char('d'), KeyModifiers::CONTROL) => Some(InputAction::Quit),
        (KeyCode::Char('q'), KeyModifiers::CONTROL) => Some(InputAction::Quit),

        // Cancel
        (KeyCode::Char('c'), KeyModifiers::CONTROL) => Some(InputAction::Cancel),
        (KeyCode::Esc, _) if is_streaming => Some(InputAction::Cancel),

        // Submit
        (KeyCode::Enter, KeyModifiers::NONE) => Some(InputAction::Submit),

        // Navigation
        (KeyCode::Up, KeyModifiers::NONE) => Some(InputAction::HistoryUp),
        (KeyCode::Down, KeyModifiers::NONE) => Some(InputAction::HistoryDown),
        (KeyCode::Left, KeyModifiers::NONE) => Some(InputAction::Left),
        (KeyCode::Right, KeyModifiers::NONE) => Some(InputAction::Right),
        (KeyCode::Home, KeyModifiers::NONE) => Some(InputAction::Home),
        (KeyCode::End, KeyModifiers::NONE) => Some(InputAction::End),

        // Emacs-style navigation
        (KeyCode::Char('a'), KeyModifiers::CONTROL) => Some(InputAction::Home),
        (KeyCode::Char('e'), KeyModifiers::CONTROL) => Some(InputAction::End),

        // Word navigation
        (KeyCode::Char('f'), KeyModifiers::ALT) => Some(InputAction::WordForward),
        (KeyCode::Char('b'), KeyModifiers::ALT) => Some(InputAction::WordBackward),
        (KeyCode::Left, KeyModifiers::CONTROL) => Some(InputAction::WordBackward),
        (KeyCode::Right, KeyModifiers::CONTROL) => Some(InputAction::WordForward),

        // Scrolling
        (KeyCode::PageUp, _) => Some(InputAction::PageUp),
        (KeyCode::PageDown, _) => Some(InputAction::PageDown),
        (KeyCode::Char('b'), KeyModifiers::CONTROL) => Some(InputAction::PageUp),
        (KeyCode::Char('f'), KeyModifiers::CONTROL) => Some(InputAction::PageDown),
        (KeyCode::Home, KeyModifiers::CONTROL) => Some(InputAction::ScrollToTop),
        (KeyCode::End, KeyModifiers::CONTROL) => Some(InputAction::ScrollToBottom),

        // Editing
        (KeyCode::Backspace, KeyModifiers::NONE) => Some(InputAction::Backspace),
        (KeyCode::Delete, KeyModifiers::NONE) => Some(InputAction::Delete),
        (KeyCode::Char('w'), KeyModifiers::CONTROL) => Some(InputAction::DeleteWord),

        // Toggle thinking (Ctrl+T)
        (KeyCode::Char('t'), KeyModifiers::CONTROL) => Some(InputAction::ToggleThinking),

        // Hide/show thinking panel (Ctrl+H)
        (KeyCode::Char('h'), KeyModifiers::CONTROL) => Some(InputAction::HideThinking),

        // Characters (only when not streaming)
        (KeyCode::Char(c), KeyModifiers::NONE) if !is_streaming => Some(InputAction::Char(c)),
        (KeyCode::Char(c), KeyModifiers::SHIFT) if !is_streaming => Some(InputAction::Char(c)),

        _ => None,
    }
}

/// TUI commands
enum TuiCommand {
    Quit,
    Clear,
    Reset,
    Help,
    Tools,
    Agents,
    History,
}

/// Parse TUI commands
fn parse_tui_command(input: &str) -> Option<TuiCommand> {
    let trimmed = input.trim();
    match trimmed {
        "/quit" | "/exit" | "/q" => Some(TuiCommand::Quit),
        "/clear" | "/c" => Some(TuiCommand::Clear),
        "/reset" => Some(TuiCommand::Reset),
        "/help" | "/?" => Some(TuiCommand::Help),
        "/tools" | "/t" => Some(TuiCommand::Tools),
        "/agents" | "/a" => Some(TuiCommand::Agents),
        "/history" | "/h" => Some(TuiCommand::History),
        _ => None,
    }
}

/// Format tools list for display
fn format_tools_list(registry: &ToolRegistry) -> String {
    let mut output = String::from("Available tools:\n\n");
    for def in registry.definitions() {
        output.push_str(&format!("  {} - {}\n", def.name, def.description));
    }
    output
}

/// Format agents list for display
fn format_agents_list(executor: &AgentExecutor) -> String {
    let agents = executor.list_agents();
    if agents.is_empty() {
        return "No agents available.".to_string();
    }

    let mut output = String::from("Available agents:\n\n");
    for agent in agents {
        let type_marker = if agent.is_internal {
            "(built-in)"
        } else {
            "(external)"
        };
        output.push_str(&format!(
            "  Agent[{}] {} - {}\n",
            agent.name, type_marker, agent.description
        ));
    }
    output
}

/// Print the conversation content to stdout using the shared markdown renderer.
fn print_conversation(content: &str) {
    crate::markdown::render_markdown(content);
}

/// Run streaming completion in a separate task
async fn run_streaming_completion(
    provider: Arc<dyn Provider>,
    tools_registry: ToolRegistry,
    extra_params: std::collections::HashMap<String, serde_json::Value>,
    model: Option<String>,
    base_messages: Vec<Message>,
    tx: mpsc::Sender<StreamEvent>,
    debug_logger: Option<Arc<DebugLogger>>,
    temperature: Option<f32>,
    max_tokens: Option<u32>,
    execution_context: ExecutionContext,
    chunker_config: ChunkerConfig,
    original_query: String,
    no_stream: bool,
    cancel_token: CancellationToken,
) {
    // Create chunk processor for large tool outputs
    let chunk_processor = ChunkProcessor::new(Arc::clone(&provider), chunker_config);
    let max_turns = 100u32;

    // Keep iteration messages separate to avoid cloning all messages each iteration
    // On each iteration, we build request from base_messages + iteration_messages
    let mut iteration_messages: Vec<Message> = Vec::new();

    for iteration in 0..max_turns {
        // Check for cancellation at the start of each iteration
        if cancel_token.is_cancelled() {
            let _ = tx
                .send(StreamEvent::Error {
                    message: "Cancelled".to_string(),
                })
                .await;
            return;
        }

        let _ = tx
            .send(StreamEvent::IterationStart {
                iteration: iteration + 1,
            })
            .await;

        if let Some(ref logger) = debug_logger {
            logger.log_iteration(iteration as usize, "tui_completion");
        }

        // Build request messages: base + iteration (avoids full clone each iteration)
        let request_messages: Vec<Message> = base_messages
            .iter()
            .chain(iteration_messages.iter())
            .cloned()
            .collect();

        // Calculate input bytes from messages
        let input_bytes = serde_json::to_string(&request_messages)
            .map(|s| s.len())
            .unwrap_or(0);

        let mut request = CompletionRequest::new(request_messages);

        if let Some(ref m) = model {
            request = request.with_model(m);
        }

        if let Some(temp) = temperature {
            request = request.with_temperature(temp);
        }

        if let Some(max_tok) = max_tokens {
            request = request.with_max_tokens(max_tok);
        }

        if !extra_params.is_empty() {
            request = request.with_extra(extra_params.clone());
        }

        request = request.with_tools(tools_registry.definitions());

        // Non-streaming mode: use complete() instead of stream()
        if no_stream {
            let response = match provider.complete(request).await {
                Ok(r) => r,
                Err(e) => {
                    execution_context.reset().await;
                    let _ = tx
                        .send(StreamEvent::Error {
                            message: e.to_string(),
                        })
                        .await;
                    return;
                }
            };

            let content = response.message.content.to_string_lossy();
            let tool_calls = response.message.tool_calls.clone();
            let output_bytes = content.len();

            // Send byte counts for this iteration
            let _ = tx
                .send(StreamEvent::ByteCount {
                    input_bytes,
                    output_bytes,
                })
                .await;

            // Send content as a single delta (for TUI display)
            if !content.is_empty() {
                let _ = tx.send(StreamEvent::ContentDelta(content.clone())).await;
            }

            // Handle tool calls if any
            if !tool_calls.is_empty() {
                // Send tool call events
                for tool_call in &tool_calls {
                    let _ = tx
                        .send(StreamEvent::ToolCallStart {
                            id: tool_call.id.clone(),
                            name: tool_call.name.clone(),
                        })
                        .await;
                }

                // Add assistant message with tool calls
                let assistant_msg =
                    Message::assistant_with_tool_calls(content.as_str(), tool_calls.clone());
                iteration_messages.push(assistant_msg.clone());

                // Send session update for the assistant message with tool calls
                let _ = tx
                    .send(StreamEvent::SessionUpdate {
                        messages: vec![assistant_msg],
                    })
                    .await;

                // Execute tools
                for tool_call in &tool_calls {
                    execution_context.push_tool(&tool_call.name).await;
                    let _ = tx
                        .send(StreamEvent::ToolExecuting {
                            name: tool_call.name.clone(),
                        })
                        .await;
                }

                // Create futures for each tool execution
                let mut futures: FuturesUnordered<_> = tool_calls
                    .iter()
                    .map(|tool_call| {
                        let registry = tools_registry.clone();
                        let tool_call_id = tool_call.id.clone();
                        let tool_name = tool_call.name.clone();
                        let arguments = tool_call.arguments.clone();

                        async move {
                            let result = if let Some(tool) = registry.get(&tool_name) {
                                match tool.execute(arguments).await {
                                    Ok(output) => ToolExecutionResult {
                                        tool_call_id: tool_call_id.clone(),
                                        content: if output.is_error {
                                            format!("Error: {}", output.content)
                                        } else {
                                            output.content
                                        },
                                        is_error: output.is_error,
                                    },
                                    Err(e) => ToolExecutionResult {
                                        tool_call_id: tool_call_id.clone(),
                                        content: format!("Error executing tool: {}", e),
                                        is_error: true,
                                    },
                                }
                            } else {
                                ToolExecutionResult {
                                    tool_call_id: tool_call_id.clone(),
                                    content: format!("Error: Unknown tool '{}'", tool_name),
                                    is_error: true,
                                }
                            };
                            (tool_name, result)
                        }
                    })
                    .collect();

                // Stream results as they complete
                let mut tool_result_messages = Vec::new();
                while let Some((tool_name, mut result)) = futures.next().await {
                    // Apply chunking if needed
                    if !result.is_error && chunk_processor.should_chunk(&result.content) {
                        if let Ok(processed) = chunk_processor
                            .process_large_content(&result.content, Some(&original_query))
                            .await
                        {
                            result.content = processed;
                        }
                    }

                    // Pop the tool context
                    execution_context.pop().await;

                    // Send completion event immediately
                    let _ = tx
                        .send(StreamEvent::ToolComplete {
                            id: result.tool_call_id.clone(),
                            name: tool_name,
                            result_len: result.content.len(),
                            is_error: result.is_error,
                        })
                        .await;

                    let tool_msg = Message::tool_result(&result.tool_call_id, &result.content);
                    iteration_messages.push(tool_msg.clone());
                    tool_result_messages.push(tool_msg);
                }

                // Send session update for tool results
                let _ = tx
                    .send(StreamEvent::SessionUpdate {
                        messages: tool_result_messages,
                    })
                    .await;

                // Continue to next iteration
                continue;
            }

            // No tool calls, we're done
            execution_context.reset().await;
            let _ = tx
                .send(StreamEvent::Done {
                    usage: Some(response.usage),
                    content: content.clone(),
                })
                .await;
            return;
        }

        // Streaming mode: use stream()
        let mut stream = match provider.stream(request).await {
            Ok(s) => s,
            Err(e) => {
                execution_context.reset().await;
                let _ = tx
                    .send(StreamEvent::Error {
                        message: e.to_string(),
                    })
                    .await;
                return;
            }
        };

        let mut content = String::new();
        let mut tool_calls: Vec<ToolCall> = Vec::new();
        let mut current_tool_call: Option<(String, String, String)> = None;
        let mut output_bytes: usize = 0;
        let mut cancelled = false;

        loop {
            tokio::select! {
                biased;

                _ = cancel_token.cancelled() => {
                    // Cancellation requested - exit the streaming loop
                    cancelled = true;
                    break;
                }

                chunk = stream.next() => {
                    match chunk {
                        Some(Ok(StreamChunk::Start { model })) => {
                            let _ = tx.send(StreamEvent::Start { model }).await;
                        }
                        Some(Ok(StreamChunk::ThinkingDelta { content: delta })) => {
                            output_bytes += delta.len();
                            let _ = tx.send(StreamEvent::ThinkingDelta(delta)).await;
                        }
                        Some(Ok(StreamChunk::Delta { content: delta })) => {
                            output_bytes += delta.len();
                            content.push_str(&delta);
                            let _ = tx.send(StreamEvent::ContentDelta(delta)).await;
                        }
                        Some(Ok(StreamChunk::ToolCallStart { id, name })) => {
                            // Finish pending tool call
                            if let Some((tc_id, tc_name, tc_args)) = current_tool_call.take() {
                                let args: serde_json::Value =
                                    serde_json::from_str(&tc_args).unwrap_or(serde_json::Value::Null);
                                tool_calls.push(ToolCall::new(tc_id, tc_name, args));
                            }
                            current_tool_call = Some((id.clone(), name.clone(), String::new()));
                            let _ = tx.send(StreamEvent::ToolCallStart { id, name }).await;
                        }
                        Some(Ok(StreamChunk::ToolCallDelta { arguments })) => {
                            output_bytes += arguments.len();
                            if let Some((_, _, ref mut args)) = current_tool_call {
                                args.push_str(&arguments);
                            }
                            let _ = tx.send(StreamEvent::ToolCallDelta { arguments }).await;
                        }
                        Some(Ok(StreamChunk::Done { usage })) => {
                            // Finish pending tool call
                            if let Some((tc_id, tc_name, tc_args)) = current_tool_call.take() {
                                let args: serde_json::Value =
                                    serde_json::from_str(&tc_args).unwrap_or(serde_json::Value::Null);
                                tool_calls.push(ToolCall::new(tc_id, tc_name, args));
                            }

                            // Send byte counts for this iteration
                            let _ = tx
                                .send(StreamEvent::ByteCount {
                                    input_bytes,
                                    output_bytes,
                                })
                                .await;

                            if tool_calls.is_empty() {
                                // No tool calls - we're done, send final content for session
                                execution_context.reset().await;
                                let _ = tx
                                    .send(StreamEvent::Done {
                                        usage,
                                        content: content.clone(),
                                    })
                                    .await;
                                return;
                            }
                            // Break to handle tool calls
                            break;
                        }
                        Some(Ok(StreamChunk::Error { message })) => {
                            execution_context.reset().await;
                            let _ = tx.send(StreamEvent::Error { message }).await;
                            return;
                        }
                        Some(Err(e)) => {
                            execution_context.reset().await;
                            let _ = tx
                                .send(StreamEvent::Error {
                                    message: e.to_string(),
                                })
                                .await;
                            return;
                        }
                        None => {
                            // Stream ended unexpectedly
                            break;
                        }
                    }
                }
            }
        }

        // If cancelled, send error and exit
        if cancelled {
            execution_context.reset().await;
            let _ = tx
                .send(StreamEvent::Error {
                    message: "Cancelled".to_string(),
                })
                .await;
            return;
        }

        // Handle tool calls
        if !tool_calls.is_empty() {
            // Add assistant message with tool calls
            let assistant_msg =
                Message::assistant_with_tool_calls(content.as_str(), tool_calls.clone());
            iteration_messages.push(assistant_msg.clone());

            // Send session update for the assistant message with tool calls
            let _ = tx
                .send(StreamEvent::SessionUpdate {
                    messages: vec![assistant_msg],
                })
                .await;

            // Execute tools with streaming - send events as each tool completes
            for tool_call in &tool_calls {
                execution_context.push_tool(&tool_call.name).await;
                let _ = tx
                    .send(StreamEvent::ToolExecuting {
                        name: tool_call.name.clone(),
                    })
                    .await;
            }

            // Create futures for each tool execution
            let mut futures: FuturesUnordered<_> = tool_calls
                .iter()
                .map(|tool_call| {
                    let registry = tools_registry.clone();
                    let tool_call_id = tool_call.id.clone();
                    let tool_name = tool_call.name.clone();
                    let arguments = tool_call.arguments.clone();

                    async move {
                        let result = if let Some(tool) = registry.get(&tool_name) {
                            match tool.execute(arguments).await {
                                Ok(output) => ToolExecutionResult {
                                    tool_call_id: tool_call_id.clone(),
                                    content: if output.is_error {
                                        format!("Error: {}", output.content)
                                    } else {
                                        output.content
                                    },
                                    is_error: output.is_error,
                                },
                                Err(e) => ToolExecutionResult {
                                    tool_call_id: tool_call_id.clone(),
                                    content: format!("Error executing tool: {}", e),
                                    is_error: true,
                                },
                            }
                        } else {
                            ToolExecutionResult {
                                tool_call_id: tool_call_id.clone(),
                                content: format!("Error: Unknown tool '{}'", tool_name),
                                is_error: true,
                            }
                        };
                        (tool_name, result)
                    }
                })
                .collect();

            // Stream results as they complete
            let mut tool_result_messages = Vec::new();
            while let Some((tool_name, mut result)) = futures.next().await {
                // Apply chunking if needed
                if !result.is_error && chunk_processor.should_chunk(&result.content) {
                    if let Ok(processed) = chunk_processor
                        .process_large_content(&result.content, Some(&original_query))
                        .await
                    {
                        result.content = processed;
                    }
                }

                // Pop the tool context
                execution_context.pop().await;

                // Send completion event immediately
                let _ = tx
                    .send(StreamEvent::ToolComplete {
                        id: result.tool_call_id.clone(),
                        name: tool_name,
                        result_len: result.content.len(),
                        is_error: result.is_error,
                    })
                    .await;

                let tool_msg = Message::tool_result(&result.tool_call_id, &result.content);
                iteration_messages.push(tool_msg.clone());
                tool_result_messages.push(tool_msg);
            }

            // Send session update for tool results
            let _ = tx
                .send(StreamEvent::SessionUpdate {
                    messages: tool_result_messages,
                })
                .await;

            // Continue to next iteration
            continue;
        }

        // No tool calls, we're done - send byte counts and final content
        let _ = tx
            .send(StreamEvent::ByteCount {
                input_bytes,
                output_bytes,
            })
            .await;
        execution_context.reset().await;
        let _ = tx
            .send(StreamEvent::Done {
                usage: None,
                content: content.clone(),
            })
            .await;
        return;
    }

    // Max iterations reached
    execution_context.reset().await;
    let _ = tx
        .send(StreamEvent::Error {
            message: format!("Max iterations ({}) reached", max_turns),
        })
        .await;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_thinking_buffer_empty() {
        let buf = ThinkingBuffer::new();
        assert!(buf.is_empty());
        assert_eq!(buf.line_count(), 0);
        assert_eq!(buf.as_str(), "");
    }

    #[test]
    fn test_thinking_buffer_single_line() {
        let mut buf = ThinkingBuffer::new();
        buf.push_str("hello world");
        assert!(!buf.is_empty());
        assert_eq!(buf.line_count(), 1);
        assert_eq!(buf.as_str(), "hello world");
    }

    #[test]
    fn test_thinking_buffer_multiple_lines() {
        let mut buf = ThinkingBuffer::new();
        buf.push_str("line1\nline2\nline3");
        assert_eq!(buf.line_count(), 3);
        assert_eq!(buf.as_str(), "line1\nline2\nline3");
    }

    #[test]
    fn test_thinking_buffer_incremental_append() {
        let mut buf = ThinkingBuffer::new();
        buf.push_str("hel");
        buf.push_str("lo\nwor");
        buf.push_str("ld");
        assert_eq!(buf.line_count(), 2);
        assert_eq!(buf.as_str(), "hello\nworld");
    }

    #[test]
    fn test_thinking_buffer_eviction() {
        let mut buf = ThinkingBuffer::new();
        // Push more than MAX_LINES lines
        for i in 0..150 {
            buf.push_str(&format!("line{}\n", i));
        }
        // Should only have MAX_LINES lines (100)
        assert_eq!(buf.line_count(), ThinkingBuffer::MAX_LINES);
        // First line should be line 50 (0-49 evicted)
        let content = buf.as_str();
        assert!(content.starts_with("line50\n"));
        assert!(content.contains("line149\n"));
    }

    #[test]
    fn test_thinking_buffer_clear() {
        let mut buf = ThinkingBuffer::new();
        buf.push_str("some content\nmore content");
        buf.clear();
        assert!(buf.is_empty());
        assert_eq!(buf.line_count(), 0);
        assert_eq!(buf.as_str(), "");
    }
}
