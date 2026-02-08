//! Markdown rendering for TUI.
//!
//! Converts markdown to ratatui `Text` using the pulldown-cmark based renderer.

use ratatui::text::Text;

use crate::markdown::render_to_text;

/// Render markdown to ratatui Text.
pub fn markdown_to_text(content: &str, width: Option<usize>) -> Text<'static> {
    render_to_text(content, width.unwrap_or(80))
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
