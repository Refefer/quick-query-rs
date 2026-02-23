//! Agent definitions and implementations for quick-query.
//!
//! This crate provides:
//! - `InternalAgent` trait for defining agent behavior
//! - Built-in agent implementations (pm, coder, explore, etc.)
//! - Configuration types for external agents

use std::collections::HashMap;

mod project_manager;
mod coder;
mod config;
mod explore;
mod planner;
mod preamble;
mod researcher;
mod reviewer;
mod summarizer;
mod writer;

pub use project_manager::ProjectManagerAgent;
pub use coder::CoderAgent;
pub use config::{AgentDefinition, AgentMemoryStrategy, AgentsConfig, BuiltinAgentOverride};
pub use preamble::{generate_preamble, PreambleContext};
pub use explore::ExploreAgent;

/// Default compaction prompt for agent memory summarization.
///
/// Used when an agent doesn't provide a specialized prompt, or as a fallback.
pub const DEFAULT_COMPACT_PROMPT: &str = r#"Summarize this agent session so it can continue effectively with reduced context. Preserve:
1. Key decisions and conclusions reached
2. Important facts, file paths, code snippets, or data discovered
3. The original task goal and any sub-goals identified
4. Tool results that would be expensive to re-obtain
5. Any pending work or unresolved issues

Be concise but comprehensive. Focus on what's needed to continue the task."#;
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

    /// Get the compaction prompt for summarizing this agent's conversation history.
    ///
    /// When an agent's memory exceeds budget, this prompt guides the LLM in
    /// creating a summary that preserves the most important context for this
    /// agent's specific role. Override to customize what gets preserved.
    fn compact_prompt(&self) -> &str {
        DEFAULT_COMPACT_PROMPT
    }

    /// Whether this agent is read-only (may not modify files or run write commands).
    ///
    /// Read-only agents receive strong reinforcement in the preamble to never
    /// write, modify, create, move, or delete files. Default: false.
    fn is_read_only(&self) -> bool {
        false
    }

    /// Memory strategy for this agent.
    ///
    /// `ObsMemory` runs observational memory inside the agent loop.
    /// `Compaction` uses post-execution LLM summarization with continuation.
    /// Default: `ObsMemory`.
    fn memory_strategy(&self) -> AgentMemoryStrategy {
        AgentMemoryStrategy::ObsMemory
    }

    /// Maximum observations before requesting wrap-up (obs-memory only).
    ///
    /// When the observation count reaches this limit, the agent is asked to
    /// wrap up. After a 3-turn grace period, execution stops.
    /// Default: `Some(10)`.
    fn max_observations(&self) -> Option<u32> {
        Some(10)
    }

    /// Per-agent observation config override (obs-memory only).
    ///
    /// When `Some(config)`, overrides `ObservationConfig::for_agents()` defaults.
    /// Default: `None` (use agent defaults).
    fn observation_config(&self) -> Option<qq_core::ObservationConfig> {
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
    ProjectManager,
    Researcher,
    Summarizer,
    Coder,
    Reviewer,
    Explore,
    Planner,
    Writer,
}

impl InternalAgentType {
    /// Get all internal agent types (excluding pm, which is special).
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

    /// Get all internal agent types including pm.
    pub fn all_with_pm() -> Vec<Self> {
        vec![
            Self::ProjectManager,
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
            Self::ProjectManager => "pm",
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
            Self::ProjectManager => Box::new(ProjectManagerAgent::new()),
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
            "pm" | "chat" => Some(Self::ProjectManager),
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
    fn test_internal_agent_types_with_pm() {
        let types = InternalAgentType::all_with_pm();
        assert_eq!(types.len(), 8);

        // Verify pm is included
        assert!(types.iter().any(|t| t.name() == "pm"));
    }

    #[test]
    fn test_agent_type_from_name() {
        assert!(InternalAgentType::from_name("pm").is_some());
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
    fn test_agent_type_from_name_backward_compat() {
        // "chat" should still resolve to ProjectManager
        let t = InternalAgentType::from_name("chat").unwrap();
        assert_eq!(t.name(), "pm");
    }

    #[test]
    fn test_agent_tool_descriptions() {
        for t in InternalAgentType::all_with_pm() {
            let agent = t.create();
            assert!(
                !agent.tool_description().is_empty(),
                "Agent {} should have a tool description",
                agent.name()
            );
        }
    }

    #[test]
    fn test_agent_is_read_only() {
        let read_only_agents = ["explore", "reviewer", "researcher", "planner"];
        let write_agents = ["pm", "coder", "writer", "summarizer"];

        for t in InternalAgentType::all_with_pm() {
            let agent = t.create();
            let name = agent.name();
            if read_only_agents.contains(&name) {
                assert!(
                    agent.is_read_only(),
                    "Agent {} should be read-only",
                    name
                );
            } else if write_agents.contains(&name) {
                assert!(
                    !agent.is_read_only(),
                    "Agent {} should NOT be read-only",
                    name
                );
            }
        }
    }

    #[test]
    fn test_agent_compact_prompts() {
        for t in InternalAgentType::all_with_pm() {
            let agent = t.create();
            let prompt = agent.compact_prompt();
            assert!(
                !prompt.is_empty(),
                "Agent {} should have a non-empty compact_prompt",
                agent.name()
            );
        }
    }

    #[test]
    fn test_agent_memory_strategy() {
        for t in InternalAgentType::all_with_pm() {
            let agent = t.create();
            let strategy = agent.memory_strategy();
            // All agents should have a valid strategy (default is ObsMemory)
            assert!(
                strategy == AgentMemoryStrategy::ObsMemory
                    || strategy == AgentMemoryStrategy::Compaction,
                "Agent {} should have a valid memory strategy",
                agent.name()
            );
        }
    }

    #[test]
    fn test_agent_max_observations() {
        for t in InternalAgentType::all_with_pm() {
            let agent = t.create();
            // Default is Some(10), should be reasonable
            if let Some(max) = agent.max_observations() {
                assert!(
                    max > 0,
                    "Agent {} max_observations should be > 0",
                    agent.name()
                );
            }
        }
    }
}
