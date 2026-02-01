//! TUI Application state and main event loop.

use std::io;
use std::panic;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use crossterm::{
    event::{self, Event, KeyCode, KeyEvent, KeyModifiers},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use futures::StreamExt;
use ratatui::{backend::CrosstermBackend, Terminal};
use tokio::sync::{mpsc, RwLock};
use tokio_util::sync::CancellationToken;
use tui_input::Input;

use qq_core::{
    execute_tools_parallel_with_chunker, ChunkProcessor, ChunkerConfig, CompletionRequest,
    Message, Provider, StreamChunk, ToolCall, ToolRegistry,
};

use crate::agents::AgentExecutor;
use crate::chat::ChatSession;
use crate::config::Config as AppConfig;
use crate::debug_log::DebugLogger;
use crate::execution_context::ExecutionContext;
use crate::Cli;

use super::events::{InputAction, StreamEvent};
use super::ui;
use super::widgets::{InputHistory, ToolCallInfo, ToolStatus};

/// TUI Application state
pub struct TuiApp {
    // Display state
    pub profile: String,
    pub content: String,
    pub thinking_content: String,
    pub show_thinking: bool,
    pub thinking_collapsed: bool,

    // Token counts
    pub prompt_tokens: u32,
    pub completion_tokens: u32,

    // Tool-call iteration tracking (for multi-turn tool calls)
    pub tool_iteration: u32,

    // Streaming state
    pub is_streaming: bool,
    pub status_message: Option<String>,

    // Scroll state
    pub scroll_offset: u16,
    pub auto_scroll: bool,
    pub content_height: u16,
    pub viewport_height: u16,

    // Tool calls
    pub tool_calls: Vec<ToolCallInfo>,

    // Input
    pub input: Input,
    pub input_history: InputHistory,

    // UI state
    pub show_help: bool,
    pub should_quit: bool,

    // Execution context (for displaying call stack)
    pub execution_context: ExecutionContext,
}

impl Default for TuiApp {
    fn default() -> Self {
        Self::new("auto", ExecutionContext::new())
    }
}

impl TuiApp {
    pub fn new(profile: &str, execution_context: ExecutionContext) -> Self {
        Self {
            profile: profile.to_string(),
            content: String::new(),
            thinking_content: String::new(),
            show_thinking: true,
            thinking_collapsed: false,
            prompt_tokens: 0,
            completion_tokens: 0,
            tool_iteration: 0,
            is_streaming: false,
            status_message: None,
            scroll_offset: 0,
            auto_scroll: true,
            content_height: 0,
            viewport_height: 0,
            tool_calls: Vec::new(),
            input: Input::default(),
            input_history: InputHistory::new(),
            show_help: false,
            should_quit: false,
            execution_context,
        }
    }

    /// Reset for a new response
    pub fn start_response(&mut self) {
        self.content.clear();
        self.thinking_content.clear();
        self.thinking_collapsed = false;
        self.is_streaming = true;
        self.tool_calls.clear();
        self.scroll_offset = 0;
        self.auto_scroll = true;
        self.status_message = None;
    }

    /// Handle a stream event
    pub fn handle_stream_event(&mut self, event: StreamEvent) {
        match event {
            StreamEvent::Start { model: _ } => {
                // Model info is available but we display profile name
            }
            StreamEvent::ThinkingDelta(delta) => {
                self.thinking_content.push_str(&delta);
            }
            StreamEvent::ContentDelta(delta) => {
                self.content.push_str(&delta);
                // Auto-scroll if enabled
                if self.auto_scroll {
                    self.scroll_to_bottom();
                }
            }
            StreamEvent::ToolCallStart { id: _, name } => {
                self.tool_calls.push(ToolCallInfo {
                    name: name.clone(),
                    args_preview: String::new(),
                    status: ToolStatus::Pending,
                });
                self.status_message = Some(format!("Tool: {}", name));
            }
            StreamEvent::ToolCallDelta { arguments: _ } => {
                // We don't update args preview in real-time to avoid noise
            }
            StreamEvent::Done { usage, content: _ } => {
                self.is_streaming = false;
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
                self.status_message = Some(format!("Error: {}", message));
            }
            StreamEvent::ToolExecuting { name } => {
                // Mark tool as executing
                if let Some(tool) = self.tool_calls.iter_mut().find(|t| t.name == name) {
                    tool.status = ToolStatus::Executing;
                }
                self.status_message = Some(format!("Running: {}", name));
            }
            StreamEvent::ToolComplete {
                id: _,
                name,
                result_len,
                is_error,
            } => {
                if let Some(tool) = self.tool_calls.iter_mut().find(|t| t.name == name) {
                    tool.status = if is_error {
                        ToolStatus::Error
                    } else {
                        ToolStatus::Complete
                    };
                    tool.args_preview = format!("{} bytes", result_len);
                }
            }
            StreamEvent::IterationStart { iteration } => {
                self.tool_iteration = iteration;
            }
        }
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
                self.scroll_up(3);
            }
            InputAction::ScrollDown => {
                self.scroll_down(3);
            }
            InputAction::PageUp => {
                self.page_up();
            }
            InputAction::PageDown => {
                self.page_down();
            }
            InputAction::ScrollToTop => {
                self.scroll_to_top();
            }
            InputAction::ScrollToBottom => {
                self.scroll_to_bottom();
            }
            InputAction::ToggleThinking => {
                if self.show_thinking && !self.thinking_content.is_empty() {
                    self.thinking_collapsed = !self.thinking_collapsed;
                }
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
                    let new_value: String = value.chars().take(new_cursor).chain(after.chars()).collect();
                    self.input = Input::new(new_value).with_cursor(new_cursor);
                }
            }
            _ => {}
        }
    }

    /// Scroll up by amount
    pub fn scroll_up(&mut self, amount: u16) {
        self.scroll_offset = self.scroll_offset.saturating_sub(amount);
        if self.scroll_offset > 0 {
            self.auto_scroll = false;
        }
    }

    /// Scroll down by amount
    pub fn scroll_down(&mut self, amount: u16) {
        let max_scroll = self.content_height.saturating_sub(self.viewport_height);
        self.scroll_offset = (self.scroll_offset + amount).min(max_scroll);
        if self.scroll_offset >= max_scroll {
            self.auto_scroll = true;
        }
    }

    /// Page up
    pub fn page_up(&mut self) {
        self.scroll_up(self.viewport_height.saturating_sub(2));
    }

    /// Page down
    pub fn page_down(&mut self) {
        self.scroll_down(self.viewport_height.saturating_sub(2));
    }

    /// Scroll to top
    pub fn scroll_to_top(&mut self) {
        self.scroll_offset = 0;
        self.auto_scroll = false;
    }

    /// Scroll to bottom
    pub fn scroll_to_bottom(&mut self) {
        let max_scroll = self.content_height.saturating_sub(self.viewport_height);
        self.scroll_offset = max_scroll;
        self.auto_scroll = true;
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
    agent_executor: Option<Arc<RwLock<AgentExecutor>>>,
    execution_context: ExecutionContext,
    chunker_config: ChunkerConfig,
) -> Result<()> {
    // Set up panic hook
    setup_panic_hook();

    // Set up debug logger if requested
    let debug_logger: Option<Arc<DebugLogger>> = if let Some(ref path) = cli.debug_file {
        match DebugLogger::new(path) {
            Ok(logger) => Some(Arc::new(logger)),
            Err(_) => None,
        }
    } else {
        None
    };

    // Initialize terminal
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, crossterm::cursor::Hide)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    // Create chat session
    let mut session = ChatSession::new(system_prompt);

    // Create TUI app
    let mut app = TuiApp::new(&profile_name, execution_context.clone());

    // Channel for stream events
    let (stream_tx, mut stream_rx) = mpsc::channel::<StreamEvent>(100);

    // Cancellation token for stopping streams
    let cancel_token = CancellationToken::new();

    // Main event loop
    let tick_rate = Duration::from_millis(33); // ~30fps

    loop {
        // Render
        terminal.draw(|f| {
            // Update viewport height for scrolling
            let content_area_height = f.area().height.saturating_sub(10); // Rough estimate
            app.viewport_height = content_area_height;
            app.content_height = app.content.lines().count() as u16;

            ui::render(&app, f);
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

        // Poll for keyboard events
        if event::poll(timeout)? {
            if let Event::Key(key) = event::read()? {
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
                            app.is_streaming = false;
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
                                            app.tool_calls.clear();
                                            app.status_message = Some("Cleared".to_string());
                                        }
                                        TuiCommand::Reset => {
                                            session.clear();
                                            app.content.clear();
                                            app.thinking_content.clear();
                                            app.tool_calls.clear();
                                            app.prompt_tokens = 0;
                                            app.completion_tokens = 0;
                                            app.status_message = Some("Session reset".to_string());
                                        }
                                        TuiCommand::Help => {
                                            app.show_help = true;
                                        }
                                        TuiCommand::Tools => {
                                            app.content = format_tools_list(&tools_registry);
                                        }
                                        TuiCommand::Agents => {
                                            if let Some(ref exec) = agent_executor {
                                                let exec = exec.read().await;
                                                app.content = format_agents_list(&exec);
                                            } else {
                                                app.content =
                                                    "Agents not configured.".to_string();
                                            }
                                        }
                                        TuiCommand::History => {
                                            app.content = format!(
                                                "Messages in conversation: {}",
                                                session.message_count()
                                            );
                                        }
                                    }
                                } else {
                                    // Regular message - start completion
                                    session.add_user_message(&input);
                                    app.start_response();

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

                                    // Spawn streaming task
                                    tokio::spawn(async move {
                                        run_streaming_completion(
                                            provider, tools, params, model, messages, tx, debug,
                                            temp, max_tok, exec_ctx, chunker_cfg, original_query,
                                        )
                                        .await
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
        }

        // Check for completion events that need session updates
        // This is handled by the completion task sending back results

        if app.should_quit {
            break;
        }
    }

    // Restore terminal
    disable_raw_mode()?;
    execute!(
        terminal.backend_mut(),
        LeaveAlternateScreen,
        crossterm::cursor::Show
    )?;
    terminal.show_cursor()?;

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

        // Scrolling
        (KeyCode::PageUp, _) => Some(InputAction::PageUp),
        (KeyCode::PageDown, _) => Some(InputAction::PageDown),
        (KeyCode::Home, KeyModifiers::CONTROL) => Some(InputAction::ScrollToTop),
        (KeyCode::End, KeyModifiers::CONTROL) => Some(InputAction::ScrollToBottom),

        // Editing
        (KeyCode::Backspace, KeyModifiers::NONE) => Some(InputAction::Backspace),
        (KeyCode::Delete, KeyModifiers::NONE) => Some(InputAction::Delete),
        (KeyCode::Char('w'), KeyModifiers::CONTROL) => Some(InputAction::DeleteWord),

        // Toggle thinking (Ctrl+T)
        (KeyCode::Char('t'), KeyModifiers::CONTROL) => Some(InputAction::ToggleThinking),

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
            "  ask_{} {} - {}\n",
            agent.name, type_marker, agent.description
        ));
    }
    output
}

