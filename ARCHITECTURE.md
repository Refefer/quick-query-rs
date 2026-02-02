# Quick-Query Architecture

This document describes the architecture of Quick-Query (`qq`), a fast, extensible CLI for interacting with Large Language Models.

## Table of Contents

1. [Design Philosophy](#design-philosophy)
2. [Crate Structure](#crate-structure)
3. [Core Abstractions](#core-abstractions)
4. [Execution Flows](#execution-flows)
5. [Configuration Architecture](#configuration-architecture)
6. [Tool System](#tool-system)
7. [Agent System](#agent-system)
8. [TUI Architecture](#tui-architecture)
9. [Extension Points](#extension-points)
10. [Design Decisions](#design-decisions)

---

## Design Philosophy

Quick-Query is built on several core principles:

| Principle | Implementation |
|-----------|----------------|
| **Pure Rust** | No ncurses dependency; terminal handling via crossterm/ratatui |
| **Streaming-First** | All LLM interactions support streaming by default |
| **Parallel Execution** | Tools execute concurrently; multiple agent calls can run in parallel |
| **Provider Agnostic** | Works with any OpenAI-compatible API |
| **Security by Default** | Filesystem sandboxing, write-disabled by default, no credential storage |

---

## Crate Structure

### Dependency Graph

```mermaid
graph TD
    CLI[qq-cli<br/>Binary Crate]
    AGENTS[qq-agents<br/>Agent Definitions]
    PROVIDERS[qq-providers<br/>LLM Providers]
    TOOLS[qq-tools<br/>Built-in Tools]
    CORE[qq-core<br/>Core Abstractions]

    CLI --> AGENTS
    CLI --> PROVIDERS
    CLI --> TOOLS
    CLI --> CORE

    AGENTS --> CORE
    PROVIDERS --> CORE
    TOOLS --> CORE
```

### Crate Responsibilities

| Crate | Purpose | Key Exports |
|-------|---------|-------------|
| **qq-core** | Foundation types and traits | `Provider`, `Tool`, `Agent`, `Message`, `ToolRegistry` |
| **qq-providers** | LLM provider implementations | `OpenAIProvider` |
| **qq-tools** | Built-in tools for agents | `create_filesystem_tools`, `create_memory_tools`, `create_web_tools` |
| **qq-agents** | Agent definitions | `ChatAgent`, `CoderAgent`, `ExploreAgent`, etc. |
| **qq-cli** | User-facing CLI binary | `qq` binary, TUI, configuration loading |

---

## Core Abstractions

### Message Model

Messages are the fundamental unit of LLM communication:

```mermaid
classDiagram
    class Message {
        +Role role
        +Content content
        +Vec~ToolCall~ tool_calls
    }

    class Role {
        <<enumeration>>
        System
        User
        Assistant
        Tool
    }

    class Content {
        <<enumeration>>
        Text(String)
        Parts(Vec~ContentPart~)
    }

    class ToolCall {
        +String id
        +String name
        +Value arguments
    }

    Message --> Role
    Message --> Content
    Message --> ToolCall
```

### Provider Trait

The `Provider` trait abstracts LLM interactions:

```rust
#[async_trait]
pub trait Provider: Send + Sync {
    fn name(&self) -> &str;
    fn default_model(&self) -> Option<&str>;

    async fn complete(&self, request: CompletionRequest) -> Result<CompletionResponse, Error>;
    async fn stream(&self, request: CompletionRequest) -> Result<StreamResult, Error>;

    fn supports_tools(&self) -> bool;
    fn supports_vision(&self) -> bool;
}
```

### Tool Trait

Tools are executable capabilities exposed to LLMs:

```rust
#[async_trait]
pub trait Tool: Send + Sync {
    fn name(&self) -> &str;
    fn description(&self) -> &str;
    fn definition(&self) -> ToolDefinition;

    async fn execute(&self, arguments: Value) -> Result<ToolOutput, Error>;
}
```

### Agent Framework

Agents coordinate LLM calls and tool execution:

```mermaid
classDiagram
    class Agent {
        +AgentConfig config
        +Provider provider
        +ToolRegistry tools
        +Vec~Message~ messages
        +run_once() String
        +process() String
    }

    class AgentConfig {
        +AgentId id
        +Option~String~ system_prompt
        +usize max_iterations
        +bool stateful
    }

    class InternalAgent {
        <<trait>>
        +name() str
        +description() str
        +system_prompt() str
        +tool_names() [str]
        +max_iterations() usize
    }

    Agent --> AgentConfig
```

---

## Execution Flows

### One-Shot Completion Flow

```mermaid
sequenceDiagram
    participant User
    participant CLI
    participant Provider
    participant Tools

    User->>CLI: qq -p "prompt"
    CLI->>Provider: complete(request)

    loop Agentic Loop
        Provider-->>CLI: response
        alt Has Tool Calls
            CLI->>Tools: execute_tools_parallel()
            Tools-->>CLI: results
            CLI->>Provider: complete(request + results)
        else No Tool Calls
            CLI-->>User: final response
        end
    end
```

### Interactive Chat Flow (TUI)

```mermaid
sequenceDiagram
    participant User
    participant TUI
    participant Provider
    participant Tools
    participant AgentTools

    User->>TUI: Enter message
    TUI->>Provider: stream(request)

    loop Stream Processing
        Provider-->>TUI: StreamChunk
        TUI->>TUI: Update display
    end

    alt Tool Calls Present
        TUI->>Tools: execute_tools_parallel()
        Tools-->>TUI: results
        alt Agent Tool Called
            TUI->>AgentTools: execute agent
            AgentTools-->>TUI: agent result
        end
        TUI->>Provider: stream(request + results)
    end

    TUI-->>User: Display response
```

### Agentic Loop Detail

```mermaid
flowchart TD
    Start([Start]) --> BuildRequest[Build CompletionRequest]
    BuildRequest --> CallLLM[Call Provider]
    CallLLM --> CheckTools{Tool Calls?}

    CheckTools -->|Yes| ExecuteTools[Execute Tools in Parallel]
    ExecuteTools --> CheckChunk{Large Output?}
    CheckChunk -->|Yes| Summarize[Chunk & Summarize]
    CheckChunk -->|No| AddResults[Add Tool Results]
    Summarize --> AddResults
    AddResults --> CheckIteration{Max Iterations?}
    CheckIteration -->|No| CallLLM
    CheckIteration -->|Yes| Error([Max Iterations Error])

    CheckTools -->|No| Return([Return Response])
```

---

## Configuration Architecture

### Layered Resolution

Configuration is resolved in layers, with later layers overriding earlier ones:

```mermaid
flowchart LR
    TOML[config.toml] --> ENV[Environment Variables]
    ENV --> CLI[CLI Flags]
    CLI --> Final[Resolved Config]
```

### Profile System

```mermaid
classDiagram
    class Config {
        +String default_profile
        +Map~String,Profile~ profiles
        +Map~String,ProviderConfig~ providers
        +Map~String,PromptEntry~ prompts
        +ToolsConfig tools
    }

    class Profile {
        +Option~String~ provider
        +Option~String~ model
        +Option~String~ prompt
        +Option~String~ agent
        +Map parameters
    }

    class ProviderConfig {
        +Option~String~ api_key
        +Option~String~ base_url
        +Option~String~ default_model
        +Map parameters
    }

    Config --> Profile
    Config --> ProviderConfig
```

### Configuration File Structure

```toml
# Profile used when none specified
default_profile = "default"

# Provider configurations
[providers.openai]
api_key = "sk-..."  # Or use OPENAI_API_KEY env var
base_url = "https://api.openai.com/v1"
default_model = "gpt-4o"

# Named prompts
[prompts.coding]
prompt = "You are an expert programmer..."

# Profile definitions
[profiles.default]
provider = "openai"
model = "gpt-4o"

[profiles.coding]
provider = "openai"
prompt = "coding"
agent = "coder"

# Tool configuration
[tools]
root = "$PWD"
allow_write = false
enable_filesystem = true
enable_memory = true
enable_web = true
```

---

## Tool System

### Security Model

```mermaid
flowchart TD
    Request[Tool Request] --> Validate{Validate Path}
    Validate -->|Invalid| Reject[Reject: Outside Root]
    Validate -->|Valid| CheckWrite{Write Operation?}
    CheckWrite -->|Yes| CheckAllow{allow_write?}
    CheckAllow -->|No| Deny[Deny: Writes Disabled]
    CheckAllow -->|Yes| Execute[Execute]
    CheckWrite -->|No| Execute
    Execute --> Return[Return Result]
```

### Tool Registry Pattern

The `ToolRegistry` manages tool registration and lookup:

```rust
// Create registry with all default tools
let mut registry = ToolRegistry::new();

// Register filesystem tools
let fs_config = FileSystemConfig::new(&root).with_write(allow_write);
for tool in create_filesystem_tools_arc(fs_config) {
    registry.register(tool);
}

// Create subset for specific agent
let agent_tools = registry.subset_from_strs(&["read_file", "write_file"]);
```

### Built-in Tools

| Category | Tools | Description |
|----------|-------|-------------|
| **Filesystem** | `read_file`, `write_file`, `list_files`, `search_files` | Sandboxed file operations |
| **Memory** | `memory_store`, `memory_get`, `memory_list`, `memory_delete` | Persistent key-value storage |
| **Web** | `fetch_webpage`, `web_search` | Web content retrieval |
| **Processing** | `process_large_data` | Chunk and summarize large outputs |

### Parallel Execution

Tools are executed concurrently for performance:

```rust
let results = execute_tools_parallel(&registry, tool_calls).await;

// With chunking support for large outputs
let results = execute_tools_parallel_with_chunker(
    &registry,
    tool_calls,
    Some(&chunk_processor),
    Some(original_prompt),
).await;
```

---

## Agent System

### Internal vs External Agents

```mermaid
flowchart TD
    subgraph Internal[Internal Agents]
        Chat[ChatAgent]
        Coder[CoderAgent]
        Explore[ExploreAgent]
        Researcher[ResearcherAgent]
        Reviewer[ReviewerAgent]
        Summarizer[SummarizerAgent]
        Planner[PlannerAgent]
        Writer[WriterAgent]
    end

    subgraph External[External Agents]
        Config[TOML Definition]
        Custom[Custom System Prompt]
    end

    InternalAgent[InternalAgent Trait] --> Internal
    AgentDefinition[AgentDefinition] --> External
```

### Agent-as-Tool Pattern

Agents can be invoked as tools, enabling recursive delegation:

```mermaid
sequenceDiagram
    participant ChatAgent
    participant LLM
    participant AgentTool[Agent[coder]]
    participant CoderAgent

    ChatAgent->>LLM: "Help me refactor this code"
    LLM-->>ChatAgent: Tool call: Agent[coder]
    ChatAgent->>AgentTool: Execute with task
    AgentTool->>CoderAgent: run_once(task)

    loop Coder's Agentic Loop
        CoderAgent->>LLM: Complete
        LLM-->>CoderAgent: Tool calls (read_file, write_file)
        CoderAgent->>CoderAgent: Execute tools
    end

    CoderAgent-->>AgentTool: Result
    AgentTool-->>ChatAgent: Tool result
    ChatAgent->>LLM: Continue with result
```

### Agent Depth Limiting

To prevent infinite recursion, agent depth is tracked:

```rust
const DEFAULT_MAX_AGENT_DEPTH: usize = 3;

// Agent tools are created with depth tracking
create_agent_tools(
    &base_tools,
    provider,
    &agents_config,
    current_depth,      // Incremented for each nested agent
    max_agent_depth,    // Stops creating agent tools at max depth
)
```

### Progress Reporting

Agents emit progress events for TUI display:

```rust
pub enum AgentProgressEvent {
    IterationStart { agent_name, iteration, max_iterations },
    ThinkingDelta { agent_name, content },
    ToolStart { agent_name, tool_name },
    ToolComplete { agent_name, tool_name, is_error },
    UsageUpdate { agent_name, usage },
    ByteCount { agent_name, input_bytes, output_bytes },
}
```

---

## TUI Architecture

### Layout System

```mermaid
flowchart TD
    subgraph Terminal
        subgraph MainArea[Main Area - 80%]
            Messages[Message History]
            Response[Current Response]
        end
        subgraph SidePanel[Side Panel - 20%]
            AgentProgress[Agent Progress]
            TokenStats[Token Statistics]
        end
        subgraph Bottom[Bottom Area]
            Input[Input Area]
            StatusBar[Status Bar]
        end
    end
```

### Component Responsibilities

| Component | Responsibility |
|-----------|----------------|
| **MessageList** | Displays conversation history with markdown rendering |
| **ResponseArea** | Shows streaming response with live updates |
| **AgentPanel** | Displays agent progress, thinking content, tool status |
| **InputArea** | Multi-line input with history |
| **StatusBar** | Shows profile, model, token count |

### Event Handling

```mermaid
flowchart LR
    KeyEvent[Key Event] --> Handler{Event Type}
    Handler -->|Input| UpdateInput[Update Input Buffer]
    Handler -->|Submit| SendMessage[Send to LLM]
    Handler -->|Command| ExecuteCommand[Execute Slash Command]
    Handler -->|Quit| Shutdown[Clean Shutdown]

    StreamEvent[Stream Chunk] --> UpdateDisplay[Update Response Display]
    AgentEvent[Agent Event] --> UpdatePanel[Update Agent Panel]
```

### Real-Time Markdown Rendering

The TUI renders markdown as tokens stream in:

1. Accumulate content in buffer
2. Re-parse markdown on each delta
3. Render with syntax highlighting via `termimad`
4. Handle code blocks with language detection

---

## Extension Points

### Adding New Providers

1. Create a new module in `qq-providers`
2. Implement the `Provider` trait
3. Handle streaming via `StreamResult`

```rust
pub struct MyProvider { /* ... */ }

#[async_trait]
impl Provider for MyProvider {
    fn name(&self) -> &str { "my-provider" }

    async fn complete(&self, request: CompletionRequest)
        -> Result<CompletionResponse, Error> {
        // Implementation
    }

    async fn stream(&self, request: CompletionRequest)
        -> Result<StreamResult, Error> {
        // Return Pin<Box<dyn Stream<Item = Result<StreamChunk>>>>
    }
}
```

### Adding New Tools

1. Create a new module in `qq-tools`
2. Implement the `Tool` trait
3. Define JSON schema parameters

```rust
pub struct MyTool { /* ... */ }

#[async_trait]
impl Tool for MyTool {
    fn name(&self) -> &str { "my_tool" }

    fn description(&self) -> &str { "Does something useful" }

    fn definition(&self) -> ToolDefinition {
        ToolDefinition::new(self.name(), self.description())
            .with_parameters(
                ToolParameters::new()
                    .add_property("input", PropertySchema::string("Input value"), true)
            )
    }

    async fn execute(&self, arguments: Value) -> Result<ToolOutput, Error> {
        let input = arguments["input"].as_str().unwrap_or("");
        Ok(ToolOutput::success(format!("Processed: {}", input)))
    }
}
```

### Adding New Agents

1. Create a new module in `qq-agents`
2. Implement the `InternalAgent` trait
3. Define system prompt and required tools

```rust
pub struct MyAgent;

impl InternalAgent for MyAgent {
    fn name(&self) -> &str { "my-agent" }

    fn description(&self) -> &str { "Custom agent for specific tasks" }

    fn system_prompt(&self) -> &str {
        "You are a specialized agent that..."
    }

    fn tool_names(&self) -> &[&str] {
        &["read_file", "write_file", "my_tool"]
    }

    fn max_iterations(&self) -> usize { 50 }
}
```

---

## Design Decisions

### Why No ncurses?

**Decision**: Use `crossterm` and `ratatui` instead of ncurses.

**Rationale**:
- Pure Rust implementation with no C dependencies
- Cross-platform support (Windows, macOS, Linux) without additional setup
- Better integration with async Rust (crossterm's event-stream feature)
- Simpler build process - no system library requirements

### Why Bundled SQLite?

**Decision**: Use `rusqlite` with the `bundled` feature.

**Rationale**:
- Eliminates runtime dependency on system SQLite
- Ensures consistent SQLite version across platforms
- Simplifies installation - single binary with no external dependencies
- Memory persistence (key-value store) works identically everywhere

### Stateful vs Stateless Agents

**Decision**: Support both modes with clear API separation.

| Mode | Use Case | API |
|------|----------|-----|
| **Stateless** | One-shot tasks, delegated work | `Agent::run_once()` |
| **Stateful** | Interactive conversations, context accumulation | `agent.process()` |

**Rationale**:
- Stateless agents are simpler and avoid context bloat
- Stateful agents needed for interactive chat with history
- Clear separation prevents accidental context leakage between tasks

### Agent-as-Tool vs Direct Invocation

**Decision**: Agents are exposed as tools to the chat agent.

**Rationale**:
- LLM can choose the right agent based on task description
- Consistent interface - all capabilities are tools
- Enables automatic delegation without explicit commands
- Depth limiting prevents infinite recursion

### Streaming by Default

**Decision**: All LLM calls stream by default; non-streaming is opt-in.

**Rationale**:
- Better user experience with visible progress
- Lower perceived latency
- Enables real-time thinking display
- Tool calls are collected during streaming
