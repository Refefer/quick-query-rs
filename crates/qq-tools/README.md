# qq-tools

Built-in tools for Quick-Query agentic workflows.

This crate provides the default tools available to LLM agents: filesystem operations, persistent memory, web fetching, and large data processing.

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
| `read_file` | Read contents of a file | No |
| `write_file` | Write content to a file | Yes |
| `list_files` | List files in a directory | No |
| `search_files` | Search file contents with regex | No |

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
  "path": "relative/path/to/file.txt"
}
```

**write_file**
```json
{
  "path": "relative/path/to/file.txt",
  "content": "file contents here"
}
```

**list_files**
```json
{
  "path": "relative/directory",
  "recursive": false,
  "pattern": "*.rs"
}
```

**search_files**
```json
{
  "path": "relative/directory",
  "pattern": "TODO|FIXME",
  "file_pattern": "*.rs"
}
```

### Memory Tools

Persistent key-value storage backed by SQLite.

| Tool | Description |
|------|-------------|
| `memory_store` | Store a key-value pair |
| `memory_get` | Retrieve a value by key |
| `memory_list` | List all stored keys |
| `memory_delete` | Delete a key-value pair |

```rust
use qq_tools::{create_memory_tools_arc, MemoryStore};
use std::sync::Arc;

// Persistent storage
let store = Arc::new(MemoryStore::new("/path/to/memory.db")?);

// Or in-memory for testing
let store = Arc::new(MemoryStore::in_memory()?);

let tools = create_memory_tools_arc(store);
```

#### Tool Parameters

**memory_store**
```json
{
  "key": "project_notes",
  "value": "Remember to update the documentation"
}
```

**memory_get**
```json
{
  "key": "project_notes"
}
```

**memory_list**
```json
{}
```

**memory_delete**
```json
{
  "key": "project_notes"
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
