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
use unicode_width::UnicodeWidthStr;

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

/// A styled cell: a sequence of logical lines, where each logical line
/// is a list of styled spans. Supports `<br>`-induced line breaks within cells.
type StyledCell = Vec<Vec<Span<'static>>>;

/// Accumulated table data collected during the event-driven first pass.
struct TableData {
    alignments: Vec<pulldown_cmark::Alignment>,
    header_rows: Vec<Vec<StyledCell>>,
    data_rows: Vec<Vec<StyledCell>>,
    current_row: Vec<StyledCell>,
    current_cell: StyledCell,
    current_cell_line: Vec<Span<'static>>,
    in_header: bool,
}

impl TableData {
    fn new(alignments: Vec<pulldown_cmark::Alignment>) -> Self {
        Self {
            alignments,
            header_rows: Vec::new(),
            data_rows: Vec::new(),
            current_row: Vec::new(),
            current_cell: Vec::new(),
            current_cell_line: Vec::new(),
            in_header: false,
        }
    }
}

/// Render markdown content to ratatui `Text`.
///
/// This is the core rendering function used by both TUI and CLI paths.
/// The `width` parameter controls table column layout.
pub fn render_to_text(content: &str, width: usize) -> Text<'static> {
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

    // Table accumulation state
    let mut table_data: Option<TableData> = None;

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
                    let url_span = Span::styled(format!(" ({})", url), effective);
                    if let Some(ref mut td) = table_data {
                        td.current_cell_line.push(url_span);
                    } else {
                        current_spans.push(url_span);
                    }
                }
            }

            Event::Code(text) => {
                if let Some(ref mut td) = table_data {
                    td.current_cell_line
                        .push(Span::styled(text.to_string(), styles.inline_code));
                } else {
                    // Prepend list item prefix if pending
                    if let Some(prefix) = pending_item_prefix.take() {
                        current_spans.extend(prefix);
                    }
                    current_spans.push(Span::styled(text.to_string(), styles.inline_code));
                }
            }

            Event::Text(text) => {
                let style = effective_style(&style_stack);

                if let Some(ref mut td) = table_data {
                    // Table mode: push styled span to current cell line
                    td.current_cell_line
                        .push(Span::styled(text.to_string(), style));
                } else {
                    // Prepend list item prefix if pending
                    if let Some(prefix) = pending_item_prefix.take() {
                        current_spans.extend(prefix);
                    }

                    if in_code_block {
                        // Code blocks: emit each line separately
                        for (i, line) in text.split('\n').enumerate() {
                            if i > 0 {
                                flush_line(&mut lines, &mut current_spans);
                            }
                            if !line.is_empty() {
                                current_spans
                                    .push(Span::styled(line.to_string(), style));
                            }
                        }
                    } else if in_blockquote {
                        // Blockquote: add "│ " prefix to each line
                        for (i, line) in text.split('\n').enumerate() {
                            if i > 0 {
                                flush_line(&mut lines, &mut current_spans);
                            }
                            if i > 0 || current_spans.is_empty() {
                                current_spans.push(Span::styled(
                                    "│ ".to_string(),
                                    styles.blockquote,
                                ));
                            }
                            if !line.is_empty() {
                                current_spans
                                    .push(Span::styled(line.to_string(), style));
                            }
                        }
                    } else if let Some(rule_line) = try_render_labeled_rule(&text, width) {
                        // Labeled horizontal rule (e.g., "─── You ───")
                        flush_line(&mut lines, &mut current_spans);
                        lines.push(rule_line);
                    } else {
                        current_spans.push(Span::styled(text.to_string(), style));
                    }
                }
            }

            Event::SoftBreak => {
                if let Some(ref mut td) = table_data {
                    td.current_cell_line.push(Span::raw(" ".to_string()));
                } else {
                    current_spans.push(Span::raw(" ".to_string()));
                }
            }
            Event::HardBreak => {
                if let Some(ref mut td) = table_data {
                    let line = std::mem::take(&mut td.current_cell_line);
                    td.current_cell.push(line);
                } else {
                    flush_line(&mut lines, &mut current_spans);
                }
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

            Event::Start(Tag::Table(alignments)) => {
                flush_line(&mut lines, &mut current_spans);
                table_data = Some(TableData::new(alignments));
            }
            Event::End(TagEnd::Table) => {
                if let Some(td) = table_data.take() {
                    let table_lines = render_styled_table(&td, width, &styles);
                    lines.extend(table_lines);
                    lines.push(Line::default());
                    had_paragraph = true;
                }
            }

            Event::Start(Tag::TableHead) => {
                if let Some(ref mut td) = table_data {
                    td.in_header = true;
                    td.current_row = Vec::new();
                }
            }
            Event::End(TagEnd::TableHead) => {
                if let Some(ref mut td) = table_data {
                    let row = std::mem::take(&mut td.current_row);
                    td.header_rows.push(row);
                    td.in_header = false;
                }
            }

            Event::Start(Tag::TableRow) => {
                if let Some(ref mut td) = table_data {
                    td.current_row = Vec::new();
                }
            }
            Event::End(TagEnd::TableRow) => {
                if let Some(ref mut td) = table_data {
                    let row = std::mem::take(&mut td.current_row);
                    td.data_rows.push(row);
                }
            }

            Event::Start(Tag::TableCell) => {
                if let Some(ref mut td) = table_data {
                    td.current_cell = Vec::new();
                    td.current_cell_line = Vec::new();
                }
            }
            Event::End(TagEnd::TableCell) => {
                if let Some(ref mut td) = table_data {
                    let line = std::mem::take(&mut td.current_cell_line);
                    if !line.is_empty() {
                        td.current_cell.push(line);
                    }
                    if td.current_cell.is_empty() {
                        td.current_cell.push(Vec::new());
                    }
                    let cell = std::mem::take(&mut td.current_cell);
                    td.current_row.push(cell);
                }
            }

            Event::InlineHtml(html) => {
                let tag = html.trim().to_lowercase();
                if tag == "<br>" || tag == "<br/>" || tag == "<br />" {
                    if let Some(ref mut td) = table_data {
                        let line = std::mem::take(&mut td.current_cell_line);
                        td.current_cell.push(line);
                    } else {
                        flush_line(&mut lines, &mut current_spans);
                    }
                }
            }

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

/// Check if text is a labeled horizontal rule (e.g., "─── You ───") and render it
/// to fill the given width with the label centered.
fn try_render_labeled_rule(text: &str, width: usize) -> Option<Line<'static>> {
    let trimmed = text.trim();
    if !trimmed.starts_with('─') || !trimmed.ends_with('─') {
        return None;
    }
    // Find label boundaries (first and last non-─ characters)
    let start = trimmed.find(|c: char| c != '─')?;
    let end = trimmed.rfind(|c: char| c != '─')? + 1;
    let label = trimmed[start..end].trim();
    if label.is_empty() {
        return None;
    }

    let label_with_space = format!(" {} ", label);
    let label_width = UnicodeWidthStr::width(label_with_space.as_str());
    let remaining = width.saturating_sub(label_width);
    let left_count = remaining / 2;
    let right_count = remaining.saturating_sub(left_count);

    let rule_style = Style::default().fg(Color::DarkGray);
    let label_style = Style::default()
        .fg(Color::Cyan)
        .add_modifier(Modifier::BOLD);

    Some(Line::from(vec![
        Span::styled("─".repeat(left_count), rule_style),
        Span::styled(label_with_space, label_style),
        Span::styled("─".repeat(right_count), rule_style),
    ]))
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

/// Compute the visible display width of a sequence of spans.
fn visible_width_of_spans(spans: &[Span]) -> usize {
    spans
        .iter()
        .map(|s| UnicodeWidthStr::width(s.content.as_ref()))
        .sum()
}

/// Generate a styled horizontal border line for a table.
fn styled_border_line(
    left: char,
    mid: char,
    right: char,
    fill: char,
    widths: &[usize],
    style: Style,
) -> Line<'static> {
    let mut s = String::new();
    s.push(left);
    for (i, &w) in widths.iter().enumerate() {
        for _ in 0..w + 2 {
            s.push(fill);
        }
        if i < widths.len() - 1 {
            s.push(mid);
        }
    }
    s.push(right);
    Line::from(Span::styled(s, style))
}

