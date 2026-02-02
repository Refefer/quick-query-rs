//! Scrollable content area widget for main response display.

use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Color, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph, Scrollbar, ScrollbarOrientation, ScrollbarState, StatefulWidget, Widget, Wrap},
};

use crate::tui::markdown::{markdown_to_lines, MarkdownStyles};

/// Main content display area with scrolling support
pub struct ContentArea<'a> {
    content: &'a str,
    scroll_offset: u16,
    is_streaming: bool,
    auto_scroll: bool,
}

impl<'a> ContentArea<'a> {
    pub fn new(content: &'a str) -> Self {
        Self {
            content,
            scroll_offset: 0,
            is_streaming: false,
            auto_scroll: true,
        }
    }

    pub fn scroll(mut self, offset: u16) -> Self {
        self.scroll_offset = offset;
        self
    }

    pub fn streaming(mut self, streaming: bool) -> Self {
        self.is_streaming = streaming;
        self
    }

    pub fn auto_scroll(mut self, auto: bool) -> Self {
        self.auto_scroll = auto;
        self
    }

    /// Calculate total lines when rendered at given width
    pub fn content_height(&self, width: u16) -> u16 {
        let styles = MarkdownStyles::default();
        let lines = markdown_to_lines(self.content, &styles);

        // Estimate wrapped line count
        let mut total = 0u16;
        for line in &lines {
            let line_len: usize = line.spans.iter().map(|s| s.content.len()).sum();
            let wrapped_lines = ((line_len as u16).max(1) + width.saturating_sub(1)) / width.max(1);
            total = total.saturating_add(wrapped_lines.max(1));
        }
        total
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

        let styles = MarkdownStyles::default();
        let lines = markdown_to_lines(self.content, &styles);

        // Estimate total wrapped lines for scroll calculations
        let inner_width = inner.width.max(1) as usize;
        let mut total_lines: u16 = 0;
        for line in &lines {
            let line_len: usize = line.spans.iter().map(|s| s.content.len()).sum();
            let wrapped = if line_len == 0 {
                1
            } else {
                (line_len + inner_width - 1) / inner_width
            };
            total_lines = total_lines.saturating_add(wrapped.max(1) as u16);
        }

        // Calculate scroll offset
        let effective_scroll = if self.auto_scroll {
            // Auto-scroll: if content exceeds viewport, scroll to show bottom
            if total_lines > inner.height {
                total_lines.saturating_sub(inner.height)
            } else {
                0
            }
        } else {
            // Manual scroll: respect user's scroll position
            self.scroll_offset.min(total_lines.saturating_sub(inner.height))
        };

        let paragraph = Paragraph::new(lines)
            .wrap(Wrap { trim: false })
            .scroll((effective_scroll, 0));

        paragraph.render(inner, buf);

        // Render scrollbar if content exceeds visible area
        if total_lines > inner.height {
            let scrollbar = Scrollbar::new(ScrollbarOrientation::VerticalRight)
                .begin_symbol(Some(""))
                .end_symbol(Some(""));

            let mut scrollbar_state = ScrollbarState::new(total_lines as usize)
                .position(effective_scroll as usize)
                .viewport_content_length(inner.height as usize);

            scrollbar.render(
                area.inner(ratatui::layout::Margin {
                    vertical: 1,
                    horizontal: 0,
                }),
                buf,
                &mut scrollbar_state,
            );

            // Show scroll position indicator
            if !self.auto_scroll && effective_scroll > 0 {
                let percent = (effective_scroll as f32 / (total_lines.saturating_sub(inner.height)) as f32 * 100.0) as u16;
                let indicator = format!(" {}% ", percent.min(100));
                let x = area.right().saturating_sub(indicator.len() as u16 + 1);
                let y = area.bottom().saturating_sub(1);
                if x >= area.left() {
                    buf.set_string(x, y, &indicator, Style::default().fg(Color::DarkGray));
                }
            }
        }
    }
}

/// Stateful content area that tracks scroll position
pub struct ContentAreaState {
    pub scroll_offset: u16,
    pub auto_scroll: bool,
    pub content_height: u16,
    pub viewport_height: u16,
}

impl Default for ContentAreaState {
    fn default() -> Self {
        Self {
            scroll_offset: 0,
            auto_scroll: true,
            content_height: 0,
            viewport_height: 0,
        }
    }
}

impl ContentAreaState {
    pub fn scroll_up(&mut self, amount: u16) {
        self.scroll_offset = self.scroll_offset.saturating_sub(amount);
        if self.scroll_offset > 0 {
            self.auto_scroll = false;
        }
    }

    pub fn scroll_down(&mut self, amount: u16) {
        let max_scroll = self.content_height.saturating_sub(self.viewport_height);
        self.scroll_offset = (self.scroll_offset + amount).min(max_scroll);

        // Re-enable auto-scroll if we're at the bottom
        if self.scroll_offset >= max_scroll {
            self.auto_scroll = true;
        }
    }

    pub fn scroll_to_top(&mut self) {
        self.scroll_offset = 0;
        self.auto_scroll = false;
    }

    pub fn scroll_to_bottom(&mut self) {
        let max_scroll = self.content_height.saturating_sub(self.viewport_height);
        self.scroll_offset = max_scroll;
        self.auto_scroll = true;
    }

    pub fn page_up(&mut self) {
        let amount = (self.viewport_height / 2).max(5);
        self.scroll_up(amount);
    }

    pub fn page_down(&mut self) {
        let amount = (self.viewport_height / 2).max(5);
        self.scroll_down(amount);
    }

    pub fn update_content_height(&mut self, height: u16) {
        self.content_height = height;
        if self.auto_scroll {
            let max_scroll = height.saturating_sub(self.viewport_height);
            self.scroll_offset = max_scroll;
        }
    }

    pub fn update_viewport(&mut self, height: u16) {
        self.viewport_height = height;
    }
}
