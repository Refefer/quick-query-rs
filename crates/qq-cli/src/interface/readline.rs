//! Readline-based interface for interactive chat.
//!
//! This provides a simple terminal interface using rustyline for input
//! and streaming text output with markdown rendering.

use std::path::PathBuf;

use anyhow::Result;
use async_trait::async_trait;
use crossterm::ExecutableCommand;
use crossterm::style::{Color, ResetColor, SetForegroundColor};
use crossterm::terminal::size;
use rustyline::error::ReadlineError;
use rustyline::history::FileHistory;
use rustyline::{Config, Editor};

use crate::markdown::MarkdownRenderer;

use super::{parse_user_input, AgentInterface, AgentOutput, InputResult, UserInput};

/// Readline-based interface for terminal interaction.
pub struct ReadlineInterface {
    /// Rustyline editor.
    editor: Option<Editor<(), FileHistory>>,

    /// History file path.
    history_path: Option<PathBuf>,

    /// Markdown renderer for streaming output.
    content_renderer: MarkdownRenderer,

    /// Markdown renderer for thinking output.
    thinking_renderer: MarkdownRenderer,

    /// Whether we're in thinking mode.
    in_thinking: bool,

    /// Whether we're in content mode.
    in_content: bool,

    /// Whether streaming is in progress.
    is_streaming: bool,

    /// Whether we should quit.
    should_quit: bool,
}

impl ReadlineInterface {
    /// Create a new ReadlineInterface.
    pub fn new() -> Self {
        Self {
            editor: None,
            history_path: get_history_path(),
            content_renderer: MarkdownRenderer::new(),
            thinking_renderer: MarkdownRenderer::new(),
            in_thinking: false,
            in_content: false,
            is_streaming: false,
            should_quit: false,
        }
    }

    /// Print a section header with styling.
    fn print_section_header(&self, title: &str) -> std::io::Result<()> {
        use std::io::Write;

        let width = size().map(|(w, _)| w as usize).unwrap_or(80);
        let title_len = title.len() + 2;
        let remaining = width.saturating_sub(title_len).saturating_sub(1);
        let left_len = remaining / 2;
        let right_len = remaining - left_len;

        let left_rule = "─".repeat(left_len);
        let right_rule = "─".repeat(right_len);

        let mut stdout = std::io::stdout();
        stdout.execute(SetForegroundColor(Color::DarkGrey))?;
        print!("{} ", left_rule);
        stdout.execute(SetForegroundColor(Color::Cyan))?;
        print!("{}", title);
        stdout.execute(SetForegroundColor(Color::DarkGrey))?;
        println!(" {}", right_rule);
        stdout.execute(ResetColor)?;
        stdout.flush()?;
        Ok(())
    }

    /// Print a tool call notification.
    fn print_tool_call(&self, name: &str, is_error: bool) -> std::io::Result<()> {
        use std::io::Write;

        let mut stdout = std::io::stdout();
        stdout.execute(SetForegroundColor(Color::DarkGrey))?;
        print!("▶ ");
        if is_error {
            stdout.execute(SetForegroundColor(Color::Red))?;
        } else {
            stdout.execute(SetForegroundColor(Color::Yellow))?;
        }
        println!("{}", name);
        stdout.execute(ResetColor)?;
        stdout.flush()?;
        Ok(())
    }

    /// Print the prompt hint.
    fn print_prompt_hint(&self) -> std::io::Result<()> {
        use std::io::Write;

        let mut stdout = std::io::stdout();
        stdout.execute(SetForegroundColor(Color::DarkGrey))?;
        println!("/help · /quit or Ctrl+D · Ctrl+C to interrupt");
        stdout.execute(ResetColor)?;
        stdout.flush()?;
        Ok(())
    }

    /// Print a status message.
    fn print_status(&self, msg: &str) -> std::io::Result<()> {
        use std::io::Write;

        let mut stdout = std::io::stdout();
        stdout.execute(SetForegroundColor(Color::Cyan))?;
        println!("{}", msg);
        stdout.execute(ResetColor)?;
        stdout.flush()?;
        Ok(())
    }

    /// Print an error message.
    fn print_error(&self, msg: &str) -> std::io::Result<()> {
        use std::io::Write;

        let mut stdout = std::io::stdout();
        stdout.execute(SetForegroundColor(Color::Red))?;
        eprintln!("Error: {}", msg);
        stdout.execute(ResetColor)?;
        stdout.flush()?;
        Ok(())
    }
}

