//! TUI Widget components.

pub mod content_area;
pub mod input_area;
pub mod status_bar;
pub mod thinking_panel;
pub mod tool_bar;

pub use content_area::ContentArea;
pub use input_area::{InputArea, InputHistory};
pub use status_bar::StatusBar;
pub use thinking_panel::ThinkingPanel;
pub use tool_bar::{ToolBar, ToolCallInfo, ToolStatus};
