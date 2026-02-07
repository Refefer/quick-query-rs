//! Streaming markdown renderer for terminal output.
//!
//! This module provides markdown rendering that works with streaming LLM output,
//! re-rendering content as it arrives to display proper formatting.

use std::io::{self, Write};

use crossterm::{
    cursor::{MoveToColumn, MoveUp},
    style::Color,
    terminal::{Clear, ClearType},
    ExecutableCommand,
};
use termimad::{MadSkin, terminal_size};

/// A streaming markdown renderer that accumulates content and re-renders.
pub struct MarkdownRenderer {
    /// Accumulated content
    content: String,
    /// Number of lines we've rendered (for clearing)
    rendered_lines: u16,
    /// The markdown skin for styling
    skin: MadSkin,
    /// Terminal width
    term_width: usize,
}

impl MarkdownRenderer {
    /// Create a new markdown renderer.
    pub fn new() -> Self {
        let (width, _) = terminal_size();
        let term_width = (width as usize).saturating_sub(2).max(40);

        Self {
            content: String::new(),
            rendered_lines: 0,
            skin: create_skin(),
            term_width,
        }
    }

    /// Add content and re-render.
    pub fn push(&mut self, text: &str) -> io::Result<()> {
        self.content.push_str(text);
        self.render()
    }

    /// Clear previous render and re-render current content.
    fn render(&mut self) -> io::Result<()> {
        let mut stdout = io::stdout();

        // Clear previously rendered lines
        if self.rendered_lines > 0 {
            // Move up and clear each line
            for _ in 0..self.rendered_lines {
                stdout.execute(MoveUp(1))?;
                stdout.execute(Clear(ClearType::CurrentLine))?;
            }
            stdout.execute(MoveToColumn(0))?;
        }

        // Preprocess tables that would be too narrow, then render markdown
        let processed = preprocess_tables(&self.content, self.term_width);
        let rendered = self.skin.text(&processed, Some(self.term_width));
        let output = format!("{}", rendered);

        // Count lines for next clear
        self.rendered_lines = output.lines().count() as u16;

        // Print rendered content
        print!("{}", output);
        stdout.flush()?;

        Ok(())
    }

    /// Finish rendering and add final newlines.
    pub fn finish(&mut self) -> io::Result<()> {
        // Do a final render to ensure everything is displayed
        if !self.content.is_empty() {
            self.render()?;
        }
        println!("\n");
        Ok(())
    }

    /// Get the accumulated content (for saving to session).
    pub fn content(&self) -> &str {
        &self.content
    }

    /// Check if any content has been added.
    pub fn is_empty(&self) -> bool {
        self.content.is_empty()
    }
}

impl Default for MarkdownRenderer {
    fn default() -> Self {
        Self::new()
    }
}

/// Render markdown content to stdout (one-shot, non-streaming).
pub fn render_markdown(content: &str) {
    let (width, _) = terminal_size();
    let term_width = (width as usize).saturating_sub(2).max(40);
    let skin = create_skin();
    let processed = preprocess_tables(content, term_width);
    let rendered = skin.text(&processed, Some(term_width));
    println!("{}", rendered);
}

/// Create a styled markdown skin for terminal output.
pub fn create_skin() -> MadSkin {
    let mut skin = MadSkin::default();

    // Customize colors for better terminal appearance
    skin.bold.set_fg(Color::White);
    skin.italic.set_fg(Color::Magenta);
    skin.inline_code.set_fg(Color::Yellow);
    skin.code_block.set_fg(Color::Yellow);

    // Headers
    skin.headers[0].set_fg(Color::Green);
    skin.headers[1].set_fg(Color::Green);
    skin.headers[2].set_fg(Color::Cyan);

    // Lists and quotes
    skin.bullet.set_fg(Color::Cyan);
    skin.quote_mark.set_fg(Color::DarkGrey);

    skin
}

