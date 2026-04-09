//! Status bar widget showing profile, activity, tokens, and streaming status.

use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph, Widget},
};

use crate::execution_context::ExecutionContext;
use crate::tui::app::StreamingState;

/// Status bar display state
pub struct StatusBar<'a> {
    profile: &'a str,
    /// Primary agent for this session (e.g., "pm", "explore")
    primary_agent: &'a str,
    prompt_tokens: u32,
    completion_tokens: u32,
    is_streaming: bool,
    streaming_state: StreamingState,
    status_message: Option<&'a str>,
    execution_context: Option<&'a ExecutionContext>,
    tool_iteration: u32,
    /// Agent progress: (agent_name, current_iteration, agent_chain)
    agent_progress: Option<(&'a str, u32, &'a [String])>,
    /// Agent byte counts: (input_bytes, output_bytes)
    agent_bytes: Option<(usize, usize)>,
    /// Session byte counts: (input_bytes, output_bytes)
    session_bytes: Option<(usize, usize)>,
    /// Whether to show a top border (hidden when thinking panel is visible, to avoid double line)
    show_top_border: bool,
}

impl<'a> StatusBar<'a> {
    pub fn new(profile: &'a str, primary_agent: &'a str) -> Self {
        Self {
            profile,
            primary_agent,
            prompt_tokens: 0,
            completion_tokens: 0,
            is_streaming: false,
            streaming_state: StreamingState::Idle,
            status_message: None,
            execution_context: None,
            tool_iteration: 0,
            agent_progress: None,
            agent_bytes: None,
            session_bytes: None,
            show_top_border: true,
        }
    }

    pub fn tokens(mut self, prompt: u32, completion: u32) -> Self {
        self.prompt_tokens = prompt;
        self.completion_tokens = completion;
        self
    }

    pub fn streaming(mut self, is_streaming: bool) -> Self {
        self.is_streaming = is_streaming;
        self
    }

    pub fn streaming_state(mut self, state: StreamingState) -> Self {
        self.streaming_state = state;
        self
    }

    pub fn status(mut self, message: &'a str) -> Self {
        self.status_message = Some(message);
        self
    }

    pub fn execution_context(mut self, context: &'a ExecutionContext) -> Self {
        self.execution_context = Some(context);
        self
    }

    pub fn iteration(mut self, iteration: u32) -> Self {
        self.tool_iteration = iteration;
        self
    }

    pub fn agent_progress(mut self, progress: Option<(&'a str, u32, &'a [String])>) -> Self {
        self.agent_progress = progress;
        self
    }

    pub fn agent_bytes(mut self, input_bytes: usize, output_bytes: usize) -> Self {
        if input_bytes > 0 || output_bytes > 0 {
            self.agent_bytes = Some((input_bytes, output_bytes));
        }
        self
    }

    pub fn session_bytes(mut self, input_bytes: usize, output_bytes: usize) -> Self {
        // Always set session bytes so the counter is always visible
        self.session_bytes = Some((input_bytes, output_bytes));
        self
    }

    pub fn show_top_border(mut self, show: bool) -> Self {
        self.show_top_border = show;
        self
    }
}

