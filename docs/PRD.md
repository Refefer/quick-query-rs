# Quick-Query (qq) - Product Requirements Document

**Version:** 0.1.0
**Last Updated:** February 2026

## Overview

Quick-Query (`qq`) is a fast, extensible command-line interface for interacting with Large Language Models. It provides both quick one-shot completions and interactive chat sessions with agentic tool-use capabilities.

### Vision

A minimal yet powerful CLI that makes LLM interactions feel native to the terminal workflow. Quick-Query prioritizes:

1. **Speed** - Fast startup, streaming responses, parallel tool execution
2. **Flexibility** - Works with any OpenAI-compatible API (local or cloud)
3. **Extensibility** - Modular architecture for tools, providers, and agents
4. **Developer Experience** - Intuitive CLI, sensible defaults, rich markdown output

## Architecture

### Crate Structure

```
quick-query-rs/
├── crates/
│   ├── qq-core/       # Core types, traits, and infrastructure
│   ├── qq-cli/        # Command-line interface and TUI
│   ├── qq-providers/  # LLM provider implementations
│   ├── qq-tools/      # Built-in tools for agentic workflows
│   └── qq-agents/     # Agent definitions and implementations
├── examples/          # Configuration examples
└── docs/              # Documentation
```

For detailed architecture information, see [ARCHITECTURE.md](../ARCHITECTURE.md).

### Core Components

#### qq-core
Foundation types and traits:
- **Messages**: `Message`, `Content`, `Role`, `ToolCall`, `ToolResult`
- **Provider**: `Provider` trait, `CompletionRequest`, `CompletionResponse`, streaming
- **Tools**: `Tool` trait, `ToolRegistry`, `ToolDefinition`, JSON schema parameters
- **Tasks**: `TaskManager`, `TaskHandle`, parallel execution helpers
- **Agents**: `Agent`, `AgentChannel`, `AgentRegistry`, inter-agent communication
- **Chunker**: `ChunkProcessor` for handling large tool outputs
- **Errors**: Typed error handling with `Error` enum

#### qq-cli
User-facing command-line interface:
- **Commands**: `chat`, `profiles`, `config`
- **TUI Mode**: Rich terminal interface with ratatui
- **Chat Mode**: Interactive REPL with readline, history, streaming markdown
- **Configuration**: TOML-based profiles, providers, prompts
- **Agent Executor**: Manual agent invocation and delegation
- **Event Bus**: Agent progress reporting for TUI

#### qq-providers
LLM provider implementations:
- **OpenAI**: Full support including streaming, tool calls, vision
- **OpenAI-Compatible**: Works with Ollama, Together, Groq, OpenRouter, vLLM, etc.

#### qq-tools
Built-in tools for agentic workflows:
- **Filesystem**: `read_file`, `write_file`, `list_files`, `search_files`
- **Memory**: `memory_store`, `memory_get`, `memory_list`, `memory_delete` (SQLite-backed)
- **Web**: `fetch_webpage`, `web_search` (with optional Perplexica integration)
- **Processing**: `process_large_data` for chunking and summarization

#### qq-agents
Agent definitions and implementations:
- **ChatAgent**: Interactive coordinator with delegation
- **ExploreAgent**: Filesystem exploration and discovery
- **ResearcherAgent**: Web research and synthesis (fast/in-depth modes)
- **CoderAgent**: Autonomous code generation
- **ReviewerAgent**: Code review with severity categorization
- **SummarizerAgent**: Content summarization with format adaptation
- **PlannerAgent**: Task decomposition and planning
- **WriterAgent**: Documentation and content creation

## Features

### Current (v0.1.x)

#### CLI Interface
- [x] One-shot completion mode (`qq -p "prompt"`)
- [x] Interactive chat mode (`qq chat`)
- [x] TUI mode with ratatui for rich terminal interface
- [x] Legacy readline mode (`--no-tui`)
- [x] Profile management (`qq profiles`)
- [x] Configuration display (`qq config`)
- [x] Streaming output with markdown rendering
- [x] Command history with readline
- [x] Agent selection (`--agent`)

#### Chat Commands
- [x] `/help` - Show help
- [x] `/quit`, `/exit` - Exit chat
- [x] `/clear` - Clear conversation
- [x] `/history` - Show message count
- [x] `/tools` - List available tools
- [x] `/system <msg>` - Set system prompt

#### Configuration
- [x] Profile-centric configuration model
- [x] Named system prompts
- [x] Provider-specific parameters
- [x] Environment variable support for API keys
- [x] Tool configuration (enable/disable, root directory, write permissions)
- [x] Layered resolution: TOML → env → CLI

#### Agentic Capabilities
- [x] Tool calling with automatic execution
- [x] Parallel tool execution
- [x] Iterative agentic loop (configurable max iterations)
- [x] Built-in filesystem, memory, and web tools
- [x] Chunk processor for large tool outputs
- [x] Agent-as-tool delegation pattern
- [x] Agent depth limiting to prevent infinite recursion

#### Built-in Agents (8 total)
- [x] **chat** - Interactive conversations and delegation
- [x] **explore** - Filesystem exploration
- [x] **researcher** - Web research (fast/in-depth modes)
- [x] **coder** - Code generation and modification
- [x] **reviewer** - Code review and analysis
- [x] **summarizer** - Content summarization
- [x] **planner** - Task decomposition
- [x] **writer** - Documentation creation

#### TUI Features
- [x] Real-time markdown rendering
- [x] Streaming response display
- [x] Agent progress panel with:
  - Current iteration and max iterations
  - Thinking/reasoning content
  - Tool execution status
  - Token usage statistics
- [x] Multi-line input support
- [x] Status bar with profile and model info

