//! Streaming markdown renderer for terminal output.
//!
//! This module provides markdown rendering that works with streaming LLM output,
//! re-rendering content as it arrives to display proper formatting.
//!
//! Uses pulldown-cmark to parse markdown and produces ratatui `Text` directly.
//! The TUI path uses `Text` as-is; the CLI path converts via `text_to_ansi()`.

use std::io::{self, Write as _};

use crossterm::{
    cursor::{MoveToColumn, MoveUp},
    terminal::{size as terminal_size, Clear, ClearType},
    ExecutableCommand,
};
use pulldown_cmark::{Event, HeadingLevel, Options, Parser, Tag, TagEnd};
use ratatui::{
    style::{Color, Modifier, Style},
    text::{Line, Span, Text},
};

/// Style definitions for markdown elements, matching the previous termimad color scheme.
struct MarkdownStyle {
    bold: Style,
    italic: Style,
    inline_code: Style,
    code_block: Style,
    h1: Style,
    h2: Style,
    h3: Style,
    h4_h6: Style,
    bullet: Style,
    blockquote: Style,
    link: Style,
    strikethrough: Style,
}

impl Default for MarkdownStyle {
    fn default() -> Self {
        Self {
            bold: Style::default().fg(Color::White).add_modifier(Modifier::BOLD),
            italic: Style::default()
                .fg(Color::Magenta)
                .add_modifier(Modifier::ITALIC),
            inline_code: Style::default().fg(Color::Yellow),
            code_block: Style::default().fg(Color::Yellow),
            h1: Style::default()
                .fg(Color::Green)
                .add_modifier(Modifier::BOLD),
            h2: Style::default()
                .fg(Color::Green)
                .add_modifier(Modifier::BOLD),
            h3: Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
            h4_h6: Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::ITALIC),
            bullet: Style::default().fg(Color::Cyan),
            blockquote: Style::default().fg(Color::DarkGray),
            link: Style::default()
                .fg(Color::Blue)
                .add_modifier(Modifier::UNDERLINED),
            strikethrough: Style::default().add_modifier(Modifier::CROSSED_OUT),
        }
    }
}