impl Default for ReadlineInterface {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl AgentInterface for ReadlineInterface {
    async fn next_input(&mut self) -> Result<Option<UserInput>> {
        // Print hint before prompt
        let _ = self.print_prompt_hint();

        let editor = self.editor.as_mut().ok_or_else(|| {
            anyhow::anyhow!("Interface not initialized")
        })?;

        match editor.readline("you> ") {
            Ok(line) => {
                // Add to history
                let _ = editor.add_history_entry(&line);
                Ok(Some(parse_user_input(&line)))
            }
            Err(ReadlineError::Interrupted) => {
                println!("^C");
                Ok(Some(UserInput::Cancel))
            }
            Err(ReadlineError::Eof) => {
                println!("Goodbye!");
                Ok(None)
            }
            Err(e) => Err(anyhow::anyhow!("Error reading input: {}", e)),
        }
    }

    async fn poll_input(&mut self) -> Result<InputResult> {
        // Readline is blocking, so we just return Pending
        // The actual input is handled by next_input
        Ok(InputResult::Pending)
    }

    async fn emit(&mut self, output: AgentOutput) -> Result<()> {
        match output {
            AgentOutput::ContentDelta(delta) => {
                if !self.in_content {
                    // Finish thinking if we were in it
                    if self.in_thinking {
                        self.thinking_renderer.finish()?;
                        self.in_thinking = false;
                    }
                    self.print_section_header("Response")?;
                    self.in_content = true;
                }
                self.content_renderer.push(&delta)?;
            }
            AgentOutput::ThinkingDelta(delta) => {
                if !self.in_thinking {
                    self.print_section_header("Thinking")?;
                    self.in_thinking = true;
                }
                self.thinking_renderer.push(&delta)?;
            }
            AgentOutput::ToolStarted { name, .. } => {
                self.print_tool_call(&name, false)?;
            }
            AgentOutput::ToolExecuting { name } => {
                // Could show a spinner here
                let _ = name;
            }
            AgentOutput::ToolCompleted { name, is_error, .. } => {
                if is_error {
                    self.print_tool_call(&format!("{} (error)", name), true)?;
                }
            }
            AgentOutput::Done { .. } => {
                // Finish any pending rendering
                if self.in_thinking && !self.in_content {
                    self.thinking_renderer.finish()?;
                } else if self.in_content {
                    self.content_renderer.finish()?;
                }
                println!();
            }
            AgentOutput::Error { message } => {
                self.print_error(&message)?;
            }
            AgentOutput::Status(msg) => {
                self.print_status(&msg)?;
            }
            AgentOutput::ClearStatus => {
                // Nothing to clear in readline mode
            }
            AgentOutput::IterationStart { .. } => {
                // Could show iteration count
            }
            AgentOutput::ByteCount { .. } => {
                // Could show byte count in debug mode
            }
            AgentOutput::StreamStart { .. } => {
                // Stream started
            }
        }
        Ok(())
    }

    fn start_response(&mut self, _user_input: &str) {
        // Reset renderers for new response
        self.content_renderer = MarkdownRenderer::new();
        self.thinking_renderer = MarkdownRenderer::new();
        self.in_thinking = false;
        self.in_content = false;
    }

    fn finish_response(&mut self) {
        self.in_thinking = false;
        self.in_content = false;
    }

    async fn initialize(&mut self) -> Result<()> {
        // Set up readline
        let config = Config::builder()
            .history_ignore_space(true)
            .history_ignore_dups(true)?
            .build();

        let mut editor: Editor<(), FileHistory> = Editor::with_config(config)?;

        // Load history if available
        if let Some(ref path) = self.history_path {
            let _ = editor.load_history(path);
        }

        self.editor = Some(editor);
        Ok(())
    }

    async fn cleanup(&mut self) -> Result<()> {
        // Save history
        if let Some(ref mut editor) = self.editor {
            if let Some(ref path) = self.history_path {
                let _ = editor.save_history(path);
            }
        }
        Ok(())
    }

    fn should_quit(&self) -> bool {
        self.should_quit
    }

    fn request_quit(&mut self) {
        self.should_quit = true;
    }

    fn is_streaming(&self) -> bool {
        self.is_streaming
    }

    fn set_streaming(&mut self, streaming: bool) {
        self.is_streaming = streaming;
    }
}

/// Get the path to the history file.
fn get_history_path() -> Option<PathBuf> {
    dirs::config_dir().map(|d| d.join("qq").join("chat_history"))
}
