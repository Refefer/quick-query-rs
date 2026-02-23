//! qq-tools: Built-in tools for quick-query
//!
//! This crate provides the default tools available to LLM agents:
//! - Filesystem: read, write, list, and search files
//! - Web: fetch and parse webpages
//! - Memory: persistent user preferences
//! - Process data: chunk and summarize large content

pub mod bash;
pub mod filesystem;
pub mod memory;
pub mod process_data;
pub mod tasks;
pub mod web;

pub use bash::{
    create_approval_channel, create_bash_tools, ApprovalChannel, ApprovalRequest,
    ApprovalResponse, BashTool, MountExternalTool, MountPoint, PermissionStore, SandboxExecutor,
    SandboxMounts,
};
pub use filesystem::{create_filesystem_tools, create_filesystem_tools_arc, FileSystemConfig};
pub use memory::{create_preference_tools, create_preference_tools_arc, MemoryStore};
pub use process_data::{create_process_data_tool, create_process_data_tool_arc, ProcessLargeDataTool};
pub use tasks::{create_task_tools, create_task_tools_arc, TaskStore};
pub use web::{create_web_tools, create_web_tools_arc, create_web_tools_with_search, WebSearchConfig};