/// Find a byte offset for breaking `text` at approximately `avail` display-width characters.
/// Prefers breaking at a space; falls back to hard-breaking at the width limit.
fn find_break_point(text: &str, avail: usize) -> usize {
    let mut width_so_far = 0usize;
    let mut last_space_byte = None;
    let mut byte_at_avail = text.len();

    for (byte_idx, ch) in text.char_indices() {
        let ch_width = unicode_width::UnicodeWidthChar::width(ch).unwrap_or(0);
        if width_so_far + ch_width > avail {
            byte_at_avail = byte_idx;
            break;
        }
        if ch == ' ' {
            last_space_byte = Some(byte_idx);
        }
        width_so_far += ch_width;
    }

    if let Some(pos) = last_space_byte {
        pos
    } else {
        byte_at_avail
    }
}

/// Wrap a sequence of styled spans to fit within `width` visible characters.
/// Returns a Vec of lines, each line being a Vec<Span>.
fn wrap_spans(spans: &[Span<'static>], width: usize) -> Vec<Vec<Span<'static>>> {
    if width == 0 {
        return vec![vec![]];
    }

    // Fast path: if total width fits, return as-is
    let total = visible_width_of_spans(spans);
    if total <= width {
        return vec![spans.to_vec()];
    }

    let mut result: Vec<Vec<Span<'static>>> = Vec::new();
    let mut current_line: Vec<Span<'static>> = Vec::new();
    let mut current_width: usize = 0;

    for span in spans {
        let span_text = span.content.as_ref();
        let span_style = span.style;
        let span_width = UnicodeWidthStr::width(span_text);

        if current_width + span_width <= width {
            current_line.push(span.clone());
            current_width += span_width;
            continue;
        }

        // Need to split this span across lines
        let mut remaining = span_text;
        while !remaining.is_empty() {
            let avail = width.saturating_sub(current_width);
            if avail == 0 {
                result.push(std::mem::take(&mut current_line));
                current_width = 0;
                continue;
            }

            let rem_width = UnicodeWidthStr::width(remaining);
            if rem_width <= avail {
                current_line.push(Span::styled(remaining.to_string(), span_style));
                current_width += rem_width;
                break;
            }

            let break_pos = find_break_point(remaining, avail);
            let (chunk, rest) = remaining.split_at(break_pos);

            if !chunk.is_empty() {
                current_line.push(Span::styled(chunk.to_string(), span_style));
            }
            result.push(std::mem::take(&mut current_line));
            current_width = 0;
            remaining = rest.trim_start();
        }
    }

    if !current_line.is_empty() {
        result.push(current_line);
    }
    if result.is_empty() {
        result.push(Vec::new());
    }

    result
}

/// Render a single row of a styled table, with word-wrapping and padding.
fn render_styled_row(
    out: &mut Vec<Line<'static>>,
    row: &[StyledCell],
    col_widths: &[usize],
    num_cols: usize,
    border_style: Style,
) {
    let empty_cell: StyledCell = vec![vec![]];

    // For each cell, produce wrapped lines
    let mut wrapped_cells: Vec<Vec<Vec<Span<'static>>>> = Vec::with_capacity(num_cols);
    for i in 0..num_cols {
        let cell = if i < row.len() { &row[i] } else { &empty_cell };
        let mut all_wrapped: Vec<Vec<Span<'static>>> = Vec::new();
        for logical_line in cell {
            let wrapped = wrap_spans(logical_line, col_widths[i]);
            all_wrapped.extend(wrapped);
        }
        if all_wrapped.is_empty() {
            all_wrapped.push(Vec::new());
        }
        wrapped_cells.push(all_wrapped);
    }

    // Find max physical lines across all cells
    let max_lines = wrapped_cells.iter().map(|c| c.len()).max().unwrap_or(1);

    let empty_spans: Vec<Span<'static>> = Vec::new();

    // Render each physical line
    for line_idx in 0..max_lines {
        let mut line_spans: Vec<Span<'static>> = Vec::new();
        line_spans.push(Span::styled("│".to_string(), border_style));
        for (col, wrapped) in wrapped_cells.iter().enumerate() {
            let cell_line = if line_idx < wrapped.len() {
                &wrapped[line_idx]
            } else {
                &empty_spans
            };
            let w = col_widths[col];
            let content_width = visible_width_of_spans(cell_line);

            line_spans.push(Span::raw(" ".to_string())); // left pad
            line_spans.extend(cell_line.iter().cloned());
            // Right-pad to fill column width
            let pad = w.saturating_sub(content_width);
            if pad > 0 {
                line_spans.push(Span::raw(" ".repeat(pad)));
            }
            line_spans.push(Span::raw(" ".to_string())); // right pad
            line_spans.push(Span::styled("│".to_string(), border_style));
        }
        out.push(Line::from(line_spans));
    }
}

/// Render a complete styled table from collected table data.
fn render_styled_table(
    td: &TableData,
    available_width: usize,
    _styles: &MarkdownStyle,
) -> Vec<Line<'static>> {
    let num_cols = td.alignments.len().max(
        td.header_rows
            .iter()
            .chain(td.data_rows.iter())
            .map(|r| r.len())
            .max()
            .unwrap_or(0),
    );
    if num_cols == 0 {
        return Vec::new();
    }

    // 1. Measure visible width of each cell
    let all_rows: Vec<&Vec<StyledCell>> =
        td.header_rows.iter().chain(td.data_rows.iter()).collect();
    let mut max_widths = vec![0usize; num_cols];
    for row in &all_rows {
        for (i, cell) in row.iter().enumerate() {
            if i < num_cols {
                let cell_width = cell
                    .iter()
                    .map(|line| visible_width_of_spans(line))
                    .max()
                    .unwrap_or(0);
                max_widths[i] = max_widths[i].max(cell_width);
            }
        }
    }

    // 2. Calculate column widths
    let col_widths = calculate_col_widths(&max_widths, num_cols, available_width);

    // 3. Render border + data lines
    let border_style = Style::default().fg(Color::DarkGray);
    let mut out_lines: Vec<Line<'static>> = Vec::new();

    // Top border
    out_lines.push(styled_border_line(
        '┌',
        '┬',
        '┐',
        '─',
        &col_widths,
        border_style,
    ));

    // Header rows
    for header in &td.header_rows {
        render_styled_row(&mut out_lines, header, &col_widths, num_cols, border_style);
    }

    // Separator
    out_lines.push(styled_border_line(
        '├',
        '┼',
        '┤',
        '─',
        &col_widths,
        border_style,
    ));

    // Data rows with separators
    for (i, row) in td.data_rows.iter().enumerate() {
        render_styled_row(&mut out_lines, row, &col_widths, num_cols, border_style);
        if i < td.data_rows.len() - 1 {
            out_lines.push(styled_border_line(
                '├',
                '┼',
                '┤',
                '─',
                &col_widths,
                border_style,
            ));
        }
    }

    // Bottom border
    out_lines.push(styled_border_line(
        '└',
        '┴',
        '┘',
        '─',
        &col_widths,
        border_style,
    ));

    out_lines
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

        let ratatui_text = render_to_text(&self.content, self.term_width);
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
    let ratatui_text = render_to_text(content, term_width);
    let output = text_to_ansi(&ratatui_text);
    println!("{}", output);
}

/// Minimum characters per column before we self-render the table.
const MIN_COL_WIDTH: usize = 10;

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_render_to_text_basic() {
        let text = render_to_text("Hello world", 80);
        assert!(!text.lines.is_empty());
        let content: String = text
            .lines
            .iter()
            .flat_map(|l| l.spans.iter().map(|s| s.content.to_string()))
            .collect();
        assert!(content.contains("Hello world"));
    }

    #[test]
    fn test_render_bold_style() {
        let text = render_to_text("This is **bold** text", 80);
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
        let text = render_to_text("# Heading 1\n\n## Heading 2\n\n### Heading 3", 80);
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
        let text = render_to_text("Use `code` here", 80);
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
        let text = render_to_text("- Item 1\n- Item 2\n- Item 3", 80);
        let cyan_spans: Vec<_> = text
            .lines
            .iter()
            .flat_map(|l| l.spans.iter())
            .filter(|s| s.style.fg == Some(Color::Cyan))
            .collect();
        assert!(!cyan_spans.is_empty());
    }

    #[test]
    fn test_render_blockquote() {
        let text = render_to_text("> This is a quote", 80);
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
        let text = render_to_text("This is ***bold italic*** text", 80);
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
        let text = render_to_text("**bold** and *italic*", 80);
        let ansi = text_to_ansi(&text);
        assert!(ansi.contains("\x1b["));
        assert!(ansi.contains("\x1b[0m"));
        assert!(ansi.contains("bold"));
        assert!(ansi.contains("italic"));
    }

    #[test]
    fn test_markdown_renderer() {
        let renderer = MarkdownRenderer::new();
        assert!(renderer.is_empty());
    }

    #[test]
    fn test_waterfall_preserves_narrow_columns() {
        let widths = calculate_col_widths(&[15, 200], 2, 80);
        assert_eq!(widths[0], 15);
        assert_eq!(widths[0] + widths[1] + 7, 80);
    }

    // --- Table styling tests ---

    #[test]
    fn test_table_bold_in_cell() {
        let table =
            "| Header |\n|--------|\n| **bold** |";
        let text = render_to_text(table, 80);
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
    fn test_table_italic_in_cell() {
        let table =
            "| Header |\n|--------|\n| *italic* |";
        let text = render_to_text(table, 80);
        let italic_spans: Vec<_> = text
            .lines
            .iter()
            .flat_map(|l| l.spans.iter())
            .filter(|s| s.style.add_modifier.contains(Modifier::ITALIC))
            .collect();
        assert!(!italic_spans.is_empty());
        assert!(italic_spans.iter().any(|s| s.content.contains("italic")));
    }

    #[test]
    fn test_table_inline_code_in_cell() {
        let table =
            "| Header |\n|--------|\n| `code` |";
        let text = render_to_text(table, 80);
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
    fn test_table_br_in_cell() {
        let table =
            "| Header |\n|--------|\n| Line 1<br>Line 2 |";
        let text = render_to_text(table, 80);
        let ansi = text_to_ansi(&text);
        // Both lines should appear
        assert!(ansi.contains("Line 1"));
        assert!(ansi.contains("Line 2"));
        // <br> should not appear literally
        assert!(!ansi.contains("<br>"));
    }

    #[test]
    fn test_table_structure_box_drawing() {
        let table = "| Header 1 | Header 2 |\n|----------|----------|\n| Cell 1   | Cell 2   |";
        let text = render_to_text(table, 80);
        let ansi = text_to_ansi(&text);
        assert!(ansi.contains('┌'));
        assert!(ansi.contains('┬'));
        assert!(ansi.contains('┐'));
        assert!(ansi.contains('├'));
        assert!(ansi.contains('┼'));
        assert!(ansi.contains('┤'));
        assert!(ansi.contains('└'));
        assert!(ansi.contains('┴'));
        assert!(ansi.contains('┘'));
        assert!(ansi.contains('│'));
    }

    #[test]
    fn test_table_border_style_dark_gray() {
        let table = "| H1 | H2 |\n|----|----|\n| a  | b  |";
        let text = render_to_text(table, 80);
        // Border lines should have DarkGray color
        let border_spans: Vec<_> = text
            .lines
            .iter()
            .flat_map(|l| l.spans.iter())
            .filter(|s| {
                s.style.fg == Some(Color::DarkGray)
                    && (s.content.contains('┌') || s.content.contains('│'))
            })
            .collect();
        assert!(!border_spans.is_empty());
    }

    #[test]
    fn test_table_in_code_block() {
        let content = "```\n| Not | A | Table |\n|-----|---|-------|\n| x   | y | z     |\n```";
        let text = render_to_text(content, 80);
        let ansi = text_to_ansi(&text);
        // Should NOT be rendered as a table - it's inside a code block
        assert!(!ansi.contains('┌'));
        assert!(ansi.contains("| Not | A | Table |"));
    }

    #[test]
    fn test_mixed_content_with_table() {
        let content = "Some text before\n\n| Col 1 | Col 2 |\n|-------|-------|\n| a     | b     |\n\nSome text after";
        let text = render_to_text(content, 80);
        let ansi = text_to_ansi(&text);
        assert!(ansi.contains("Some text before"));
        assert!(ansi.contains("Some text after"));
        assert!(ansi.contains('┌'));
    }

    #[test]
    fn test_table_many_columns_narrow_width() {
        let table = "| A | B | C | D | E | F |\n|---|---|---|---|---|---|\n| 1 | 2 | 3 | 4 | 5 | 6 |";
        let text = render_to_text(table, 50);
        let ansi = text_to_ansi(&text);
        assert!(ansi.contains('┌'));
        assert!(ansi.contains('├'));
        assert!(ansi.contains('└'));
    }

    #[test]
    fn test_table_with_word_wrapping() {
        let table = "| Header |\n|--------|\n| This is a very long cell content that should wrap |";
        let text = render_to_text(table, 30);
        let ansi = text_to_ansi(&text);
        // Content should be present (possibly wrapped)
        assert!(ansi.contains("This"));
        assert!(ansi.contains("wrap"));
    }
}
