# qq-core

Core types, traits, and infrastructure for Quick-Query.

This crate provides the foundational abstractions used throughout the Quick-Query LLM CLI tool. It defines the core interfaces for providers, tools, agents, and messages.

## Overview

`qq-core` is a library crate that other `qq-*` crates depend on. It contains no provider implementations or tools itself—just the contracts and infrastructure they use.

## Key Types

### Messages

The message model represents LLM conversations:

| Type | Description |
|------|-------------|
| `Message` | A single message with role, content, and optional tool calls |
| `Role` | Message role: `System`, `User`, `Assistant`, or `Tool` |
| `Content` | Message content: `Text` or multi-part `Parts` |
| `ToolCall` | A tool invocation with ID, name, and arguments |
| `ToolResult` | Result from tool execution |
| `StreamChunk` | Streaming response chunk (delta, tool call, done, error) |

```rust
use qq_core::{Message, Role, Content};

// Create messages
let system = Message::system("You are a helpful assistant");
let user = Message::user("Hello!");
let assistant = Message::assistant("Hi there!");

// Message with tool calls
let msg = Message::assistant_with_tool_calls("", vec![tool_call]);

// Tool result
let result = Message::tool_result("call_123", "File contents here");

// Memory tracking
let bytes = msg.byte_count(); // Counts content + tool_calls + tool_call_id + reasoning_content
let observable = msg.observable_byte_count(); // Excludes reasoning_content (for compaction thresholds)
```

### Provider Trait

The `Provider` trait abstracts LLM API interactions:

```rust
use qq_core::{Provider, CompletionRequest, CompletionResponse, StreamResult};

#[async_trait]
pub trait Provider: Send + Sync {
    fn name(&self) -> &str;
    fn default_model(&self) -> Option<&str>;

    /// Non-streaming completion
    async fn complete(&self, request: CompletionRequest) -> Result<CompletionResponse, Error>;

    /// Streaming completion
    async fn stream(&self, request: CompletionRequest) -> Result<StreamResult, Error>;

}
```

### Tool Trait

Tools are executable capabilities exposed to LLMs:

```rust
use qq_core::{Tool, ToolDefinition, ToolParameters, PropertySchema, ToolOutput};

#[async_trait]
pub trait Tool: Send + Sync {
    fn name(&self) -> &str;
    fn description(&self) -> &str;
    fn definition(&self) -> ToolDefinition;

    async fn execute(&self, arguments: Value) -> Result<ToolOutput, Error>;
}
```

### Tool Registry

Manages tool registration and lookup:

```rust
use qq_core::ToolRegistry;

let mut registry = ToolRegistry::new();

// Register tools
registry.register(Arc::new(MyTool::new()));

// Get tool by name
if let Some(tool) = registry.get("my_tool") {
    let result = tool.execute(args).await?;
}

// Get all definitions for LLM
let definitions = registry.definitions();

// Create subset for specific agent
let subset = registry.subset_from_strs(&["read_file", "write_file"]);
```

### Agent Framework

Agents coordinate LLM calls and tool execution:

```rust
use qq_core::{Agent, AgentConfig, AgentId};

// Stateless one-shot execution
let result = Agent::run_once(
    provider,
    tools,
    AgentConfig::new("coder").with_system_prompt("You are a coding assistant"),
    vec![Message::user("Write a function that...")],
).await?;

// Stateful agent with history
let config = AgentConfig::new("chat")
    .with_system_prompt("You are helpful")
    .with_max_iterations(20)
    .stateful();

let mut agent = Agent::new_stateful(provider, tools, config);
let r1 = agent.process("Hello").await?;
let r2 = agent.process("Tell me more").await?;  // Has context from r1
```

### Agent Memory

Scoped memory for agent instances across invocations:

```rust
use qq_core::{AgentMemory, AgentInstanceState};

// Central store keyed by scope path (e.g., "pm/explore", "pm/coder/explore")
let memory = AgentMemory::new();

// Store messages for a scope
memory.store_messages("pm/explore", messages).await;

// Retrieve prior history
let history = memory.get_messages("pm/explore").await;

// Clear a scope (for new_instance: true)
memory.clear_scope("pm/explore").await;
```

Each scope has a 200KB budget (`DEFAULT_MAX_INSTANCE_BYTES`). When exceeded, `AgentInstanceState::trim_to_budget()` removes oldest messages at safe boundaries, preserving tool call/result pairs.

### Parallel Execution

Execute multiple tools or LLM calls concurrently:

```rust
use qq_core::{execute_tools_parallel, complete_parallel};

// Execute tools in parallel
let results = execute_tools_parallel(&registry, tool_calls).await;

// With chunking for large outputs
let results = execute_tools_parallel_with_chunker(
    &registry,
    tool_calls,
    Some(&chunk_processor),
    Some(original_prompt),
).await;
```

### Chunk Processor

Handles large tool outputs by summarizing chunks:

```rust
use qq_core::{ChunkProcessor, ChunkerConfig};

let config = ChunkerConfig {
    enabled: true,
    threshold_bytes: 50_000,
    chunk_size_bytes: 10_000,
    max_chunks: 5,
};

let processor = ChunkProcessor::new(provider, config);
let summarized = processor.process("very long content...", "user's original question").await?;
```

## Module Reference

| Module | Description |
|--------|-------------|
| `message` | Message types, roles, content, tool calls |
| `provider` | Provider trait, request/response types, streaming |
| `tool` | Tool trait, definitions, parameters, registry |
| `agent` | Agent framework, channels, registry, progress events, `AgentMemory`, `AgentInstanceState` |
| `task` | Task manager, parallel execution helpers |
| `chunker` | Large output processing |
| `error` | Error types |
| `blocking` | Blocking runtime helpers |

## Features

This crate has no optional features. All functionality is always available.

## Dependencies

- `tokio` - Async runtime
- `tokio-stream` - Stream utilities
- `futures` - Async stream traits
- `serde` / `serde_json` - Serialization
- `async-trait` - Async trait support
- `thiserror` - Error handling
- `tracing` - Logging

## Usage

Add to your `Cargo.toml`:

```toml
[dependencies]
qq-core = { path = "../qq-core" }
```

Or if published:

```toml
[dependencies]
qq-core = "0.1"
```
