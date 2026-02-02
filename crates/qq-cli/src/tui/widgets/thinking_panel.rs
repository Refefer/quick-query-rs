//! Expandable thinking/reasoning panel widget.

use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph, Widget, Wrap},
};

/// Thinking panel that can be expanded to fullscreen
pub struct ThinkingPanel<'a> {
    content: &'a str,
    is_expanded: bool,
    is_streaming: bool,
    auto_scroll: bool,
}

impl<'a> ThinkingPanel<'a> {
    pub fn new(content: &'a str) -> Self {
        Self {
            content,
            is_expanded: false,
            is_streaming: false,
            auto_scroll: true,
        }
    }

    pub fn expanded(mut self, expanded: bool) -> Self {
        self.is_expanded = expanded;
        self
    }

    pub fn streaming(mut self, streaming: bool) -> Self {
        self.is_streaming = streaming;
        self
    }

    pub fn auto_scroll(mut self, auto_scroll: bool) -> Self {
        self.auto_scroll = auto_scroll;
        self
    }
}

impl Widget for ThinkingPanel<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let title_style = Style::default()
            .fg(Color::DarkGray)
            .add_modifier(Modifier::DIM);

        // Show expansion state and streaming status in title
        let title = if self.is_streaming {
            if self.is_expanded {
                " Thinking... [Ctrl+T to shrink] "
            } else {
                " Thinking... [Ctrl+T to expand] "
            }
        } else if self.is_expanded {
            " Thinking [Ctrl+T to shrink] "
        } else {
            " Thinking [Ctrl+T to expand] "
        };

        let block = Block::default()
            .title(Span::styled(title, title_style))
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::DarkGray));

        // Render thinking content as plain text (no markdown parsing)
        // Thinking content is raw model output, not formatted markdown
        let text_style = Style::default().fg(Color::DarkGray);
        let lines: Vec<Line> = self.content
            .lines()
            .map(|line| Line::from(Span::styled(line.to_string(), text_style)))
            .collect();

        // Calculate scroll offset for auto-scroll
        // Inner area is area minus borders (2 for top/bottom, 2 for left/right)
        let inner_height = area.height.saturating_sub(2);
        let inner_width = area.width.saturating_sub(2).max(1) as usize;

        // Estimate wrapped line count by calculating how many display lines
        // each logical line will take when wrapped
        let wrapped_height: u16 = lines
            .iter()
            .map(|line| {
                let line_width: usize = line.spans.iter().map(|s| s.content.len()).sum();
                if line_width == 0 {
                    1 // Empty lines still take 1 row
                } else {
                    ((line_width + inner_width - 1) / inner_width) as u16
                }
            })
            .sum();

        let scroll_offset = if self.auto_scroll && wrapped_height > inner_height {
            wrapped_height.saturating_sub(inner_height)
        } else {
            0
        };

        let paragraph = Paragraph::new(lines)
            .block(block)
            .wrap(Wrap { trim: false })
            .scroll((scroll_offset, 0));

        paragraph.render(area, buf);
    }
}
