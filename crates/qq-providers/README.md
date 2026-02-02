# qq-providers

LLM provider implementations for Quick-Query.

This crate provides implementations of the `Provider` trait from `qq-core` for various LLM APIs.

## Overview

Currently implements:
- **OpenAI Provider** - Full support for OpenAI API and any OpenAI-compatible endpoint

## Supported Providers

### OpenAI Provider

The `OpenAIProvider` works with:

| Service | Notes |
|---------|-------|
| **OpenAI** | Full support including GPT-4, GPT-4o, o1-series |
| **Azure OpenAI** | Set custom `base_url` |
| **Ollama** | Local models via OpenAI-compatible API |
| **vLLM** | High-performance local inference |
| **Together AI** | Cloud inference |
| **Groq** | Fast inference |
| **OpenRouter** | Multi-provider routing |
| **LM Studio** | Local desktop inference |

## Usage

### Basic Usage

```rust
use qq_providers::OpenAIProvider;
use qq_core::{Provider, CompletionRequest, Message};

// Create provider with API key
let provider = OpenAIProvider::new("sk-your-api-key")
    .with_default_model("gpt-4o");

// Make a completion request
let request = CompletionRequest::new(vec![
    Message::system("You are a helpful assistant"),
    Message::user("Hello!"),
]);

let response = provider.complete(request).await?;
println!("{}", response.message.content);
```

### Streaming

```rust
use futures::StreamExt;
use qq_core::StreamChunk;

let request = CompletionRequest::new(vec![Message::user("Tell me a story")])
    .with_stream(true);

let mut stream = provider.stream(request).await?;

while let Some(chunk) = stream.next().await {
    match chunk? {
        StreamChunk::Delta { content } => print!("{}", content),
        StreamChunk::Done { usage } => println!("\n[Done]"),
        StreamChunk::Error { message } => eprintln!("Error: {}", message),
        _ => {}
    }
}
```

### With Tools

```rust
use qq_core::{ToolDefinition, ToolParameters, PropertySchema};

let tool = ToolDefinition::new("get_weather", "Get the current weather")
    .with_parameters(
        ToolParameters::new()
            .add_property("location", PropertySchema::string("City name"), true)
    );

let request = CompletionRequest::new(messages)
    .with_tools(vec![tool]);

let response = provider.complete(request).await?;

// Check for tool calls
for tool_call in &response.message.tool_calls {
    println!("Tool: {} Args: {}", tool_call.name, tool_call.arguments);
}
```

### Custom Base URL

For self-hosted or alternative providers:

```rust
// Ollama
let provider = OpenAIProvider::new("ollama")  // Key not needed for Ollama
    .with_base_url("http://localhost:11434/v1")
    .with_default_model("llama2");

// vLLM
let provider = OpenAIProvider::new("vllm")
    .with_base_url("http://localhost:8000/v1")
    .with_default_model("meta-llama/Llama-2-7b-chat-hf");

// Together AI
let provider = OpenAIProvider::new("your-together-key")
    .with_base_url("https://api.together.xyz/v1")
    .with_default_model("meta-llama/Llama-2-70b-chat-hf");
```

### Extra Parameters

Pass provider-specific parameters:

```rust
let request = CompletionRequest::new(messages)
    .with_extra_param("reasoning_effort", serde_json::json!("high"))  // o1-series
    .with_extra_param("seed", serde_json::json!(42));  // Reproducibility
```

## API Reference

### OpenAIProvider

```rust
impl OpenAIProvider {
    /// Create with API key
    pub fn new(api_key: impl Into<String>) -> Self;

    /// Set custom base URL
    pub fn with_base_url(self, url: impl Into<String>) -> Self;

    /// Set default model
    pub fn with_default_model(self, model: impl Into<String>) -> Self;
}
```

### Provider Trait Implementation

```rust
impl Provider for OpenAIProvider {
    fn name(&self) -> &str;
    fn default_model(&self) -> Option<&str>;

    async fn complete(&self, request: CompletionRequest)
        -> Result<CompletionResponse, Error>;

    async fn stream(&self, request: CompletionRequest)
        -> Result<StreamResult, Error>;

    fn supports_tools(&self) -> bool;  // true
    fn supports_vision(&self) -> bool; // false (may change)
}
```

## Stream Chunk Types

| Chunk | Description |
|-------|-------------|
| `Start { model }` | Stream started with model name |
| `Delta { content }` | Content delta |
| `ThinkingDelta { content }` | Reasoning/thinking content (o1-series) |
| `ToolCallStart { id, name }` | Tool call started |
| `ToolCallDelta { arguments }` | Tool call arguments delta |
| `Done { usage }` | Stream complete with usage stats |
| `Error { message }` | Stream error |

## Configuration via qq-cli

In `~/.config/qq/config.toml`:

```toml
[providers.openai]
api_key = "sk-..."  # Or use OPENAI_API_KEY env var
base_url = "https://api.openai.com/v1"  # Optional
default_model = "gpt-4o"

[providers.openai.parameters]
# Extra parameters for all requests
temperature = 0.7

[providers.ollama]
api_key = "ollama"  # Placeholder
base_url = "http://localhost:11434/v1"
default_model = "llama2"
```

## Adding New Providers

To add a new provider:

1. Create a new module in `src/`
2. Implement the `Provider` trait from `qq-core`
3. Handle streaming via `StreamResult` (a pinned async stream)
4. Export from `lib.rs`

See `openai.rs` for a complete implementation example.

## Dependencies

- `qq-core` - Core types and traits
- `reqwest` - HTTP client with streaming
- `reqwest-eventsource` - Server-sent events for streaming
- `tokio` / `tokio-stream` - Async runtime and streams
- `serde` / `serde_json` - JSON serialization
