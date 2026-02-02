//! Terminal User Interface module using ratatui.
//!
//! Provides a proper TUI with separate panels for thinking, content, and input.

pub mod app;
pub mod events;
pub mod layout;
pub mod markdown;
pub mod scroll;
pub mod ui;
pub mod widgets;

pub use app::run_tui;
