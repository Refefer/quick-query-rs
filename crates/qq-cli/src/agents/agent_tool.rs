//! Agent tools that expose agents as callable tools for the LLM.
//!
//! Each agent becomes a tool (e.g., `ask_researcher`, `ask_explore`) that
//! the LLM can invoke. Agents only have access to their specified tools,
//! NOT to other agent tools, preventing recursive loops.

use std::sync::Arc;

use async_trait::async_trait;
use serde::Deserialize;

use qq_core::{Agent, AgentConfig, Error, PropertySchema, Provider, Tool, ToolDefinition, ToolOutput, ToolParameters, ToolRegistry};

use super::InternalAgent;
use crate::config::AgentDefinition;

/// A tool that wraps an internal agent.
pub struct InternalAgentTool {
    /// Tool name (e.g., "ask_researcher")
    tool_name: String,
    /// The internal agent
    agent: Box<dyn InternalAgent>,
    /// Tools available to this agent (subset, no agent tools)
    agent_tools: Arc<ToolRegistry>,
    /// Provider for LLM calls
    provider: Arc<dyn Provider>,
}

impl InternalAgentTool {
    pub fn new(
        agent: Box<dyn InternalAgent>,
        all_tools: &ToolRegistry,
        provider: Arc<dyn Provider>,
    ) -> Self {
        let tool_name = format!("ask_{}", agent.name());

        // Create tool subset with ONLY the agent's specified tools
        // This prevents agents from calling other agents
        let agent_tools = Arc::new(all_tools.subset_from_strs(agent.tool_names()));

        Self {
            tool_name,
            agent,
            agent_tools,
            provider,
        }
    }
}

#[derive(Deserialize)]
struct AgentArgs {
    task: String,
}

#[async_trait]
impl Tool for InternalAgentTool {
    fn name(&self) -> &str {
        &self.tool_name
    }

    fn description(&self) -> &str {
        self.agent.description()
    }

    fn definition(&self) -> ToolDefinition {
        // Build a compelling description that encourages the LLM to use agents
        let description = match self.agent.name() {
            "explore" => "RECOMMENDED for filesystem questions. Systematically explores directories and files to answer questions about codebases, project structure, or file contents. More thorough than calling list_files directly.".to_string(),
            "researcher" => "RECOMMENDED for web research. Fetches and synthesizes information from multiple web sources to provide comprehensive answers.".to_string(),
            "coder" => "RECOMMENDED for code tasks. Reads existing code for context, then writes or modifies code following established patterns.".to_string(),
            "reviewer" => "RECOMMENDED for code review. Thoroughly analyzes code for bugs, security issues, and improvements.".to_string(),
            "summarizer" => "Summarizes long content into concise, accurate summaries.".to_string(),
            "planner" => "Breaks down complex tasks into clear, actionable steps.".to_string(),
            _ => self.agent.description().to_string(),
        };

        ToolDefinition::new(self.name(), description).with_parameters(
            ToolParameters::new()
                .add_property(
                    "task",
                    PropertySchema::string("The task or question. Be specific about what you want to know or accomplish."),
                    true,
                ),
        )
    }

    async fn execute(&self, arguments: serde_json::Value) -> Result<ToolOutput, Error> {
        let args: AgentArgs = serde_json::from_value(arguments)
            .map_err(|e| Error::tool(&self.tool_name, format!("Invalid arguments: {}", e)))?;

        // Show agent execution start (always visible)
        eprintln!(
            "[Agent '{}' running with tools: {}]",
            self.agent.name(),
            self.agent_tools.names().join(", ")
        );

        let config = AgentConfig::new(self.agent.name())
            .with_system_prompt(self.agent.system_prompt())
            .with_max_iterations(self.agent.max_iterations());

        // Context is ONLY the task - no chat history, no chat system prompt
        let context = vec![qq_core::Message::user(args.task.as_str())];

        match Agent::run_once(
            Arc::clone(&self.provider),
            Arc::clone(&self.agent_tools),
            config,
            context,
        ).await {
            Ok(result) => Ok(ToolOutput::success(result)),
            Err(e) => Ok(ToolOutput::error(format!("Agent error: {}", e))),
        }
    }
}

