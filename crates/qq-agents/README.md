# qq-agents

Agent definitions and implementations for Quick-Query.

This crate provides pre-built agents with specialized behaviors for different tasks: coding, research, exploration, planning, writing, and more.

## Overview

Agents are LLM-powered assistants with specific system prompts and tool access. They can:
- Execute tasks autonomously using tools
- Delegate to other agents (agent-as-tool pattern)
- Run in stateful or stateless modes
- Persist conversation history across invocations (scoped memory)

## Built-in Agents

| Agent | Purpose | Tools |
|-------|---------|-------|
| **pm** | Project manager: coordinates agents, tracks tasks, ensures delivery | `create_task`, `update_task`, `list_tasks`, `delete_task` |
| **explore** | Filesystem exploration and discovery | `read_file`, `find_files`, `search_files` |
| **researcher** | Web research and synthesis | `web_search`, `fetch_webpage`, `read_preference` |
| **coder** | Code generation and modification | `read_file`, `edit_file`, `write_file`, `move_file`, `copy_file`, `create_directory`, `rm_file`, `rm_directory`, `find_files`, `search_files` |
| **reviewer** | Code review and analysis | `read_file`, `find_files`, `search_files` |
| **summarizer** | Content summarization | (none — pure LLM) |
| **planner** | Task decomposition and planning | `read_preference` (plus agent delegation tools) |
| **writer** | Documentation and content creation | `read_file`, `write_file`, `edit_file`, `copy_file`, `create_directory`, `find_files`, `search_files` |

## Agent Details

### ProjectManagerAgent

The default interactive agent. Coordinates work by scoping requirements, planning, assembling agent teams, tracking tasks, and ensuring delivery quality.

```rust
use qq_agents::ProjectManagerAgent;

let agent = ProjectManagerAgent::new();
// Or with custom prompt
let agent = ProjectManagerAgent::with_prompt("You are a coding mentor...".into());
```

**Key behaviors:**
- Tracks work via `create_task`, `update_task`, `list_tasks`, `delete_task` tools
- Delegates substantive work to sub-agents, reviews results before reporting
- Parallelizes independent agent calls for efficiency
- Uses `inform_user` tool to send status messages to the user without ending its turn

### ExploreAgent

Autonomous filesystem exploration for finding and understanding files.

**Use cases:**
- "Find all config files in this directory"
- "What's in the Downloads folder?"
- "Search for log files from today"

**Strategy:**
- Top-down directory listing
- Pattern-based file search
- Content inspection and summarization

### ResearcherAgent

Web research with two modes:

| Mode | When | Behavior |
|------|------|----------|
| **Fast** | Default | One search, synthesized summary |
| **In-depth** | When requested | Multiple searches, deep source reading |

**Use cases:**
- "Best practices for Rust error handling"
- "In-depth research on CRDTs vs OT for collaboration"
- "Current weather in San Francisco"

### CoderAgent

Autonomous code generation following existing patterns.

**Philosophy:**
1. Understand the goal
2. Read existing code first
3. Follow established patterns
4. Implement minimally
5. Verify changes

**Use cases:**
- "Add input validation to the login form"
- "Implement retry with exponential backoff"
- "Refactor config module for multiple profiles"

### ReviewerAgent

Code review with prioritized findings.

**Review categories (by priority):**
1. **Critical**: Bugs, crashes, security issues
2. **Important**: Logic errors, edge cases
3. **Moderate**: Performance, maintainability
4. **Minor**: Style, naming, docs

**Use cases:**
- "Review src/auth.rs for security issues"
- "Check this function for bugs"
- "Security audit of the upload handler"

### SummarizerAgent

Content summarization with format adaptation.

**Strategies:**
- Executive summary for reports
- Action-focused for meetings
- Problem-focused for incidents
- Learning-focused for technical content

**Output scales with input:**
- Short (<500 words): 2-3 sentences
- Medium: Bullet points
- Long: Structured sections

### PlannerAgent

Task decomposition and implementation planning.

**Output structure:**
```
## Goal Summary
## Prerequisites
## Phase 1: [Name]
  1. Step
  2. Step (depends on: 1)
## Phase 2: ...
## Risks & Considerations
## Verification
```

**Use cases:**
- "Plan migration from SQLite to PostgreSQL"
- "Plan adding authentication to the API"
- "Break down the refactoring task"

### WriterAgent

Documentation and content creation.

**Strategies:**
- README: Quick start first, details later
- API docs: Consistent format, examples
- Guides: Hook reader, build understanding
- Changelog: What changed, why, migration

**Use cases:**
- "Write a README for this project"
- "Create API documentation for users.rs"
- "Write a getting started guide"

## Using Agents Programmatically

### Creating Agent Instances

