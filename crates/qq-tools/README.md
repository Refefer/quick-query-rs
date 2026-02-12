# qq-tools

Built-in tools for Quick-Query agentic workflows.

This crate provides the default tools available to LLM agents: filesystem operations, persistent user preferences, web fetching, and large data processing.

## Overview

Tools are executable capabilities exposed to LLMs via function calling. Each tool:
- Has a name and description
- Defines JSON schema parameters
- Executes asynchronously
- Returns success or error output

## Tool Categories

### Filesystem Tools

Sandboxed file operations with configurable root directory.

| Tool | Description | Write Required |
|------|-------------|----------------|
| `read_file` | Read file contents with line ranges, grep filtering, head/tail | No |
| `list_files` | List files in a directory (non-recursive) | No |
| `find_files` | Recursive file discovery with gitignore support | No |
| `search_files` | Search file contents with regex | No |
| `write_file` | Write full content to a file (creates or overwrites) | Yes |
| `edit_file` | Precision editing: search/replace, insert, delete, replace_lines | Yes |
| `move_file` | Move or rename a file or directory | Yes |
| `copy_file` | Copy a file to a new location | Yes |
| `create_directory` | Create a new directory (recursive by default) | Yes |
| `rm_file` | Remove a file | Yes |
| `rm_directory` | Remove a directory (optional recursive) | Yes |

```rust
use qq_tools::{create_filesystem_tools_arc, FileSystemConfig};

let config = FileSystemConfig::new("/home/user/projects")
    .with_write(true);  // Enable write access

let tools = create_filesystem_tools_arc(config);
```

#### Security Model

- All paths are validated against the configured root
- Paths outside the root are rejected
- Symlinks that escape the root are blocked
- Write operations require explicit `allow_write = true`

#### Tool Parameters

**read_file**
```json
{
  "path": "relative/path/to/file.txt",
  "start_line": 10,
  "end_line": 50,
  "grep": "TODO|FIXME",
  "context": 2,
  "head": 20,
  "tail": 10,
  "line_numbers": true
}
```
Only `path` is required. Use `start_line`/`end_line` for line ranges, `head`/`tail` for shortcuts, or `grep` with optional `context` for regex filtering.

**list_files**
```json
{
  "path": "relative/directory",
  "pattern": "*.rs"
}
```

**find_files**
```json
{
  "path": "src",
  "pattern": "**/*.rs",
  "extensions": ["rs", "toml"],
  "max_depth": 3,
  "file_type": "file",
  "include_hidden": false,
  "limit": 100
}
```
All parameters are optional. Respects `.gitignore` by default.

**search_files**
```json
{
  "pattern": "TODO|FIXME",
  "path": "src",
  "file_pattern": "*.rs"
}
```
Only `pattern` (regex) is required.

**write_file**
```json
{
  "path": "relative/path/to/file.txt",
  "content": "file contents here"
}
```

**edit_file**
```json
{
  "path": "src/main.rs",
  "edits": [
    { "old": "fn old_name()", "new": "fn new_name()" },
    { "operation": "insert", "after_line": 10, "content": "// new comment" },
    { "operation": "delete", "start_line": 5, "end_line": 8 }
  ],
  "create_if_missing": false,
  "show_diff": true,
  "dry_run": false
}
```
Supports `replace` (default), `insert`, `delete`, and `replace_lines` operations. Multiple edits are applied in sequence.

**move_file**
```json
{
  "source": "old/path.rs",
  "destination": "new/path.rs"
}
```

**copy_file**
```json
{
  "source": "original/file.rs",
  "destination": "copy/file.rs"
}
```

**create_directory**
```json
{
  "path": "new/nested/directory",
  "recursive": true
}
```

**rm_file**
```json
{
  "path": "file/to/remove.txt"
}
```

**rm_directory**
```json
{
  "path": "directory/to/remove",
  "recursive": false
}
```

### User Preference Tools

Persistent user preference storage backed by SQLite.

| Tool | Description |
|------|-------------|
| `update_preference` | Store or update a user preference (creates or overwrites) |
| `read_preference` | Retrieve a preference by name |
| `list_preferences` | List all stored preference names |
| `delete_preference` | Delete a preference by name |

```rust
use qq_tools::{create_preference_tools_arc, MemoryStore};
use std::sync::Arc;

// Persistent storage
let store = Arc::new(MemoryStore::new("/path/to/memory.db")?);

// Or in-memory for testing
let store = Arc::new(MemoryStore::in_memory()?);

let tools = create_preference_tools_arc(store);
```

