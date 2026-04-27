//! UI layout rendering for the TUI.

use std::collections::HashMap;

use ratatui::{
    layout::Rect,
    style::{Color, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Paragraph},
    Frame,
};

use super::app::{ProfilesPickerStage, ProfilesTarget, TuiApp};
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
                .map(|(name, iter, chain)| (name.as_str(), *iter, chain.as_slice()));
            // Show top border only when thinking panel isn't visible (avoids double line)
            let thinking_visible = has_thinking
                && layout
                    .get(&PaneId::Thinking)
                    .is_some_and(|r| r.height > 0);

            let mut status_bar = StatusBar::new(&app.profile, &app.primary_agent)
                .tokens(app.prompt_tokens, app.completion_tokens)
                .streaming(app.is_streaming)
                .streaming_state(app.streaming_state)
                .execution_context(&app.execution_context)
                .iteration(app.tool_iteration)
                .agent_progress(agent_progress)
                .agent_bytes(app.agent_input_bytes, app.agent_output_bytes)
                .session_bytes(app.session_input_bytes, app.session_output_bytes)
                .show_top_border(!thinking_visible);

            if let Some(ref msg) = app.status_message {
                status_bar = status_bar.status(msg);
            }

            frame.render_widget(status_bar, status_rect);
        }
    }

    // Render Input area (at bottom)
    if let Some(&input_rect) = layout.get(&PaneId::Input) {
        if input_rect.height > 0 {
            let pending_hint;
            let input_hint = if app.is_streaming {
                "Press Ctrl+C to cancel"
            } else if !app.mouse_captured {
                "SELECT MODE \u{2014} Ctrl+Y to resume scroll"
            } else if !app.pending_content.is_empty() {
                pending_hint = format!(
                    "{} image(s) attached | /clear-attachments | /help",
                    app.pending_content.len()
                );
                &pending_hint
            } else {
                "/help | /quit | PgUp/PgDn scroll | Shift+select to copy"
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
        render_approval_overlay(frame, request, app.denial_reason_input.as_deref());
    }

    // Show /profiles picker overlay if open (always on top)
    if let Some(ref stage) = app.profiles_picker {
        render_profiles_overlay(frame, stage);
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
    let overlay_height = 35u16.min(area.height.saturating_sub(4));

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
        Line::from("  /attach <p>  Attach an image file"),
        Line::from("  /attachments List pending attachments"),
        Line::from("  /clear-attachments  Remove all attachments"),
        Line::from("  /profiles    Switch profile for chat or any agent"),
        Line::from(""),
        Line::from(Span::styled("Other:", Style::default().fg(Color::Cyan))),
        Line::from("  Shift+drag   Select text (works in most terminals)"),
        Line::from("  Ctrl+Y       Toggle select mode (fallback for copy)"),
        Line::from("  Alt+V        Paste image from clipboard"),
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
fn render_approval_overlay(
    frame: &mut Frame,
    request: &qq_tools::ApprovalRequest,
    denial_reason: Option<&str>,
) {
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
        request.full_command.len().div_ceil(inner_width)
    } else {
        1
    };
    // Cap command display to a reasonable number of wrapped lines
    let max_cmd_lines: usize = 8;
    let cmd_lines = cmd_lines.min(max_cmd_lines);

    // Height: border(1) + header(1) + blank(1) + cmd_lines + triggers(0..1) + blank(1) + keys/reason(1..3) + border(1)
    let triggers_line = if triggers.is_empty() { 0u16 } else { 1 };
    let reason_lines = if denial_reason.is_some() { 2u16 } else { 0 };
    let overlay_height =
        (6 + cmd_lines as u16 + triggers_line + reason_lines).min(area.height.saturating_sub(4));

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

    lines.push(Line::from(""));
    if let Some(reason_text) = denial_reason {
        lines.push(Line::from(Span::styled(
            "Deny reason (Enter to submit, Esc to skip):",
            Style::default().fg(Color::Red),
        )));
        lines.push(Line::from(vec![
            Span::styled("> ", Style::default().fg(Color::Red)),
            Span::styled(reason_text.to_string(), Style::default().fg(Color::White)),
            Span::styled("_", Style::default().fg(Color::DarkGray)),
        ]));
    } else {
        lines.push(Line::from(vec![
            Span::styled("[a]", Style::default().fg(Color::Green)),
            Span::raw("llow once  "),
            Span::styled("[s]", Style::default().fg(Color::Cyan)),
            Span::raw("ession allow  "),
            Span::styled("[d]", Style::default().fg(Color::Red)),
            Span::raw("eny"),
        ]));
    }

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

/// Render the `/profiles` picker overlay.
fn render_profiles_overlay(frame: &mut Frame, stage: &ProfilesPickerStage) {
    let area = frame.area();

    let (title, rows, cursor) = build_profiles_rows(stage);

    // Reserve room for: title (1) + blank (1) + rows + blank (1) + hint (1) + 2 borders.
    let max_inner_height = area.height.saturating_sub(4) as usize;
    let desired_inner = rows.len() + 4;
    let inner_height = desired_inner.min(max_inner_height.max(5));

    let overlay_width = 60u16
        .max(title.len() as u16 + 6)
        .min(area.width.saturating_sub(4));
    let overlay_height = (inner_height as u16 + 2).min(area.height.saturating_sub(2));
    let x = (area.width.saturating_sub(overlay_width)) / 2;
    let y = (area.height.saturating_sub(overlay_height)) / 2;
    let overlay_area = Rect::new(x, y, overlay_width, overlay_height);

    frame.render_widget(Clear, overlay_area);

    let mut lines: Vec<Line> = Vec::with_capacity(rows.len() + 4);
    lines.push(Line::from(Span::styled(
        title,
        Style::default().fg(Color::Cyan),
    )));
    lines.push(Line::from(""));

    // Render rows with the cursor row highlighted. Truncate the visible rows
    // to the overlay height so we don't draw past the box.
    let visible_rows = inner_height.saturating_sub(4);
    let (start, end) = visible_window(cursor, rows.len(), visible_rows);
    for (i, row) in rows.iter().enumerate().take(end).skip(start) {
        let style = if i == cursor {
            Style::default().fg(Color::Black).bg(Color::Cyan)
        } else {
            Style::default().fg(Color::White)
        };
        lines.push(Line::from(Span::styled(row.clone(), style)));
    }

    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        "Up/Down to move, Enter to select, Esc to cancel",
        Style::default().fg(Color::DarkGray),
    )));

    let paragraph = Paragraph::new(lines)
        .block(
            Block::default()
                .title(" Profiles ")
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::Cyan)),
        )
        .style(Style::default().bg(Color::Black));

    frame.render_widget(paragraph, overlay_area);
}

