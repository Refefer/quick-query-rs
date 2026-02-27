//! UI layout rendering for the TUI.

use std::collections::HashMap;

use ratatui::{
    layout::Rect,
    style::{Color, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Paragraph},
    Frame,
};

use super::app::TuiApp;
use super::layout::PaneId;
use super::widgets::{ContentArea, InputArea, StatusBar, ThinkingPanel};

/// Render the entire TUI using a pre-computed layout.
///
/// The layout must be computed by the caller to ensure scroll dimensions
/// are calculated with the same layout used for rendering.
pub fn render(app: &TuiApp, frame: &mut Frame, layout: &HashMap<PaneId, Rect>) {
    let has_thinking = app.show_thinking && !app.thinking_content.is_empty();

    // Render Content area (at top now)
    if let Some(&content_rect) = layout.get(&PaneId::Content) {
        if content_rect.height > 0 {
            let mut content = ContentArea::new(&app.content)
                .scroll(app.scroll.effective_offset())
                .streaming(app.is_streaming);

            // Use cached rendered text if available (avoids re-parsing markdown every frame)
            if let Some(cached_text) = app.get_cached_content() {
                content = content.with_cached_text(cached_text);
            }

            frame.render_widget(content, content_rect);
        }
    }

    // Render Thinking panel (below content)
    if let Some(&thinking_rect) = layout.get(&PaneId::Thinking) {
        if thinking_rect.height > 0 && has_thinking {
            let is_thinking_streaming = app.is_streaming && app.content.is_empty();
            let thinking_str = app.thinking_content.as_str();
            let thinking = ThinkingPanel::new(&thinking_str)
                .tool_notifications(&app.tool_notifications)
                .expanded(app.thinking_expanded)
                .streaming(is_thinking_streaming)
                .auto_scroll(true);

            frame.render_widget(thinking, thinking_rect);
        }
    }

    // Render Status bar (below thinking)
    if let Some(&status_rect) = layout.get(&PaneId::Status) {
        if status_rect.height > 0 {
            let agent_progress = app
                .agent_progress
                .as_ref()
                .map(|(name, iter, max)| (name.as_str(), *iter, *max));
            let mut status_bar = StatusBar::new(&app.profile, &app.primary_agent)
                .tokens(app.prompt_tokens, app.completion_tokens)
                .streaming(app.is_streaming)
                .streaming_state(app.streaming_state)
                .execution_context(&app.execution_context)
                .iteration(app.tool_iteration)
                .agent_progress(agent_progress)
                .agent_bytes(app.agent_input_bytes, app.agent_output_bytes)
                .session_bytes(app.session_input_bytes, app.session_output_bytes);

            if let Some(ref msg) = app.status_message {
                status_bar = status_bar.status(msg);
            }

            frame.render_widget(status_bar, status_rect);
        }
    }

    // Render Input area (at bottom)
    if let Some(&input_rect) = layout.get(&PaneId::Input) {
        if input_rect.height > 0 {
            let input_hint = if app.is_streaming {
                "Press Ctrl+C to cancel"
            } else if !app.mouse_captured {
                "SELECT MODE \u{2014} Ctrl+Y to resume mouse scroll"
            } else {
                "/help | /quit | PgUp/PgDn scroll | Ctrl+Y select mode"
            };

            let input = InputArea::new(&app.input)
                .active(!app.is_streaming)
                .hint(input_hint);

            frame.render_widget(input, input_rect);
        }
    }

    // Show help overlay if requested
    if app.show_help {
        render_help_overlay(frame);
    }

    // Show approval overlay if pending (renders on top of everything)
    if let Some(ref request) = app.pending_approval {
        render_approval_overlay(frame, request);
    }
}

/// Calculate the number of wrapped lines for input text.
pub fn calculate_input_lines(input_text: &str, available_width: u16) -> u16 {
    let prompt_len = 5; // "you> "
    let text_width = available_width.saturating_sub(prompt_len) as usize;

    if text_width == 0 || input_text.is_empty() {
        return 1; // Minimum 1 line for input
    }

    // Calculate wrapped line count
    let wrapped_lines = input_text.len().div_ceil(text_width);
    wrapped_lines as u16
}