/// Render markdown content to ratatui `Text`.
///
/// This is the core rendering function used by both TUI and CLI paths.
pub fn render_to_text(content: &str) -> Text<'static> {
    let styles = MarkdownStyle::default();
    let options = Options::ENABLE_TABLES | Options::ENABLE_STRIKETHROUGH;
    let parser = Parser::new_ext(content, options);

    let mut lines: Vec<Line<'static>> = Vec::new();
    let mut current_spans: Vec<Span<'static>> = Vec::new();
    let mut style_stack: Vec<Style> = Vec::new();
    let mut had_paragraph = false;
    let mut in_code_block = false;
    let mut in_blockquote = false;

    // List tracking: stack of (is_ordered, next_number)
    let mut list_stack: Vec<(bool, u64)> = Vec::new();
    let mut pending_item_prefix: Option<Vec<Span<'static>>> = None;

    // Link URL storage
    let mut link_url: Option<String> = None;

    for event in parser {
        match event {
            Event::Start(Tag::Paragraph) => {
                if had_paragraph {
                    flush_line(&mut lines, &mut current_spans);
                }
            }
            Event::End(TagEnd::Paragraph) => {
                flush_line(&mut lines, &mut current_spans);
                lines.push(Line::default());
                had_paragraph = true;
            }

            Event::Start(Tag::Heading { level, .. }) => {
                let style = match level {
                    HeadingLevel::H1 => styles.h1,
                    HeadingLevel::H2 => styles.h2,
                    HeadingLevel::H3 => styles.h3,
                    _ => styles.h4_h6,
                };
                let prefix = match level {
                    HeadingLevel::H1 => "# ",
                    HeadingLevel::H2 => "## ",
                    HeadingLevel::H3 => "### ",
                    HeadingLevel::H4 => "#### ",
                    HeadingLevel::H5 => "##### ",
                    HeadingLevel::H6 => "###### ",
                };
                style_stack.push(style);
                current_spans.push(Span::styled(prefix.to_string(), style));
            }
            Event::End(TagEnd::Heading(_)) => {
                style_stack.pop();
                flush_line(&mut lines, &mut current_spans);
                lines.push(Line::default());
                had_paragraph = true;
            }

            Event::Start(Tag::Strong) => {
                style_stack.push(styles.bold);
            }
            Event::End(TagEnd::Strong) => {
                style_stack.pop();
            }

            Event::Start(Tag::Emphasis) => {
                style_stack.push(styles.italic);
            }
            Event::End(TagEnd::Emphasis) => {
                style_stack.pop();
            }

            Event::Start(Tag::Strikethrough) => {
                style_stack.push(styles.strikethrough);
            }
            Event::End(TagEnd::Strikethrough) => {
                style_stack.pop();
            }

            Event::Start(Tag::BlockQuote(_)) => {
                in_blockquote = true;
                style_stack.push(styles.blockquote);
            }
            Event::End(TagEnd::BlockQuote(_)) => {
                in_blockquote = false;
                style_stack.pop();
                flush_line(&mut lines, &mut current_spans);
            }

            Event::Start(Tag::CodeBlock(_)) => {
                in_code_block = true;
                style_stack.push(styles.code_block);
            }
            Event::End(TagEnd::CodeBlock) => {
                in_code_block = false;
                style_stack.pop();
                // Ensure blank line after code block
                if lines.last().is_some_and(|l| !l.spans.is_empty()) {
                    lines.push(Line::default());
                }
            }

            Event::Start(Tag::List(start)) => {
                if let Some(n) = start {
                    list_stack.push((true, n));
                } else {
                    list_stack.push((false, 0));
                }
            }
            Event::End(TagEnd::List(_)) => {
                list_stack.pop();
                // Blank line after list ends (only for top-level lists)
                if list_stack.is_empty()
                    && lines.last().is_none_or(|l| !l.spans.is_empty())
                {
                    lines.push(Line::default());
                }
            }

            Event::Start(Tag::Item) => {
                let indent_level = list_stack.len().saturating_sub(1);
                let indent = "  ".repeat(indent_level);
                if let Some((is_ordered, ref mut num)) = list_stack.last_mut() {
                    if *is_ordered {
                        let prefix = format!("{}{}. ", indent, num);
                        *num += 1;
                        pending_item_prefix =
                            Some(vec![Span::styled(prefix, styles.bullet)]);
                    } else {
                        let prefix = format!("{}- ", indent);
                        pending_item_prefix =
                            Some(vec![Span::styled(prefix, styles.bullet)]);
                    }
                }
            }
            Event::End(TagEnd::Item) => {
                flush_line(&mut lines, &mut current_spans);
            }

            Event::Start(Tag::Link { dest_url, .. }) => {
                link_url = Some(dest_url.to_string());
                style_stack.push(styles.link);
            }
            Event::End(TagEnd::Link) => {
                style_stack.pop();
                if let Some(url) = link_url.take() {
                    let effective = effective_style(&style_stack);
                    current_spans.push(Span::styled(
                        format!(" ({})", url),
                        effective,
                    ));
                }
            }

            Event::Code(text) => {
                // Prepend list item prefix if pending
                if let Some(prefix) = pending_item_prefix.take() {
                    current_spans.extend(prefix);
                }
                current_spans.push(Span::styled(text.to_string(), styles.inline_code));
            }

            Event::Text(text) => {
                // Prepend list item prefix if pending
                if let Some(prefix) = pending_item_prefix.take() {
                    current_spans.extend(prefix);
                }

                let style = effective_style(&style_stack);

                if in_code_block {
                    // Code blocks: emit each line separately
                    for (i, line) in text.split('\n').enumerate() {
                        if i > 0 {
                            flush_line(&mut lines, &mut current_spans);
                        }
                        if !line.is_empty() {
                            current_spans.push(Span::styled(line.to_string(), style));
                        }
                    }
                } else if in_blockquote {
                    // Blockquote: add "│ " prefix to each line
                    for (i, line) in text.split('\n').enumerate() {
                        if i > 0 {
                            flush_line(&mut lines, &mut current_spans);
                        }
                        if i > 0 || current_spans.is_empty() {
                            current_spans
                                .push(Span::styled("│ ".to_string(), styles.blockquote));
                        }
                        if !line.is_empty() {
                            current_spans.push(Span::styled(line.to_string(), style));
                        }
                    }
                } else {
                    current_spans.push(Span::styled(text.to_string(), style));
                }
            }

            Event::SoftBreak => {
                current_spans.push(Span::raw(" ".to_string()));
            }
            Event::HardBreak => {
                flush_line(&mut lines, &mut current_spans);
            }

            Event::Rule => {
                flush_line(&mut lines, &mut current_spans);
                let rule = "─".repeat(40);
                lines.push(Line::from(Span::styled(
                    rule,
                    Style::default().fg(Color::DarkGray),
                )));
                lines.push(Line::default());
            }

            // Table events - shouldn't occur after preprocess_tables, but handle gracefully
            Event::Start(Tag::Table(_))
            | Event::End(TagEnd::Table)
            | Event::Start(Tag::TableHead)
            | Event::End(TagEnd::TableHead)
            | Event::Start(Tag::TableRow)
            | Event::End(TagEnd::TableRow)
            | Event::Start(Tag::TableCell)
            | Event::End(TagEnd::TableCell) => {}

            _ => {}
        }
    }

    // Flush any remaining content
    if !current_spans.is_empty() {
        flush_line(&mut lines, &mut current_spans);
    }

    // Remove trailing empty lines
    while lines.last().is_some_and(|l| l.spans.is_empty()) {
        lines.pop();
    }

    Text::from(lines)
}

