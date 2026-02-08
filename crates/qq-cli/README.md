# qq-cli

Command-line interface and TUI for Quick-Query.

This crate provides the `qq` binary with both one-shot completion and interactive chat modes.

## Overview

`qq-cli` is the user-facing application that combines all other `qq-*` crates:
- Loads configuration from TOML files
- Creates providers, tools, and agents
- Provides interactive TUI and legacy readline interfaces
- Handles streaming output and markdown rendering

## Installation

### From Source

```bash
# Clone and build
git clone https://github.com/andrew/quick-query-rs.git
cd quick-query-rs
cargo build --release

# Install globally
cargo install --path .

# Binary is now at ~/.cargo/bin/qq
```

### Prerequisites

- Rust 1.75+ (install via [rustup](https://rustup.rs))
- C compiler (for bundled SQLite)

## Quick Start

```bash
# One-shot completion
qq -p "Explain async/await in Rust"

# Interactive project management mode
qq manage

# With specific profile
qq -P coding -p "Review this function"

# With specific agent
qq -A researcher -p "Best practices for error handling"
```

## CLI Reference

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
      --log-level <LEVEL>    Log level (trace, debug, info, warn, error; default: warn)
  -d, --debug                Enable debug logging (shorthand for --log-level debug)
      --log-file <FILE>      Write debug log to file (JSON-lines format)
      --no-tools             Disable all tools
      --no-agents            Disable all agents
      --minimal              Minimal mode (no tools, no agents)
      --no-tui               Use legacy readline instead of TUI
  -A, --agent <AGENT>        Primary agent to use

Commands:
  manage     Interactive project management mode
  profiles   List configured profiles
  config     Show current configuration
```

## Modes

### One-Shot Completion

Quick, single-prompt usage for scripts or ad-hoc queries:

```bash
qq -p "What is the capital of France?"
qq -p "Summarize this: $(cat file.txt)"
qq -P coding -p "Review this code: $(cat main.rs)"
```

Features:
- Streaming output with markdown rendering
- Automatic tool execution
- Agentic loop for multi-step tasks

### Interactive Chat (TUI)

Rich terminal interface with:
- Real-time streaming display
- Agent progress panel
- Token usage statistics
- Multi-line input
- Conversation history

```bash
qq manage
qq manage --system "You are a Rust expert"
qq manage --agent coder
```

### Legacy Readline Mode

For environments without full terminal support:

```bash
qq manage --no-tui
```

## Chat Commands

| Command | Aliases | Description |
|---------|---------|-------------|
| `/help` | `/?` | Show help summary |
| `/quit` | `/exit`, `/q` | Exit chat |
| `/clear` | `/c` | Clear conversation history |
| `/reset` | | Clear conversation + all agent memory |
| `/history` | `/h` | Show message count |
| `/memory` | `/mem` | Show memory usage diagnostics |
| `/tools` | `/t` | List available tools |
| `/agents` | `/a` | List available agents |
| `/delegate` | `/d` | Delegate to agent: `/delegate <agent> <task>` |
| `/system` | `/sys` | Set system prompt |
| `/debug` | | Debug commands (messages, count, dump) |
| `@agent <task>` | | Quick agent invocation |

## Configuration

### Config File Location

```
~/.config/qq/config.toml
```

Or set `QQ_CONFIG_PATH` environment variable.

### Minimal Configuration

```toml
default_profile = "default"

[providers.openai]
api_key = "sk-..."  # Or use OPENAI_API_KEY env var

[profiles.default]
provider = "openai"
model = "gpt-4o"
```

### Full Configuration Example

```toml
default_profile = "default"

# Provider configurations
[providers.openai]
api_key = "sk-..."
base_url = "https://api.openai.com/v1"
default_model = "gpt-4o"

[providers.ollama]
api_key = "ollama"
base_url = "http://localhost:11434/v1"
default_model = "llama2"

# Named prompts
[prompts.coding]
prompt = """
You are an expert programmer. Write clean, efficient code.
Follow best practices and explain your decisions.
"""

# Profiles
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

[profiles.local]
provider = "ollama"
model = "codellama"

# Tool configuration
[tools]
root = "$PWD"
allow_write = false
enable_filesystem = true
enable_memory = true
enable_web = true

# Optional web search
[tools.web_search]
host = "http://localhost:3000"
chat_model = "gpt-4o"
embed_model = "text-embedding-ada-002"

# Chunker for large outputs
[tools.chunker]
enabled = true
threshold_bytes = 50000
chunk_size_bytes = 10000
max_chunks = 5
```

### Environment Variables

| Variable | Description |
|----------|-------------|
| `OPENAI_API_KEY` | API key (if not in config) |
| `QQ_CONFIG_PATH` | Custom config file path |
| `RUST_LOG` | Log level (info, debug, trace) |

## TUI Architecture

### Layout

```
┌─────────────────────────────────────┬──────────────────┐
│                                     │  Agent Progress  │
│        Message History              │  - Iteration 2/20│
│                                     │  - Thinking...   │
│                                     │  - [tool] read   │
│─────────────────────────────────────│                  │
│        Current Response             │  Token Usage     │
│        (streaming)                  │  - Prompt: 1234  │
│                                     │  - Completion: 56│
├─────────────────────────────────────┴──────────────────┤
│ > Your input here...                                   │
├────────────────────────────────────────────────────────┤
│ Profile: coding | Model: gpt-4o | Tokens: 1290        │
└────────────────────────────────────────────────────────┘
```

### Key Bindings

| Key | Action |
|-----|--------|
| `Enter` | Send message |
| `Shift+Enter` | New line (multi-line input) |
| `Ctrl+C` | Exit |
| `Ctrl+L` | Clear screen |
| `Up/Down` | Scroll history |

## Module Structure

```
qq-cli/
├── src/
│   ├── main.rs          # Entry point, CLI parsing
│   ├── chat.rs          # Chat session management and readline chat
│   ├── config.rs        # Configuration loading
│   ├── markdown.rs      # Markdown rendering
│   ├── agents/          # Agent execution
│   │   ├── mod.rs       # Agent executor
│   │   ├── agent_tool.rs    # Agent-as-tool wrappers, scoped memory
│   │   ├── continuation.rs  # Continuation/summarization for long runs
│   │   └── inform_user.rs   # Non-blocking user notification tool
│   ├── tui/             # TUI implementation
│   │   ├── mod.rs
│   │   ├── app.rs       # Main TUI app state and event loop
│   │   ├── ui.rs        # UI rendering
│   │   ├── layout.rs    # Layout calculations
│   │   ├── scroll.rs    # Scroll state management
│   │   ├── markdown.rs  # TUI markdown renderer
│   │   ├── events.rs    # Event processing
│   │   └── widgets/     # Custom TUI widgets
│   ├── event_bus.rs     # Agent event bus
│   ├── execution_context.rs  # Agent/tool stack tracking
│   └── debug_log.rs     # Debug logging
```

## Debugging

### Debug Mode

```bash
qq -d -p "prompt"                # Debug to stderr
qq --log-file debug.jsonl        # Debug to file (JSON-lines)
qq --log-level trace -p "prompt" # Maximum verbosity
```

### View Configuration

```bash
qq config    # Show resolved configuration
qq profiles  # List all profiles with details
```

### Minimal Mode

Test basic chat loop without tools or agents:

```bash
qq manage --minimal
```

## Dependencies

| Category | Crates |
|----------|--------|
| **Core** | `qq-core`, `qq-agents`, `qq-providers`, `qq-tools` |
| **CLI** | `clap` |
| **Config** | `figment`, `toml`, `dirs` |
| **TUI** | `ratatui`, `crossterm`, `tui-input` |
| **Terminal** | `termimad`, `rustyline` |
| **Async** | `tokio`, `futures`, `tokio-stream` |
| **Serialization** | `serde`, `serde_json` |

## Building

```bash
# Debug build
cargo build -p qq-cli

# Release build
cargo build -p qq-cli --release

# Run directly
cargo run -p qq-cli -- -p "Hello"

# Run tests
cargo test -p qq-cli
```
