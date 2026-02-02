//! Input area widget with line editing support.

use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Widget},
};
use tui_input::Input;

/// Input area widget with basic line editing
pub struct InputArea<'a> {
    input: &'a Input,
    prompt: &'a str,
    is_active: bool,
    hint: Option<&'a str>,
}

impl<'a> InputArea<'a> {
    pub fn new(input: &'a Input) -> Self {
        Self {
            input,
            prompt: "you> ",
            is_active: true,
            hint: None,
        }
    }

    pub fn active(mut self, active: bool) -> Self {
        self.is_active = active;
        self
    }

    pub fn hint(mut self, hint: &'a str) -> Self {
        self.hint = Some(hint);
        self
    }
}

impl Widget for InputArea<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let block = Block::default()
            .borders(Borders::TOP)
            .border_style(Style::default().fg(Color::DarkGray));

        let inner = block.inner(area);
        block.render(area, buf);

        let prompt_len = self.prompt.len();
        let input_value = self.input.value();
        let cursor_pos = self.input.visual_cursor();

        let prompt_style = if self.is_active {
            Style::default().fg(Color::Green).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::DarkGray)
        };

        let input_style = Style::default().fg(Color::White);

        // Calculate available width for text after prompt (only on first line)
        let first_line_width = inner.width.saturating_sub(prompt_len as u16) as usize;
        let full_line_width = inner.width as usize;

        if first_line_width == 0 || full_line_width == 0 {
            return;
        }

        // Split input into wrapped lines
        let mut lines: Vec<Line> = Vec::new();
        let mut remaining = input_value;
        let mut is_first_line = true;

        while !remaining.is_empty() || is_first_line {
            let line_width = if is_first_line { first_line_width } else { full_line_width };

            let (line_text, rest) = if remaining.len() <= line_width {
                (remaining, "")
            } else {
                remaining.split_at(line_width)
            };

            if is_first_line {
                lines.push(Line::from(vec![
                    Span::styled(self.prompt, prompt_style),
                    Span::styled(line_text.to_string(), input_style),
                ]));
                is_first_line = false;
            } else {
                lines.push(Line::from(Span::styled(line_text.to_string(), input_style)));
            }

            remaining = rest;

            // Safety: break if we've rendered enough lines for the area
            if lines.len() >= inner.height.saturating_sub(1) as usize {
                break;
            }
        }

        // Handle empty input case
        if lines.is_empty() {
            lines.push(Line::from(Span::styled(self.prompt, prompt_style)));
        }

        // Calculate how many lines we have for input (reserve 1 for hint)
        let input_lines_available = inner.height.saturating_sub(1) as usize;
        let input_lines_count = lines.len().min(input_lines_available);

        // Render input lines
        for (i, line) in lines.iter().take(input_lines_count).enumerate() {
            let y = inner.y + i as u16;
            if y < inner.bottom() {
                buf.set_line(inner.x, y, line, inner.width);
            }
        }

        // Draw cursor if active
        if self.is_active {
            // Calculate cursor position accounting for wrapped lines
            let (cursor_line, cursor_col) = if cursor_pos <= first_line_width {
                (0, prompt_len + cursor_pos)
            } else {
                let pos_after_first = cursor_pos - first_line_width;
                let line_num = 1 + pos_after_first / full_line_width;
                let col = pos_after_first % full_line_width;
                (line_num, col)
            };

            let cursor_y = inner.y + cursor_line as u16;
            let cursor_x = inner.x + cursor_col as u16;

            if cursor_y < inner.bottom().saturating_sub(1) && cursor_x < inner.right() {
                // Get the character at cursor position or use space
                let cursor_char = input_value.chars().nth(cursor_pos).unwrap_or(' ');

                buf.set_string(
                    cursor_x,
                    cursor_y,
                    cursor_char.to_string(),
                    Style::default()
                        .bg(Color::White)
                        .fg(Color::Black),
                );
            }
        }

        // Render hint line at the bottom
        if let Some(hint) = self.hint {
            let hint_y = inner.y + inner.height.saturating_sub(1);
            if hint_y < inner.bottom() && hint_y > inner.y {
                buf.set_string(
                    inner.x,
                    hint_y,
                    hint,
                    Style::default().fg(Color::DarkGray),
                );
            }
        }
    }
}

/// Input history for up/down arrow navigation
#[derive(Clone)]
pub struct InputHistory {
    entries: Vec<String>,
    position: Option<usize>,
    current_input: String,
}

impl Default for InputHistory {
    fn default() -> Self {
        Self::new()
    }
}

impl InputHistory {
    pub fn new() -> Self {
        Self {
            entries: Vec::new(),
            position: None,
            current_input: String::new(),
        }
    }

    pub fn add(&mut self, entry: String) {
        if !entry.is_empty() {
            // Don't add duplicates of the last entry
            if self.entries.last() != Some(&entry) {
                self.entries.push(entry);
            }
        }
        self.position = None;
        self.current_input.clear();
    }

    pub fn navigate_up(&mut self, current: &str) -> Option<&str> {
        if self.entries.is_empty() {
            return None;
        }

        match self.position {
            None => {
                // First up press - save current input and go to last entry
                self.current_input = current.to_string();
                self.position = Some(self.entries.len() - 1);
                Some(&self.entries[self.entries.len() - 1])
            }
            Some(pos) if pos > 0 => {
                // Go to earlier entry
                self.position = Some(pos - 1);
                Some(&self.entries[pos - 1])
            }
            _ => {
                // Already at oldest entry
                Some(&self.entries[0])
            }
        }
    }

    pub fn navigate_down(&mut self, _current: &str) -> Option<&str> {
        match self.position {
            None => None, // Not navigating history
            Some(pos) if pos + 1 < self.entries.len() => {
                // Go to newer entry
                self.position = Some(pos + 1);
                Some(&self.entries[pos + 1])
            }
            Some(_) => {
                // At newest entry, return to current input
                self.position = None;
                Some(&self.current_input)
            }
        }
    }

    pub fn reset(&mut self) {
        self.position = None;
        self.current_input.clear();
    }
}
