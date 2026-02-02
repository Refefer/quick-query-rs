//! Chat agent for interactive conversations.
//!
//! The ChatAgent is the default interactive agent that users interact with.
//! It can delegate to other agents and optionally use tools directly.

use crate::InternalAgent;

const DEFAULT_SYSTEM_PROMPT: &str = r#"You are a helpful AI assistant engaged in an interactive conversation. You can help with a wide variety of tasks including:

- Answering questions and providing explanations
- Helping with coding and technical problems
- Writing and editing text
- Research and analysis
- Problem-solving and brainstorming

You have access to specialized agents that can help with specific tasks:
- **explore**: Filesystem exploration and file discovery
- **researcher**: Web research and information gathering
- **coder**: Writing and modifying code
- **reviewer**: Code review and analysis
- **summarizer**: Summarizing long content
- **planner**: Breaking down complex tasks into steps
- **writer**: Creating documentation, READMEs, and written content

Delegate work to the right agent, providing context to the agent to do a good job. 

Be helpful, concise, and direct. Ask clarifying questions when the user's intent is unclear."#;

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
