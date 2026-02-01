//! Input area widget with line editing support.

use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph, Widget},
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

    pub fn prompt(mut self, prompt: &'a str) -> Self {
        self.prompt = prompt;
        self
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

        // Calculate positions
        let prompt_len = self.prompt.len() as u16;
        let input_value = self.input.value();
        let cursor_pos = self.input.visual_cursor();

        // Build the line with prompt and input
        let prompt_style = if self.is_active {
            Style::default().fg(Color::Green).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::DarkGray)
        };

        let input_style = Style::default().fg(Color::White);

        // Calculate visible portion of input (handle overflow)
        let available_width = inner.width.saturating_sub(prompt_len) as usize;
        let (visible_input, cursor_offset) = if cursor_pos >= available_width {
            // Cursor would be off screen, scroll input
            let start = cursor_pos.saturating_sub(available_width / 2);
            let end = (start + available_width).min(input_value.len());
            (&input_value[start..end], cursor_pos - start)
        } else {
            let end = available_width.min(input_value.len());
            (&input_value[..end], cursor_pos)
        };

        let spans = vec![
            Span::styled(self.prompt, prompt_style),
            Span::styled(visible_input, input_style),
        ];

        let paragraph = Paragraph::new(Line::from(spans));
        paragraph.render(inner, buf);

        // Draw cursor if active
        if self.is_active {
            let cursor_x = inner.x + prompt_len + cursor_offset as u16;
            let cursor_y = inner.y;

            if cursor_x < inner.right() {
                // Get the character at cursor position or use space
                let cursor_char = if cursor_offset < visible_input.len() {
                    visible_input.chars().nth(cursor_offset).unwrap_or(' ')
                } else {
                    ' '
                };

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

        // Render hint line below input if there's space
        if let Some(hint) = self.hint {
            if inner.height > 1 {
                let hint_y = inner.y + 1;
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