/// Flush the current span accumulator into a completed line.
fn flush_line(lines: &mut Vec<Line<'static>>, spans: &mut Vec<Span<'static>>) {
    if spans.is_empty() {
        return;
    }
    lines.push(Line::from(std::mem::take(spans)));
}

/// Compute the effective style by patching all styles on the stack together.
fn effective_style(stack: &[Style]) -> Style {
    let mut style = Style::default();
    for s in stack {
        style = style.patch(*s);
    }
    style
}

/// Convert ratatui `Text` to an ANSI-escaped string for direct terminal output.
pub fn text_to_ansi(text: &Text) -> String {
    let mut out = String::new();
    for (i, line) in text.lines.iter().enumerate() {
        if i > 0 {
            out.push('\n');
        }
        for span in &line.spans {
            let style = span.style;
            let has_style = style.fg.is_some()
                || style.bg.is_some()
                || style
                    .add_modifier
                    .intersects(Modifier::BOLD | Modifier::ITALIC | Modifier::UNDERLINED | Modifier::CROSSED_OUT);

            if has_style {
                out.push_str(&style_to_ansi(&style));
                out.push_str(&span.content);
                out.push_str("\x1b[0m");
            } else {
                out.push_str(&span.content);
            }
        }
    }
    out
}

/// Convert a ratatui Style to an ANSI SGR escape sequence.
fn style_to_ansi(style: &Style) -> String {
    let mut codes = Vec::new();

    if style.add_modifier.contains(Modifier::BOLD) {
        codes.push("1".to_string());
    }
    if style.add_modifier.contains(Modifier::ITALIC) {
        codes.push("3".to_string());
    }
    if style.add_modifier.contains(Modifier::UNDERLINED) {
        codes.push("4".to_string());
    }
    if style.add_modifier.contains(Modifier::CROSSED_OUT) {
        codes.push("9".to_string());
    }

    if let Some(fg) = style.fg {
        if let Some(code) = color_to_ansi_fg(fg) {
            codes.push(code);
        }
    }

    if let Some(bg) = style.bg {
        if let Some(code) = color_to_ansi_bg(bg) {
            codes.push(code);
        }
    }

    if codes.is_empty() {
        String::new()
    } else {
        format!("\x1b[{}m", codes.join(";"))
    }
}

/// Map a ratatui Color to an ANSI foreground color code.
fn color_to_ansi_fg(color: Color) -> Option<String> {
    match color {
        Color::Black => Some("30".to_string()),
        Color::Red => Some("31".to_string()),
        Color::Green => Some("32".to_string()),
        Color::Yellow => Some("33".to_string()),
        Color::Blue => Some("34".to_string()),
        Color::Magenta => Some("35".to_string()),
        Color::Cyan => Some("36".to_string()),
        Color::White => Some("37".to_string()),
        Color::DarkGray => Some("90".to_string()),
        Color::LightRed => Some("91".to_string()),
        Color::LightGreen => Some("92".to_string()),
        Color::LightYellow => Some("93".to_string()),
        Color::LightBlue => Some("94".to_string()),
        Color::LightMagenta => Some("95".to_string()),
        Color::LightCyan => Some("96".to_string()),
        Color::Gray => Some("37".to_string()),
        Color::Indexed(n) => Some(format!("38;5;{}", n)),
        Color::Rgb(r, g, b) => Some(format!("38;2;{};{};{}", r, g, b)),
        _ => None,
    }
}

