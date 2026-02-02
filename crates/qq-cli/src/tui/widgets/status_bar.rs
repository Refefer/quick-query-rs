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
    status_message: Option<&'a str>,
    execution_context: Option<&'a ExecutionContext>,
    tool_iteration: u32,
    /// Agent progress: (agent_name, current_iteration, max_iterations)
    agent_progress: Option<(&'a str, u32, u32)>,
}

impl<'a> StatusBar<'a> {
    pub fn new(profile: &'a str) -> Self {
        Self {
            profile,
            prompt_tokens: 0,
            completion_tokens: 0,
            is_streaming: false,
            status_message: None,
            execution_context: None,
            tool_iteration: 0,
            agent_progress: None,
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

        let mut spans = Vec::new();

        // Always show profile first
        spans.push(Span::styled(" ", style_dim));
        spans.push(Span::styled(self.profile, style_profile));

        // Show activity: either from execution context or status message
        let activity = self.get_activity();
        if let Some(activity) = activity {
            spans.push(Span::styled(" › ", style_dim));
            spans.push(Span::styled(activity, style_activity));
        }

        // Show agent progress if an agent is running
        if let Some((agent_name, iteration, max_iterations)) = self.agent_progress {
            let style_agent = Style::default().fg(Color::Magenta);
            spans.push(Span::styled(" › ", style_dim));
            spans.push(Span::styled(
                format!("Agent[{}] turn {}/{}", agent_name, iteration, max_iterations),
                style_agent,
            ));
        } else if self.tool_iteration > 1 {
            // Show iteration count if in a tool loop (but not when agent is running)
            spans.push(Span::styled(format!(" (turn {})", self.tool_iteration), style_dim));
        }

        // Streaming indicator
        if self.is_streaming {
            spans.push(Span::styled(" ", style_dim));
            spans.push(Span::styled("●", style_streaming));
        }

        // Push tokens to the right side
        let total = self.prompt_tokens + self.completion_tokens;
        if total > 0 {
            // Calculate space needed for right-aligned tokens
            let tokens_text = format!("{}t", total);
            let tokens_detail = format!(" ({}p/{}c)", self.prompt_tokens, self.completion_tokens);

            // Add flexible space before tokens
            let current_len: usize = spans.iter().map(|s| s.content.len()).sum();
            let tokens_len = tokens_text.len() + tokens_detail.len() + 2; // +2 for padding
            let available = area.width as usize;

            if current_len + tokens_len < available {
                let padding = available - current_len - tokens_len;
                spans.push(Span::styled(" ".repeat(padding), style_dim));
            } else {
                spans.push(Span::styled(" | ", style_dim));
            }

            spans.push(Span::styled(tokens_text, style_tokens));
            spans.push(Span::styled(tokens_detail, style_dim));
            spans.push(Span::styled(" ", style_dim));
        }

        let paragraph = Paragraph::new(Line::from(spans))
            .block(Block::default().borders(Borders::BOTTOM).border_style(style_dim));

        paragraph.render(area, buf);
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
