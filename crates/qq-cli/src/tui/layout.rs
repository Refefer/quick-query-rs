//! Layout management for the TUI.
//!
//! Provides a flexible pane-based layout system with dynamic sizing.

use std::collections::HashMap;

use ratatui::layout::{Constraint, Direction, Layout, Rect};

/// Identifier for each pane in the TUI layout.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PaneId {
    /// Main content/response area
    Content,
    /// Thinking/reasoning panel (collapsible)
    Thinking,
    /// Status bar showing tokens, profile, etc.
    Status,
    /// User input area
    Input,
}

/// Specification for pane sizing behavior.
#[derive(Debug, Clone, Copy)]
pub enum PaneSize {
    /// Fixed number of lines
    Fixed(u16),
    /// Takes all remaining space
    Fill,
    /// Dynamic sizing based on content, with min/max bounds
    Dynamic {
        min: u16,
        max: u16,
        /// Current content lines (used to compute actual size)
        content_lines: u16,
    },
    /// Percentage of total height
    Percentage(u16),
}

impl PaneSize {
    /// Convert to a ratatui Constraint
    fn to_constraint(self) -> Constraint {
        match self {
            PaneSize::Fixed(lines) => Constraint::Length(lines),
            PaneSize::Fill => Constraint::Min(5), // Minimum 5 lines for content
            PaneSize::Dynamic { min, max, content_lines } => {
                // Add 2 for borders
                let desired = content_lines.saturating_add(2);
                let clamped = desired.clamp(min, max);
                Constraint::Length(clamped)
            }
            PaneSize::Percentage(pct) => Constraint::Percentage(pct),
        }
    }
}

/// Specification for a single pane.
#[derive(Debug, Clone)]
pub struct PaneSpec {
    pub id: PaneId,
    pub visible: bool,
    pub size: PaneSize,
}

impl PaneSpec {
    pub fn new(id: PaneId, size: PaneSize) -> Self {
        Self {
            id,
            visible: true,
            size,
        }
    }

}

/// Layout configuration defining pane arrangement.
///
/// Panes are arranged top-to-bottom in the order they appear in the `panes` vector.
/// The new default order is: Content > Thinking > Status > Input
#[derive(Debug, Clone)]
pub struct LayoutConfig {
    panes: Vec<PaneSpec>,
}

impl Default for LayoutConfig {
    fn default() -> Self {
        Self::new()
    }
}

impl LayoutConfig {
    /// Create a new layout with default pane order: Content > Thinking > Status > Input
    pub fn new() -> Self {
        Self {
            panes: vec![
                PaneSpec::new(PaneId::Content, PaneSize::Fill),
                PaneSpec::new(PaneId::Thinking, PaneSize::Dynamic {
                    min: 8,
                    max: 10,
                    content_lines: 0,
                }),
                PaneSpec::new(PaneId::Status, PaneSize::Fixed(2)),
                PaneSpec::new(PaneId::Input, PaneSize::Dynamic {
                    min: 3,
                    max: 10,
                    content_lines: 1,
                }),
            ],
        }
    }

    /// Update the visibility and size of a specific pane.
    pub fn set_pane(&mut self, id: PaneId, visible: bool, size: PaneSize) {
        if let Some(pane) = self.panes.iter_mut().find(|p| p.id == id) {
            pane.visible = visible;
            pane.size = size;
        }
    }

    /// Update thinking pane for expanded/collapsed state.
    pub fn set_thinking(&mut self, visible: bool, expanded: bool, content_lines: u16) {
        let size = if !visible {
            PaneSize::Fixed(0)
        } else if expanded {
            PaneSize::Percentage(70)
        } else {
            PaneSize::Dynamic {
                min: 8,
                max: 10,
                content_lines,
            }
        };
        self.set_pane(PaneId::Thinking, visible, size);
    }

    /// Update input pane size based on content.
    pub fn set_input_lines(&mut self, content_lines: u16) {
        self.set_pane(
            PaneId::Input,
            true,
            PaneSize::Dynamic {
                min: 3,
                max: 10,
                content_lines,
            },
        );
    }

    /// Compute the actual rectangles for each visible pane.
    ///
    /// Returns a HashMap mapping PaneId to its computed Rect.
    /// Panes that are not visible will have Rect::default() (0,0,0,0).
    pub fn compute(&self, area: Rect) -> HashMap<PaneId, Rect> {
        let mut result = HashMap::new();

        // Initialize all panes with default rect
        for pane in &self.panes {
            result.insert(pane.id, Rect::default());
        }

        // Collect visible panes and their constraints
        let visible_panes: Vec<&PaneSpec> = self.panes.iter().filter(|p| p.visible).collect();

        if visible_panes.is_empty() {
            return result;
        }

        let constraints: Vec<Constraint> = visible_panes
            .iter()
            .map(|p| p.size.to_constraint())
            .collect();

        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints(constraints)
            .split(area);

        // Map computed chunks back to pane IDs
        for (pane, chunk) in visible_panes.iter().zip(chunks.iter()) {
            result.insert(pane.id, *chunk);
        }

        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_layout() {
        let config = LayoutConfig::new();
        let area = Rect::new(0, 0, 80, 40);
        let layout = config.compute(area);

        // All panes should be present
        assert!(layout.contains_key(&PaneId::Content));
        assert!(layout.contains_key(&PaneId::Thinking));
        assert!(layout.contains_key(&PaneId::Status));
        assert!(layout.contains_key(&PaneId::Input));
    }

    #[test]
    fn test_hidden_thinking() {
        let mut config = LayoutConfig::new();
        config.set_pane(PaneId::Thinking, false, PaneSize::Fixed(0));

        let area = Rect::new(0, 0, 80, 40);
        let layout = config.compute(area);

        // Thinking should have zero size
        let thinking = layout.get(&PaneId::Thinking).unwrap();
        assert_eq!(thinking.height, 0);
    }

    #[test]
    fn test_expanded_thinking() {
        let mut config = LayoutConfig::new();
        config.set_thinking(true, true, 20);

        let area = Rect::new(0, 0, 80, 40);
        let layout = config.compute(area);

        // Thinking should take ~70%
        let thinking = layout.get(&PaneId::Thinking).unwrap();
        assert!(thinking.height >= 25); // ~70% of 40
    }

    #[test]
    fn test_pane_order() {
        let config = LayoutConfig::new();
        let area = Rect::new(0, 0, 80, 40);
        let layout = config.compute(area);

        let content = layout.get(&PaneId::Content).unwrap();
        let thinking = layout.get(&PaneId::Thinking).unwrap();
        let status = layout.get(&PaneId::Status).unwrap();
        let input = layout.get(&PaneId::Input).unwrap();

        // Verify order: Content < Thinking < Status < Input (by y position)
        assert!(content.y < thinking.y, "Content should be above Thinking");
        assert!(thinking.y < status.y, "Thinking should be above Status");
        assert!(status.y < input.y, "Status should be above Input");
    }
}
