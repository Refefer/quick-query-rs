//! Status bar widget showing profile, activity, tokens, and streaming status.

use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph, Widget},
};

use crate::execution_context::ExecutionContext;

/// Status bar display state
pub struct StatusBar<'a> {
    profile: &'a str,
    prompt_tokens: u32,
    completion_tokens: u32,
    is_streaming: bool,
    is_waiting: bool,
    status_message: Option<&'a str>,
    execution_context: Option<&'a ExecutionContext>,
    tool_iteration: u32,
    /// Agent progress: (agent_name, current_iteration, max_iterations)
    agent_progress: Option<(&'a str, u32, u32)>,
    /// Agent byte counts: (input_bytes, output_bytes)
    agent_bytes: Option<(usize, usize)>,
    /// Session byte counts: (input_bytes, output_bytes)
    session_bytes: Option<(usize, usize)>,
}

impl<'a> StatusBar<'a> {
    pub fn new(profile: &'a str) -> Self {
        Self {
            profile,
            prompt_tokens: 0,
            completion_tokens: 0,
            is_streaming: false,
            is_waiting: false,
            status_message: None,
            execution_context: None,
            tool_iteration: 0,
            agent_progress: None,
            agent_bytes: None,
            session_bytes: None,
        }
    }

    pub fn tokens(mut self, prompt: u32, completion: u32) -> Self {
        self.prompt_tokens = prompt;
        self.completion_tokens = completion;
        self
    }

    pub fn streaming(mut self, is_streaming: bool) -> Self {
        self.is_streaming = is_streaming;
        self
    }

    pub fn waiting(mut self, is_waiting: bool) -> Self {
        self.is_waiting = is_waiting;
        self
    }

    pub fn status(mut self, message: &'a str) -> Self {
        self.status_message = Some(message);
        self
    }

    pub fn execution_context(mut self, context: &'a ExecutionContext) -> Self {
        self.execution_context = Some(context);
        self
    }

    pub fn iteration(mut self, iteration: u32) -> Self {
        self.tool_iteration = iteration;
        self
    }

    pub fn agent_progress(mut self, progress: Option<(&'a str, u32, u32)>) -> Self {
        self.agent_progress = progress;
        self
    }

    pub fn agent_bytes(mut self, input_bytes: usize, output_bytes: usize) -> Self {
        if input_bytes > 0 || output_bytes > 0 {
            self.agent_bytes = Some((input_bytes, output_bytes));
        }
        self
    }

    pub fn session_bytes(mut self, input_bytes: usize, output_bytes: usize) -> Self {
        // Always set session bytes so the counter is always visible
        self.session_bytes = Some((input_bytes, output_bytes));
        self
    }
}

impl Widget for StatusBar<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let style_dim = Style::default().fg(Color::DarkGray);
        let style_profile = Style::default().fg(Color::Cyan);
        let style_activity = Style::default().fg(Color::Yellow);
        let style_tokens = Style::default().fg(Color::White);
        let style_streaming = Style::default()
            .fg(Color::Green)
            .add_modifier(Modifier::BOLD);
        let style_waiting = Style::default()
            .fg(Color::Yellow)
            .add_modifier(Modifier::BOLD);

        let mut spans = Vec::new();

        // Always show profile first
        spans.push(Span::styled(" ", style_dim));
        spans.push(Span::styled(self.profile, style_profile));

        // Show agent progress if an agent is running (takes precedence over execution context)
        if let Some((agent_name, iteration, max_iterations)) = self.agent_progress {
            let style_agent = Style::default().fg(Color::Magenta);
            spans.push(Span::styled(" › ", style_dim));
            spans.push(Span::styled(
                format!("Agent[{}] turn {}/{}", agent_name, iteration, max_iterations),
                style_agent,
            ));
            // Show byte counts for agent
            if let Some((input_bytes, output_bytes)) = self.agent_bytes {
                spans.push(Span::styled(
                    format!(" {}", format_bytes(input_bytes + output_bytes)),
                    style_dim,
                ));
            }
        } else {
            // No agent running - show activity from execution context or status message
            let activity = self.get_activity();
            if let Some(activity) = activity {
                spans.push(Span::styled(" › ", style_dim));
                spans.push(Span::styled(activity, style_activity));
            }

            // Show iteration count if in a tool loop
            if self.tool_iteration > 1 {
                spans.push(Span::styled(format!(" (turn {})", self.tool_iteration), style_dim));
            }
        }

        // Waiting indicator (before streaming indicator)
        if self.is_waiting && self.is_streaming {
            spans.push(Span::styled(" ", style_dim));
            spans.push(Span::styled("Waiting...", style_waiting));
        }

        // Streaming indicator
        if self.is_streaming {
            spans.push(Span::styled(" ", style_dim));
            spans.push(Span::styled("●", style_streaming));
        }

        // Build right side content: session bytes and/or tokens
        let mut right_content = Vec::new();

        // Session bytes
        if let Some((input_bytes, output_bytes)) = self.session_bytes {
            let total = input_bytes + output_bytes;
            right_content.push(Span::styled(format_bytes(total), style_tokens));
            right_content.push(Span::styled(
                format!(" ({}↑/{}↓)", format_bytes(input_bytes), format_bytes(output_bytes)),
                style_dim,
            ));
        }

        // Tokens (if we have them)
        let total_tokens = self.prompt_tokens + self.completion_tokens;
        if total_tokens > 0 {
            if !right_content.is_empty() {
                right_content.push(Span::styled(" | ", style_dim));
            }
            right_content.push(Span::styled(format!("{}t", total_tokens), style_tokens));
        }

        // Add right-aligned content
        if !right_content.is_empty() {
            let current_len: usize = spans.iter().map(|s| s.content.len()).sum();
            let right_len: usize = right_content.iter().map(|s| s.content.len()).sum();
            let available = area.width as usize;

            if current_len + right_len + 2 < available {
                let padding = available - current_len - right_len - 2;
                spans.push(Span::styled(" ".repeat(padding), style_dim));
            } else {
                spans.push(Span::styled(" | ", style_dim));
            }

            spans.extend(right_content);
            spans.push(Span::styled(" ", style_dim));
        }

        let paragraph = Paragraph::new(Line::from(spans))
            .block(Block::default().borders(Borders::BOTTOM).border_style(style_dim));

        paragraph.render(area, buf);
    }
}

/// Format byte count with Kb/Mb suffixes for readability
fn format_bytes(bytes: usize) -> String {
    if bytes >= 1_000_000 {
        format!("{:.1}Mb", bytes as f64 / 1_000_000.0)
    } else if bytes >= 1_000 {
        format!("{:.1}Kb", bytes as f64 / 1_000.0)
    } else {
        format!("{}b", bytes)
    }
}

impl StatusBar<'_> {
    /// Get the current activity description
    fn get_activity(&self) -> Option<String> {
        // First check execution context for the current activity
        if let Some(ctx) = self.execution_context {
            if let Some(activity) = ctx.current_activity_blocking() {
                return Some(activity);
            }
        }

        // Fall back to status message (e.g., "Tool: list_files")
        if let Some(msg) = self.status_message {
            // Clean up common prefixes
            let cleaned = msg
                .strip_prefix("Tool: ")
                .or_else(|| msg.strip_prefix("Running: "))
                .unwrap_or(msg);
            return Some(cleaned.to_string());
        }

        None
    }
}
