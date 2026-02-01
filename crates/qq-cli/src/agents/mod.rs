//! Agent system for quick-query.
//!
//! This module provides:
//! - Internal agents (researcher, summarizer, coder, reviewer, explore, planner)
//! - Support for external agents defined in agents.toml
//! - Agent tools that expose agents as callable tools for the LLM
//! - AgentExecutor for manual agent invocation via chat commands

pub mod agent_tool;
pub mod coder;
pub mod explore;
pub mod planner;
pub mod researcher;
pub mod reviewer;
pub mod summarizer;

pub use agent_tool::create_agent_tools;

use std::collections::HashMap;
use std::sync::Arc;

use anyhow::Result;

use qq_core::{Agent, AgentConfig, Provider, ToolRegistry};

use crate::config::{AgentDefinition, AgentsConfig};

/// Information about an available agent
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

/// Trait for internal agents
pub trait InternalAgent: Send + Sync {
    /// Get the agent name
    fn name(&self) -> &str;

    /// Get the agent description
    fn description(&self) -> &str;

    /// Get the system prompt for this agent
    fn system_prompt(&self) -> &str;

    /// Get the tool names this agent needs
    fn tool_names(&self) -> &[&str];

    /// Get the default max iterations
    fn max_iterations(&self) -> usize {
        20
    }
}

/// All internal agent types
pub enum InternalAgentType {
    Researcher,
    Summarizer,
    Coder,
    Reviewer,
    Explore,
    Planner,
}

impl InternalAgentType {
    /// Get all internal agent types
    pub fn all() -> Vec<Self> {
        vec![
            Self::Researcher,
            Self::Summarizer,
            Self::Coder,
            Self::Reviewer,
            Self::Explore,
            Self::Planner,
        ]
    }

    /// Get the agent name
    pub fn name(&self) -> &str {
        match self {
            Self::Researcher => "researcher",
            Self::Summarizer => "summarizer",
            Self::Coder => "coder",
            Self::Reviewer => "reviewer",
            Self::Explore => "explore",
            Self::Planner => "planner",
        }
    }

    /// Create the internal agent instance
    pub fn create(&self) -> Box<dyn InternalAgent> {
        match self {
            Self::Researcher => Box::new(researcher::ResearcherAgent::new()),
            Self::Summarizer => Box::new(summarizer::SummarizerAgent::new()),
            Self::Coder => Box::new(coder::CoderAgent::new()),
            Self::Reviewer => Box::new(reviewer::ReviewerAgent::new()),
            Self::Explore => Box::new(explore::ExploreAgent::new()),
            Self::Planner => Box::new(planner::PlannerAgent::new()),
        }
    }

    /// Parse a name into an agent type
    pub fn from_name(name: &str) -> Option<Self> {
        match name {
            "researcher" => Some(Self::Researcher),
            "summarizer" => Some(Self::Summarizer),
            "coder" => Some(Self::Coder),
            "reviewer" => Some(Self::Reviewer),
            "explore" => Some(Self::Explore),
            "planner" => Some(Self::Planner),
            _ => None,
        }
    }
}

/// The agent executor manages both internal and external agents.
pub struct AgentExecutor {
    /// Internal agents (built-in)
    internal_agents: HashMap<String, Box<dyn InternalAgent>>,
    /// External agents (from config)
    external_agents: AgentsConfig,
    /// Available tools registry
    tools: Arc<ToolRegistry>,
    /// Provider for LLM calls
    provider: Arc<dyn Provider>,
    /// Model to use (can be overridden per-agent)
    default_model: Option<String>,
    /// Enabled agents filter (None = all enabled)
    enabled_agents: Option<Vec<String>>,
}

impl AgentExecutor {
    /// Create a new agent executor.
    pub fn new(
        provider: Arc<dyn Provider>,
        tools: ToolRegistry,
        external_config: AgentsConfig,
        enabled_agents: Option<Vec<String>>,
        default_model: Option<String>,
    ) -> Self {
        // Create all internal agents
        let mut internal_agents: HashMap<String, Box<dyn InternalAgent>> = HashMap::new();
        for agent_type in InternalAgentType::all() {
            let agent = agent_type.create();
            internal_agents.insert(agent.name().to_string(), agent);
        }

        Self {
            internal_agents,
            external_agents: external_config,
            tools: Arc::new(tools),
            provider,
            default_model,
            enabled_agents,
        }
    }

