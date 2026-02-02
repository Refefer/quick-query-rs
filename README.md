# Quick-Query (`qq`)

A fast, extensible command-line interface for interacting with Large Language Models.

[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE)
[![Rust](https://img.shields.io/badge/rust-1.75%2B-orange.svg)](https://www.rust-lang.org)

## Features

- **One-shot completions** — Quick, single-prompt queries for scripts or ad-hoc use
- **Interactive chat** — Rich TUI with streaming, markdown rendering, and conversation history
- **Agentic workflows** — 8 built-in agents (coder, researcher, planner, etc.) with automatic tool use
- **Provider agnostic** — Works with OpenAI, Ollama, vLLM, Together, Groq, and any OpenAI-compatible API
- **Pure Rust** — No ncurses dependency, cross-platform, fast startup

## Quick Start

### Installation

```bash
# Clone and install
git clone https://github.com/andrew/quick-query-rs.git
cd quick-query-rs
cargo install --path .

# Verify installation
qq --version
```

### Configuration

Create `~/.config/qq/config.toml`:

```toml
default_profile = "default"

[providers.openai]
api_key = "sk-..."  # Or set OPENAI_API_KEY env var

[profiles.default]
provider = "openai"
model = "gpt-4o"
```

### Usage

```bash
# One-shot completion
qq -p "Explain async/await in Rust"

# Interactive chat
qq chat

# With specific agent
qq -A researcher -p "Best practices for error handling in Rust"

# With specific profile
qq -P coding chat
```

## Built-in Agents

| Agent | Purpose |
|-------|---------|
| **chat** | Interactive conversations and delegation |
| **explore** | Filesystem exploration and discovery |
| **researcher** | Web research and synthesis |
| **coder** | Code generation and modification |
| **reviewer** | Code review and analysis |
| **summarizer** | Content summarization |
| **planner** | Task decomposition and planning |
| **writer** | Documentation and content creation |

## CLI Reference

```
qq [OPTIONS] [COMMAND]

Options:
  -p, --prompt <PROMPT>      Prompt for quick completion
  -P, --profile <PROFILE>    Profile to use
  -m, --model <MODEL>        Model override
  -A, --agent <AGENT>        Primary agent
  -d, --debug                Enable debug output

Commands:
  chat       Interactive chat mode
  profiles   List configured profiles
  config     Show current configuration
```

See `qq --help` for full options.

## Documentation

| Document | Description |
|----------|-------------|
| [ARCHITECTURE.md](ARCHITECTURE.md) | System design, data flows, extension points |
| [CHANGELOG.md](CHANGELOG.md) | Version history and changes |
| [docs/PRD.md](docs/PRD.md) | Product requirements and roadmap |
| [examples/](examples/) | Configuration examples |

### Crate Documentation

| Crate | Description |
|-------|-------------|
| [qq-core](crates/qq-core/README.md) | Core types, traits, and infrastructure |
| [qq-providers](crates/qq-providers/README.md) | LLM provider implementations |
| [qq-tools](crates/qq-tools/README.md) | Built-in tools for agentic workflows |
| [qq-agents](crates/qq-agents/README.md) | Agent definitions and behaviors |
| [qq-cli](crates/qq-cli/README.md) | CLI binary and TUI |

## Configuration

### Profile-Centric Model

Configuration is organized around **profiles** that bundle provider, model, prompt, and parameters:

```toml
default_profile = "default"

[providers.openai]
api_key = "sk-..."
default_model = "gpt-4o"

[profiles.default]
provider = "openai"
model = "gpt-4o"

[profiles.coding]
provider = "openai"
model = "gpt-4o"
agent = "coder"

[profiles.coding.parameters]
temperature = 0.2
```

### Tool Configuration

```toml
[tools]
root = "$PWD"           # Filesystem sandbox root
allow_write = false     # Write operations disabled by default
enable_filesystem = true
enable_memory = true
enable_web = true
```

### Example Configurations

See the [examples/](examples/) directory:
- `config.basic.toml` — Minimal setup
- `config.full.toml` — All options
- `config.local-llm.toml` — Ollama/vLLM setup
- `config.multi-provider.toml` — Multiple providers

## Building from Source

### Prerequisites

- Rust 1.75+ (install via [rustup.rs](https://rustup.rs))
- C compiler (for bundled SQLite)

### Build

```bash
# Debug build
cargo build

# Release build
cargo build --release

# Run tests
cargo test --workspace

# Generate docs
cargo doc --workspace --no-deps --open
```

## Contributing

1. Fork the repository
2. Create a feature branch: `git checkout -b feat/your-feature`
3. Run tests: `cargo test --workspace`
4. Run lints: `cargo clippy -- -D warnings`
5. Format code: `cargo fmt`
6. Submit a pull request

## License

MIT License — see [LICENSE](LICENSE) for details.

## Troubleshooting

| Problem | Solution |
|---------|----------|
| `qq: command not found` | Add `~/.cargo/bin` to your PATH |
| `401 Unauthorized` | Check API key in config or `OPENAI_API_KEY` env var |
| Build fails with SQLite error | Ensure C compiler is installed |
| Garbled TUI output | Use a terminal with ANSI support, or try `--no-tui` |

For more issues, see [GitHub Issues](https://github.com/andrew/quick-query-rs/issues).
