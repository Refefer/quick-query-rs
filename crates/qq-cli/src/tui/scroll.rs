//! Scroll state management for the TUI.
//!
//! Provides viewport-aware scroll handling that automatically adjusts
//! the scroll offset when the viewport size changes (e.g., when toggling
//! the thinking panel).

/// Scroll state with viewport-aware offset management.
///
/// Key feature: when the viewport shrinks (e.g., thinking panel expands),
/// the scroll offset is automatically clamped to remain valid, preventing
/// the "scroll breaks after toggle" bug.
#[derive(Debug, Clone)]
pub struct ScrollState {
    /// Current scroll offset (lines from top)
    offset: u16,
    /// Total content height in lines
    content_height: u16,
    /// Visible viewport height in lines
    viewport_height: u16,
    /// Whether to automatically scroll to bottom on new content
    auto_scroll: bool,
}

impl Default for ScrollState {
    fn default() -> Self {
        Self {
            offset: 0,
            content_height: 0,
            viewport_height: 0,
            auto_scroll: true,
        }
    }
}

#[allow(dead_code)] // Some methods used only in tests or for future use
impl ScrollState {
    /// Create new scroll state with initial viewport height.
    pub fn new(viewport_height: u16) -> Self {
        Self {
            offset: 0,
            content_height: 0,
            viewport_height,
            auto_scroll: true,
        }
    }

    /// Get the current scroll offset.
    pub fn offset(&self) -> u16 {
        self.offset
    }

    /// Get the viewport height.
    pub fn viewport_height(&self) -> u16 {
        self.viewport_height
    }

    /// Check if auto-scroll is enabled.
    pub fn is_auto_scroll(&self) -> bool {
        self.auto_scroll
    }

    /// Get the maximum valid scroll offset.
    fn max_offset(&self) -> u16 {
        self.content_height.saturating_sub(self.viewport_height)
    }

    /// Clamp the offset to valid range.
    fn clamp_offset(&mut self) {
        let max = self.max_offset();
        if self.offset > max {
            self.offset = max;
        }
    }

    /// Get the effective scroll offset for rendering.
    ///
    /// If auto_scroll is enabled and content exceeds viewport,
    /// returns offset to show the bottom of content.
    pub fn effective_offset(&self) -> u16 {
        if self.auto_scroll {
            self.max_offset()
        } else {
            self.offset.min(self.max_offset())
        }
    }

    /// Update viewport height.
    ///
    /// **Key fix:** This automatically adjusts the scroll offset if it would
    /// become invalid (i.e., if the viewport grew larger than remaining content).
    /// This prevents the bug where toggling the thinking panel causes scroll issues.
    pub fn set_viewport_height(&mut self, height: u16) {
        self.viewport_height = height;
        // Clamp offset to remain valid with new viewport size
        self.clamp_offset();
    }

    /// Update content height.
    ///
    /// If auto_scroll is enabled, will scroll to show new content at bottom.
    pub fn set_content_height(&mut self, height: u16) {
        self.content_height = height;
        if self.auto_scroll {
            self.offset = self.max_offset();
        } else {
            // Ensure offset is still valid
            self.clamp_offset();
        }
    }

    /// Scroll up by the given amount.
    ///
    /// Disables auto_scroll since user is manually scrolling.
    pub fn scroll_up(&mut self, amount: u16) {
        self.offset = self.offset.saturating_sub(amount);
        self.auto_scroll = false;
    }

    /// Scroll down by the given amount.
    ///
    /// Re-enables auto_scroll if we reach the bottom.
    pub fn scroll_down(&mut self, amount: u16) {
        let max = self.max_offset();
        self.offset = (self.offset + amount).min(max);

        // Re-enable auto-scroll when reaching the bottom
        self.auto_scroll = self.offset >= max && max > 0;
    }

    /// Page up (scroll by half viewport).
    pub fn page_up(&mut self) {
        let amount = (self.viewport_height / 2).max(5);
        self.scroll_up(amount);
    }

