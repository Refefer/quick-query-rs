//! Simplified markdown styling for TUI.
//!
//! Provides inline styling without re-parsing - designed for streaming content.

use ratatui::{
    style::{Color, Modifier, Style},
    text::{Line, Span},
};

/// Style configuration for markdown elements
pub struct MarkdownStyles {
    pub normal: Style,
    pub bold: Style,
    pub italic: Style,
    pub code: Style,
    pub code_block: Style,
    pub header1: Style,
    pub header2: Style,
    pub header3: Style,
    pub link: Style,
    pub list_marker: Style,
    pub quote: Style,
}

impl Default for MarkdownStyles {
    fn default() -> Self {
        Self {
            normal: Style::default(),
            bold: Style::default().add_modifier(Modifier::BOLD),
            italic: Style::default().add_modifier(Modifier::ITALIC).fg(Color::Magenta),
            code: Style::default().fg(Color::Yellow),
            code_block: Style::default().fg(Color::Yellow),
            header1: Style::default().fg(Color::Green).add_modifier(Modifier::BOLD),
            header2: Style::default().fg(Color::Green),
            header3: Style::default().fg(Color::Cyan),
            link: Style::default().fg(Color::Blue).add_modifier(Modifier::UNDERLINED),
            list_marker: Style::default().fg(Color::Cyan),
            quote: Style::default().fg(Color::DarkGray).add_modifier(Modifier::ITALIC),
        }
    }
}

/// Convert markdown text to styled Lines for ratatui.
/// This is a simplified parser that handles common inline elements.
pub fn markdown_to_lines(text: &str, styles: &MarkdownStyles) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    let mut in_code_block = false;

    for line_text in text.lines() {
        if line_text.starts_with("```") {
            in_code_block = !in_code_block;
            if in_code_block {
                // Start of code block - optionally show language
                let lang = line_text.trim_start_matches("```").trim();
                if !lang.is_empty() {
                    lines.push(Line::from(Span::styled(
                        format!("  {}", lang),
                        Style::default().fg(Color::DarkGray),
                    )));
                }
            }
            continue;
        }

        if in_code_block {
            // Code block content - just style it
            lines.push(Line::from(Span::styled(
                format!("  {}", line_text),
                styles.code_block,
            )));
            continue;
        }

        // Check for headers
        if line_text.starts_with("### ") {
            lines.push(Line::from(Span::styled(
                line_text[4..].to_string(),
                styles.header3,
            )));
            continue;
        }
        if line_text.starts_with("## ") {
            lines.push(Line::from(Span::styled(
                line_text[3..].to_string(),
                styles.header2,
            )));
            continue;
        }
        if line_text.starts_with("# ") {
            lines.push(Line::from(Span::styled(
                line_text[2..].to_string(),
                styles.header1,
            )));
            continue;
        }

        // Check for blockquote
        if line_text.starts_with("> ") {
            let content = &line_text[2..];
            lines.push(Line::from(vec![
                Span::styled("  ", styles.quote),
                Span::styled(content.to_string(), styles.quote),
            ]));
            continue;
        }

        // Check for list items
        if line_text.starts_with("- ") || line_text.starts_with("* ") {
            let content = &line_text[2..];
            let styled_content = style_inline(content, styles);
            let mut spans = vec![Span::styled("  ", styles.list_marker)];
            spans.extend(styled_content);
            lines.push(Line::from(spans));
            continue;
        }

        // Check for numbered list
        if let Some(rest) = parse_numbered_list(line_text) {
            let styled_content = style_inline(rest, styles);
            let mut spans = vec![Span::styled("  ", styles.list_marker)];
            spans.extend(styled_content);
            lines.push(Line::from(spans));
            continue;
        }

        // Regular line with inline styling
        let spans = style_inline(line_text, styles);
        lines.push(Line::from(spans));
    }

    // Handle trailing code block
    if in_code_block {
        // Unclosed code block - that's fine for streaming
    }

    lines
}