impl Widget for StatusBar<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let style_dim = Style::default().fg(Color::DarkGray);
        let style_profile = Style::default().fg(Color::Cyan);
        let style_activity = Style::default().fg(Color::Yellow);
        let style_tokens = Style::default().fg(Color::White);
        let style_streaming = Style::default()
            .fg(Color::Green)
            .add_modifier(Modifier::BOLD);
        let style_waiting = Style::default()
            .fg(Color::Yellow)
            .add_modifier(Modifier::BOLD);

        let mut spans = Vec::new();

        // Format primary agent name with proper capitalization
        let agent_display = capitalize_first(self.primary_agent);

        // Always show "Profile > Agent[X]" first
        spans.push(Span::styled(" ", style_dim));
        spans.push(Span::styled(self.profile, style_profile));
        spans.push(Span::styled(" > ", style_dim));
        spans.push(Span::styled(
            format!("Agent[{}]", agent_display),
            Style::default().fg(Color::Magenta),
        ));

        // Show nested agent progress if a sub-agent is running
        if let Some((agent_name, iteration, agent_chain)) = self.agent_progress {
            let style_agent = Style::default().fg(Color::Magenta);

            // Use the scope-derived agent chain from the event (correct for parallel agents).
            // Skip the first element since it's the primary agent already shown above.
            let nested = &agent_chain[1.min(agent_chain.len())..];

            if nested.is_empty() {
                // No chain or only primary agent — show leaf from event
                spans.push(Span::styled(" > ", style_dim));
                spans.push(Span::styled(
                    format!(
                        "Agent[{}] turn {}",
                        capitalize_first(agent_name),
                        iteration,
                    ),
                    style_agent,
                ));
            } else {
                let collapsed = collapse_chain(nested);
                let prefix_width: usize = spans.iter().map(|s| s.content.len()).sum();
                let right_reserve = estimate_right_width(
                    self.streaming_state,
                    self.session_bytes,
                    self.agent_bytes,
                    self.prompt_tokens + self.completion_tokens,
                );
                let total_width = area.width as usize;
                let budget = total_width.saturating_sub(prefix_width + right_reserve);

                let chain_spans = build_chain_spans(
                    &collapsed,
                    iteration,
                    budget,
                    style_dim,
                    style_agent,
                );
                spans.extend(chain_spans);
            }

            // Show byte counts for agent
            if let Some((input_bytes, output_bytes)) = self.agent_bytes {
                spans.push(Span::styled(
                    format!(" {}", format_bytes(input_bytes + output_bytes)),
                    style_dim,
                ));
            }
        } else {
            // No sub-agent running - show activity from execution context or status message
            let activity = self.get_activity();
            if let Some(activity) = activity {
                spans.push(Span::styled(" › ", style_dim));
                spans.push(Span::styled(activity, style_activity));
            }

            // Show iteration count if in a tool loop
            if self.tool_iteration > 1 {
                spans.push(Span::styled(format!(" (turn {})", self.tool_iteration), style_dim));
            }
        }

        // Streaming state indicator
        match self.streaming_state {
            StreamingState::Idle => {
                spans.push(Span::styled(" ", style_dim));
                spans.push(Span::styled(
                    "Waiting for input...",
                    Style::default().fg(Color::DarkGray).add_modifier(Modifier::ITALIC),
                ));
            }
            StreamingState::Asking => {
                spans.push(Span::styled(" ", style_dim));
                spans.push(Span::styled("Asking...", style_waiting));
            }
            StreamingState::Thinking => {
                spans.push(Span::styled(" ", style_dim));
                spans.push(Span::styled("Thinking...", style_waiting));
            }
            StreamingState::Listening => {
                spans.push(Span::styled(" ", style_dim));
                spans.push(Span::styled("Listening...", style_streaming));
            }
        }

        // Build right side content: session bytes and/or tokens
        let mut right_content = Vec::new();

        // Session bytes
        if let Some((input_bytes, output_bytes)) = self.session_bytes {
            let total = input_bytes + output_bytes;
            right_content.push(Span::styled(format_bytes(total), style_tokens));
            right_content.push(Span::styled(
                format!(" ({}↑/{}↓)", format_bytes(input_bytes), format_bytes(output_bytes)),
                style_dim,
            ));
        }

        // Tokens (if we have them)
        let total_tokens = self.prompt_tokens + self.completion_tokens;
        if total_tokens > 0 {
            if !right_content.is_empty() {
                right_content.push(Span::styled(" | ", style_dim));
            }
            right_content.push(Span::styled(format!("{}t", total_tokens), style_tokens));
        }

        // Add right-aligned content
        if !right_content.is_empty() {
            let current_len: usize = spans.iter().map(|s| s.content.len()).sum();
            let right_len: usize = right_content.iter().map(|s| s.content.len()).sum();
            let available = area.width as usize;

            if current_len + right_len + 2 < available {
                let padding = available - current_len - right_len - 2;
                spans.push(Span::styled(" ".repeat(padding), style_dim));
            } else {
                spans.push(Span::styled(" | ", style_dim));
            }

            spans.extend(right_content);
            spans.push(Span::styled(" ", style_dim));
        }

        let borders = if self.show_top_border {
            Borders::TOP
        } else {
            Borders::NONE
        };
        let paragraph = Paragraph::new(Line::from(spans))
            .block(
                Block::default()
                    .borders(borders)
                    .border_style(Style::default().fg(Color::DarkGray)),
            );

        paragraph.render(area, buf);
    }
}

/// Format byte count with Kb/Mb suffixes for readability
fn format_bytes(bytes: usize) -> String {
    if bytes >= 1_000_000 {
        format!("{:.1}Mb", bytes as f64 / 1_000_000.0)
    } else if bytes >= 1_000 {
        format!("{:.1}Kb", bytes as f64 / 1_000.0)
    } else {
        format!("{}b", bytes)
    }
}

/// Capitalize the first letter of a string
fn capitalize_first(s: &str) -> String {
    let mut chars = s.chars();
    match chars.next() {
        None => String::new(),
        Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
    }
}

impl StatusBar<'_> {
    /// Get the current activity description
    fn get_activity(&self) -> Option<String> {
        // First check execution context for the current activity
        if let Some(ctx) = self.execution_context {
            if let Some(activity) = ctx.current_activity_blocking() {
                return Some(activity);
            }
        }

        // Fall back to status message (e.g., "Tool: list_files")
        if let Some(msg) = self.status_message {
            // Clean up common prefixes
            let cleaned = msg
                .strip_prefix("Tool: ")
                .or_else(|| msg.strip_prefix("Running: "))
                .or_else(|| msg.strip_prefix("Agent tool: "))
                .unwrap_or(msg);
            return Some(cleaned.to_string());
        }

        None
    }
}

