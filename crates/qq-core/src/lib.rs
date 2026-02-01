//! qq-core: Core types and traits for quick-query
//!
//! This crate provides the foundational types and traits used throughout
//! the quick-query LLM CLI tool.

pub mod agent;
pub mod error;
pub mod message;
pub mod provider;
pub mod task;
pub mod tool;

pub use agent::{
    Agent, AgentChannel, AgentConfig, AgentId, AgentMessage, AgentRegistry, AgentSender,
};
pub use error::Error;
pub use message::{Content, ContentPart, Message, Role, StreamChunk, ToolCall, ToolResult, Usage};
pub use provider::{
    CompletionRequest, CompletionResponse, FinishReason, Provider, ProviderConfig, StreamResult,
};
pub use task::{
    complete_parallel, execute_tools_parallel, TaskHandle, TaskId, TaskInfo, TaskManager,
    TaskState, ToolExecutionResult,
};
pub use tool::{PropertySchema, Tool, ToolDefinition, ToolOutput, ToolParameters, ToolRegistry};

pub type Result<T> = std::result::Result<T, Error>;
