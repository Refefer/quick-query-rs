//! Terminal User Interface module using ratatui.
//!
//! Provides a proper TUI with separate panels for thinking, content, tools, and input.

mod app;
mod events;
mod markdown;
mod ui;
mod widgets;

pub use app::run_tui;
