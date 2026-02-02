//! Terminal User Interface module using ratatui.
//!
//! Provides a proper TUI with separate panels for thinking, content, tools, and input.

pub mod app;
pub mod events;
pub mod markdown;
pub mod ui;
pub mod widgets;

pub use app::run_tui;