/// Minimum characters per column before we self-render the table.
const MIN_COL_WIDTH: usize = 10;

/// Preprocess markdown content to self-render tables whose columns would be
/// too narrow for termimad to display readably. Tables that fit are left as-is.
pub(crate) fn preprocess_tables(content: &str, width: usize) -> String {
    let mut result = String::with_capacity(content.len());
    let mut in_code_block = false;
    let mut table_lines: Vec<&str> = Vec::new();
    let lines: Vec<&str> = content.lines().collect();

    for line in &lines {
        // Track fenced code block state
        if line.trim_start().starts_with("```") {
            if in_code_block {
                in_code_block = false;
                result.push_str(line);
                result.push('\n');
                continue;
            } else {
                // Flush any pending table before entering code block
                if !table_lines.is_empty() {
                    flush_table(&table_lines, width, &mut result);
                    table_lines.clear();
                }
                in_code_block = true;
                result.push_str(line);
                result.push('\n');
                continue;
            }
        }

        if in_code_block {
            result.push_str(line);
            result.push('\n');
            continue;
        }

        // Detect table rows: lines starting with `|`
        if line.trim_start().starts_with('|') {
            table_lines.push(line);
        } else {
            // Non-table line: flush any accumulated table first
            if !table_lines.is_empty() {
                flush_table(&table_lines, width, &mut result);
                table_lines.clear();
            }
            result.push_str(line);
            result.push('\n');
        }
    }

    // Flush any trailing table
    if !table_lines.is_empty() {
        flush_table(&table_lines, width, &mut result);
    }

    // Remove trailing newline if input didn't have one
    if !content.ends_with('\n') && result.ends_with('\n') {
        result.pop();
    }

    result
}

/// Parse a markdown table row into cells by splitting on `|` and trimming.
fn parse_row(line: &str) -> Vec<String> {
    let trimmed = line.trim();
    // Strip leading and trailing `|`
    let inner = trimmed
        .strip_prefix('|')
        .unwrap_or(trimmed)
        .strip_suffix('|')
        .unwrap_or(trimmed);
    inner.split('|').map(|c| c.trim().to_string()).collect()
}

/// Check if a row is a separator row (cells contain only `-`, `:`, spaces).
fn is_separator_row(cells: &[String]) -> bool {
    cells.iter().all(|c| {
        !c.is_empty() && c.chars().all(|ch| ch == '-' || ch == ':' || ch == ' ')
    })
}

/// Process accumulated table lines: either pass through or self-render.
fn flush_table(table_lines: &[&str], width: usize, result: &mut String) {
    // Parse all rows
    let rows: Vec<Vec<String>> = table_lines.iter().map(|l| parse_row(l)).collect();

    if rows.is_empty() {
        return;
    }

    // Find separator row index and determine header vs data
    let sep_idx = rows.iter().position(|r| is_separator_row(r));

    let num_cols = rows.iter().map(|r| r.len()).max().unwrap_or(0);
    if num_cols == 0 {
        // Not a real table, pass through
        for line in table_lines {
            result.push_str(line);
            result.push('\n');
        }
        return;
    }

    // Check if columns would be too narrow with termimad
    // termimad uses: (width - num_cols - 1) / num_cols for each column
    let effective_col_width = if num_cols > 0 {
        width.saturating_sub(num_cols + 1) / num_cols
    } else {
        width
    };

    if effective_col_width >= MIN_COL_WIDTH {
        // Table fits fine, leave as markdown for termimad
        for line in table_lines {
            result.push_str(line);
            result.push('\n');
        }
        return;
    }

    // Self-render the table
    let (headers, data_rows) = if let Some(si) = sep_idx {
        let headers: Vec<Vec<String>> = rows[..si].to_vec();
        let data: Vec<Vec<String>> = rows[si + 1..].to_vec();
        (headers, data)
    } else {
        // No separator found, treat first row as header
        (vec![rows[0].clone()], rows[1..].to_vec())
    };

    let rendered = render_table(&headers, &data_rows, num_cols, width);
    result.push_str("```\n");
    result.push_str(&rendered);
    result.push_str("```\n");
}