```rust
use qq_agents::{InternalAgentType, InternalAgent};

// Get a specific agent
let agent = InternalAgentType::Coder.create();
println!("Name: {}", agent.name());
println!("Description: {}", agent.description());

// Get all agents (excluding pm)
for agent_type in InternalAgentType::all() {
    let agent = agent_type.create();
    println!("{}: {}", agent.name(), agent.description());
}

// Parse agent name
if let Some(agent_type) = InternalAgentType::from_name("researcher") {
    let agent = agent_type.create();
}
```

### Agent Execution with qq-core

```rust
use qq_core::{Agent, AgentConfig};
use qq_agents::CoderAgent;

let coder = CoderAgent::new();

let config = AgentConfig::new(coder.name())
    .with_system_prompt(coder.system_prompt())
    .with_max_iterations(coder.max_turns());

// Stateless one-shot execution
let result = Agent::run_once(
    provider,
    tools.subset_from_strs(coder.tool_names()),
    config,
    vec![Message::user("Add error handling to parse_config")],
).await?;
```

## External Agents

Define custom agents in configuration:

```toml
# ~/.config/qq/agents.toml

[agents.sql-expert]
description = "SQL query optimization specialist"
prompt = """
You are an expert SQL developer specializing in query optimization.
When given a query, analyze it for:
- Index usage opportunities
- Join optimization
- Subquery elimination
- Query plan analysis
"""
tools = ["read_file", "write_file"]
max_iterations = 50
```

Load external agents:

```rust
use qq_agents::{AgentsConfig, AgentDefinition};

let config = AgentsConfig::load()?;

if let Some(agent_def) = config.agents.get("sql-expert") {
    println!("Description: {}", agent_def.description);
    println!("Tools: {:?}", agent_def.tools);
}
```

## InternalAgent Trait

Implement this trait to create new internal agents:

```rust
use qq_agents::InternalAgent;
use std::collections::HashMap;

pub struct MyAgent;

impl InternalAgent for MyAgent {
    fn name(&self) -> &str {
        "my-agent"
    }

    fn description(&self) -> &str {
        "Custom agent for specific tasks..."
    }

    fn system_prompt(&self) -> &str {
        r#"You are a specialized agent that...

## Your Mission
...

## How You Think
1. Understand the goal
2. Gather context
3. Execute strategically

## Your Tools
- tool_a: For X
- tool_b: For Y
"#
    }

    fn tool_names(&self) -> &[&str] {
        &["tool_a", "tool_b"]
    }

    fn max_turns(&self) -> usize {
        50  // Default is 20
    }

    // Optional: per-tool call limits
    fn tool_limits(&self) -> Option<HashMap<String, usize>> {
        let mut limits = HashMap::new();
        limits.insert("tool_a".to_string(), 10);
        Some(limits)
    }

    // Optional: custom compaction prompt for memory summarization
    fn compact_prompt(&self) -> &str {
        "Summarize this agent conversation, preserving key findings and decisions."
    }
}
```

## Agent-as-Tool Pattern

In qq-cli, agents are exposed as tools to the PM agent:

```
User: Help me refactor the config module
PM -> LLM: [tools: Agent[coder], Agent[explore], create_task, ...]
LLM -> PM: Tool call: Agent[coder] with task
PM -> CoderAgent: run_once(task)
CoderAgent: (reads files, writes code, verifies)
CoderAgent -> PM: Result
PM -> User: Done! Here's what I changed...
```

This enables:
- Automatic agent selection by the LLM
- Recursive delegation (with depth limits)
- Consistent tool interface

### Scoped Memory

Each agent invocation is identified by its call chain path, creating isolated memory scopes:

- `"pm/explore"` — explore agent called by chat
- `"pm/coder/explore"` — explore agent called by coder, which was called by chat

The `new_instance` parameter (default: `false`) controls memory:
- `false`: The agent continues with full context from prior calls in this scope.
- `true`: Clears the agent's memory for a fresh start. Use when prior context would be misleading.

Each scope has a 200KB memory budget. When exceeded, older messages are automatically trimmed at safe boundaries (preserving tool call/result pairs).

### Continuation

When an agent exhausts its `max_turns`, its progress is automatically summarized and it is re-invoked with that summary as context. This allows long tasks to complete without losing progress. Up to 3 continuation attempts are made by default.

### Compaction Prompts

Each agent can customize how its conversation history is summarized when memory is compacted, via the `compact_prompt()` method. For example, the coder agent's compaction preserves file paths, code patterns, and design decisions, while the researcher preserves sources, key findings, and conflicting information.

## Configuration via qq-cli

In `~/.config/qq/config.toml`:

```toml
[profiles.coding]
provider = "openai"
model = "gpt-4o"
agent = "coder"  # Primary agent for this profile

[profiles.coding.parameters]
temperature = 0.2
```

Select agent at runtime:

```bash
qq -A coder -p "Add input validation"
qq manage --agent researcher
```

## Dependencies

- `qq-core` - Core types and traits
- `serde` - Configuration serialization
- `toml` - Config file parsing
- `dirs` - Config directory resolution
