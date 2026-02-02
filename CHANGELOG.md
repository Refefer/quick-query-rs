# Changelog

All notable changes to Quick-Query (`qq`) will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added
- Initial documentation suite (ARCHITECTURE.md, crate READMEs)

## [0.1.0] - 2026-02

### Added

#### CLI Interface
- One-shot completion mode (`qq -p "prompt"`) with streaming output
- Interactive chat mode (`qq chat`) with readline history
- TUI mode with ratatui for rich terminal interface
- Profile management (`qq profiles`) to list configured profiles
- Configuration display (`qq config`) for debugging
- Agent selection via `--agent` flag for specialized workflows

#### Chat Commands
- `/help` - Show help summary
- `/quit`, `/exit` - Exit chat session
- `/clear` - Clear conversation history
- `/history` - Show message count
- `/tools` - List available tools
- `/system <msg>` - Override system prompt

#### Configuration System
- TOML-based configuration (`~/.config/qq/config.toml`)
- Profile-centric model bundling provider, model, prompt, and parameters
- Named system prompts with profile references
- Provider-specific parameter overrides
- Environment variable support for API keys
- Layered resolution: TOML → environment → CLI flags

#### Providers (qq-providers)
- OpenAI provider with full streaming support
- OpenAI-compatible generic provider (Ollama, vLLM, Together, Groq, OpenRouter)
- Tool calling support with parallel execution
- Vision support detection

#### Tools (qq-tools)
- **Filesystem**: `read_file`, `write_file`, `list_files`, `search_files`
  - Sandboxed to configurable root directory
  - Write access disabled by default
- **Memory**: `memory_store`, `memory_get`, `memory_list`, `memory_delete`
  - Persistent SQLite-backed key-value storage
  - In-memory mode for testing
- **Web**: `fetch_webpage`, `web_search`
  - HTML to markdown extraction
  - Optional Perplexica integration for web search
- **Process Data**: `process_large_data`
  - Chunk and summarize large tool outputs

#### Agents (qq-agents)
- **chat**: Interactive coordinator with agent delegation
- **explore**: Filesystem exploration and file discovery
- **researcher**: Web research and information synthesis
- **coder**: Autonomous code generation and modification
- **reviewer**: Code review and security analysis
- **summarizer**: Content summarization with format adaptation
- **planner**: Task decomposition and implementation planning
- **writer**: Documentation and content creation

#### Agent Framework (qq-core)
- Agent-as-tool pattern for recursive delegation
- Stateful and stateless agent execution modes
- Inter-agent communication via channels
- Agent progress events for TUI reporting
- Configurable max iterations per agent
- Chunk processor for handling large tool outputs

#### TUI Features
- Real-time markdown rendering with syntax highlighting
- Streaming response display
- Agent progress panel showing:
  - Current iteration and max iterations
  - Thinking/reasoning content
  - Tool execution status
  - Token usage statistics
- Input area with multi-line support
- Status bar with model and profile info

#### Core Infrastructure (qq-core)
- Message model with roles, content, tool calls
- Provider trait for LLM abstraction
- Tool trait with JSON schema parameters
- ToolRegistry for tool management
- Parallel tool execution with `execute_tools_parallel`
- Async task manager for background work
- Streaming support via async streams

### Security
- Filesystem tools sandboxed to configured root directory
- Write operations disabled by default
- API keys via environment variables (never logged)
- No credential storage for web tools

---

[Unreleased]: https://github.com/andrew/quick-query-rs/compare/v0.1.0...HEAD
[0.1.0]: https://github.com/andrew/quick-query-rs/releases/tag/v0.1.0