/// Render a table with box-drawing characters.
fn render_table(
    headers: &[Vec<String>],
    data_rows: &[Vec<String>],
    num_cols: usize,
    available_width: usize,
) -> String {
    // Collect all rows for width calculation
    let all_rows: Vec<&Vec<String>> = headers.iter().chain(data_rows.iter()).collect();

    // Calculate max content width per column
    let mut max_widths: Vec<usize> = vec![0; num_cols];
    for row in &all_rows {
        for (i, cell) in row.iter().enumerate() {
            if i < num_cols {
                max_widths[i] = max_widths[i].max(cell.len());
            }
        }
    }

    // Calculate column widths
    let col_widths = calculate_col_widths(&max_widths, num_cols, available_width);

    let mut out = String::new();

    // Top border: ┌──┬──┐
    out.push_str(&border_line('┌', '┬', '┐', '─', &col_widths));

    // Header rows
    for header in headers {
        render_wrapped_row(&mut out, header, &col_widths, num_cols);
    }

    // Separator: ├──┼──┤
    out.push_str(&border_line('├', '┼', '┤', '─', &col_widths));

    // Data rows with separators between them
    for (i, row) in data_rows.iter().enumerate() {
        render_wrapped_row(&mut out, row, &col_widths, num_cols);
        if i < data_rows.len() - 1 {
            out.push_str(&border_line('├', '┼', '┤', '─', &col_widths));
        }
    }

    // Bottom border: └──┴──┘
    out.push_str(&border_line('└', '┴', '┘', '─', &col_widths));

    out
}

/// Calculate column widths, respecting min width and available space.
fn calculate_col_widths(
    max_widths: &[usize],
    num_cols: usize,
    available_width: usize,
) -> Vec<usize> {
    // Each column needs: 1 (border) + width + 1 (padding on each side)
    // Plus 1 final border: │ col1 │ col2 │
    // Overhead = num_cols + 1 (for borders) + num_cols * 2 (for padding)
    let overhead = num_cols + 1 + num_cols * 2;
    let usable = available_width.saturating_sub(overhead);

    // Start with desired widths (content width, at least MIN_COL_WIDTH)
    let mut widths: Vec<usize> = max_widths
        .iter()
        .map(|&w| w.max(MIN_COL_WIDTH))
        .collect();

    // Pad to num_cols if needed
    while widths.len() < num_cols {
        widths.push(MIN_COL_WIDTH);
    }

    let total: usize = widths.iter().sum();

    if total <= usable {
        // Fits, use as-is
        return widths;
    }

    // Need to shrink. Proportionally reduce but never below MIN_COL_WIDTH.
    // First check if we can fit at all with min widths
    let min_total = num_cols * MIN_COL_WIDTH;
    if usable <= min_total {
        // Can't even fit minimums, give equal share but at least 1
        let per_col = (usable / num_cols).max(1);
        return vec![per_col; num_cols];
    }

    // Proportionally distribute usable space
    let scale = usable as f64 / total as f64;
    let mut result: Vec<usize> = widths
        .iter()
        .map(|&w| ((w as f64 * scale).floor() as usize).max(MIN_COL_WIDTH))
        .collect();

    // Adjust to exactly fill usable space
    let current_total: usize = result.iter().sum();
    if current_total < usable {
        // Distribute remaining space to largest columns first
        let mut remaining = usable - current_total;
        let mut indices: Vec<usize> = (0..num_cols).collect();
        indices.sort_by(|&a, &b| result[b].cmp(&result[a]));
        for &i in &indices {
            if remaining == 0 {
                break;
            }
            result[i] += 1;
            remaining -= 1;
        }
    } else if current_total > usable {
        // Take space from largest columns
        let mut excess = current_total - usable;
        let mut indices: Vec<usize> = (0..num_cols).collect();
        indices.sort_by(|&a, &b| result[b].cmp(&result[a]));
        for &i in &indices {
            if excess == 0 {
                break;
            }
            if result[i] > MIN_COL_WIDTH {
                let can_take = (result[i] - MIN_COL_WIDTH).min(excess);
                result[i] -= can_take;
                excess -= can_take;
            }
        }
    }

    result
}