/// Render help overlay
fn render_help_overlay(frame: &mut Frame) {
    let area = frame.area();

    // Create centered overlay
    let overlay_width = 60u16.min(area.width.saturating_sub(4));
    let overlay_height = 31u16.min(area.height.saturating_sub(4));

    let x = (area.width.saturating_sub(overlay_width)) / 2;
    let y = (area.height.saturating_sub(overlay_height)) / 2;

    let overlay_area = Rect::new(x, y, overlay_width, overlay_height);

    // Clear the area (Clear replaces characters with spaces; Block alone only changes style)
    frame.render_widget(Clear, overlay_area);

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
        Line::from("  Ctrl+H       Hide/show thinking panel"),
        Line::from("  Mouse wheel  Scroll content (when captured)"),
        Line::from(""),
        Line::from(Span::styled("Commands:", Style::default().fg(Color::Cyan))),
        Line::from("  /help        Show this help"),
        Line::from("  /quit        Exit the application"),
        Line::from("  /clear       Clear conversation + counters"),
        Line::from("  /reset       Full reset (clear + agent memory + tasks)"),
        Line::from("  /history     Show message count"),
        Line::from("  /memory      Show memory diagnostics"),
        Line::from("  /tools       List available tools"),
        Line::from("  /agents      List available agents"),
        Line::from("  /mount <p>   Add read-only bash sandbox mount"),
        Line::from("  /mounts      List bash sandbox mounts"),
        Line::from(""),
        Line::from(Span::styled("Other:", Style::default().fg(Color::Cyan))),
        Line::from("  Ctrl+Y       Toggle select mode (for copy)"),
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

/// Render approval overlay modal for per-call approval (bash commands, file operations, mounts)
fn render_approval_overlay(frame: &mut Frame, request: &qq_tools::ApprovalRequest) {
    let area = frame.area();

    // Use up to 80% of terminal width for the overlay
    let overlay_width = (area.width * 4 / 5).max(40).min(area.width.saturating_sub(4));
    // Inner width available for text (subtract 2 for borders)
    let inner_width = overlay_width.saturating_sub(2) as usize;

    let triggers = if request.trigger_commands.is_empty() {
        String::new()
    } else {
        format!("Triggered by: {}", request.trigger_commands.join(", "))
    };

    // Calculate how many lines the command will wrap to
    let cmd_lines = if inner_width > 0 {
        (request.full_command.len() + inner_width - 1) / inner_width
    } else {
        1
    };
    // Cap command display to a reasonable number of wrapped lines
    let max_cmd_lines: usize = 8;
    let cmd_lines = cmd_lines.min(max_cmd_lines);

    // Height: border(1) + header(1) + blank(1) + cmd_lines + triggers(0..1) + blank(1) + keys(1) + border(1)
    let triggers_line = if triggers.is_empty() { 0u16 } else { 1 };
    let overlay_height = (6 + cmd_lines as u16 + triggers_line).min(area.height.saturating_sub(4));

    let x = (area.width.saturating_sub(overlay_width)) / 2;
    let y = (area.height.saturating_sub(overlay_height)) / 2;

    let overlay_area = Rect::new(x, y, overlay_width, overlay_height);

    // Clear the area behind the overlay (Clear replaces characters with spaces)
    frame.render_widget(Clear, overlay_area);

    // Truncate command only if it exceeds the wrapped line cap
    let max_chars = inner_width * max_cmd_lines;
    let cmd_display = if request.full_command.len() > max_chars {
        format!("{}...", &request.full_command[..max_chars.saturating_sub(3)])
    } else {
        request.full_command.clone()
    };

    let header_text = format!("{} Approval Required", request.category);
    let mut lines = vec![
        Line::from(Span::styled(
            header_text,
            Style::default().fg(Color::Yellow),
        )),
        Line::from(""),
    ];

    // Manually wrap the command text so each Line gets styled correctly
    let cmd_bytes = cmd_display.as_bytes();
    for chunk_start in (0..cmd_display.len()).step_by(inner_width.max(1)) {
        let chunk_end = (chunk_start + inner_width).min(cmd_display.len());
        let chunk = String::from_utf8_lossy(&cmd_bytes[chunk_start..chunk_end]);
        lines.push(Line::from(Span::styled(
            chunk.into_owned(),
            Style::default().fg(Color::White),
        )));
    }

    if !triggers.is_empty() {
        lines.push(Line::from(Span::styled(
            triggers,
            Style::default().fg(Color::DarkGray),
        )));
    }

    lines.extend([
        Line::from(""),
        Line::from(vec![
            Span::styled("[a]", Style::default().fg(Color::Green)),
            Span::raw("llow once  "),
            Span::styled("[s]", Style::default().fg(Color::Cyan)),
            Span::raw("ession allow  "),
            Span::styled("[d]", Style::default().fg(Color::Red)),
            Span::raw("eny"),
        ]),
    ]);

    let paragraph = Paragraph::new(lines)
        .block(
            Block::default()
                .title(format!(" Approve {} ", request.category))
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::Yellow)),
        )
        .style(Style::default().bg(Color::Black));

    frame.render_widget(paragraph, overlay_area);
}