/// Check if line starts with "N. " pattern
fn parse_numbered_list(line: &str) -> Option<&str> {
    let mut chars = line.char_indices();

    // Skip digits
    loop {
        match chars.next() {
            Some((_, c)) if c.is_ascii_digit() => continue,
            Some((i, '.')) => {
                // Check for space after
                if let Some((_, ' ')) = chars.next() {
                    return Some(&line[i + 2..]);
                }
                return None;
            }
            _ => return None,
        }
    }
}

/// Apply inline styling (bold, italic, code) to a line.
fn style_inline(text: &str, styles: &MarkdownStyles) -> Vec<Span<'static>> {
    let mut spans = Vec::new();
    let mut current = String::new();
    let mut chars = text.chars().peekable();

    while let Some(c) = chars.next() {
        match c {
            '`' => {
                // Inline code
                if !current.is_empty() {
                    spans.push(Span::styled(std::mem::take(&mut current), styles.normal));
                }

                // Collect until closing backtick
                let mut code = String::new();
                while let Some(&next) = chars.peek() {
                    if next == '`' {
                        chars.next();
                        break;
                    }
                    code.push(chars.next().unwrap());
                }
                if !code.is_empty() {
                    spans.push(Span::styled(code, styles.code));
                }
            }
            '*' | '_' => {
                // Check for bold (**) or italic (*)
                let is_double = chars.peek() == Some(&c);

                if is_double {
                    chars.next(); // consume second */_
                    if !current.is_empty() {
                        spans.push(Span::styled(std::mem::take(&mut current), styles.normal));
                    }

                    // Bold - collect until **
                    let mut bold_text = String::new();
                    while let Some(next) = chars.next() {
                        if next == c {
                            if chars.peek() == Some(&c) {
                                chars.next();
                                break;
                            }
                        }
                        bold_text.push(next);
                    }
                    if !bold_text.is_empty() {
                        spans.push(Span::styled(bold_text, styles.bold));
                    }
                } else {
                    if !current.is_empty() {
                        spans.push(Span::styled(std::mem::take(&mut current), styles.normal));
                    }

                    // Italic - collect until single */_
                    let mut italic_text = String::new();
                    while let Some(next) = chars.next() {
                        if next == c {
                            break;
                        }
                        italic_text.push(next);
                    }
                    if !italic_text.is_empty() {
                        spans.push(Span::styled(italic_text, styles.italic));
                    }
                }
            }
            '[' => {
                // Check for link [text](url)
                if !current.is_empty() {
                    spans.push(Span::styled(std::mem::take(&mut current), styles.normal));
                }

                let mut link_text = String::new();
                let mut found_close = false;
                while let Some(next) = chars.next() {
                    if next == ']' {
                        found_close = true;
                        break;
                    }
                    link_text.push(next);
                }

                if found_close && chars.peek() == Some(&'(') {
                    chars.next(); // consume (
                    // Skip the URL
                    while let Some(next) = chars.next() {
                        if next == ')' {
                            break;
                        }
                    }
                    spans.push(Span::styled(link_text, styles.link));
                } else {
                    // Not a link, just regular text
                    current.push('[');
                    current.push_str(&link_text);
                    if found_close {
                        current.push(']');
                    }
                }
            }
            _ => {
                current.push(c);
            }
        }
    }

    if !current.is_empty() {
        spans.push(Span::styled(current, styles.normal));
    }

    if spans.is_empty() {
        spans.push(Span::raw(""));
    }

    spans
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_simple_text() {
        let styles = MarkdownStyles::default();
        let lines = markdown_to_lines("Hello world", &styles);
        assert_eq!(lines.len(), 1);
    }

    #[test]
    fn test_header() {
        let styles = MarkdownStyles::default();
        let lines = markdown_to_lines("# Header", &styles);
        assert_eq!(lines.len(), 1);
    }

    #[test]
    fn test_code_block() {
        let styles = MarkdownStyles::default();
        let text = "```rust\nlet x = 1;\n```";
        let lines = markdown_to_lines(text, &styles);
        assert_eq!(lines.len(), 2); // language hint + code line
    }

    #[test]
    fn test_inline_code() {
        let styles = MarkdownStyles::default();
        let lines = markdown_to_lines("Use `code` here", &styles);
        assert_eq!(lines.len(), 1);
    }
}
