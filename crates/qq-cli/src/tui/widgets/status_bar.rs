//! Status bar widget showing model, tokens, and status information.

use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph, Widget},
};

/// Status bar display state
pub struct StatusBar<'a> {
    model: &'a str,
    prompt_tokens: u32,
    completion_tokens: u32,
    iteration: u32,
    max_iterations: u32,
    is_streaming: bool,
    status_message: Option<&'a str>,
}

impl<'a> StatusBar<'a> {
    pub fn new(model: &'a str) -> Self {
        Self {
            model,
            prompt_tokens: 0,
            completion_tokens: 0,
            iteration: 0,
            max_iterations: 10,
            is_streaming: false,
            status_message: None,
        }
    }

    pub fn tokens(mut self, prompt: u32, completion: u32) -> Self {
        self.prompt_tokens = prompt;
        self.completion_tokens = completion;
        self
    }

    pub fn iteration(mut self, current: u32, max: u32) -> Self {
        self.iteration = current;
        self.max_iterations = max;
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
}

impl Widget for StatusBar<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let style_label = Style::default().fg(Color::DarkGray);
        let style_value = Style::default().fg(Color::White);
        let style_streaming = Style::default()
            .fg(Color::Green)
            .add_modifier(Modifier::BOLD);

        let mut spans = vec![
            Span::styled(" Model: ", style_label),
            Span::styled(self.model, style_value),
        ];

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

        // Show iteration if in multi-turn
        if self.iteration > 0 {
            spans.push(Span::styled(" | Iter: ", style_label));
            spans.push(Span::styled(
                format!("{}/{}", self.iteration, self.max_iterations),
                style_value,
            ));
        }

        // Show streaming indicator
        if self.is_streaming {
            spans.push(Span::styled(" | ", style_label));
            spans.push(Span::styled("STREAMING", style_streaming));
        }

        // Show status message if present
        if let Some(msg) = self.status_message {
            spans.push(Span::styled(" | ", style_label));
            spans.push(Span::styled(msg, Style::default().fg(Color::Yellow)));
        }

        let paragraph = Paragraph::new(Line::from(spans))
            .block(Block::default().borders(Borders::BOTTOM).border_style(
                Style::default().fg(Color::DarkGray),
            ));

        paragraph.render(area, buf);
    }
}