/// Map a ratatui Color to an ANSI background color code.
fn color_to_ansi_bg(color: Color) -> Option<String> {
    match color {
        Color::Black => Some("40".to_string()),
        Color::Red => Some("41".to_string()),
        Color::Green => Some("42".to_string()),
        Color::Yellow => Some("43".to_string()),
        Color::Blue => Some("44".to_string()),
        Color::Magenta => Some("45".to_string()),
        Color::Cyan => Some("46".to_string()),
        Color::White => Some("47".to_string()),
        Color::DarkGray => Some("100".to_string()),
        Color::LightRed => Some("101".to_string()),
        Color::LightGreen => Some("102".to_string()),
        Color::LightYellow => Some("103".to_string()),
        Color::LightBlue => Some("104".to_string()),
        Color::LightMagenta => Some("105".to_string()),
        Color::LightCyan => Some("106".to_string()),
        Color::Gray => Some("47".to_string()),
        Color::Indexed(n) => Some(format!("48;5;{}", n)),
        Color::Rgb(r, g, b) => Some(format!("48;2;{};{};{}", r, g, b)),
        _ => None,
    }
}

/// A streaming markdown renderer that accumulates content and re-renders.
pub struct MarkdownRenderer {
    /// Accumulated content
    content: String,
    /// Number of lines we've rendered (for clearing)
    rendered_lines: u16,
    /// Terminal width
    term_width: usize,
}