/// A tool that wraps an external (config-defined) agent.
pub struct ExternalAgentTool {
    /// Tool name (e.g., "ask_doc_researcher")
    tool_name: String,
    /// Agent name
    agent_name: String,
    /// Agent definition from config
    definition: AgentDefinition,
    /// Tools available to this agent (subset, no agent tools)
    agent_tools: Arc<ToolRegistry>,
    /// Provider for LLM calls
    provider: Arc<dyn Provider>,
}

impl ExternalAgentTool {
    pub fn new(
        name: &str,
        definition: AgentDefinition,
        all_tools: &ToolRegistry,
        provider: Arc<dyn Provider>,
    ) -> Self {
        let tool_name = format!("ask_{}", name);

        // Create tool subset with ONLY the agent's specified tools
        let agent_tools = Arc::new(all_tools.subset(&definition.tools));

        Self {
            tool_name,
            agent_name: name.to_string(),
            definition,
            agent_tools,
            provider,
        }
    }
}

#[async_trait]
impl Tool for ExternalAgentTool {
    fn name(&self) -> &str {
        &self.tool_name
    }

    fn description(&self) -> &str {
        &self.definition.description
    }

    fn definition(&self) -> ToolDefinition {
        ToolDefinition::new(self.name(), &self.definition.description).with_parameters(
            ToolParameters::new()
                .add_property(
                    "task",
                    PropertySchema::string("The task or question. Be specific about what you want to know or accomplish."),
                    true,
                ),
        )
    }

    async fn execute(&self, arguments: serde_json::Value) -> Result<ToolOutput, Error> {
        let args: AgentArgs = serde_json::from_value(arguments)
            .map_err(|e| Error::tool(&self.tool_name, format!("Invalid arguments: {}", e)))?;

        // Show agent execution start (always visible)
        eprintln!(
            "[Agent '{}' running with tools: {}]",
            self.agent_name,
            self.agent_tools.names().join(", ")
        );

        let config = AgentConfig::new(self.agent_name.as_str())
            .with_system_prompt(&self.definition.system_prompt)
            .with_max_iterations(self.definition.max_iterations);

        // Context is ONLY the task - no chat history, no chat system prompt
        let context = vec![qq_core::Message::user(args.task.as_str())];

        match Agent::run_once(
            Arc::clone(&self.provider),
            Arc::clone(&self.agent_tools),
            config,
            context,
        ).await {
            Ok(result) => Ok(ToolOutput::success(result)),
            Err(e) => Ok(ToolOutput::error(format!("Agent error: {}", e))),
        }
    }
}

/// Create agent tools for all enabled agents.
///
/// Returns a vector of tools that can be registered with the main tool registry.
/// The `base_tools` should NOT include any agent tools to prevent recursion.
pub fn create_agent_tools(
    base_tools: &ToolRegistry,
    provider: Arc<dyn Provider>,
    external_agents: &crate::config::AgentsConfig,
    enabled_agents: &Option<Vec<String>>,
) -> Vec<Arc<dyn Tool>> {
    let mut tools: Vec<Arc<dyn Tool>> = Vec::new();

    let is_enabled = |name: &str| -> bool {
        match enabled_agents {
            None => true,
            Some(list) => list.iter().any(|n| n == name),
        }
    };

    // Create tools for internal agents
    for agent_type in super::InternalAgentType::all() {
        let agent = agent_type.create();
        let name = agent.name();

        if is_enabled(name) {
            tools.push(Arc::new(InternalAgentTool::new(
                agent,
                base_tools,
                Arc::clone(&provider),
            )));
        }
    }

    // Create tools for external agents
    for (name, def) in &external_agents.agents {
        if is_enabled(name) {
            tools.push(Arc::new(ExternalAgentTool::new(
                name,
                def.clone(),
                base_tools,
                Arc::clone(&provider),
            )));
        }
    }

    tools
}
