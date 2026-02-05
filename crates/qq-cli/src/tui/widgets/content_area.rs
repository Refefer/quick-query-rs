//! Scrollable content area widget for main response display.

use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Color, Style},
    text::{Line, Span, Text},
    widgets::{Block, Borders, Paragraph, Widget, Wrap},
};

use crate::tui::markdown::markdown_to_text;

/// Main content display area with scrolling support.
/// Can accept either raw content (which will be parsed) or pre-rendered text (cached).
pub struct ContentArea<'a> {
    /// Raw content string (used if pre_rendered is None)
    content: &'a str,
    /// Pre-rendered text from cache (avoids re-parsing markdown)
    pre_rendered: Option<&'a Text<'static>>,
    scroll_offset: u16,
    is_streaming: bool,
}

impl<'a> ContentArea<'a> {
    pub fn new(content: &'a str) -> Self {
        Self {
            content,
            pre_rendered: None,
            scroll_offset: 0,
            is_streaming: false,
        }
    }

    /// Use pre-rendered text from cache instead of parsing markdown.
    /// This significantly improves performance for large content.
    pub fn with_cached_text(mut self, text: &'a Text<'static>) -> Self {
        self.pre_rendered = Some(text);
        self
    }

    pub fn scroll(mut self, offset: u16) -> Self {
        self.scroll_offset = offset;
        self
    }

    pub fn streaming(mut self, streaming: bool) -> Self {
        self.is_streaming = streaming;
        self
    }
}

impl Widget for ContentArea<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let title = if self.is_streaming {
            " Response... "
        } else if self.content.is_empty() {
            " Response "
        } else {
            " Response "
        };

        let block = Block::default()
            .title(Span::styled(title, Style::default().fg(Color::Cyan)))
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::DarkGray));

        let inner = block.inner(area);
        block.render(area, buf);

        if self.content.is_empty() {
            // Show placeholder
            let placeholder = if self.is_streaming {
                "Waiting for response..."
            } else {
                "Enter a message to start chatting"
            };
            let paragraph = Paragraph::new(Line::from(Span::styled(
                placeholder,
                Style::default().fg(Color::DarkGray),
            )));
            paragraph.render(inner, buf);
            return;
        }

        // Use pre-rendered text if available, otherwise parse markdown
        let text: Text<'_> = if let Some(cached) = self.pre_rendered {
            cached.clone()
        } else {
            let inner_width = inner.width.max(1) as usize;
            markdown_to_text(self.content, Some(inner_width))
        };

        // Calculate total lines for scroll calculations
        let total_lines = text.lines.len() as u16;

        // Use the scroll offset from ScrollState, clamping to valid range.
        // ScrollState is the single source of truth for scroll position.
        let max_scroll = total_lines.saturating_sub(inner.height);
        let effective_scroll = self.scroll_offset.min(max_scroll);

        let paragraph = Paragraph::new(text)
            .wrap(Wrap { trim: false })
            .scroll((effective_scroll, 0));

        paragraph.render(inner, buf);
    }
}
