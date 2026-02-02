//! Expandable thinking/reasoning panel widget with tool notifications.

use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph, Widget, Wrap},
};

/// Status of a tool notification.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolNotificationStatus {
    /// Tool call has started (arguments being streamed)
    Started,
    /// Tool is currently executing
    Executing,
    /// Tool completed successfully
    Completed,
    /// Tool execution failed
    Error,
}

/// A tool notification to display in the thinking panel.
#[derive(Debug, Clone)]
pub struct ToolNotification {
    pub tool_name: String,
    pub status: ToolNotificationStatus,
    /// Optional preview of arguments or result
    pub preview: String,
}

impl ToolNotification {
    pub fn new(tool_name: String, status: ToolNotificationStatus) -> Self {
        Self {
            tool_name,
            status,
            preview: String::new(),
        }
    }

    /// Get the status icon for this notification.
    fn icon(&self) -> (&'static str, Style) {
        match self.status {
            ToolNotificationStatus::Started => (
                "",
                Style::default().fg(Color::DarkGray),
            ),
            ToolNotificationStatus::Executing => (
                "",
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            ),
            ToolNotificationStatus::Completed => (
                "",
                Style::default().fg(Color::Green),
            ),
            ToolNotificationStatus::Error => (
                "",
                Style::default().fg(Color::Red),
            ),
        }
    }
}

/// Thinking panel that can be expanded to fullscreen.
/// Also displays tool notifications at the bottom.
pub struct ThinkingPanel<'a> {
    content: &'a str,
    tool_notifications: &'a [ToolNotification],
    is_expanded: bool,
    is_streaming: bool,
    auto_scroll: bool,
}

impl<'a> ThinkingPanel<'a> {
    pub fn new(content: &'a str) -> Self {
        Self {
            content,
            tool_notifications: &[],
            is_expanded: false,
            is_streaming: false,
            auto_scroll: true,
        }
    }

    pub fn tool_notifications(mut self, notifications: &'a [ToolNotification]) -> Self {
        self.tool_notifications = notifications;
        self
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

    /// Render tool notifications as lines.
    fn render_tool_notifications(&self) -> Vec<Line<'a>> {
        if self.tool_notifications.is_empty() {
            return Vec::new();
        }

        let mut lines = Vec::new();

        // Separator line
        lines.push(Line::from(Span::styled(
            "─ Tools ─────────────────────────────",
            Style::default().fg(Color::DarkGray),
        )));

        // Show up to 5 most recent notifications (newest first)
        let to_show: Vec<_> = self
            .tool_notifications
            .iter()
            .rev()
            .take(5)
            .collect();

        for notification in to_show {
            let (icon, icon_style) = notification.icon();
            let mut spans = vec![
                Span::styled(icon, icon_style),
                Span::styled(" ", Style::default()),
                Span::styled(
                    notification.tool_name.clone(),
                    Style::default().fg(Color::Yellow),
                ),
            ];

            if !notification.preview.is_empty() {
                // Truncate preview to avoid overflow
                let max_preview = 40;
                let preview = if notification.preview.len() > max_preview {
                    format!("{}...", &notification.preview[..max_preview])
                } else {
                    notification.preview.clone()
                };
                spans.push(Span::styled(
                    format!(" {}", preview),
                    Style::default().fg(Color::DarkGray),
                ));
            }

            lines.push(Line::from(spans));
        }

        // Show count if there are more
        if self.tool_notifications.len() > 5 {
            let hidden = self.tool_notifications.len() - 5;
            lines.push(Line::from(Span::styled(
                format!("  (+{} more)", hidden),
                Style::default().fg(Color::DarkGray),
            )));
        }

        lines
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
        let mut lines: Vec<Line> = self
            .content
            .lines()
            .map(|line| Line::from(Span::styled(line.to_string(), text_style)))
            .collect();

        // Add tool notifications at the bottom
        let tool_lines = self.render_tool_notifications();
        if !tool_lines.is_empty() {
            lines.push(Line::from("")); // Empty line separator
            lines.extend(tool_lines);
        }

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