/// Run streaming completion in a separate task
async fn run_streaming_completion(
    provider: Arc<dyn Provider>,
    tools_registry: ToolRegistry,
    extra_params: std::collections::HashMap<String, serde_json::Value>,
    model: Option<String>,
    mut messages: Vec<Message>,
    tx: mpsc::Sender<StreamEvent>,
    debug_logger: Option<Arc<DebugLogger>>,
    temperature: Option<f32>,
    max_tokens: Option<u32>,
    execution_context: ExecutionContext,
    chunker_config: ChunkerConfig,
    original_query: String,
) {
    // Create chunk processor for large tool outputs
    let chunk_processor = ChunkProcessor::new(Arc::clone(&provider), chunker_config);
    let max_iterations = 100u32;

    for iteration in 0..max_iterations {
        let _ = tx.send(StreamEvent::IterationStart { iteration: iteration + 1 }).await;

        if let Some(ref logger) = debug_logger {
            logger.log_iteration(iteration as usize, "tui_completion");
        }

        let mut request = CompletionRequest::new(messages.clone());

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

        // Stream the response
        let stream_result = provider.stream(request).await;
        let mut stream = match stream_result {
            Ok(s) => s,
            Err(e) => {
                execution_context.reset().await;
                let _ = tx.send(StreamEvent::Error { message: e.to_string() }).await;
                return;
            }
        };

        let mut content = String::new();
        let mut tool_calls: Vec<ToolCall> = Vec::new();
        let mut current_tool_call: Option<(String, String, String)> = None;

        while let Some(chunk) = stream.next().await {
            match chunk {
                Ok(StreamChunk::Start { model }) => {
                    let _ = tx.send(StreamEvent::Start { model }).await;
                }
                Ok(StreamChunk::ThinkingDelta { content: delta }) => {
                    let _ = tx.send(StreamEvent::ThinkingDelta(delta)).await;
                }
                Ok(StreamChunk::Delta { content: delta }) => {
                    content.push_str(&delta);
                    let _ = tx.send(StreamEvent::ContentDelta(delta)).await;
                }
                Ok(StreamChunk::ToolCallStart { id, name }) => {
                    // Finish pending tool call
                    if let Some((tc_id, tc_name, tc_args)) = current_tool_call.take() {
                        let args: serde_json::Value =
                            serde_json::from_str(&tc_args).unwrap_or(serde_json::Value::Null);
                        tool_calls.push(ToolCall::new(tc_id, tc_name, args));
                    }
                    current_tool_call = Some((id.clone(), name.clone(), String::new()));
                    let _ = tx.send(StreamEvent::ToolCallStart { id, name }).await;
                }
                Ok(StreamChunk::ToolCallDelta { arguments }) => {
                    if let Some((_, _, ref mut args)) = current_tool_call {
                        args.push_str(&arguments);
                    }
                    let _ = tx.send(StreamEvent::ToolCallDelta { arguments }).await;
                }
                Ok(StreamChunk::Done { usage }) => {
                    // Finish pending tool call
                    if let Some((tc_id, tc_name, tc_args)) = current_tool_call.take() {
                        let args: serde_json::Value =
                            serde_json::from_str(&tc_args).unwrap_or(serde_json::Value::Null);
                        tool_calls.push(ToolCall::new(tc_id, tc_name, args));
                    }

                    if tool_calls.is_empty() {
                        // No tool calls - we're done, send final content for session
                        execution_context.reset().await;
                        let _ = tx.send(StreamEvent::Done { usage, content: content.clone() }).await;
                        return;
                    }
                }
                Ok(StreamChunk::Error { message }) => {
                    execution_context.reset().await;
                    let _ = tx.send(StreamEvent::Error { message }).await;
                    return;
                }
                Err(e) => {
                    execution_context.reset().await;
                    let _ = tx.send(StreamEvent::Error { message: e.to_string() }).await;
                    return;
                }
            }
        }

        // Handle tool calls
        if !tool_calls.is_empty() {
            // Add assistant message with tool calls
            let assistant_msg = Message::assistant_with_tool_calls(content.as_str(), tool_calls.clone());
            messages.push(assistant_msg.clone());

            // Send session update for the assistant message with tool calls
            let _ = tx.send(StreamEvent::SessionUpdate {
                messages: vec![assistant_msg],
            }).await;

            // Execute tools - push context for each tool as it starts
            for tool_call in &tool_calls {
                execution_context.push_tool(&tool_call.name).await;
                let _ = tx.send(StreamEvent::ToolExecuting { name: tool_call.name.clone() }).await;
            }

            let results = execute_tools_parallel_with_chunker(
                &tools_registry,
                tool_calls.clone(),
                Some(&chunk_processor),
                Some(&original_query),
            )
            .await;

            let mut tool_result_messages = Vec::new();
            for result in results {
                let tool_name = tool_calls
                    .iter()
                    .find(|tc| tc.id == result.tool_call_id)
                    .map(|tc| tc.name.clone())
                    .unwrap_or_default();

                // Pop the tool context
                execution_context.pop().await;

                let _ = tx.send(StreamEvent::ToolComplete {
                    id: result.tool_call_id.clone(),
                    name: tool_name,
                    result_len: result.content.len(),
                    is_error: result.is_error,
                }).await;

                let tool_msg = Message::tool_result(&result.tool_call_id, &result.content);
                messages.push(tool_msg.clone());
                tool_result_messages.push(tool_msg);
            }

            // Send session update for tool results
            let _ = tx.send(StreamEvent::SessionUpdate {
                messages: tool_result_messages,
            }).await;

            // Continue to next iteration
            continue;
        }

        // No tool calls, we're done - send final content
        execution_context.reset().await;
        let _ = tx.send(StreamEvent::Done { usage: None, content: content.clone() }).await;
        return;
    }

    // Max iterations reached
    execution_context.reset().await;
    let _ = tx.send(StreamEvent::Error {
        message: format!("Max iterations ({}) reached", max_iterations),
    }).await;
}
