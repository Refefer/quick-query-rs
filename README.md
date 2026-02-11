# Quick-Query (`qq`)

A fast, extensible command-line interface for interacting with Large Language Models.

[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE)
[![Rust](https://img.shields.io/badge/rust-1.75%2B-orange.svg)](https://www.rust-lang.org)

## Features

- **One-shot completions** — Quick, single-prompt queries for scripts or ad-hoc use
- **Interactive chat** — Rich TUI with streaming, markdown rendering, and conversation history
- **Agentic workflows** — 8 built-in agents (coder, researcher, planner, etc.) with automatic tool use
- **Multi-provider** — Native support for OpenAI, Anthropic Claude, and Google Gemini, plus any OpenAI-compatible API (Ollama, vLLM, Groq, etc.)
- **Pure Rust** — No ncurses dependency, cross-platform, fast startup

## Quick Start

### Installation

```bash
# Clone and install
git clone https://github.com/andrew/quick-query-rs.git
cd quick-query-rs
cargo install --path crates/qq-cli

# Verify installation
qq --version
```

### Configuration

Create `~/.config/qq/config.toml`:

```toml
default_profile = "default"

# OpenAI
[providers.openai]
api_key = "sk-..."  # Or set OPENAI_API_KEY env var

# Anthropic (native)
[providers.anthropic]
api_key = "sk-ant-..."  # Or set ANTHROPIC_API_KEY env var

# Google Gemini (native)
[providers.gemini]
api_key = "AIza..."  # Or set GEMINI_API_KEY env var

[profiles.default]
provider = "openai"
model = "gpt-4o"
```

### Usage

```bash
# One-shot completion
qq -p "Explain async/await in Rust"

# Interactive project management mode
qq manage

# With specific agent
qq -A researcher -p "Best practices for error handling in Rust"

# With specific profile
qq -P coding manage
```

## Built-in Agents

| Agent | Purpose |
|-------|---------|
| **pm** | Project manager: coordinates agents, tracks tasks, ensures delivery |
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
      --provider <PROVIDER>  Provider override
      --base-url <URL>       Base URL for API
  -s, --system <SYSTEM>      System prompt override
  -t, --temperature <TEMP>   Temperature (0.0-2.0)
      --max-tokens <N>       Maximum tokens to generate
  -A, --agent <AGENT>        Primary agent
      --log-level <LEVEL>    Log level (trace, debug, info, warn, error)
  -d, --debug                Enable debug logging (shorthand for --log-level debug)
      --log-file <FILE>      Write debug log to file (JSON-lines format)
      --no-stream            Disable streaming output
      --no-tui               Disable TUI, use readline
      --classic              Use built-in search tools instead of bash (no bash tools)
      --insecure             Allow bash tools without kernel sandbox isolation
      --no-tools             Disable all tools
      --no-agents            Disable all agents
      --minimal              No tools, no agents

Commands:
  manage     Interactive project management mode
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
allow_write = true      # Write operations enabled by default
enable_filesystem = true
enable_memory = true
enable_web = true
```

### Bash Sandbox

Agents have access to a sandboxed bash tool that runs commands inside a
kernel-level container (Linux user/mount/PID namespaces via
[hakoniwa](https://crates.io/crates/hakoniwa)). The project root is mounted
read-write; everything else is read-only or blocked at the kernel level.

If the sandbox probe fails at startup, `qq` exits with setup instructions
rather than silently degrading.

**Flags:**

| Flag | Effect |
|------|--------|
| *(default)* | Requires a working kernel sandbox. Exits if unavailable. |
| `--classic` | Disables bash tools entirely. Restores built-in `list_files`, `find_files`, and `search_files` tools. |
| `--insecure` | Allows bash tools without kernel sandbox isolation. Simple commands only (no pipes or redirects). Not recommended for untrusted models. |

The two flags are mutually exclusive.

**AppArmor (Ubuntu 24.04+ / containers):**

Distributions with `apparmor_restrict_unprivileged_userns=1` block unprivileged
user namespace creation, which the sandbox requires. Run the setup script to
create an AppArmor profile granting `qq` the `userns` permission:

    sudo ./scripts/setup-apparmor.sh

This is common in Ubuntu 24.04+, Debian trixie, and container images based on
these distros.

**Platform notes:**

| Platform | Sandbox support | Notes |
|----------|----------------|-------|
| Linux (native) | Full | Works out of the box on most distros. May need AppArmor setup (see above). |
| WSL2 | Full | Real Linux kernel — user namespaces typically enabled by default. |
| WSL1 | None | Syscall translation only, no namespace support. Use `--classic`. |
| macOS | None | hakoniwa is Linux-only. Build with `cargo install --path crates/qq-cli --no-default-features --features native-tls` and use `--classic`. |

On platforms without sandbox support, `--classic` is the recommended fallback —
agents use the built-in filesystem search tools instead of bash.

### Example Configurations

See the [examples/](examples/) directory:
- `config.basic.toml` — Minimal setup
- `config.full.toml` — All options
- `config.anthropic.toml` — Anthropic Claude setup
- `config.gemini.toml` — Google Gemini setup
- `config.local-llm.toml` — Ollama/vLLM setup
- `config.multi-provider.toml` — Multiple providers
- `config.openai-compatible.toml` — OpenAI-compatible APIs
- `config.profiles.toml` — Profile examples
- `agents.toml` — Agent customization

## Building from Source

### Prerequisites

- Rust 1.75+ (install via [rustup.rs](https://rustup.rs))
- C compiler (for bundled SQLite)

### Build

There are three ways to build `qq`, depending on how you plan to use it:

#### Standard build (Linux / macOS)

Uses the system TLS stack (OpenSSL on Linux, Security.framework on macOS) and
the system CA certificate store. Requires OpenSSL dev headers on Linux.

```bash
cargo install --path crates/qq-cli
```

#### macOS without sandbox

The kernel sandbox is Linux-only. On macOS, disable it and use `--classic` at
runtime for the built-in filesystem tools instead of bash.

```bash
cargo install --path crates/qq-cli --no-default-features --features native-tls
# then run with: qq --classic ...
```

#### Static binary (Linux x86_64)

Produces a single binary with **zero dynamic dependencies** — copy it to any
Linux x86_64 machine and run it. Uses rustls (pure Rust TLS) with bundled
Mozilla CA roots instead of OpenSSL.

```bash
# One-time setup
rustup target add x86_64-unknown-linux-musl
sudo apt install musl-tools   # provides musl-gcc

# Build
cargo build --release --target x86_64-unknown-linux-musl \
  -p qq-cli --no-default-features --features static-tls

# Binary is at target/x86_64-unknown-linux-musl/release/qq
# Verify — should say "statically linked"
file target/x86_64-unknown-linux-musl/release/qq
```

#### Build comparison

| Command | TLS | Certificates | Dynamic deps | Platforms |
|---------|-----|--------------|--------------|-----------|
| `cargo install --path crates/qq-cli` | native-tls (OpenSSL / Security.framework) | System CA store | Yes (libc, libssl) | Linux, macOS |
| `cargo build --release --target x86_64-unknown-linux-musl -p qq-cli --no-default-features --features static-tls` | rustls (pure Rust) | Bundled Mozilla roots | None | Linux x86_64 |

Standard builds use the OS certificate store, which supports corporate CAs and
OS-managed certificates. The static build bundles its own CA roots — ideal for
deployment to servers and containers but won't trust custom corporate CAs.

#### Development

```bash
cargo build              # debug build
cargo test --workspace   # run tests
cargo doc --workspace --no-deps --open  # generate docs
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
| `401 Unauthorized` | Check API key in config or env var (`OPENAI_API_KEY`, `ANTHROPIC_API_KEY`, `GEMINI_API_KEY`) |
| musl build fails with linker errors | Install `musl-tools` (`sudo apt install musl-tools`) and add target (`rustup target add x86_64-unknown-linux-musl`) |
| Build fails with SQLite error | Ensure C compiler is installed |
| Garbled TUI output | Use a terminal with ANSI support, or try `--no-tui` |
| `Kernel sandbox unavailable` at startup | Run `sudo ./scripts/setup-apparmor.sh` (AppArmor), or use `--classic` / `--insecure` |
| `Kernel sandbox unavailable` on macOS | Build with `--no-default-features --features native-tls` and run with `--classic` |
| `Kernel sandbox unavailable` on WSL1 | Use `--classic` (WSL2 works out of the box) |

For more issues, see [GitHub Issues](https://github.com/andrew/quick-query-rs/issues).
