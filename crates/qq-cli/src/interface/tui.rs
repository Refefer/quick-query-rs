//! TUI-based interface for interactive chat.
//!
//! This provides a rich terminal interface using ratatui for rendering
//! and a sophisticated event-driven architecture.

use std::io;
use std::panic;
use std::time::Duration;

use anyhow::Result;
use async_trait::async_trait;
use crossterm::{
    event::{
        self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEvent, KeyModifiers,
        MouseEventKind,
    },
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{backend::CrosstermBackend, Terminal};
use tokio::sync::mpsc;
use tui_input::Input;

use crate::execution_context::ExecutionContext;
use crate::tui::events::InputAction;
use crate::tui::layout::{LayoutConfig, PaneId};
use crate::tui::markdown::{markdown_to_lines, MarkdownStyles};
use crate::tui::scroll::ScrollState;
use crate::tui::ui;
use crate::tui::widgets::{InputHistory, ToolNotification, ToolNotificationStatus};

use super::{parse_user_input, AgentInterface, AgentOutput, InputResult, UserInput};

/// TUI Application state (extracted from tui/app.rs TuiApp).
pub struct TuiState {
    // Display state
    pub profile: String,
    pub primary_agent: String,
    pub content: String,
    pub thinking_content: String,
    pub show_thinking: bool,
    pub thinking_expanded: bool,

    // Token counts
    pub prompt_tokens: u32,
    pub completion_tokens: u32,

    // Tool-call iteration tracking
    pub tool_iteration: u32,

    // Streaming state
    pub is_streaming: bool,
    pub status_message: Option<String>,

    // Scroll state
    pub scroll: ScrollState,

    // Tool notifications (displayed in thinking panel)
    pub tool_notifications: Vec<ToolNotification>,

    // Input
    pub input: Input,
    pub input_history: InputHistory,

    // UI state
    pub show_help: bool,
    pub should_quit: bool,

    // Execution context
    pub execution_context: ExecutionContext,

    // Agent progress
    pub agent_progress: Option<(String, u32, u32)>,

    // Byte counts
    pub agent_input_bytes: usize,
    pub agent_output_bytes: usize,
    pub session_input_bytes: usize,
    pub session_output_bytes: usize,

    // Waiting state
    pub is_waiting: bool,
}

impl TuiState {
    pub fn new(profile: &str, primary_agent: &str, execution_context: ExecutionContext) -> Self {
        Self {
            profile: profile.to_string(),
            primary_agent: primary_agent.to_string(),
            content: String::new(),
            thinking_content: String::new(),
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
            input_history: InputHistory::new(),
            show_help: false,
            should_quit: false,
            execution_context,
            agent_progress: None,
            agent_input_bytes: 0,
            agent_output_bytes: 0,
            session_input_bytes: 0,
            session_output_bytes: 0,
            is_waiting: false,
        }
    }

    /// Reset for a new response
    pub fn start_response(&mut self, user_input: &str) {
        if !self.content.is_empty() {
            self.content.push_str("\n\n---\n\n");
        }
        self.content.push_str("**You:** ");
        self.content.push_str(user_input);
        self.content.push_str("\n\n**Assistant:** ");

        self.thinking_content.clear();
        self.thinking_expanded = false;
        self.is_streaming = true;
        self.is_waiting = true;
        self.tool_notifications.clear();
        self.scroll.enable_auto_scroll();
        self.status_message = None;
        self.agent_progress = None;
        self.agent_input_bytes = 0;
        self.agent_output_bytes = 0;
    }

    /// Handle input action
    pub fn handle_input_action(&mut self, action: InputAction) {
        match action {
            InputAction::Char(c) => {
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
            InputAction::HistoryUp => {
                if let Some(entry) = self.input_history.navigate_up(self.input.value()) {
                    let s: String = entry.to_string();
                    self.input = Input::new(s);
                }
            }
            InputAction::HistoryDown => {
                if let Some(entry) = self.input_history.navigate_down(self.input.value()) {
                    let s: String = entry.to_string();
                    self.input = Input::new(s);
                }
            }
            InputAction::ScrollUp => self.scroll.scroll_up(3),
            InputAction::ScrollDown => self.scroll.scroll_down(3),
            InputAction::PageUp => self.scroll.page_up(),
            InputAction::PageDown => self.scroll.page_down(),
            InputAction::ScrollToTop => self.scroll.scroll_to_top(),
            InputAction::ScrollToBottom => self.scroll.scroll_to_bottom(),
            InputAction::ToggleThinking => {
                if self.show_thinking && !self.thinking_content.is_empty() {
                    self.thinking_expanded = !self.thinking_expanded;
                }
            }
            InputAction::Help => {
                self.show_help = !self.show_help;
            }
            InputAction::Quit => {
                self.should_quit = true;
            }
            InputAction::DeleteWord => {
                let value = self.input.value().to_string();
                let cursor = self.input.visual_cursor();
                if cursor > 0 {
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
    pub fn update_scroll_dimensions(&mut self, viewport_height: u16, content_width: u16) {
        self.scroll.set_viewport_height(viewport_height);

        let styles = MarkdownStyles::default();
        let lines = markdown_to_lines(&self.content, &styles);
        let content_height = calculate_wrapped_height(&lines, content_width);
        self.scroll.set_content_height(content_height);
    }

    pub fn take_input(&mut self) -> String {
        let value = self.input.value().to_string();
        self.input_history.add(value.clone());
        self.input = Input::default();
        value
    }
}

/// Calculate the total wrapped line height for a list of lines.
fn calculate_wrapped_height(lines: &[ratatui::text::Line], width: u16) -> u16 {
    let width = width.max(1) as usize;
    lines
        .iter()
        .map(|line| {
            let line_len: usize = line.spans.iter().map(|s| s.content.len()).sum();
            if line_len == 0 {
                1
            } else {
                ((line_len + width - 1) / width) as u16
            }
        })
        .sum()
}

/// TUI-based interface for interactive chat.
pub struct TuiInterface {
    /// TUI state.
    state: TuiState,

    /// Terminal.
    terminal: Option<Terminal<CrosstermBackend<io::Stdout>>>,

    /// Pending input from event loop.
    pending_input: Option<UserInput>,

    /// Input sender for async input.
    input_tx: Option<mpsc::Sender<UserInput>>,

    /// Input receiver for async input.
    input_rx: Option<mpsc::Receiver<UserInput>>,
}

impl TuiInterface {
    /// Create a new TuiInterface.
    pub fn new(profile: &str, primary_agent: &str, execution_context: ExecutionContext) -> Self {
        let (tx, rx) = mpsc::channel(10);
        Self {
            state: TuiState::new(profile, primary_agent, execution_context),
            terminal: None,
            pending_input: None,
            input_tx: Some(tx),
            input_rx: Some(rx),
        }
    }

    /// Set up panic hook to restore terminal.
    fn setup_panic_hook() {
        let original_hook = panic::take_hook();
        panic::set_hook(Box::new(move |panic_info| {
            let _ = disable_raw_mode();
            let _ = execute!(
                io::stdout(),
                LeaveAlternateScreen,
                DisableMouseCapture,
                crossterm::cursor::Show
            );
            original_hook(panic_info);
        }));
    }

    /// Convert key event to input action.
    fn key_to_action(key: KeyEvent, is_streaming: bool) -> Option<InputAction> {
        match (key.code, key.modifiers) {
            (KeyCode::Char('d'), KeyModifiers::CONTROL) => Some(InputAction::Quit),
            (KeyCode::Char('q'), KeyModifiers::CONTROL) => Some(InputAction::Quit),
            (KeyCode::Char('c'), KeyModifiers::CONTROL) => Some(InputAction::Cancel),
            (KeyCode::Esc, _) if is_streaming => Some(InputAction::Cancel),
            (KeyCode::Enter, KeyModifiers::NONE) => Some(InputAction::Submit),
            (KeyCode::Up, KeyModifiers::NONE) => Some(InputAction::HistoryUp),
            (KeyCode::Down, KeyModifiers::NONE) => Some(InputAction::HistoryDown),
            (KeyCode::Left, KeyModifiers::NONE) => Some(InputAction::Left),
            (KeyCode::Right, KeyModifiers::NONE) => Some(InputAction::Right),
            (KeyCode::Home, KeyModifiers::NONE) => Some(InputAction::Home),
            (KeyCode::End, KeyModifiers::NONE) => Some(InputAction::End),
            (KeyCode::PageUp, _) => Some(InputAction::PageUp),
            (KeyCode::PageDown, _) => Some(InputAction::PageDown),
            (KeyCode::Char('b'), KeyModifiers::CONTROL) => Some(InputAction::PageUp),
            (KeyCode::Char('f'), KeyModifiers::CONTROL) => Some(InputAction::PageDown),
            (KeyCode::Home, KeyModifiers::CONTROL) => Some(InputAction::ScrollToTop),
            (KeyCode::End, KeyModifiers::CONTROL) => Some(InputAction::ScrollToBottom),
            (KeyCode::Backspace, KeyModifiers::NONE) => Some(InputAction::Backspace),
            (KeyCode::Delete, KeyModifiers::NONE) => Some(InputAction::Delete),
            (KeyCode::Char('w'), KeyModifiers::CONTROL) => Some(InputAction::DeleteWord),
            (KeyCode::Char('t'), KeyModifiers::CONTROL) => Some(InputAction::ToggleThinking),
            (KeyCode::Char(c), KeyModifiers::NONE) if !is_streaming => Some(InputAction::Char(c)),
            (KeyCode::Char(c), KeyModifiers::SHIFT) if !is_streaming => Some(InputAction::Char(c)),
            _ => None,
        }
    }

    /// Render the current state.
    fn render(&mut self) -> Result<()> {
        if let Some(ref mut terminal) = self.terminal {
            terminal.draw(|f| {
                let area = f.area();

                // Build layout config to compute viewport height BEFORE rendering
                let mut layout_config = LayoutConfig::new();
                let has_thinking =
                    self.state.show_thinking && !self.state.thinking_content.is_empty();
                let thinking_lines = self.state.thinking_content.lines().count() as u16;
                layout_config.set_thinking(has_thinking, self.state.thinking_expanded, thinking_lines);

                // Compute layout to get content area dimensions
                let layout = layout_config.compute(area);
                if let Some(&content_rect) = layout.get(&PaneId::Content) {
                    let viewport_height = content_rect.height.saturating_sub(2);
                    let content_width = content_rect.width.saturating_sub(2);
                    self.state.update_scroll_dimensions(viewport_height, content_width);
                }

                // Use the existing UI render with our state
                render_tui_state(&self.state, f);
            })?;
        }
        Ok(())
    }

    /// Process pending keyboard and mouse events.
    fn process_events(&mut self) -> Result<Option<UserInput>> {
        let timeout = if self.state.is_streaming {
            Duration::from_millis(16)
        } else {
            Duration::from_millis(33)
        };

        if event::poll(timeout)? {
            match event::read()? {
                Event::Key(key) => {
                    if self.state.show_help {
                        self.state.show_help = false;
                        return Ok(None);
                    }

                    let action = Self::key_to_action(key, self.state.is_streaming);

                    match action {
                        Some(InputAction::Quit) => {
                            self.state.should_quit = true;
                            return Ok(Some(UserInput::Command(super::InterfaceCommand::Quit)));
                        }
                        Some(InputAction::Cancel) => {
                            if self.state.is_streaming {
                                return Ok(Some(UserInput::Cancel));
                            }
                        }
                        Some(InputAction::Submit) => {
                            if !self.state.is_streaming {
                                let input = self.state.take_input();
                                if !input.is_empty() {
                                    return Ok(Some(parse_user_input(&input)));
                                }
                            }
                        }
                        Some(action) => {
                            self.state.handle_input_action(action);
                        }
                        None => {}
                    }
                }
                Event::Mouse(mouse) => {
                    // Handle mouse scroll events
                    match mouse.kind {
                        MouseEventKind::ScrollUp => {
                            self.state.scroll.scroll_up(3);
                        }
                        MouseEventKind::ScrollDown => {
                            self.state.scroll.scroll_down(3);
                        }
                        _ => {}
                    }
                }
                Event::Resize(_, _) => {
                    // Terminal resized - next render will update dimensions
                }
                _ => {}
            }
        }

        Ok(None)
    }
}

#[async_trait]
impl AgentInterface for TuiInterface {
    async fn next_input(&mut self) -> Result<Option<UserInput>> {
        loop {
            // Render
            self.render()?;

            // Check for events
            if let Some(input) = self.process_events()? {
                return Ok(Some(input));
            }

            if self.state.should_quit {
                return Ok(None);
            }
        }
    }

    async fn poll_input(&mut self) -> Result<InputResult> {
        // Render
        self.render()?;

        // Check for events non-blocking
        if let Some(input) = self.process_events()? {
            return Ok(InputResult::Input(input));
        }

        if self.state.should_quit {
            return Ok(InputResult::Quit);
        }

        Ok(InputResult::Pending)
    }

    async fn emit(&mut self, output: AgentOutput) -> Result<()> {
        match output {
            AgentOutput::ContentDelta(delta) => {
                self.state.is_waiting = false;
                self.state.content.push_str(&delta);
            }
            AgentOutput::ThinkingDelta(delta) => {
                self.state.is_waiting = false;
                self.state.thinking_content.push_str(&delta);
            }
            AgentOutput::ToolStarted { name, .. } => {
                self.state.tool_notifications.push(ToolNotification::new(
                    name.clone(),
                    ToolNotificationStatus::Started,
                ));
                self.state.status_message = Some(format!("Tool: {}", name));
            }
            AgentOutput::ToolExecuting { name } => {
                if let Some(notif) = self
                    .state
                    .tool_notifications
                    .iter_mut()
                    .find(|n| n.tool_name == name)
                {
                    notif.status = ToolNotificationStatus::Executing;
                }
                self.state.status_message = Some(format!("Running: {}", name));
            }
            AgentOutput::ToolCompleted {
                name,
                result_len,
                is_error,
                ..
            } => {
                if let Some(notif) = self
                    .state
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
            AgentOutput::Done { usage, .. } => {
                self.state.is_streaming = false;
                self.state.is_waiting = false;
                if let Some(u) = usage {
                    self.state.prompt_tokens = u.prompt_tokens;
                    self.state.completion_tokens = u.completion_tokens;
                }
                self.state.status_message = None;
            }
            AgentOutput::Error { message } => {
                self.state.is_streaming = false;
                self.state.is_waiting = false;
                self.state.status_message = Some(format!("Error: {}", message));
            }
            AgentOutput::Status(msg) => {
                // For status messages in TUI, we show them in the content area
                if !self.state.content.is_empty() {
                    self.state.content.push_str("\n\n");
                }
                self.state.content.push_str(&msg);
            }
            AgentOutput::ClearStatus => {
                self.state.status_message = None;
            }
            AgentOutput::IterationStart { iteration } => {
                self.state.tool_iteration = iteration;
                self.state.is_waiting = true;
            }
            AgentOutput::ByteCount {
                input_bytes,
                output_bytes,
            } => {
                self.state.session_input_bytes += input_bytes;
                self.state.session_output_bytes += output_bytes;
            }
            AgentOutput::StreamStart { .. } => {
                self.state.is_waiting = true;
            }
        }

        // Render after each output
        self.render()?;

        Ok(())
    }

    fn start_response(&mut self, user_input: &str) {
        self.state.start_response(user_input);
    }

    fn finish_response(&mut self) {
        self.state.is_streaming = false;
        self.state.is_waiting = false;
        self.state.agent_progress = None;
    }

    async fn initialize(&mut self) -> Result<()> {
        Self::setup_panic_hook();

        enable_raw_mode()?;
        let mut stdout = io::stdout();
        execute!(
            stdout,
            EnterAlternateScreen,
            EnableMouseCapture,
            crossterm::cursor::Hide
        )?;
        let backend = CrosstermBackend::new(stdout);
        self.terminal = Some(Terminal::new(backend)?);

        Ok(())
    }

    async fn cleanup(&mut self) -> Result<()> {
        if let Some(ref mut terminal) = self.terminal {
            disable_raw_mode()?;
            execute!(
                terminal.backend_mut(),
                LeaveAlternateScreen,
                DisableMouseCapture,
                crossterm::cursor::Show
            )?;
            terminal.show_cursor()?;
        }
        Ok(())
    }

    fn should_quit(&self) -> bool {
        self.state.should_quit
    }

    fn request_quit(&mut self) {
        self.state.should_quit = true;
    }

    fn is_streaming(&self) -> bool {
        self.state.is_streaming
    }

    fn set_streaming(&mut self, streaming: bool) {
        self.state.is_streaming = streaming;
    }
}

/// Render TUI state to frame (compatibility with existing UI module).
fn render_tui_state(state: &TuiState, f: &mut ratatui::Frame) {
    // Create a temporary TuiApp-compatible struct for the existing render function
    // This is a bridge until we fully migrate the UI code
    let app = crate::tui::app::TuiApp {
        profile: state.profile.clone(),
        primary_agent: state.primary_agent.clone(),
        content: state.content.clone(),
        thinking_content: state.thinking_content.clone(),
        show_thinking: state.show_thinking,
        thinking_expanded: state.thinking_expanded,
        prompt_tokens: state.prompt_tokens,
        completion_tokens: state.completion_tokens,
        tool_iteration: state.tool_iteration,
        is_streaming: state.is_streaming,
        status_message: state.status_message.clone(),
        scroll: state.scroll.clone(),
        tool_notifications: state.tool_notifications.clone(),
        input: state.input.clone(),
        input_history: state.input_history.clone(),
        show_help: state.show_help,
        should_quit: state.should_quit,
        execution_context: state.execution_context.clone(),
        agent_progress: state.agent_progress.clone(),
        agent_input_bytes: state.agent_input_bytes,
        agent_output_bytes: state.agent_output_bytes,
        session_input_bytes: state.session_input_bytes,
        session_output_bytes: state.session_output_bytes,
        is_waiting: state.is_waiting,
    };

    ui::render(&app, f);
}