/// Build (title, row-strings, cursor) for the picker stage.
fn build_profiles_rows(stage: &ProfilesPickerStage) -> (String, Vec<String>, usize) {
    match stage {
        ProfilesPickerStage::PickTarget { items, cursor } => {
            let title = "Choose target".to_string();
            let rows: Vec<String> = items
                .iter()
                .map(|item| {
                    let name = match &item.target {
                        ProfilesTarget::Default => "(default profile)".to_string(),
                        ProfilesTarget::Agent(n) => match &item.primary_agent {
                            // Mark the primary agent so the user knows which
                            // agent's row drives the active chat session.
                            Some(_) => format!("{} (chat)", n),
                            None => n.clone(),
                        },
                    };
                    format!("{:<24} → {}", name, item.current_profile)
                })
                .collect();
            (title, rows, *cursor)
        }
        ProfilesPickerStage::PickProfile {
            target,
            profiles,
            current_profile,
            cursor,
        } => {
            let target_label = match target {
                ProfilesTarget::Default => "default profile".to_string(),
                ProfilesTarget::Agent(n) => n.clone(),
            };
            let title = format!("Profile for {}", target_label);
            let rows: Vec<String> = profiles
                .iter()
                .map(|p| {
                    if p == current_profile {
                        format!("* {}", p)
                    } else {
                        format!("  {}", p)
                    }
                })
                .collect();
            (title, rows, *cursor)
        }
    }
}

/// Compute a (start, end) row window centered on the cursor.
fn visible_window(cursor: usize, total: usize, visible: usize) -> (usize, usize) {
    if visible == 0 || total == 0 {
        return (0, 0);
    }
    if total <= visible {
        return (0, total);
    }
    let half = visible / 2;
    let start = cursor.saturating_sub(half);
    let end = (start + visible).min(total);
    let start = end.saturating_sub(visible);
    (start, end)
}
