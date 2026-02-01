//! Status bar widget showing model, tokens, and status information.

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
}

impl Widget for StatusBar<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let style_label = Style::default().fg(Color::DarkGray);
        let style_value = Style::default().fg(Color::White);
        let style_streaming = Style::default()
            .fg(Color::Green)
            .add_modifier(Modifier::BOLD);
        let style_context = Style::default().fg(Color::Cyan);
        let style_context_separator = Style::default().fg(Color::DarkGray);

        let mut spans = Vec::new();

        // Track if context is active (showing tool/agent info)
        let context_is_active = self.execution_context.map(|ctx| ctx.is_active()).unwrap_or(false);

        // Show execution context if active (more than just Chat)
        if let Some(ctx) = self.execution_context {
            if ctx.is_active() {
                // Format context stack with colored separators
                let context_str = ctx.format_blocking();
                spans.push(Span::styled(" ", style_label));

                // Split and colorize the context - collect into owned Strings
                let parts: Vec<String> = context_str.split(" > ").map(|s| s.to_string()).collect();
                for (i, part) in parts.iter().enumerate() {
                    if i > 0 {
                        spans.push(Span::styled(" > ", style_context_separator));
                    }
                    spans.push(Span::styled(part.clone(), style_context));
                }
            } else {
                // Just show profile when idle
                spans.push(Span::styled(" Profile: ", style_label));
                spans.push(Span::styled(self.profile, style_value));
            }
        } else {
            spans.push(Span::styled(" Profile: ", style_label));
            spans.push(Span::styled(self.profile, style_value));
        }

        // Show tokens if any
        let total = self.prompt_tokens + self.completion_tokens;
        if total > 0 {
            spans.push(Span::styled(" | Tokens: ", style_label));
            spans.push(Span::styled(format!("{}", total), style_value));
            spans.push(Span::styled(
                format!(" ({}p/{}c)", self.prompt_tokens, self.completion_tokens),
                Style::default().fg(Color::DarkGray),
            ));
        }

        // Show streaming indicator
        if self.is_streaming {
            spans.push(Span::styled(" | ", style_label));
            spans.push(Span::styled("STREAMING", style_streaming));
        }

        // Show status message only if context is NOT active
        // (context already shows what's running, so "Running: X" would be redundant)
        if !context_is_active {
            if let Some(msg) = self.status_message {
                spans.push(Span::styled(" | ", style_label));
                spans.push(Span::styled(msg, Style::default().fg(Color::Yellow)));
            }
        }

        let paragraph = Paragraph::new(Line::from(spans))
            .block(Block::default().borders(Borders::BOTTOM).border_style(
                Style::default().fg(Color::DarkGray),
            ));

        paragraph.render(area, buf);
    }
}
