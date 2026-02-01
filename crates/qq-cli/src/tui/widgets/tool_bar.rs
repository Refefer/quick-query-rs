//! Tool bar widget showing current tool call activity.

use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph, Widget},
};

/// A pending or completed tool call for display
#[derive(Clone)]
pub struct ToolCallInfo {
    pub name: String,
    pub args_preview: String,
    pub status: ToolStatus,
}

#[derive(Clone, PartialEq)]
pub enum ToolStatus {
    Pending,
    Executing,
    Complete,
    Error,
}

/// Tool bar showing current/recent tool calls
pub struct ToolBar<'a> {
    tools: &'a [ToolCallInfo],
    max_display: usize,
}

impl<'a> ToolBar<'a> {
    pub fn new(tools: &'a [ToolCallInfo]) -> Self {
        Self {
            tools,
            max_display: 3,
        }
    }

    pub fn max_display(mut self, max: usize) -> Self {
        self.max_display = max;
        self
    }
}

impl Widget for ToolBar<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let block = Block::default()
            .borders(Borders::TOP)
            .border_style(Style::default().fg(Color::DarkGray));

        let inner = block.inner(area);
        block.render(area, buf);

        if self.tools.is_empty() {
            return;
        }

        // Show most recent tools (up to max_display)
        let tools_to_show: Vec<_> = self
            .tools
            .iter()
            .rev()
            .take(self.max_display)
            .rev()
            .collect();

        let mut spans = Vec::new();

        for (i, tool) in tools_to_show.iter().enumerate() {
            if i > 0 {
                spans.push(Span::styled(" | ", Style::default().fg(Color::DarkGray)));
            }

            let (icon, icon_style) = match tool.status {
                ToolStatus::Pending => ("", Style::default().fg(Color::DarkGray)),
                ToolStatus::Executing => (
                    "",
                    Style::default()
                        .fg(Color::Yellow)
                        .add_modifier(Modifier::BOLD),
                ),
                ToolStatus::Complete => ("", Style::default().fg(Color::Green)),
                ToolStatus::Error => ("", Style::default().fg(Color::Red)),
            };

            spans.push(Span::styled(icon, icon_style));
            spans.push(Span::styled(" ", Style::default()));
            spans.push(Span::styled(
                &tool.name,
                Style::default().fg(Color::Yellow),
            ));

            if !tool.args_preview.is_empty() {
                // Truncate args preview to fit
                let max_args_len = 40;
                let preview = if tool.args_preview.len() > max_args_len {
                    format!("{}...", &tool.args_preview[..max_args_len])
                } else {
                    tool.args_preview.clone()
                };
                spans.push(Span::styled(
                    format!(" {}", preview),
                    Style::default().fg(Color::DarkGray),
                ));
            }
        }

        // Show count if there are more tools
        if self.tools.len() > self.max_display {
            let hidden = self.tools.len() - self.max_display;
            spans.push(Span::styled(
                format!(" (+{} more)", hidden),
                Style::default().fg(Color::DarkGray),
            ));
        }

        let paragraph = Paragraph::new(Line::from(spans));
        paragraph.render(inner, buf);
    }
}

/// Format tool call arguments for display preview
pub fn format_tool_args(args: &serde_json::Value) -> String {
    match args {
        serde_json::Value::Object(map) => {
            let parts: Vec<String> = map
                .iter()
                .take(3) // Limit to first 3 arguments
                .map(|(k, v)| {
                    let val = match v {
                        serde_json::Value::String(s) => {
                            if s.len() > 20 {
                                format!("\"{}...\"", &s[..17])
                            } else {
                                format!("\"{}\"", s)
                            }
                        }
                        serde_json::Value::Array(arr) => format!("[{} items]", arr.len()),
                        other => {
                            let s = other.to_string();
                            if s.len() > 20 {
                                format!("{}...", &s[..17])
                            } else {
                                s
                            }
                        }
                    };
                    format!("{}={}", k, val)
                })
                .collect();

            let result = parts.join(", ");
            if map.len() > 3 {
                format!("{}, ...", result)
            } else {
                result
            }
        }
        serde_json::Value::Null => String::new(),
        other => {
            let s = other.to_string();
            if s.len() > 50 {
                format!("{}...", &s[..47])
            } else {
                s
            }
        }
    }
}
