//! UI layout rendering for the TUI.

use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph},
    Frame,
};

use super::app::TuiApp;
use super::widgets::{
    ContentArea, InputArea, StatusBar, ThinkingPanel, ToolBar,
};

/// Render the entire TUI
pub fn render(app: &TuiApp, frame: &mut Frame) {
    let area = frame.area();

    // Calculate layout based on what's visible
    let chunks = create_layout(area, app);

    // Status bar at top
    let agent_progress = app.agent_progress.as_ref().map(|(name, iter, max)| (name.as_str(), *iter, *max));
    let mut status_bar = StatusBar::new(&app.profile, &app.primary_agent)
        .tokens(app.prompt_tokens, app.completion_tokens)
        .streaming(app.is_streaming)
        .waiting(app.is_waiting)
        .execution_context(&app.execution_context)
        .iteration(app.tool_iteration)
        .agent_progress(agent_progress)
        .agent_bytes(app.agent_input_bytes, app.agent_output_bytes)
        .session_bytes(app.session_input_bytes, app.session_output_bytes);

    if let Some(ref msg) = app.status_message {
        status_bar = status_bar.status(msg);
    }

    frame.render_widget(status_bar, chunks.status);

    // Thinking panel (if visible and has content)
    if app.show_thinking && !app.thinking_content.is_empty() {
        let is_thinking_streaming = app.is_streaming && app.content.is_empty();
        let thinking = ThinkingPanel::new(&app.thinking_content)
            .expanded(app.thinking_expanded)
            .streaming(is_thinking_streaming)
            .auto_scroll(true); // Always auto-scroll thinking panel

        frame.render_widget(thinking, chunks.thinking);
    }

    // Main content area
    let content = ContentArea::new(&app.content)
        .scroll(app.scroll_offset)
        .streaming(app.is_streaming)
        .auto_scroll(app.auto_scroll);

    frame.render_widget(content, chunks.content);

    // Tool bar (if there are tools)
    if !app.tool_calls.is_empty() {
        let tool_bar = ToolBar::new(&app.tool_calls);
        frame.render_widget(tool_bar, chunks.tools);
    }

    // Input area
    let input_hint = if app.is_streaming {
        "Press Ctrl+C to cancel"
    } else {
        "/help | /quit | PgUp/PgDn scroll | Ctrl+T toggle thinking"
    };

    let input = InputArea::new(&app.input)
        .active(!app.is_streaming)
        .hint(input_hint);

    frame.render_widget(input, chunks.input);

    // Show help overlay if requested
    if app.show_help {
        render_help_overlay(frame, area);
    }
}

/// Layout regions
struct LayoutRegions {
    status: Rect,
    thinking: Rect,
    content: Rect,
    tools: Rect,
    input: Rect,
}

/// Calculate the height needed for the input area based on text wrapping
fn calculate_input_height(input_text: &str, available_width: u16) -> u16 {
    let prompt_len = 5; // "you> "
    let text_width = available_width.saturating_sub(prompt_len) as usize;

    if text_width == 0 || input_text.is_empty() {
        return 3; // Minimum: 1 border + 1 input line + 1 hint line
    }

    // Calculate wrapped line count
    let wrapped_lines = (input_text.len() + text_width - 1) / text_width;

    // 1 for border + wrapped lines + 1 for hint, max 10 lines total
    let height = (1 + wrapped_lines + 1) as u16;
    height.clamp(3, 10)
}

/// Create layout based on current app state
fn create_layout(area: Rect, app: &TuiApp) -> LayoutRegions {
    let has_thinking = app.show_thinking && !app.thinking_content.is_empty();
    let has_tools = !app.tool_calls.is_empty();

    // Build constraints dynamically
    let mut constraints = vec![
        Constraint::Length(2), // Status bar
    ];

    // Thinking panel - normal or expanded
    // Normal: min 8 lines (6 content + 2 border), max 10
    // Expanded: takes ~70% of the space
    if has_thinking {
        if app.thinking_expanded {
            // Expanded: use percentage to take most of the screen
            constraints.push(Constraint::Percentage(70));
        } else {
            // Normal: based on content, min 8 (6 content lines), max 10
            let lines = app.thinking_content.lines().count() as u16;
            let thinking_height = (lines + 2).min(10).max(8); // +2 for borders, min 8, max 10
            constraints.push(Constraint::Length(thinking_height));
        }
    }

    // Main content - takes remaining space (min 6 lines when thinking expanded)
    let min_content_height = if has_thinking && app.thinking_expanded { 6 } else { 5 };
    constraints.push(Constraint::Min(min_content_height));

    // Tool bar
    let tools_height = if has_tools { 2 } else { 0 };
    if has_tools {
        constraints.push(Constraint::Length(tools_height));
    }

    // Input area - dynamic height based on text length
    let input_height = calculate_input_height(app.input.value(), area.width);
    constraints.push(Constraint::Length(input_height));

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints(constraints)
        .split(area);

    let mut idx = 0;

    let status = chunks[idx];
    idx += 1;

    let thinking = if has_thinking {
        let r = chunks[idx];
        idx += 1;
        r
    } else {
        Rect::default()
    };

    let content = chunks[idx];
    idx += 1;

    let tools = if has_tools {
        let r = chunks[idx];
        idx += 1;
        r
    } else {
        Rect::default()
    };

    let input = chunks[idx];

    LayoutRegions {
        status,
        thinking,
        content,
        tools,
        input,
    }
}

/// Render help overlay
fn render_help_overlay(frame: &mut Frame, area: Rect) {
    // Create centered overlay
    let overlay_width = 60u16.min(area.width.saturating_sub(4));
    let overlay_height = 20u16.min(area.height.saturating_sub(4));

    let x = (area.width.saturating_sub(overlay_width)) / 2;
    let y = (area.height.saturating_sub(overlay_height)) / 2;

    let overlay_area = Rect::new(x, y, overlay_width, overlay_height);

    // Clear the area
    frame.render_widget(
        Block::default().style(Style::default().bg(Color::Black)),
        overlay_area,
    );

    let help_text = vec![
        Line::from(Span::styled(
            "Quick Query TUI Help",
            Style::default().fg(Color::Green),
        )),
        Line::from(""),
        Line::from(Span::styled("Navigation:", Style::default().fg(Color::Cyan))),
        Line::from("  PgUp/Ctrl+B  Page up"),
        Line::from("  PgDn/Ctrl+F  Page down"),
        Line::from("  Ctrl+Home    Scroll to top"),
        Line::from("  Ctrl+End     Scroll to bottom"),
        Line::from("  Ctrl+T       Expand/shrink thinking panel"),
        Line::from(""),
        Line::from(Span::styled("Commands:", Style::default().fg(Color::Cyan))),
        Line::from("  /help        Show this help"),
        Line::from("  /quit        Exit the application"),
        Line::from("  /clear       Clear conversation"),
        Line::from("  /reset       Reset session (clear + reset tokens)"),
        Line::from("  /tools       List available tools"),
        Line::from("  /agents      List available agents"),
        Line::from(""),
        Line::from(Span::styled("Other:", Style::default().fg(Color::Cyan))),
        Line::from("  Ctrl+C       Cancel streaming"),
        Line::from("  Ctrl+D       Exit"),
        Line::from(""),
        Line::from(Span::styled(
            "Press any key to close",
            Style::default().fg(Color::DarkGray),
        )),
    ];

    let help = Paragraph::new(help_text)
        .block(
            Block::default()
                .title(" Help ")
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::Cyan)),
        )
        .style(Style::default().bg(Color::Black));

    frame.render_widget(help, overlay_area);
}
