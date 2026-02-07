//! Markdown rendering for TUI using termimad and ansi-to-tui.
//!
//! This module converts markdown to ratatui `Text` by:
//! 1. Using termimad to render markdown to ANSI-styled strings
//! 2. Using ansi-to-tui to convert ANSI strings to ratatui `Text`

use ansi_to_tui::IntoText;
use ratatui::text::Text;

use crate::markdown::create_skin;

/// Render markdown to ratatui Text.
///
/// Uses termimad to render markdown to ANSI-styled output, then converts
/// that to ratatui Text using ansi-to-tui.
pub fn markdown_to_text(content: &str, width: Option<usize>) -> Text<'static> {
    let skin = create_skin();
    let processed = if let Some(w) = width {
        crate::markdown::preprocess_tables(content, w)
    } else {
        content.to_string()
    };
    let rendered = skin.text(&processed, width);
    let ansi_string = format!("{}", rendered);

    ansi_string
        .into_text()
        .unwrap_or_else(|_| Text::raw(content.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_simple_text() {
        let text = markdown_to_text("Hello world", Some(80));
        assert!(!text.lines.is_empty());
    }

    #[test]
    fn test_header() {
        let text = markdown_to_text("# Header", Some(80));
        assert!(!text.lines.is_empty());
    }

    #[test]
    fn test_code_block() {
        let content = "```rust\nlet x = 1;\n```";
        let text = markdown_to_text(content, Some(80));
        assert!(!text.lines.is_empty());
    }

    #[test]
    fn test_inline_code() {
        let text = markdown_to_text("Use `code` here", Some(80));
        assert!(!text.lines.is_empty());
    }

    #[test]
    fn test_table() {
        let content = "| Header 1 | Header 2 |\n|----------|----------|\n| Cell 1   | Cell 2   |";
        let text = markdown_to_text(content, Some(80));
        assert!(!text.lines.is_empty());
    }

    #[test]
    fn test_bold_and_italic() {
        let text = markdown_to_text("This is **bold** and *italic*", Some(80));
        assert!(!text.lines.is_empty());
    }

    #[test]
    fn test_list() {
        let content = "- Item 1\n- Item 2\n- Item 3";
        let text = markdown_to_text(content, Some(80));
        assert!(!text.lines.is_empty());
    }

    #[test]
    fn test_blockquote() {
        let text = markdown_to_text("> This is a quote", Some(80));
        assert!(!text.lines.is_empty());
    }

    #[test]
    fn test_empty_content() {
        let text = markdown_to_text("", Some(80));
        // Empty content should produce empty or minimal text
        assert!(text.lines.is_empty() || text.lines.len() == 1);
    }

    #[test]
    fn test_no_width() {
        let text = markdown_to_text("Hello world", None);
        assert!(!text.lines.is_empty());
    }
}