/// Collapse consecutive duplicate agent names into (name, count) pairs.
///
/// e.g., `["researcher", "researcher", "explore", "explore", "explore"]`
/// becomes `[("researcher", 2), ("explore", 3)]`
fn collapse_chain(chain: &[String]) -> Vec<(&str, usize)> {
    let mut collapsed: Vec<(&str, usize)> = Vec::new();
    for name in chain {
        if let Some(last) = collapsed.last_mut() {
            if last.0 == name.as_str() {
                last.1 += 1;
                continue;
            }
        }
        collapsed.push((name.as_str(), 1));
    }
    collapsed
}

/// Format a single agent chain segment: `"Agent[Name]"` or `"Agent[Name] ×N"`.
fn format_agent_segment(name: &str, count: usize) -> String {
    let cap = capitalize_first(name);
    if count > 1 {
        format!("Agent[{}] \u{00d7}{}", cap, count)
    } else {
        format!("Agent[{}]", cap)
    }
}

/// Format the leaf (last) agent segment with turn info.
fn format_leaf_segment(name: &str, count: usize, iteration: u32) -> String {
    let cap = capitalize_first(name);
    if count > 1 {
        format!(
            "Agent[{}] \u{00d7}{} turn {}",
            cap, count, iteration
        )
    } else {
        format!("Agent[{}] turn {}", cap, iteration)
    }
}

/// Estimate the character width of the right-side status bar content
/// (streaming state + agent bytes + session bytes + tokens).
fn estimate_right_width(
    streaming_state: StreamingState,
    session_bytes: Option<(usize, usize)>,
    agent_bytes: Option<(usize, usize)>,
    total_tokens: u32,
) -> usize {
    let mut width = 0;

    // Agent bytes: " 1.2Mb"
    if let Some((input, output)) = agent_bytes {
        width += 1 + format_bytes(input + output).len();
    }

    // Streaming state indicator: " Asking..." etc.
    width += 1 + match streaming_state {
        StreamingState::Idle => "Waiting for input...".len(),
        StreamingState::Asking => "Asking...".len(),
        StreamingState::Thinking => "Thinking...".len(),
        StreamingState::Listening => "Listening...".len(),
    };

    // Right-aligned content (session bytes + tokens)
    if let Some((input, output)) = session_bytes {
        let total = input + output;
        width += format_bytes(total).len();
        width += format!(
            " ({}\u{2191}/{}\u{2193})",
            format_bytes(input),
            format_bytes(output)
        )
        .len();
        width += 2; // padding minimum
    }

    if total_tokens > 0 {
        if session_bytes.is_some() {
            width += 3; // " | "
        }
        width += format!("{}t", total_tokens).len();
    }

    // Trailing space
    width += 1;

    width
}