impl MarkdownRenderer {
    /// Create a new markdown renderer.
    pub fn new() -> Self {
        let (width, _) = terminal_size().unwrap_or((80, 24));
        let term_width = (width as usize).saturating_sub(2).max(40);

        Self {
            content: String::new(),
            rendered_lines: 0,
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
        let ratatui_text = render_to_text(&processed);
        let output = text_to_ansi(&ratatui_text);

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
    let (width, _) = terminal_size().unwrap_or((80, 24));
    let term_width = (width as usize).saturating_sub(2).max(40);
    let processed = preprocess_tables(content, term_width);
    let ratatui_text = render_to_text(&processed);
    let output = text_to_ansi(&ratatui_text);
    println!("{}", output);
}

/// Minimum characters per column before we self-render the table.
const MIN_COL_WIDTH: usize = 10;

/// Preprocess markdown content to self-render tables with box-drawing characters.
/// All tables are self-rendered regardless of column width, ensuring consistent
/// readable output.
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
/// Converts HTML `<br>` tags to newlines so they render as line breaks within cells.
fn parse_row(line: &str) -> Vec<String> {
    let trimmed = line.trim();
    // Strip leading and trailing `|`
    let inner = trimmed
        .strip_prefix('|')
        .unwrap_or(trimmed)
        .strip_suffix('|')
        .unwrap_or(trimmed);
    inner
        .split('|')
        .map(|c| {
            let cell = c.trim().to_string();
            // Convert <br>, <br/>, <br /> to newlines
            let cell = cell.replace("<br/>", "\n");
            let cell = cell.replace("<br />", "\n");
            let cell = cell.replace("<br>", "\n");
            // Trim whitespace around newlines
            cell.lines()
                .map(|l| l.trim())
                .collect::<Vec<_>>()
                .join("\n")
        })
        .collect()
}

/// Check if a row is a separator row (cells contain only `-`, `:`, spaces).
fn is_separator_row(cells: &[String]) -> bool {
    cells.iter().all(|c| {
        !c.is_empty() && c.chars().all(|ch| ch == '-' || ch == ':' || ch == ' ')
    })
}

/// Process accumulated table lines: always self-render with box-drawing characters.
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

    // Always self-render the table
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

    // Calculate max content width per column (widest line within each cell)
    let mut max_widths: Vec<usize> = vec![0; num_cols];
    for row in &all_rows {
        for (i, cell) in row.iter().enumerate() {
            if i < num_cols {
                // For cells with embedded newlines, measure the widest line
                let widest_line = cell
                    .lines()
                    .map(|l| l.chars().count())
                    .max()
                    .unwrap_or(cell.chars().count());
                max_widths[i] = max_widths[i].max(widest_line);
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
/// Uses a waterfall algorithm: narrow columns keep their natural width,
/// only wide columns get shrunk to share remaining space.
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

    // Can't even fit minimums: give equal share
    let min_total = num_cols * MIN_COL_WIDTH;
    if usable <= min_total {
        let per_col = (usable / num_cols).max(1);
        return vec![per_col; num_cols];
    }

    // Waterfall algorithm: preserve narrow columns at natural width,
    // only shrink columns wider than their fair share.
    let mut result = widths.clone();
    let mut fixed = vec![false; num_cols];

    loop {
        let unfixed_count = fixed.iter().filter(|&&f| !f).count();
        if unfixed_count == 0 {
            break;
        }

        let fixed_total: usize = (0..num_cols).filter(|&i| fixed[i]).map(|i| result[i]).sum();
        let remaining = usable.saturating_sub(fixed_total);
        let fair_share = remaining / unfixed_count;

        // Fix columns that already fit within their fair share
        let mut newly_fixed = false;
        for i in 0..num_cols {
            if !fixed[i] && result[i] <= fair_share {
                fixed[i] = true;
                newly_fixed = true;
            }
        }

        if !newly_fixed {
            // All unfixed columns exceed fair share; distribute remaining evenly
            let fixed_total: usize =
                (0..num_cols).filter(|&i| fixed[i]).map(|i| result[i]).sum();
            let remaining = usable.saturating_sub(fixed_total);
            let per_col = remaining / unfixed_count;
            let mut extra = remaining % unfixed_count;
            for i in 0..num_cols {
                if !fixed[i] {
                    result[i] = per_col
                        + if extra > 0 {
                            extra -= 1;
                            1
                        } else {
                            0
                        };
                }
            }
            break;
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
            // Pad to column width (use char count for correct alignment)
            out.push(' ');
            out.push_str(text);
            let text_width = text.chars().count();
            for _ in text_width..width {
                out.push(' ');
            }
            out.push(' ');
            out.push('│');
        }
        out.push('\n');
    }
}

/// Word-wrap text to a given width. Tries to break at word boundaries.
/// Handles embedded newlines by splitting on them first.
/// Width is measured in characters (not bytes) so multi-byte UTF-8 is safe.
fn wrap_text(text: &str, width: usize) -> Vec<String> {
    if width == 0 {
        return vec![String::new()];
    }
    if text.is_empty() {
        return vec![String::new()];
    }

    // Handle embedded newlines: split first, then wrap each line
    if text.contains('\n') {
        let mut all_lines = Vec::new();
        for line in text.split('\n') {
            all_lines.extend(wrap_single_line(line, width));
        }
        return all_lines;
    }

    wrap_single_line(text, width)
}

/// Wrap a single line of text (no embedded newlines) to the given width.
fn wrap_single_line(text: &str, width: usize) -> Vec<String> {
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
    fn test_render_to_text_basic() {
        let text = render_to_text("Hello world");
        assert!(!text.lines.is_empty());
        // Should contain the text
        let content: String = text
            .lines
            .iter()
            .flat_map(|l| l.spans.iter().map(|s| s.content.to_string()))
            .collect();
        assert!(content.contains("Hello world"));
    }

    #[test]
    fn test_render_bold_style() {
        let text = render_to_text("This is **bold** text");
        let bold_spans: Vec<_> = text
            .lines
            .iter()
            .flat_map(|l| l.spans.iter())
            .filter(|s| s.style.add_modifier.contains(Modifier::BOLD))
            .collect();
        assert!(!bold_spans.is_empty());
        assert!(bold_spans.iter().any(|s| s.content.contains("bold")));
    }

    #[test]
    fn test_render_heading_styles() {
        let text = render_to_text("# Heading 1\n\n## Heading 2\n\n### Heading 3");
        // Should have green and cyan colored spans
        let green_spans: Vec<_> = text
            .lines
            .iter()
            .flat_map(|l| l.spans.iter())
            .filter(|s| s.style.fg == Some(Color::Green))
            .collect();
        assert!(!green_spans.is_empty());

        let cyan_spans: Vec<_> = text
            .lines
            .iter()
            .flat_map(|l| l.spans.iter())
            .filter(|s| s.style.fg == Some(Color::Cyan))
            .collect();
        assert!(!cyan_spans.is_empty());
    }

    #[test]
    fn test_render_inline_code() {
        let text = render_to_text("Use `code` here");
        let code_spans: Vec<_> = text
            .lines
            .iter()
            .flat_map(|l| l.spans.iter())
            .filter(|s| s.style.fg == Some(Color::Yellow))
            .collect();
        assert!(!code_spans.is_empty());
        assert!(code_spans.iter().any(|s| s.content.contains("code")));
    }

    #[test]
    fn test_render_list_bullets() {
        let text = render_to_text("- Item 1\n- Item 2\n- Item 3");
        let cyan_spans: Vec<_> = text
            .lines
            .iter()
            .flat_map(|l| l.spans.iter())
            .filter(|s| s.style.fg == Some(Color::Cyan))
            .collect();
        // Should have bullet markers in cyan
        assert!(!cyan_spans.is_empty());
    }

    #[test]
    fn test_render_blockquote() {
        let text = render_to_text("> This is a quote");
        let gray_spans: Vec<_> = text
            .lines
            .iter()
            .flat_map(|l| l.spans.iter())
            .filter(|s| s.style.fg == Some(Color::DarkGray))
            .collect();
        assert!(!gray_spans.is_empty());
    }

    #[test]
    fn test_render_nested_styles() {
        let text = render_to_text("This is ***bold italic*** text");
        let bold_italic_spans: Vec<_> = text
            .lines
            .iter()
            .flat_map(|l| l.spans.iter())
            .filter(|s| {
                s.style.add_modifier.contains(Modifier::BOLD)
                    || s.style.add_modifier.contains(Modifier::ITALIC)
            })
            .collect();
        assert!(!bold_italic_spans.is_empty());
    }

    #[test]
    fn test_text_to_ansi_basic() {
        let text = render_to_text("**bold** and *italic*");
        let ansi = text_to_ansi(&text);
        // Should contain ANSI escape codes
        assert!(ansi.contains("\x1b["));
        // Should contain reset
        assert!(ansi.contains("\x1b[0m"));
        // Should contain the text
        assert!(ansi.contains("bold"));
        assert!(ansi.contains("italic"));
    }

    #[test]
    fn test_markdown_renderer() {
        let renderer = MarkdownRenderer::new();
        assert!(renderer.is_empty());
    }

    #[test]
    fn test_table_always_self_renders() {
        let table = "| Header 1 | Header 2 |\n|----------|----------|\n| Cell 1   | Cell 2   |";
        let result = preprocess_tables(table, 80);
        // All tables now self-render with box-drawing characters
        assert!(result.contains('┌'));
        assert!(result.contains('│'));
        assert!(result.contains('┘'));
        assert!(result.contains("```"));
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

    #[test]
    fn test_waterfall_preserves_narrow_columns() {
        // Narrow col 1, very wide col 2 — col 1 should keep its natural width
        let widths = calculate_col_widths(&[15, 200], 2, 80);
        // Col 1 should stay at 15 (its natural width), not be proportionally shrunk
        assert_eq!(widths[0], 15);
        // Col 2 gets the rest: 80 - overhead(7) - 15 = 58
        assert_eq!(widths[0] + widths[1] + 7, 80);
    }

    #[test]
    fn test_br_tags_become_newlines() {
        let cells = parse_row("| Point A.<br>Point B.<br/>Point C. |");
        assert_eq!(cells.len(), 1);
        assert!(cells[0].contains('\n'));
        let lines: Vec<&str> = cells[0].lines().collect();
        assert_eq!(lines, vec!["Point A.", "Point B.", "Point C."]);
    }

    #[test]
    fn test_wrap_text_with_newlines() {
        let wrapped = wrap_text("Line one\nLine two\nLine three", 20);
        assert_eq!(wrapped, vec!["Line one", "Line two", "Line three"]);
    }

    #[test]
    fn test_table_with_br_tags() {
        let table = "| Area | Details |\n|------|--------|\n| **Short** | First point.<br>Second point.<br>Third point. |";
        let result = preprocess_tables(table, 80);
        // <br> should not appear literally
        assert!(!result.contains("<br>"));
        // Box drawing should be present
        assert!(result.contains('┌'));
    }
}