    /// Check if an agent is enabled.
    pub fn is_enabled(&self, name: &str) -> bool {
        match &self.enabled_agents {
            None => true, // All agents enabled
            Some(list) => list.iter().any(|n| n == name),
        }
    }

    /// Get information about all available (enabled) agents.
    pub fn list_agents(&self) -> Vec<AgentInfo> {
        let mut agents = Vec::new();

        // Internal agents
        for (name, agent) in &self.internal_agents {
            if self.is_enabled(name) {
                agents.push(AgentInfo {
                    name: name.clone(),
                    description: agent.description().to_string(),
                    is_internal: true,
                    tools: agent.tool_names().iter().map(|s| s.to_string()).collect(),
                });
            }
        }

        // External agents
        for (name, def) in &self.external_agents.agents {
            if self.is_enabled(name) {
                agents.push(AgentInfo {
                    name: name.clone(),
                    description: def.description.clone(),
                    is_internal: false,
                    tools: def.tools.clone(),
                });
            }
        }

        // Sort by name
        agents.sort_by(|a, b| a.name.cmp(&b.name));
        agents
    }

    /// Check if an agent exists and is enabled.
    pub fn has_agent(&self, name: &str) -> bool {
        if !self.is_enabled(name) {
            return false;
        }
        self.internal_agents.contains_key(name) || self.external_agents.contains(name)
    }

    /// Run an agent with the given task.
    ///
    /// Returns the agent's response.
    pub async fn run(&self, agent_name: &str, task: &str) -> Result<String> {
        if !self.is_enabled(agent_name) {
            anyhow::bail!("Agent '{}' is not enabled in the current profile", agent_name);
        }

        // Try internal agent first
        if let Some(internal) = self.internal_agents.get(agent_name) {
            return self.run_internal(internal.as_ref(), task).await;
        }

        // Try external agent
        if let Some(external) = self.external_agents.get(agent_name) {
            return self.run_external(external, task).await;
        }

        anyhow::bail!("Unknown agent: {}", agent_name);
    }

    /// Run an internal agent.
    async fn run_internal(&self, agent: &dyn InternalAgent, task: &str) -> Result<String> {
        // Build tool subset for this agent
        let agent_tools = self.tools.subset_from_strs(agent.tool_names());
        let agent_tools = Arc::new(agent_tools);

        // Build config
        let config = AgentConfig::new(agent.name())
            .with_system_prompt(agent.system_prompt())
            .with_max_iterations(agent.max_iterations());

        // Build context with the task
        let context = vec![qq_core::Message::user(task)];

        // Run the agent
        let result = Agent::run_once(
            Arc::clone(&self.provider),
            agent_tools,
            config,
            context,
        )
        .await?;

        Ok(result)
    }

    /// Run an external agent.
    async fn run_external(&self, def: &AgentDefinition, task: &str) -> Result<String> {
        // Build tool subset for this agent
        let agent_tools = self.tools.subset(&def.tools);
        let agent_tools = Arc::new(agent_tools);

        // Build config
        let config = AgentConfig::new("external")
            .with_system_prompt(&def.system_prompt)
            .with_max_iterations(def.max_iterations);

        // Build context with the task
        let context = vec![qq_core::Message::user(task)];

        // Run the agent
        // Note: In the future, we could support provider/model overrides per external agent
        let result = Agent::run_once(
            Arc::clone(&self.provider),
            agent_tools,
            config,
            context,
        )
        .await?;

        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_internal_agent_types() {
        let types = InternalAgentType::all();
        assert_eq!(types.len(), 6);

        for t in types {
            let agent = t.create();
            assert!(!agent.name().is_empty());
            assert!(!agent.description().is_empty());
            assert!(!agent.system_prompt().is_empty());
        }
    }

    #[test]
    fn test_agent_type_from_name() {
        assert!(InternalAgentType::from_name("researcher").is_some());
        assert!(InternalAgentType::from_name("summarizer").is_some());
        assert!(InternalAgentType::from_name("coder").is_some());
        assert!(InternalAgentType::from_name("reviewer").is_some());
        assert!(InternalAgentType::from_name("explore").is_some());
        assert!(InternalAgentType::from_name("planner").is_some());
        assert!(InternalAgentType::from_name("unknown").is_none());
    }
}