/// Generate a horizontal border line.
fn border_line(left: char, mid: char, right: char, fill: char, widths: &[usize]) -> String {
    let mut line = String::new();
    line.push(left);
    for (i, &w) in widths.iter().enumerate() {
        // +2 for padding on each side
        for _ in 0..w + 2 {
            line.push(fill);
        }
        if i < widths.len() - 1 {
            line.push(mid);
        }
    }
    line.push(right);
    line.push('\n');
    line
}

/// Render a row with word wrapping into multiple physical lines if needed.
fn render_wrapped_row(
    out: &mut String,
    row: &[String],
    col_widths: &[usize],
    num_cols: usize,
) {
    // Wrap each cell's content to its column width
    let mut wrapped_cells: Vec<Vec<String>> = Vec::with_capacity(num_cols);
    for i in 0..num_cols {
        let content = if i < row.len() { &row[i] } else { "" };
        wrapped_cells.push(wrap_text(content, col_widths[i]));
    }

    // Find max number of lines across all cells
    let max_lines = wrapped_cells.iter().map(|c| c.len()).max().unwrap_or(1);

    // Render each physical line
    for line_idx in 0..max_lines {
        out.push('│');
        for (col, wrapped) in wrapped_cells.iter().enumerate() {
            let text = if line_idx < wrapped.len() {
                &wrapped[line_idx]
            } else {
                ""
            };
            let width = col_widths[col];
            // Pad to column width
            out.push(' ');
            out.push_str(text);
            for _ in text.len()..width {
                out.push(' ');
            }
            out.push(' ');
            out.push('│');
        }
        out.push('\n');
    }
}

