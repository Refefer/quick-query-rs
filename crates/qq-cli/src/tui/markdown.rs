//! Simplified markdown styling for TUI.
//!
//! Provides inline styling without re-parsing - designed for streaming content.

use ratatui::{
    style::{Color, Modifier, Style},
    text::{Line, Span},
};
use unicode_width::UnicodeWidthStr;

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
    pub table_border: Style,
    pub table_header: Style,
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
            table_border: Style::default().fg(Color::DarkGray),
            table_header: Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD),
        }
    }
}

/// A buffered table row for column width calculation
#[derive(Debug)]
struct TableRow {
    cells: Vec<String>,
    is_separator: bool,
    is_header: bool,
}

/// Convert markdown text to styled Lines for ratatui.
/// This is a simplified parser that handles common inline elements.
pub fn markdown_to_lines(text: &str, styles: &MarkdownStyles) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    let mut in_code_block = false;
    let mut table_buffer: Vec<TableRow> = Vec::new();
    let mut is_table_header = true;

    for line_text in text.lines() {
        if line_text.starts_with("```") {
            // Flush any buffered table before code block
            if !table_buffer.is_empty() {
                lines.extend(render_buffered_table(&table_buffer, styles));
                table_buffer.clear();
                is_table_header = true;
            }

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

        // Check for table row (starts with |)
        let trimmed = line_text.trim();
        if trimmed.starts_with('|') || (!table_buffer.is_empty() && trimmed.contains('|')) {
            // Check if this is a separator row (|---|---|)
            if is_table_separator(trimmed) {
                table_buffer.push(TableRow {
                    cells: parse_table_cells(trimmed),
                    is_separator: true,
                    is_header: false,
                });
                is_table_header = false;
                continue;
            }

            // Regular table row
            table_buffer.push(TableRow {
                cells: parse_table_cells(trimmed),
                is_separator: false,
                is_header: is_table_header,
            });
            continue;
        } else if !table_buffer.is_empty() {
            // Exiting table - render buffered table
            lines.extend(render_buffered_table(&table_buffer, styles));
            table_buffer.clear();
            is_table_header = true;
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

    // Flush any remaining buffered table
    if !table_buffer.is_empty() {
        lines.extend(render_buffered_table(&table_buffer, styles));
    }

    lines
}

/// Parse table cells from a row, stripping the leading/trailing |
fn parse_table_cells(line: &str) -> Vec<String> {
    let trimmed = line.trim();
    // Remove leading and trailing |
    let inner = trimmed
        .strip_prefix('|')
        .unwrap_or(trimmed)
        .strip_suffix('|')
        .unwrap_or(trimmed);

    inner
        .split('|')
        .map(|cell| cell.trim().to_string())
        .collect()
}

/// Render a buffered table with calculated column widths
fn render_buffered_table(rows: &[TableRow], styles: &MarkdownStyles) -> Vec<Line<'static>> {
    if rows.is_empty() {
        return Vec::new();
    }

    // Calculate max width for each column (using unicode width)
    let num_cols = rows.iter().map(|r| r.cells.len()).max().unwrap_or(0);
    let mut col_widths: Vec<usize> = vec![0; num_cols];

    for row in rows {
        if row.is_separator {
            continue; // Don't count separator dashes in width calculation
        }
        for (i, cell) in row.cells.iter().enumerate() {
            if i < col_widths.len() {
                let width = UnicodeWidthStr::width(cell.as_str());
                col_widths[i] = col_widths[i].max(width);
            }
        }
    }

    // Render each row with padded cells
    let mut lines = Vec::new();
    for row in rows {
        if row.is_separator {
            lines.push(render_separator_row(&col_widths, styles));
        } else {
            lines.push(render_data_row(&row.cells, &col_widths, row.is_header, styles));
        }
    }

    lines
}

/// Render a separator row with proper column widths
fn render_separator_row(col_widths: &[usize], styles: &MarkdownStyles) -> Line<'static> {
    let mut spans = Vec::new();
    spans.push(Span::styled("|", styles.table_border));

    for (i, &width) in col_widths.iter().enumerate() {
        // Add padding (1 space each side) + dashes for content
        let dashes = "-".repeat(width + 2);
        spans.push(Span::styled(dashes, styles.table_border));
        spans.push(Span::styled("|", styles.table_border));

        // Don't add trailing separator after last column
        if i == col_widths.len() - 1 {
            break;
        }
    }

    Line::from(spans)
}

/// Render a data row with padded cells
fn render_data_row(
    cells: &[String],
    col_widths: &[usize],
    is_header: bool,
    styles: &MarkdownStyles,
) -> Line<'static> {
    let mut spans = Vec::new();
    let cell_style = if is_header {
        styles.table_header
    } else {
        styles.normal
    };

    spans.push(Span::styled("|", styles.table_border));

    for (i, width) in col_widths.iter().enumerate() {
        let cell_content = cells.get(i).map(|s| s.as_str()).unwrap_or("");
        let cell_width = UnicodeWidthStr::width(cell_content);
        let padding = width.saturating_sub(cell_width);

        // Add space, content, padding, space
        spans.push(Span::styled(" ", styles.normal));
        spans.push(Span::styled(cell_content.to_string(), cell_style));
        if padding > 0 {
            spans.push(Span::styled(" ".repeat(padding), styles.normal));
        }
        spans.push(Span::styled(" ", styles.normal));
        spans.push(Span::styled("|", styles.table_border));
    }

    Line::from(spans)
}

/// Check if a line is a table separator (e.g., |---|---|)
fn is_table_separator(line: &str) -> bool {
    let trimmed = line.trim().trim_matches('|').trim();
    if trimmed.is_empty() {
        return false;
    }
    // A separator contains only -, :, |, and whitespace
    trimmed.chars().all(|c| c == '-' || c == ':' || c == '|' || c.is_whitespace())
        && trimmed.contains('-')
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

    #[test]
    fn test_table() {
        let styles = MarkdownStyles::default();
        let text = "| Header 1 | Header 2 |\n|----------|----------|\n| Cell 1   | Cell 2   |";
        let lines = markdown_to_lines(text, &styles);
        assert_eq!(lines.len(), 3); // header, separator, data row
    }

    #[test]
    fn test_table_separator_detection() {
        assert!(is_table_separator("|---|---|"));
        assert!(is_table_separator("| --- | --- |"));
        assert!(is_table_separator("|:---:|:---:|"));
        assert!(!is_table_separator("| text | text |"));
        assert!(!is_table_separator("not a table"));
    }

    #[test]
    fn test_table_column_alignment() {
        let styles = MarkdownStyles::default();
        // Table with varying cell widths
        let text = "| ID | Name |\n|---|---|\n| 1 | Short |\n| 2 | Much longer name |";
        let lines = markdown_to_lines(text, &styles);

        // All rows should have the same rendered width
        let widths: Vec<usize> = lines
            .iter()
            .map(|line| {
                line.spans
                    .iter()
                    .map(|s| UnicodeWidthStr::width(s.content.as_ref()))
                    .sum()
            })
            .collect();

        // All rows should be the same width
        assert!(
            widths.windows(2).all(|w| w[0] == w[1]),
            "All rows should have same width, got: {:?}",
            widths
        );
    }

    #[test]
    fn test_parse_table_cells() {
        let cells = parse_table_cells("| Header 1 | Header 2 |");
        assert_eq!(cells, vec!["Header 1", "Header 2"]);

        let cells = parse_table_cells("|---|---|");
        assert_eq!(cells, vec!["---", "---"]);
    }
}
