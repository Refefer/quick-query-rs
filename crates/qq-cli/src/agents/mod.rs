//! Agent system for quick-query CLI.
//!
//! This module provides:
//! - Re-exports from qq-agents crate (agent definitions and traits)
//! - Agent tools that expose agents as callable tools for the LLM
//! - AgentExecutor for manual agent invocation via chat commands

pub mod agent_tool;

pub use agent_tool::{create_agent_tools, DEFAULT_MAX_AGENT_DEPTH};

// Re-export everything from qq-agents
pub use qq_agents::{AgentDefinition, AgentInfo, AgentsConfig, InternalAgent, InternalAgentType};

use std::collections::HashMap;
use std::sync::Arc;

use anyhow::Result;

use qq_core::{Agent, AgentConfig, Provider, ToolRegistry};

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
    #[allow(dead_code)]
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
            .with_max_turns(agent.max_turns());

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
            .with_max_turns(def.max_turns);

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
}
