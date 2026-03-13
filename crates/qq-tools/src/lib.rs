//! qq-tools: Built-in tools for quick-query
//!
//! This crate provides the default tools available to LLM agents:
//! - Run: sandboxed shell command execution (replaces all filesystem/memory tools)
//! - Web: fetch and parse webpages
//! - Tasks: session-scoped task tracking

pub mod approval;
pub mod bash;
pub mod tasks;
pub mod web;

pub use approval::{create_approval_channel, ApprovalChannel, ApprovalRequest, ApprovalResponse};
pub use bash::{
    create_run_tools, RunTool, MountExternalTool, MountPoint, PermissionStore,
    RequestNetworkAccessTool, RequestSensitiveAccessTool, SandboxExecutor, SandboxMounts,
    SandboxPathPolicy,
};
pub use tasks::{create_task_tools, create_task_tools_arc, TaskStore};
pub use web::{create_web_tools, create_web_tools_arc, create_web_tools_with_search, WebSearchConfig};
