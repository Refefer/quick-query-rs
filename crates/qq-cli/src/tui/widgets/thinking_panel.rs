//! Collapsible thinking/reasoning panel widget.

use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph, Widget, Wrap},
};

use crate::tui::markdown::{markdown_to_lines, MarkdownStyles};

/// Thinking panel that can be collapsed
pub struct ThinkingPanel<'a> {
    content: &'a str,
    is_collapsed: bool,
    is_streaming: bool,
}

impl<'a> ThinkingPanel<'a> {
    pub fn new(content: &'a str) -> Self {
        Self {
            content,
            is_collapsed: false,
            is_streaming: false,
        }
    }

    pub fn collapsed(mut self, collapsed: bool) -> Self {
        self.is_collapsed = collapsed;
        self
    }

    pub fn streaming(mut self, streaming: bool) -> Self {
        self.is_streaming = streaming;
        self
    }
}

impl Widget for ThinkingPanel<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let title_style = Style::default()
            .fg(Color::DarkGray)
            .add_modifier(Modifier::DIM);

        let title = if self.is_collapsed {
            " Thinking [+] "
        } else if self.is_streaming {
            " Thinking... "
        } else {
            " Thinking "
        };

        let block = Block::default()
            .title(Span::styled(title, title_style))
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::DarkGray));

        if self.is_collapsed {
            // Just render the block with collapsed indicator
            let collapsed_text = format!("({} chars) Press 't' to expand", self.content.len());
            let paragraph = Paragraph::new(Line::from(Span::styled(
                collapsed_text,
                Style::default().fg(Color::DarkGray),
            )))
            .block(block);
            paragraph.render(area, buf);
        } else {
            // Render content with dimmed styling
            let mut styles = MarkdownStyles::default();
            // Dim all styles for thinking content
            styles.normal = Style::default().fg(Color::DarkGray);
            styles.bold = Style::default()
                .fg(Color::Gray)
                .add_modifier(Modifier::BOLD);
            styles.italic = Style::default()
                .fg(Color::Gray)
                .add_modifier(Modifier::ITALIC);
            styles.code = Style::default().fg(Color::DarkGray);
            styles.header1 = Style::default().fg(Color::Gray);
            styles.header2 = Style::default().fg(Color::Gray);
            styles.header3 = Style::default().fg(Color::DarkGray);

            let lines = markdown_to_lines(self.content, &styles);

            let paragraph = Paragraph::new(lines).block(block).wrap(Wrap { trim: false });

            paragraph.render(area, buf);
        }
    }
}