#### Infrastructure
- [x] Async task manager for background work
- [x] Agent framework with channels for communication
- [x] Stateful and stateless agent modes
- [x] Streaming support for agent-to-agent communication
- [x] Agent progress events (`AgentProgressHandler`)

### Planned (v0.2.x)

#### Enhanced Chat Experience
- [ ] Syntax highlighting for code blocks in TUI
- [ ] Conversation save/load
- [ ] Configurable key bindings
- [ ] Image/file attachments

#### Background Tasks
- [ ] `/spawn <prompt>` - Start background agent task
- [ ] `/tasks` - List running tasks
- [ ] `/cancel <id>` - Cancel a task
- [ ] Task status notifications

#### Additional Tools
- [ ] `run_command` - Execute shell commands (sandboxed)
- [ ] `edit_file` - Structured file editing with diffs
- [ ] `git_*` - Git operations

#### Provider Enhancements
- [ ] Anthropic Claude native provider
- [ ] Google Gemini provider
- [ ] Provider fallback chains
- [ ] Request retry with backoff

### Future (v0.3.x+)

#### MCP Integration
- [ ] Model Context Protocol server support
- [ ] External tool discovery
- [ ] Resource management

#### Advanced Features
- [ ] Conversation branching
- [ ] Response caching
- [ ] Token usage tracking and budgets
- [ ] Custom tool plugins (dynamic loading)

## Configuration

### Profile-Centric Model

Configuration is organized around profiles that bundle provider, model, prompt, and parameters:

```toml
default_profile = "default"

[providers.openai]
api_key = "sk-..."  # Or use OPENAI_API_KEY env var
base_url = "https://api.openai.com/v1"  # Optional
default_model = "gpt-4o"

[providers.openai.parameters]
# Extra parameters passed to API

[prompts.coding]
prompt = "You are an expert programmer..."

[profiles.default]
provider = "openai"
model = "gpt-4o"

[profiles.coding]
provider = "openai"
prompt = "coding"
model = "gpt-4o"
agent = "coder"

[profiles.coding.parameters]
temperature = 0.2

[tools]
root = "$PWD"
allow_write = false
enable_filesystem = true
enable_memory = true
enable_web = true
```

### CLI Options

```
qq [OPTIONS] [COMMAND]

Options:
  -p, --prompt <PROMPT>      Prompt for quick completion
  -P, --profile <PROFILE>    Profile to use
  -m, --model <MODEL>        Model override
  -s, --system <SYSTEM>      System prompt override
  -t, --temperature <TEMP>   Temperature (0.0-2.0)
      --max-tokens <N>       Max tokens to generate
      --base-url <URL>       API base URL override
      --provider <NAME>      Provider override
      --no-stream            Disable streaming
  -d, --debug                Enable debug output
  -A, --agent <AGENT>        Primary agent to use

Commands:
  chat       Interactive chat mode
  profiles   List configured profiles
  config     Show configuration
```

## Technical Details

### Streaming Architecture

1. Provider returns `StreamResult` (pinned async stream of `StreamChunk`)
2. Chunks include: `Start`, `Delta`, `ThinkingDelta`, `ToolCallStart`, `ToolCallDelta`, `Done`, `Error`
3. `MarkdownRenderer` accumulates content and re-renders with terminal formatting
4. Tool calls are collected during streaming and executed in parallel when complete

### Parallel Execution

```rust
// Execute multiple tools concurrently
let results = execute_tools_parallel(registry, tool_calls).await;

// Execute multiple LLM requests concurrently
let responses = complete_parallel(provider, requests).await;
```

### Agent Framework

```rust
// Stateless one-shot execution
let result = Agent::run_once(provider, tools, config, context).await?;

// Stateful agent with history
let mut agent = Agent::new_stateful(provider, tools, config);
let r1 = agent.process("First query").await?;
let r2 = agent.process("Follow-up").await?;  // Has context

// With progress reporting
let result = Agent::run_once_with_progress(
    provider, tools, config, context, Some(progress_handler)
).await?;
```

### Agent Progress Handler

Receives real-time updates during agent execution:

```rust
pub enum AgentProgressEvent {
    IterationStart { agent_name, iteration, max_iterations },
    ThinkingDelta { agent_name, content },
    ToolStart { agent_name, tool_name },
    ToolComplete { agent_name, tool_name, is_error },
    UsageUpdate { agent_name, usage },
    ByteCount { agent_name, input_bytes, output_bytes },
}

#[async_trait]
pub trait AgentProgressHandler: Send + Sync {
    async fn on_progress(&self, event: AgentProgressEvent);
}
```

## Dependencies

### Runtime
- `tokio` - Async runtime
- `reqwest` - HTTP client with streaming
- `rustyline` - Readline for interactive input
- `crossterm` - Terminal control
- `ratatui` - TUI framework
- `termimad` - Markdown rendering

### Serialization
- `serde` / `serde_json` - JSON serialization
- `toml` - Configuration parsing

### Storage
- `rusqlite` (bundled) - SQLite for memory persistence

## Performance Goals

- CLI startup: < 50ms
- Time to first token: Provider-dependent + < 100ms overhead
- Tool execution: Parallel by default
- Memory: < 50MB baseline for CLI

## Security Considerations

- API keys: Environment variables preferred, config file permissions warning
- Filesystem tools: Sandboxed to configured root, write disabled by default
- Web tools: No credential storage, user-agent identification
- Command execution: Not included by default, requires explicit enable
- Agent depth: Limited to prevent infinite recursion

## Compatibility

- **Rust**: 1.75+ (2021 edition)
- **OS**: Linux, macOS, Windows (WSL recommended)
- **Terminal**: Any terminal with ANSI support
- **Providers**: Any OpenAI-compatible API