    /// Page down (scroll by half viewport).
    pub fn page_down(&mut self) {
        let amount = (self.viewport_height / 2).max(5);
        self.scroll_down(amount);
    }

    /// Scroll to the top of content.
    pub fn scroll_to_top(&mut self) {
        self.offset = 0;
        self.auto_scroll = false;
    }

    /// Scroll to the bottom of content.
    ///
    /// Enables auto_scroll to follow new content.
    pub fn scroll_to_bottom(&mut self) {
        self.offset = self.max_offset();
        self.auto_scroll = true;
    }

    /// Enable auto-scroll without changing offset.
    pub fn enable_auto_scroll(&mut self) {
        self.auto_scroll = true;
    }

    /// Disable auto-scroll without changing offset.
    pub fn disable_auto_scroll(&mut self) {
        self.auto_scroll = false;
    }

    /// Check if content is scrollable (content exceeds viewport).
    pub fn is_scrollable(&self) -> bool {
        self.content_height > self.viewport_height
    }

    /// Get scroll percentage (0-100).
    pub fn scroll_percentage(&self) -> u16 {
        let max = self.max_offset();
        if max == 0 {
            100
        } else {
            ((self.effective_offset() as f32 / max as f32) * 100.0) as u16
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_initial_state() {
        let state = ScrollState::new(20);
        assert_eq!(state.offset(), 0);
        assert_eq!(state.viewport_height(), 20);
        assert!(state.is_auto_scroll());
    }

    #[test]
    fn test_auto_scroll_follows_content() {
        let mut state = ScrollState::new(20);
        state.set_content_height(50);

        // Auto-scroll should position at bottom
        assert_eq!(state.effective_offset(), 30); // 50 - 20
    }

    #[test]
    fn test_manual_scroll_disables_auto() {
        let mut state = ScrollState::new(20);
        state.set_content_height(50);
        state.scroll_up(10);

        assert!(!state.is_auto_scroll());
        assert_eq!(state.offset(), 20); // Was 30, scrolled up 10
    }

    #[test]
    fn test_viewport_shrink_clamps_offset() {
        let mut state = ScrollState::new(20);
        state.set_content_height(50);
        state.scroll_up(5); // offset = 25, auto_scroll = false

        // Viewport shrinks (thinking panel expanded)
        state.set_viewport_height(10);

        // Max offset is now 40 (50-10), our offset 25 is still valid
        assert_eq!(state.offset(), 25);

        // Now if offset was higher than new max, it should clamp
        let mut state2 = ScrollState::new(20);
        state2.set_content_height(50);
        state2.auto_scroll = false;
        state2.offset = 35; // Set manually

        state2.set_viewport_height(30); // max_offset = 20
        assert_eq!(state2.offset(), 20); // Clamped to max
    }

    #[test]
    fn test_scroll_to_bottom_enables_auto() {
        let mut state = ScrollState::new(20);
        state.set_content_height(50);
        state.scroll_up(10);
        assert!(!state.is_auto_scroll());

        state.scroll_to_bottom();
        assert!(state.is_auto_scroll());
        assert_eq!(state.offset(), 30);
    }

    #[test]
    fn test_page_navigation() {
        let mut state = ScrollState::new(20);
        state.set_content_height(100);

        // Start from top (disable auto_scroll which positions at bottom)
        state.scroll_to_top();
        assert_eq!(state.offset(), 0);

        state.page_down();
        assert_eq!(state.offset(), 10); // Half of 20

        state.page_up();
        assert_eq!(state.offset(), 0);
    }

    #[test]
    fn test_scroll_percentage() {
        let mut state = ScrollState::new(20);
        state.set_content_height(120); // max_offset = 100

        state.scroll_to_top();
        assert_eq!(state.scroll_percentage(), 0);

        state.scroll_to_bottom();
        assert_eq!(state.scroll_percentage(), 100);

        state.auto_scroll = false;
        state.offset = 50;
        assert_eq!(state.scroll_percentage(), 50);
    }
}