/// Word-wrap text to a given width. Tries to break at word boundaries.
/// Width is measured in characters (not bytes) so multi-byte UTF-8 is safe.
fn wrap_text(text: &str, width: usize) -> Vec<String> {
    if width == 0 {
        return vec![String::new()];
    }
    if text.is_empty() {
        return vec![String::new()];
    }
    if text.chars().count() <= width {
        return vec![text.to_string()];
    }

    let mut lines = Vec::new();
    let mut remaining = text;

    while !remaining.is_empty() {
        let char_count = remaining.chars().count();
        if char_count <= width {
            lines.push(remaining.to_string());
            break;
        }

        // Find the byte offset of the `width`-th character
        let byte_at_width = remaining
            .char_indices()
            .nth(width)
            .map(|(i, _)| i)
            .unwrap_or(remaining.len());

        // Try to find a word break point (space) within the first `width` chars
        let slice = &remaining[..byte_at_width];
        let break_byte = if let Some(pos) = slice.rfind(' ') {
            pos
        } else {
            // No space found, hard break at width
            byte_at_width
        };

        let (chunk, rest) = remaining.split_at(break_byte);
        lines.push(chunk.to_string());

        // Skip the space at the break point
        remaining = rest.trim_start();
    }

    if lines.is_empty() {
        lines.push(String::new());
    }

    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_create_skin() {
        let skin = create_skin();
        // Just verify it creates without panic
        let _ = skin.text("# Hello\n\nThis is **bold**.", Some(80));
    }

    #[test]
    fn test_markdown_renderer() {
        let renderer = MarkdownRenderer::new();
        assert!(renderer.is_empty());
    }

    #[test]
    fn test_table_that_fits() {
        let table = "| Header 1 | Header 2 |\n|----------|----------|\n| Cell 1   | Cell 2   |";
        let result = preprocess_tables(table, 80);
        // Should be left as-is (still contains markdown pipe syntax)
        assert!(result.contains('|'));
        assert!(!result.contains('┌'));
    }

    #[test]
    fn test_table_too_narrow() {
        let table = "| Col A | Col B | Col C | Col D | Col E |\n|-------|-------|-------|-------|-------|\n| data1 | data2 | data3 | data4 | data5 |";
        // 5 columns in 50 chars: effective = (50 - 6) / 5 = 8, which is < MIN_COL_WIDTH
        let result = preprocess_tables(table, 50);
        // Should be self-rendered with box-drawing chars inside a code fence
        assert!(result.contains('┌'));
        assert!(result.contains('│'));
        assert!(result.contains('┘'));
        assert!(result.contains("```"));
    }

    #[test]
    fn test_table_in_code_block() {
        let content = "```\n| Not | A | Table |\n|-----|---|-------|\n| x   | y | z     |\n```";
        let result = preprocess_tables(content, 40);
        // Should NOT be processed - table is inside a code block
        assert!(!result.contains('┌'));
        assert!(result.contains("| Not | A | Table |"));
    }

    #[test]
    fn test_mixed_content() {
        let content = "Some text before\n\n| Col 1 | Col 2 |\n|-------|-------|\n| a     | b     |\n\nSome text after";
        let result = preprocess_tables(content, 80);
        assert!(result.contains("Some text before"));
        assert!(result.contains("Some text after"));
    }

    #[test]
    fn test_word_wrapping() {
        let wrapped = wrap_text("hello world this is a long sentence", 12);
        // Should break at word boundaries
        for line in &wrapped {
            assert!(line.chars().count() <= 12);
        }
        // Verify content is preserved
        let joined = wrapped.join(" ");
        assert_eq!(joined, "hello world this is a long sentence");
    }

    #[test]
    fn test_wrap_text_empty() {
        let wrapped = wrap_text("", 10);
        assert_eq!(wrapped, vec![""]);
    }

    #[test]
    fn test_wrap_text_fits() {
        let wrapped = wrap_text("short", 10);
        assert_eq!(wrapped, vec!["short"]);
    }

    #[test]
    fn test_wrap_text_multibyte_chars() {
        // Non-breaking hyphen U+2011 is 3 bytes in UTF-8
        let text = "**Ad\u{2011}hoc / Low\u{2011}value** (<$50 K)";
        // Should not panic on multi-byte characters
        let wrapped = wrap_text(text, 20);
        for line in &wrapped {
            assert!(line.chars().count() <= 20);
        }
        // All content preserved
        let joined = wrapped.join(" ");
        assert_eq!(joined, text);
    }

    #[test]
    fn test_parse_row() {
        let cells = parse_row("| Header 1 | Header 2 | Header 3 |");
        assert_eq!(cells, vec!["Header 1", "Header 2", "Header 3"]);
    }

    #[test]
    fn test_is_separator_row() {
        assert!(is_separator_row(&[
            "---".to_string(),
            "---".to_string(),
        ]));
        assert!(is_separator_row(&[
            ":---:".to_string(),
            "---:".to_string(),
        ]));
        assert!(!is_separator_row(&[
            "data".to_string(),
            "more".to_string(),
        ]));
    }

    #[test]
    fn test_self_rendered_table_structure() {
        let table = "| A | B | C | D | E | F |\n|---|---|---|---|---|---|\n| 1 | 2 | 3 | 4 | 5 | 6 |";
        let result = preprocess_tables(table, 50);
        // With 6 columns in 50 chars, columns would be ~6 chars each - too narrow
        assert!(result.contains('┌'));
        assert!(result.contains('├'));
        assert!(result.contains('└'));
        // Should have proper structure
        let lines: Vec<&str> = result.lines().collect();
        // First line is code fence, second is top border
        assert!(lines[1].starts_with('┌'));
    }
}