/// Build spans for the agent chain, collapsing duplicates and truncating with
/// ellipsis if the chain exceeds the available width budget.
fn build_chain_spans(
    collapsed: &[(&str, usize)],
    iteration: u32,
    budget: usize,
    style_dim: Style,
    style_agent: Style,
) -> Vec<Span<'static>> {
    if collapsed.is_empty() {
        return vec![];
    }

    let separator = " > ";
    let sep_len = separator.len();

    // Build all segment strings
    let mut total_len = 0;
    let mut segments: Vec<String> = Vec::with_capacity(collapsed.len());
    for (i, &(name, count)) in collapsed.iter().enumerate() {
        let seg = if i == collapsed.len() - 1 {
            format_leaf_segment(name, count, iteration)
        } else {
            format_agent_segment(name, count)
        };
        total_len += sep_len + seg.len();
        segments.push(seg);
    }

    // Full chain fits
    if total_len <= budget {
        let mut spans = Vec::new();
        for seg in segments {
            spans.push(Span::styled(separator.to_string(), style_dim));
            spans.push(Span::styled(seg, style_agent));
        }
        return spans;
    }

    // Ellipsis truncation: show first + … + last
    if collapsed.len() > 2 {
        let first_seg = &segments[0];
        let last_seg = &segments[segments.len() - 1];
        let ellipsis = "\u{2026}";
        let ellipsis_len =
            sep_len + first_seg.len() + sep_len + ellipsis.len() + sep_len + last_seg.len();

        if ellipsis_len <= budget {
            return vec![
                Span::styled(separator.to_string(), style_dim),
                Span::styled(first_seg.clone(), style_agent),
                Span::styled(separator.to_string(), style_dim),
                Span::styled(ellipsis.to_string(), style_dim),
                Span::styled(separator.to_string(), style_dim),
                Span::styled(last_seg.clone(), style_agent),
            ];
        }
    }

    // Last resort: just show the leaf agent
    let last_seg = &segments[segments.len() - 1];
    vec![
        Span::styled(separator.to_string(), style_dim),
        Span::styled("\u{2026}".to_string(), style_dim),
        Span::styled(separator.to_string(), style_dim),
        Span::styled(last_seg.clone(), style_agent),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn collapse_chain_no_duplicates() {
        let chain: Vec<String> = vec!["pm".into(), "researcher".into(), "explore".into()];
        let collapsed = collapse_chain(&chain);
        assert_eq!(
            collapsed,
            vec![("pm", 1), ("researcher", 1), ("explore", 1)]
        );
    }

    #[test]
    fn collapse_chain_consecutive_duplicates() {
        let chain: Vec<String> = vec![
            "researcher".into(),
            "researcher".into(),
            "explore".into(),
            "explore".into(),
            "explore".into(),
        ];
        let collapsed = collapse_chain(&chain);
        assert_eq!(collapsed, vec![("researcher", 2), ("explore", 3)]);
    }

    #[test]
    fn collapse_chain_all_same() {
        let chain: Vec<String> =
            vec!["explore".into(), "explore".into(), "explore".into(), "explore".into()];
        let collapsed = collapse_chain(&chain);
        assert_eq!(collapsed, vec![("explore", 4)]);
    }

    #[test]
    fn collapse_chain_single() {
        let chain: Vec<String> = vec!["explore".into()];
        let collapsed = collapse_chain(&chain);
        assert_eq!(collapsed, vec![("explore", 1)]);
    }

    #[test]
    fn collapse_chain_empty() {
        let chain: Vec<String> = vec![];
        let collapsed = collapse_chain(&chain);
        assert!(collapsed.is_empty());
    }

    #[test]
    fn collapse_chain_non_consecutive_duplicates() {
        let chain: Vec<String> = vec!["explore".into(), "researcher".into(), "explore".into()];
        let collapsed = collapse_chain(&chain);
        assert_eq!(
            collapsed,
            vec![("explore", 1), ("researcher", 1), ("explore", 1)]
        );
    }

    #[test]
    fn format_segment_no_repeat() {
        assert_eq!(format_agent_segment("explore", 1), "Agent[Explore]");
    }

    #[test]
    fn format_segment_with_repeat() {
        assert_eq!(
            format_agent_segment("explore", 3),
            "Agent[Explore] \u{00d7}3"
        );
    }

    #[test]
    fn format_leaf_no_repeat() {
        assert_eq!(
            format_leaf_segment("explore", 1, 26),
            "Agent[Explore] turn 26"
        );
    }

    #[test]
    fn format_leaf_with_repeat() {
        assert_eq!(
            format_leaf_segment("explore", 3, 26),
            "Agent[Explore] \u{00d7}3 turn 26"
        );
    }

    #[test]
    fn build_spans_fits_in_budget() {
        let collapsed = vec![("researcher", 2), ("explore", 3)];
        let style = Style::default();
        let spans = build_chain_spans(&collapsed, 26, 200, style, style);
        let text: String = spans.iter().map(|s| s.content.to_string()).collect();
        assert!(text.contains("Researcher"));
        assert!(text.contains("\u{00d7}2"));
        assert!(text.contains("Explore"));
        assert!(text.contains("\u{00d7}3"));
        assert!(text.contains("turn 26"));
    }

    #[test]
    fn build_spans_ellipsis_when_tight() {
        let collapsed = vec![("researcher", 2), ("coder", 1), ("explore", 3)];
        let style = Style::default();
        // Budget too small for all three but enough for ellipsis form
        let spans = build_chain_spans(&collapsed, 26, 65, style, style);
        let text: String = spans.iter().map(|s| s.content.to_string()).collect();
        assert!(text.contains("\u{2026}")); // ellipsis
        assert!(text.contains("Researcher"));
        assert!(text.contains("Explore"));
        assert!(!text.contains("Coder")); // middle element dropped
    }

    #[test]
    fn build_spans_last_resort() {
        let collapsed = vec![("researcher", 2), ("coder", 1), ("explore", 3)];
        let style = Style::default();
        // Very tight budget — only leaf should show
        let spans = build_chain_spans(&collapsed, 26, 40, style, style);
        let text: String = spans.iter().map(|s| s.content.to_string()).collect();
        assert!(text.contains("\u{2026}"));
        assert!(text.contains("Explore"));
        assert!(text.contains("turn 26"));
    }

    #[test]
    fn build_spans_empty() {
        let collapsed: Vec<(&str, usize)> = vec![];
        let style = Style::default();
        let spans = build_chain_spans(&collapsed, 26, 200, style, style);
        assert!(spans.is_empty());
    }
}
