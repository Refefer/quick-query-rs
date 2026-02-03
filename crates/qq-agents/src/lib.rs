//! Agent definitions and implementations for quick-query.
//!
//! This crate provides:
//! - `InternalAgent` trait for defining agent behavior
//! - Built-in agent implementations (chat, coder, explore, etc.)
//! - Configuration types for external agents

use std::collections::HashMap;

mod chat;
mod coder;
mod config;
mod explore;
mod planner;
mod researcher;
mod reviewer;
mod summarizer;
mod writer;

pub use chat::ChatAgent;
pub use coder::CoderAgent;
pub use config::{AgentDefinition, AgentsConfig, BuiltinAgentOverride};
pub use explore::ExploreAgent;
pub use planner::PlannerAgent;
pub use researcher::ResearcherAgent;
pub use reviewer::ReviewerAgent;
pub use summarizer::SummarizerAgent;
pub use writer::WriterAgent;

/// Trait for internal agents.
///
/// Internal agents are built-in agents that implement specific behaviors
/// like coding, research, exploration, etc. Each agent has a system prompt
/// that guides its behavior and a set of tools it can use.
pub trait InternalAgent: Send + Sync {
    /// Get the agent name (e.g., "coder", "explore")
    fn name(&self) -> &str;

    /// Get the agent description for display
    fn description(&self) -> &str;

    /// Get the system prompt for this agent
    fn system_prompt(&self) -> &str;

    /// Get the tool names this agent needs
    fn tool_names(&self) -> &[&str];

    /// Get the default max iterations for the agentic loop
    fn max_turns(&self) -> usize {
        20
    }

    /// Get the description for when this agent is exposed as a tool to an LLM.
    ///
    /// This should be concise and guide proper usage (goals not commands).
    /// Default: falls back to `description()`.
    fn tool_description(&self) -> &str {
        self.description()
    }

    /// Get per-tool call limits for this agent.
    ///
    /// Returns a map of tool names to maximum allowed calls per agent execution.
    /// When a tool reaches its limit, the agent receives an error message instead.
    /// Default: None (no limits).
    fn tool_limits(&self) -> Option<HashMap<String, usize>> {
        None
    }
}

/// Information about an available agent.
#[derive(Debug, Clone)]
pub struct AgentInfo {
    /// Unique agent name/identifier
    pub name: String,
    /// Human-readable description
    pub description: String,
    /// Whether this is an internal (built-in) agent
    pub is_internal: bool,
    /// Tool names this agent uses
    pub tools: Vec<String>,
}

/// All internal agent types.
pub enum InternalAgentType {
    Chat,
    Researcher,
    Summarizer,
    Coder,
    Reviewer,
    Explore,
    Planner,
    Writer,
}

impl InternalAgentType {
    /// Get all internal agent types (excluding chat, which is special).
    pub fn all() -> Vec<Self> {
        vec![
            Self::Researcher,
            Self::Summarizer,
            Self::Coder,
            Self::Reviewer,
            Self::Explore,
            Self::Planner,
            Self::Writer,
        ]
    }

    /// Get all internal agent types including chat.
    pub fn all_with_chat() -> Vec<Self> {
        vec![
            Self::Chat,
            Self::Researcher,
            Self::Summarizer,
            Self::Coder,
            Self::Reviewer,
            Self::Explore,
            Self::Planner,
            Self::Writer,
        ]
    }

    /// Get the agent name.
    pub fn name(&self) -> &str {
        match self {
            Self::Chat => "chat",
            Self::Researcher => "researcher",
            Self::Summarizer => "summarizer",
            Self::Coder => "coder",
            Self::Reviewer => "reviewer",
            Self::Explore => "explore",
            Self::Planner => "planner",
            Self::Writer => "writer",
        }
    }

    /// Create the internal agent instance.
    pub fn create(&self) -> Box<dyn InternalAgent> {
        match self {
            Self::Chat => Box::new(ChatAgent::new()),
            Self::Researcher => Box::new(ResearcherAgent::new()),
            Self::Summarizer => Box::new(SummarizerAgent::new()),
            Self::Coder => Box::new(CoderAgent::new()),
            Self::Reviewer => Box::new(ReviewerAgent::new()),
            Self::Explore => Box::new(ExploreAgent::new()),
            Self::Planner => Box::new(PlannerAgent::new()),
            Self::Writer => Box::new(WriterAgent::new()),
        }
    }

    /// Parse a name into an agent type.
    pub fn from_name(name: &str) -> Option<Self> {
        match name {
            "chat" => Some(Self::Chat),
            "researcher" => Some(Self::Researcher),
            "summarizer" => Some(Self::Summarizer),
            "coder" => Some(Self::Coder),
            "reviewer" => Some(Self::Reviewer),
            "explore" => Some(Self::Explore),
            "planner" => Some(Self::Planner),
            "writer" => Some(Self::Writer),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_internal_agent_types() {
        let types = InternalAgentType::all();
        assert_eq!(types.len(), 7);

        for t in types {
            let agent = t.create();
            assert!(!agent.name().is_empty());
            assert!(!agent.description().is_empty());
            assert!(!agent.system_prompt().is_empty());
        }
    }

    #[test]
    fn test_internal_agent_types_with_chat() {
        let types = InternalAgentType::all_with_chat();
        assert_eq!(types.len(), 8);

        // Verify chat is included
        assert!(types.iter().any(|t| t.name() == "chat"));
    }

    #[test]
    fn test_agent_type_from_name() {
        assert!(InternalAgentType::from_name("chat").is_some());
        assert!(InternalAgentType::from_name("researcher").is_some());
        assert!(InternalAgentType::from_name("summarizer").is_some());
        assert!(InternalAgentType::from_name("coder").is_some());
        assert!(InternalAgentType::from_name("reviewer").is_some());
        assert!(InternalAgentType::from_name("explore").is_some());
        assert!(InternalAgentType::from_name("planner").is_some());
        assert!(InternalAgentType::from_name("writer").is_some());
        assert!(InternalAgentType::from_name("unknown").is_none());
    }

    #[test]
    fn test_agent_tool_descriptions() {
        for t in InternalAgentType::all_with_chat() {
            let agent = t.create();
            assert!(
                !agent.tool_description().is_empty(),
                "Agent {} should have a tool description",
                agent.name()
            );
        }
    }
}
