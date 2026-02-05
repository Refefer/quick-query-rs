//! qq-core: Core types and traits for quick-query
//!
//! This crate provides the foundational types and traits used throughout
//! the quick-query LLM CLI tool.

pub mod agent;
pub mod blocking;
pub mod chunker;
pub mod error;
pub mod message;
pub mod provider;
pub mod task;
pub mod tool;

pub use agent::{
    Agent, AgentChannel, AgentConfig, AgentId, AgentMessage, AgentProgressEvent,
    AgentProgressHandler, AgentRegistry, AgentRunResult, AgentSender,
};
pub use error::Error;
pub use message::{Content, ContentPart, Message, Role, StreamChunk, ToolCall, ToolResult, Usage, strip_thinking_tags};
pub use provider::{
    CompletionRequest, CompletionResponse, FinishReason, Provider, ProviderConfig, StreamResult,
};
pub use task::{
    complete_parallel, execute_tools_parallel, execute_tools_parallel_with_chunker,
    TaskHandle, TaskId, TaskInfo, TaskManager, TaskState, ToolExecutionResult,
};
pub use tool::{PropertySchema, Tool, ToolDefinition, ToolOutput, ToolParameters, ToolRegistry};
pub use chunker::{ChunkProcessor, ChunkerConfig};
pub use blocking::run_blocking;

pub type Result<T> = std::result::Result<T, Error>;