#### Tool Parameters

**update_preference**
```json
{
  "name": "indent_style",
  "value": "tabs"
}
```

**read_preference**
```json
{
  "name": "indent_style"
}
```

**list_preferences**
```json
{}
```

**delete_preference**
```json
{
  "name": "indent_style"
}
```

### Web Tools

Web content retrieval and search.

| Tool | Description |
|------|-------------|
| `fetch_webpage` | Fetch a URL and extract text content |
| `web_search` | Search the web (requires Perplexica) |

```rust
use qq_tools::{create_web_tools_arc, create_web_tools_with_search, WebSearchConfig};

// Basic web tools (fetch only)
let tools = create_web_tools_arc();

// With web search (requires Perplexica instance)
let search_config = WebSearchConfig::new(
    "http://localhost:3000",  // Perplexica host
    "gpt-4o",                 // Chat model
    "text-embedding-ada-002"  // Embedding model
);
let tools = create_web_tools_with_search(Some(search_config));
```

#### Tool Parameters

**fetch_webpage**
```json
{
  "url": "https://example.com/page",
  "selector": "article"
}
```

**web_search**
```json
{
  "query": "rust async programming best practices",
  "focus": "webSearch"
}
```

### Bash Tools (Linux)

Sandboxed shell execution via [hakoniwa](https://crates.io/crates/hakoniwa) kernel containers.

| Tool | Description |
|------|-------------|
| `bash` | Execute a shell command inside the sandbox |
| `mount_external` | Mount an external directory read-only |

```rust
use qq_tools::{create_bash_tools, SandboxMounts, PermissionStore, create_approval_channel};
use std::sync::Arc;

let mounts = Arc::new(SandboxMounts::new("/home/user/project".into()));
let permissions = Arc::new(PermissionStore::new(Default::default()));
let (approval_tx, approval_rx) = create_approval_channel();

let tools = create_bash_tools(mounts, permissions, approval_tx);
```

#### Sandbox Architecture

Commands run inside a Linux container with isolated user/mount/PID namespaces:

- **Project root** is mounted read-write
- **System directories** (`/bin`, `/usr`, `/lib`, `/etc`, `/sbin`) are mounted read-only
- **External mounts** added via `mount_external` are read-only
- `/tmp` is a private tmpfs, `/proc` and `/dev` are virtual filesystems
- No network namespace isolation (commands can access the network)

#### Permission Model

Commands are classified into three tiers before execution:

| Tier | Behavior | Examples |
|------|----------|---------|
| **Session** | Auto-approved, runs without prompting | `ls`, `cat`, `grep`, `find`, `git status`, `git log`, `cargo build`, `cargo test`, `npm test`, `pip list` |
| **Per-call** | Requires user approval (allow once, allow for session, or deny) | `git commit`, `git push`, `cargo run`, `npm install`, `rm`, `mv`, `python` |
| **Restricted** | Always blocked | `sudo`, `su`, `mkfs`, `dd`, `shutdown` |

Pipeline commands (pipes, redirects) are parsed into individual commands, and each
is checked independently. The highest-risk tier in a pipeline determines the overall
classification.

Tools like `cargo`, `git`, `npm`, `yarn`, `pnpm`, `pip`, `pip3`, and `poetry` support
subcommand-level classification. For example, `cargo build` is session-tier (auto-approved)
while `cargo run` is per-call (requires approval). Unrecognized subcommands or flag-only
invocations (e.g., `cargo --version`) fall back to per-call.

Permission overrides can be configured in `config.toml`:

```toml
[tools.bash_permissions]
session = ["make", "npm test"]
per_call = ["docker"]
restricted = ["rm -rf /"]
```

#### Approval Channel

Per-call commands surface an approval request to the user via a tokio mpsc/oneshot
channel. The TUI renders this as a modal overlay; the CLI uses a stdin prompt. Three
responses are available:

- **Allow once** — run this command, ask again next time
- **Allow for session** — promote to session tier for the rest of this session
- **Deny** — block the command

#### Platform Limitations

The kernel sandbox requires Linux user namespaces. It is **not available** on:

- **macOS** — hakoniwa depends on Linux namespaces, cgroups, and procfs. The
  `sandbox` feature (default-on) will fail to compile. Build with
  `cargo install --path crates/qq-cli --no-default-features` and use `--classic`.
- **WSL1** — uses syscall translation without real namespace support. The sandbox
  probe fails at runtime. Use `--classic`.
- **Containers** — works if the container runtime allows user namespaces (common
  with Docker's `--userns=host` or rootless mode). Containers based on Ubuntu 24.04+
  may need the AppArmor setup script (`sudo ./scripts/setup-apparmor.sh`).

**WSL2** works out of the box (real Linux kernel with namespaces enabled by default).

When the sandbox is unavailable, the app-level fallback exists but is severely
restricted: simple commands only (no pipes, redirects, or shell operators), and
only session-tier commands are allowed. This is intentionally limited — `--classic`
mode (built-in search tools) is the recommended fallback for full functionality.

### Process Data Tool

Handles large tool outputs by chunking and summarizing.

```rust
use qq_tools::{create_process_data_tool_arc, ProcessLargeDataTool};
use qq_core::ChunkerConfig;

let config = ChunkerConfig {
    enabled: true,
    threshold_bytes: 50_000,
    chunk_size_bytes: 10_000,
    max_chunks: 5,
};

let tool = create_process_data_tool_arc(provider, config);
```

#### Tool Parameters

**process_large_data**
```json
{
  "data": "very long content that needs summarization...",
  "query": "What are the key points?"
}
```

## Creating a Default Registry

The simplest way to get all tools:

```rust
use qq_tools::{create_default_registry, ToolsConfig};

let config = ToolsConfig {
    root: PathBuf::from("/home/user/projects"),
    allow_write: false,
    memory_db: Some(PathBuf::from("/home/user/.config/qq/memory.db")),
    enable_web: true,
    web_search: None,
};

let registry = create_default_registry(config)?;
```

Or with the builder pattern:

```rust
let config = ToolsConfig::new()
    .with_root("/home/user/projects")
    .with_write(false)
    .with_memory_db("/home/user/.config/qq/memory.db")
    .with_web(true);
```

## Configuration via qq-cli

In `~/.config/qq/config.toml`:

```toml
[tools]
root = "$PWD"           # Filesystem root (supports $HOME, $PWD)
allow_write = false     # Disable writes by default
memory_db = "~/.config/qq/memory.db"
enable_filesystem = true
enable_memory = true
enable_web = true

# Optional web search (requires Perplexica)
[tools.web_search]
host = "http://localhost:3000"
chat_model = "gpt-4o"
embed_model = "text-embedding-ada-002"

# Chunker settings for large outputs
[tools.chunker]
enabled = true
threshold_bytes = 50000
chunk_size_bytes = 10000
max_chunks = 5
```

## Implementing Custom Tools

Create tools by implementing the `Tool` trait:

```rust
use qq_core::{Tool, ToolDefinition, ToolParameters, PropertySchema, ToolOutput, Error};
use async_trait::async_trait;
use serde_json::Value;

pub struct MyCustomTool {
    // Tool state
}

#[async_trait]
impl Tool for MyCustomTool {
    fn name(&self) -> &str {
        "my_custom_tool"
    }

    fn description(&self) -> &str {
        "Does something useful with the provided input"
    }

    fn definition(&self) -> ToolDefinition {
        ToolDefinition::new(self.name(), self.description())
            .with_parameters(
                ToolParameters::new()
                    .add_property(
                        "input",
                        PropertySchema::string("The input to process"),
                        true  // required
                    )
                    .add_property(
                        "format",
                        PropertySchema::enum_string(
                            "Output format",
                            vec!["json".into(), "text".into()]
                        ),
                        false  // optional
                    )
            )
    }

    async fn execute(&self, arguments: Value) -> Result<ToolOutput, Error> {
        let input = arguments["input"]
            .as_str()
            .ok_or_else(|| Error::tool("input is required"))?;

        let format = arguments["format"]
            .as_str()
            .unwrap_or("text");

        // Process the input...
        let result = format!("Processed: {} (format: {})", input, format);

        Ok(ToolOutput::success(result))
    }
}
```

Register custom tools:

```rust
use std::sync::Arc;

let mut registry = create_default_registry(config)?;
registry.register(Arc::new(MyCustomTool::new()));
```

## Dependencies

- `qq-core` - Core types and traits
- `tokio` - Async runtime
- `rusqlite` (bundled) - SQLite for memory storage
- `reqwest` - HTTP client for web tools
- `scraper` - HTML parsing
- `glob` - File pattern matching
- `regex` - Content search
- `hakoniwa` (optional, Linux only) - Kernel sandbox for bash tools
