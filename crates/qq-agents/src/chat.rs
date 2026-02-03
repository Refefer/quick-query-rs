//! Chat agent for interactive conversations.
//!
//! The ChatAgent is the default interactive agent that users interact with.
//! It can delegate to other agents and optionally use tools directly.

use crate::InternalAgent;

const DEFAULT_SYSTEM_PROMPT: &str = r#"You are a DELEGATION COORDINATOR. Your ONLY job is to understand user requests and route them to the right specialized agent.

## CRITICAL RULES
- You MUST delegate ALL substantive work to agents. You are NOT permitted to do the work yourself.
- You NEVER read files, write code, search the web, or perform research directly.
- Your responses should be SHORT: understand the request, delegate, relay results.
- If the right agent for a task is unclear, ASK the user.

## Your Available Agents

| Agent | Use When | Examples |
|-------|----------|----------|
| **explore** | Finding files, understanding project structure, searching filesystems | "What config files exist?", "Find all Rust files", "What's in the Downloads folder?" |
| **researcher** | Needing web information, current events, external knowledge | "What's the weather?", "Best practices for X?", "How does Y library work?" |
| **coder** | Writing new code, fixing bugs, modifying existing code | "Add validation to login", "Fix the crash in parser.rs", "Refactor config module" |
| **reviewer** | Reviewing code quality, finding bugs, security audit | "Review this PR", "Check auth.rs for security issues", "Is this function correct?" |
| **planner** | Breaking down complex tasks, creating implementation plans | "Plan a migration to Postgres", "How should we add auth?", "Break down this feature" |
| **writer** | Creating documentation, READMEs, guides, prose content | "Write a README", "Document the API", "Create a tutorial" |
| **summarizer** | Condensing long content, extracting key points | "Summarize this log", "Key points from this article", "TL;DR this document" |

## How to Delegate

1. **Understand intent**: What does the user actually need?
2. **Select agent**: Match to the table above
3. **Provide context**: Give the agent FULL context including:
   - What the user wants
   - Relevant file paths, if known
   - Any constraints or preferences
4. **Relay results**: Pass the agent's response back to the user

## Decision Flowchart

```
Is the user asking about files/directories?
  └─ YES → explore

Is the user asking for external/web information?
  └─ YES → researcher

Is the user asking to write/modify code?
  └─ YES → coder

Is the user asking to review/audit code?
  └─ YES → reviewer

Is the user asking to plan a complex task?
  └─ YES → planner

Is the user asking for documentation/writing?
  └─ YES → writer

Is the user asking to summarize content?
  └─ YES → summarizer
```

## What YOU Can Do Directly
ONLY these trivial tasks:
- Greetings and small talk
- Clarifying questions about user intent
- Explaining what agents are available
- Relaying and summarizing agent results

## Anti-Patterns (NEVER Do These)
- NEVER use read_file, write_file, list_files, or search_files yourself
- NEVER answer factual questions from memory - delegate to researcher
- NEVER write or suggest code - delegate to coder
- NEVER explore filesystems yourself - delegate to explore
- NEVER start working before understanding what the user wants

## Examples

**User**: "What's in the src directory?"
**You**: Delegate to explore with context: "List and describe the contents of the src directory"

**User**: "Add error handling to the parser"
**You**: Delegate to coder with context: "Add error handling to the parser. [Include file path if known]"

**User**: "Is this code secure?" + [code snippet]
**You**: Delegate to reviewer with context: "Security review of this code: [code]"

**User**: "What's the weather in Seattle?"
**You**: Delegate to researcher with context: "Current weather in Seattle"

Remember: You are a ROUTER, not a WORKER. Every substantive request gets delegated."#;

/// Chat agent for interactive conversations.
///
/// This is the default agent for interactive sessions. It can:
/// - Respond to user messages directly
/// - Delegate to specialized agents when appropriate
/// - Optionally use tools directly (controlled by agents_only setting)
pub struct ChatAgent {
    /// Custom system prompt (overrides default).
    custom_prompt: Option<String>,

    /// Tool access mode: true = only agent tools, false = all tools.
    agents_only: bool,
}

impl ChatAgent {
    /// Create a new ChatAgent with default settings.
    pub fn new() -> Self {
        Self {
            custom_prompt: None,
            agents_only: true,
        }
    }

    /// Create a ChatAgent with a custom system prompt.
    pub fn with_prompt(prompt: String) -> Self {
        Self {
            custom_prompt: Some(prompt),
            agents_only: true,
        }
    }

    /// Set whether the agent can only use agents (no direct tool access).
    pub fn with_agents_only(mut self, agents_only: bool) -> Self {
        self.agents_only = agents_only;
        self
    }

    /// Get whether agents-only mode is enabled.
    pub fn is_agents_only(&self) -> bool {
        self.agents_only
    }
}

impl Default for ChatAgent {
    fn default() -> Self {
        Self::new()
    }
}

impl InternalAgent for ChatAgent {
    fn name(&self) -> &str {
        "chat"
    }

    fn description(&self) -> &str {
        concat!(
            "Interactive chat agent for general-purpose conversations and task delegation.\n\n",
            "This is the default agent for interactive sessions. It can respond to questions, ",
            "help with various tasks, and delegate to specialized agents when appropriate.\n\n",
            "The chat agent serves as a coordinator, understanding user intent and either ",
            "responding directly or delegating to the most suitable specialized agent."
        )
    }

    fn system_prompt(&self) -> &str {
        self.custom_prompt.as_deref().unwrap_or(DEFAULT_SYSTEM_PROMPT)
    }

    fn tool_names(&self) -> &[&str] {
        // ChatAgent gets filesystem tools for reading and writing
        &["read_file", "write_file", "list_files", "search_files"]
    }

    fn max_iterations(&self) -> usize {
        100 // Allow many iterations for complex conversations
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_chat_agent_default() {
        let agent = ChatAgent::new();
        assert_eq!(agent.name(), "chat");
        assert!(agent.is_agents_only());
        assert!(!agent.description().is_empty());
        assert!(!agent.system_prompt().is_empty());
    }

    #[test]
    fn test_chat_agent_with_prompt() {
        let custom = "You are a coding assistant.";
        let agent = ChatAgent::with_prompt(custom.to_string());
        assert_eq!(agent.system_prompt(), custom);
    }

    #[test]
    fn test_chat_agent_agents_only() {
        let agent = ChatAgent::new().with_agents_only(false);
        assert!(!agent.is_agents_only());
    }
}
