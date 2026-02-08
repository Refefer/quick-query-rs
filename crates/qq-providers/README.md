# qq-providers

LLM provider implementations for Quick-Query.

This crate provides implementations of the `Provider` trait from `qq-core` for various LLM APIs.

## Overview

Implements three native providers:
- **OpenAI Provider** - OpenAI API and any OpenAI-compatible endpoint (Ollama, vLLM, Groq, etc.)
- **Anthropic Provider** - Native Anthropic Claude Messages API
- **Gemini Provider** - Native Google Gemini Generative Language API

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

### Anthropic Provider

The `AnthropicProvider` implements the Anthropic Messages API natively:

| Feature | Details |
|---------|---------|
| **Auth** | `x-api-key` header |
| **API version** | `2023-06-01` |
| **Streaming** | SSE with typed events (`content_block_start`, `content_block_delta`, etc.) |
| **Thinking** | Native `thinking` content blocks extracted to `CompletionResponse::thinking` |
| **Tool calls** | `tool_use` / `tool_result` content blocks |
| **Models** | `claude-sonnet-4-20250514`, `claude-opus-4-20250514`, `claude-haiku-3-5-20241022` |

**Note:** `max_tokens` is required by Anthropic. Defaults to 8192 when not specified.

### Gemini Provider

The `GeminiProvider` implements the Google Generative Language API natively:

| Feature | Details |
|---------|---------|
| **Auth** | API key in query parameter (`?key=`) |
| **Streaming** | SSE with `alt=sse`, each event is a full response chunk |
| **Tool calls** | `functionCall` / `functionResponse` parts |
| **Tool call IDs** | Synthetic (`gemini_tc_0`, `gemini_tc_1`, ...) — Gemini doesn't provide IDs |
| **Models** | `gemini-2.5-pro`, `gemini-2.5-flash`, `gemini-2.0-flash` |

## Usage

### Basic Usage

```rust
use qq_providers::{OpenAIProvider, AnthropicProvider, GeminiProvider};
use qq_core::{Provider, CompletionRequest, Message};

// OpenAI
let provider = OpenAIProvider::new("sk-your-api-key")
    .with_default_model("gpt-4o");

// Anthropic
let provider = AnthropicProvider::new("sk-ant-your-key")
    .with_default_model("claude-sonnet-4-20250514");

// Gemini
let provider = GeminiProvider::new("AIza-your-key")
    .with_default_model("gemini-2.5-flash");

// All providers implement the same trait
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
// Ollama (OpenAI-compatible)
let provider = OpenAIProvider::new("ollama")
    .with_base_url("http://localhost:11434/v1")
    .with_default_model("llama2");

// Anthropic proxy
let provider = AnthropicProvider::new("sk-ant-key")
    .with_base_url("https://my-proxy.example.com/v1");
```

## Configuration via qq-cli

In `~/.config/qq/config.toml`:

```toml
# OpenAI (type auto-detected from name)
[providers.openai]
api_key = "sk-..."  # Or use OPENAI_API_KEY env var
default_model = "gpt-4o"

# Anthropic (type auto-detected from name)
[providers.anthropic]
api_key = "sk-ant-..."  # Or use ANTHROPIC_API_KEY env var
default_model = "claude-sonnet-4-20250514"

# Gemini (type auto-detected from name)
[providers.gemini]
api_key = "AIza..."  # Or use GEMINI_API_KEY env var
default_model = "gemini-2.5-flash"

# Explicit type for custom-named providers
[providers.my-claude-proxy]
type = "anthropic"
api_key = "sk-ant-..."
base_url = "https://proxy.example.com/v1"

# OpenAI-compatible (auto-detected when base_url is set)
[providers.ollama]
base_url = "http://localhost:11434/v1"
default_model = "llama3.1"
```

## Provider Type Resolution

1. Explicit `type` field always wins
2. If no `type` but `base_url` is set → defaults to `"openai"` (OpenAI-compatible)
3. If no `type` and no `base_url` → infer from provider name:
   - `"anthropic"` or `"claude"` → Anthropic
   - `"gemini"` or `"google"` → Gemini
   - Everything else → OpenAI

## Stream Chunk Types

| Chunk | Description |
|-------|-------------|
| `Start { model }` | Stream started with model name |
| `Delta { content }` | Content delta |
| `ThinkingDelta { content }` | Reasoning/thinking content |
| `ToolCallStart { id, name }` | Tool call started |
| `ToolCallDelta { arguments }` | Tool call arguments delta |
| `Done { usage }` | Stream complete with usage stats |
| `Error { message }` | Stream error |

## Dependencies

- `qq-core` - Core types and traits
- `reqwest` - HTTP client with streaming
- `reqwest-eventsource` - Server-sent events for streaming
- `tokio` / `tokio-stream` - Async runtime and streams
- `serde` / `serde_json` - JSON serialization
