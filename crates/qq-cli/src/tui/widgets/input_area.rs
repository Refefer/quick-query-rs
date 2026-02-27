//! Input area widget with line editing support.

use std::path::PathBuf;

use chrono::{DateTime, Utc};
use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Widget},
};
use unicode_width::UnicodeWidthChar;
use serde::{Deserialize, Serialize};
use tui_input::Input;

/// Maximum number of history entries to persist
const MAX_HISTORY_ENTRIES: usize = 1000;

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

            let (line_text, rest) = {
                let mut width = 0;
                let mut split_byte = remaining.len();
                for (byte_idx, ch) in remaining.char_indices() {
                    let ch_width = UnicodeWidthChar::width(ch).unwrap_or(0);
                    if width + ch_width > line_width {
                        split_byte = byte_idx;
                        break;
                    }
                    width += ch_width;
                }
                remaining.split_at(split_byte)
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

/// A single history entry with metadata
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct HistoryEntry {
    pub text: String,
    pub timestamp: DateTime<Utc>,
}

impl HistoryEntry {
    pub fn new(text: String) -> Self {
        Self {
            text,
            timestamp: Utc::now(),
        }
    }
}

/// File format for persisted history
#[derive(Clone, Debug, Serialize, Deserialize)]
struct PersistedHistory {
    version: u32,
    entries: Vec<HistoryEntry>,
}

/// Input history for up/down arrow navigation
#[derive(Clone)]
pub struct InputHistory {
    entries: Vec<HistoryEntry>,
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

    /// Returns the default history file path: ~/.config/qq/input_history.json
    fn history_file_path() -> Option<PathBuf> {
        dirs::config_dir().map(|p| p.join("qq").join("input_history.json"))
    }

    /// Load history from the default path, returning empty history on any error
    pub fn load() -> Self {
        Self::load_from_path(Self::history_file_path())
    }

    /// Load history from a specific path (for testing)
    pub fn load_from_path(path: Option<PathBuf>) -> Self {
        let Some(path) = path else {
            return Self::new();
        };

        let content = match std::fs::read_to_string(&path) {
            Ok(c) => c,
            Err(_) => return Self::new(),
        };

        let persisted: PersistedHistory = match serde_json::from_str(&content) {
            Ok(p) => p,
            Err(_) => return Self::new(),
        };

        Self {
            entries: persisted.entries,
            position: None,
            current_input: String::new(),
        }
    }

    /// Save history to the default path, silently ignoring errors
    pub fn save(&self) {
        self.save_to_path(Self::history_file_path());
    }

    /// Save history to a specific path (for testing)
    pub fn save_to_path(&self, path: Option<PathBuf>) {
        let Some(path) = path else {
            return;
        };

        // Create parent directory if needed
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }

        let persisted = PersistedHistory {
            version: 1,
            entries: self.entries.clone(),
        };

        if let Ok(content) = serde_json::to_string_pretty(&persisted) {
            let _ = std::fs::write(&path, content);
        }
    }

    pub fn add(&mut self, entry: String) {
        // Skip empty/whitespace-only entries
        if entry.trim().is_empty() {
            self.position = None;
            self.current_input.clear();
            return;
        }

        // Remove existing entry with same text (full deduplication)
        self.entries.retain(|e| e.text != entry);

        // Add new entry with current timestamp
        self.entries.push(HistoryEntry::new(entry));

        // Prune oldest entries if over limit
        if self.entries.len() > MAX_HISTORY_ENTRIES {
            let excess = self.entries.len() - MAX_HISTORY_ENTRIES;
            self.entries.drain(0..excess);
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
                Some(&self.entries[self.entries.len() - 1].text)
            }
            Some(pos) if pos > 0 => {
                // Go to earlier entry
                self.position = Some(pos - 1);
                Some(&self.entries[pos - 1].text)
            }
            _ => {
                // Already at oldest entry
                Some(&self.entries[0].text)
            }
        }
    }

    pub fn navigate_down(&mut self, _current: &str) -> Option<&str> {
        match self.position {
            None => None, // Not navigating history
            Some(pos) if pos + 1 < self.entries.len() => {
                // Go to newer entry
                self.position = Some(pos + 1);
                Some(&self.entries[pos + 1].text)
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

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_history_entry_creation() {
        let before = Utc::now();
        let entry = HistoryEntry::new("test query".to_string());
        let after = Utc::now();

        assert_eq!(entry.text, "test query");
        assert!(entry.timestamp >= before);
        assert!(entry.timestamp <= after);
    }

    #[test]
    fn test_add_and_navigate() {
        let mut history = InputHistory::new();
        history.add("first".to_string());
        history.add("second".to_string());
        history.add("third".to_string());

        // Navigate up through history
        assert_eq!(history.navigate_up("current"), Some("third"));
        assert_eq!(history.navigate_up("current"), Some("second"));
        assert_eq!(history.navigate_up("current"), Some("first"));
        // At oldest, should stay at first
        assert_eq!(history.navigate_up("current"), Some("first"));

        // Navigate back down
        assert_eq!(history.navigate_down("current"), Some("second"));
        assert_eq!(history.navigate_down("current"), Some("third"));
        // Past newest, back to current input
        assert_eq!(history.navigate_down("current"), Some("current"));
    }

    #[test]
    fn test_deduplication() {
        let mut history = InputHistory::new();
        history.add("first".to_string());
        history.add("second".to_string());
        history.add("first".to_string()); // Re-add "first"

        // Should have 2 entries, with "first" moved to the end
        assert_eq!(history.entries.len(), 2);
        assert_eq!(history.entries[0].text, "second");
        assert_eq!(history.entries[1].text, "first");
    }

    #[test]
    fn test_max_entries_pruning() {
        let mut history = InputHistory::new();

        // Add more than MAX_HISTORY_ENTRIES
        for i in 0..MAX_HISTORY_ENTRIES + 50 {
            history.add(format!("entry {}", i));
        }

        assert_eq!(history.entries.len(), MAX_HISTORY_ENTRIES);
        // Oldest entries should be pruned
        assert_eq!(history.entries[0].text, "entry 50");
        assert_eq!(
            history.entries[MAX_HISTORY_ENTRIES - 1].text,
            format!("entry {}", MAX_HISTORY_ENTRIES + 49)
        );
    }

    #[test]
    fn test_persistence_roundtrip() {
        let temp_dir = TempDir::new().unwrap();
        let path = temp_dir.path().join("test_history.json");

        // Create and save history
        let mut history = InputHistory::new();
        history.add("query one".to_string());
        history.add("query two".to_string());
        history.save_to_path(Some(path.clone()));

        // Load history
        let loaded = InputHistory::load_from_path(Some(path));

        assert_eq!(loaded.entries.len(), 2);
        assert_eq!(loaded.entries[0].text, "query one");
        assert_eq!(loaded.entries[1].text, "query two");
    }

    #[test]
    fn test_load_missing_file() {
        let temp_dir = TempDir::new().unwrap();
        let path = temp_dir.path().join("nonexistent.json");

        let history = InputHistory::load_from_path(Some(path));
        assert!(history.entries.is_empty());
    }

    #[test]
    fn test_load_corrupt_json() {
        let temp_dir = TempDir::new().unwrap();
        let path = temp_dir.path().join("corrupt.json");

        std::fs::write(&path, "not valid json {{{").unwrap();

        let history = InputHistory::load_from_path(Some(path));
        assert!(history.entries.is_empty());
    }

    #[test]
    fn test_empty_input_not_added() {
        let mut history = InputHistory::new();
        history.add("valid".to_string());
        history.add("".to_string());
        history.add("   ".to_string());
        history.add("\t\n".to_string());

        assert_eq!(history.entries.len(), 1);
        assert_eq!(history.entries[0].text, "valid");
    }
}
