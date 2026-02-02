//! Agent tools that expose agents as callable tools for the LLM.
//!
//! Each agent becomes a tool (e.g., `Agent[researcher]`, `Agent[explore]`) that
//! the LLM can invoke. Agents can call other agents up to a maximum depth,
//! after which they only have access to base tools.

use std::sync::Arc;

use async_trait::async_trait;
use serde::Deserialize;

use qq_core::{Agent, AgentConfig, Error, PropertySchema, Provider, Tool, ToolDefinition, ToolOutput, ToolParameters, ToolRegistry};

use super::InternalAgent;
use crate::config::{AgentDefinition, AgentsConfig};
use crate::event_bus::AgentEventBus;
use crate::ExecutionContext;

/// Maximum nesting depth for agent calls.
/// At depth 0, agents can call other agents. At max_depth, they only get base tools.
pub const DEFAULT_MAX_AGENT_DEPTH: u32 = 3;

/// A tool that wraps an internal agent.
pub struct InternalAgentTool {
    /// Tool name (e.g., "Agent[researcher]")
    tool_name: String,
    /// The internal agent
    agent: Box<dyn InternalAgent>,
    /// Base tools (filesystem, memory, web) - used to build agent's tool set
    base_tools: Arc<ToolRegistry>,
    /// Provider for LLM calls
    provider: Arc<dyn Provider>,
    /// External agents config (for creating nested agent tools)
    external_agents: AgentsConfig,
    /// Enabled agents filter
    enabled_agents: Option<Vec<String>>,
    /// Current nesting depth (0 = called from main chat)
    current_depth: u32,
    /// Maximum allowed depth
    max_depth: u32,
    /// Execution context for tracking the call stack
    execution_context: Option<ExecutionContext>,
    /// Event bus for progress reporting
    event_bus: Option<AgentEventBus>,
}

impl InternalAgentTool {
    pub fn new(
        agent: Box<dyn InternalAgent>,
        base_tools: &ToolRegistry,
        provider: Arc<dyn Provider>,
        external_agents: AgentsConfig,
        enabled_agents: Option<Vec<String>>,
        current_depth: u32,
        max_depth: u32,
        execution_context: Option<ExecutionContext>,
        event_bus: Option<AgentEventBus>,
    ) -> Self {
        let tool_name = format!("Agent[{}]", agent.name());

        Self {
            tool_name,
            agent,
            base_tools: Arc::new(base_tools.clone()),
            provider,
            external_agents,
            enabled_agents,
            current_depth,
            max_depth,
            execution_context,
            event_bus,
        }
    }

    /// Get the agent name without the wrapper (for display purposes)
    pub fn agent_name(&self) -> &str {
        self.agent.name()
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
        // Build descriptions that guide proper agent usage:
        // - Agents are autonomous workers, not simple tools
        // - Give high-level goals, not mechanical commands
        // - The agent decides HOW to accomplish the task
        let description = match self.agent.name() {
            "explore" => concat!(
                "Autonomous agent for understanding codebases and file structures. ",
                "Give it a GOAL or QUESTION, not a mechanical command. ",
                "GOOD: 'Find where user authentication is implemented' or 'Understand the project architecture'. ",
                "BAD: 'List files in src/' or 'Read main.rs'. ",
                "The agent will systematically explore, read files, and synthesize an answer."
            ).to_string(),
            "researcher" => concat!(
                "Autonomous agent for web research. ",
                "Give it a RESEARCH QUESTION, not a URL to fetch. ",
                "GOOD: 'What are best practices for Rust error handling?' or 'Compare React vs Vue for this use case'. ",
                "BAD: 'Fetch https://example.com'. ",
                "The agent will search, read multiple sources, and synthesize findings."
            ).to_string(),
            "coder" => concat!(
                "Autonomous agent for writing or modifying code. ",
                "Give it a GOAL describing what you want built or changed. ",
                "GOOD: 'Add input validation to the login form' or 'Refactor the database module to use connection pooling'. ",
                "BAD: 'Write a function called validate_input'. ",
                "The agent will read existing code for context, then implement following project patterns."
            ).to_string(),
            "reviewer" => concat!(
                "Autonomous agent for code review. ",
                "Give it CODE or a FILE PATH and ask for specific feedback. ",
                "GOOD: 'Review src/auth.rs for security issues' or 'Check this PR for potential bugs'. ",
                "The agent will thoroughly analyze and provide actionable feedback."
            ).to_string(),
            "summarizer" => concat!(
                "Autonomous agent for summarizing content. ",
                "Give it CONTENT and specify what aspects to focus on. ",
                "GOOD: 'Summarize this error log, focusing on the root cause' or 'Summarize this document's key decisions'. ",
                "The agent will produce a concise, accurate summary."
            ).to_string(),
            "planner" => concat!(
                "Autonomous agent for breaking down complex tasks. ",
                "Give it a GOAL and ask for a plan. ",
                "GOOD: 'Plan how to migrate from SQLite to PostgreSQL' or 'Plan the implementation of user roles'. ",
                "The agent will produce clear, actionable steps."
            ).to_string(),
            _ => self.agent.description().to_string(),
        };

        // The task parameter description also guides proper usage
        let task_description = concat!(
            "A high-level goal or question for the agent. ",
            "Describe WHAT you want to achieve, not HOW to do it. ",
            "The agent autonomously decides which tools to use and how to accomplish the task."
        );

        ToolDefinition::new(self.name(), description).with_parameters(
            ToolParameters::new()
                .add_property(
                    "task",
                    PropertySchema::string(task_description),
                    true,
                ),
        )
    }

    async fn execute(&self, arguments: serde_json::Value) -> Result<ToolOutput, Error> {
        let args: AgentArgs = serde_json::from_value(arguments)
            .map_err(|e| Error::tool(&self.tool_name, format!("Invalid arguments: {}", e)))?;

        // Push agent context
        if let Some(ref ctx) = self.execution_context {
            ctx.push_agent(self.agent.name()).await;
        }

        // Build tools for this agent: start with base tools it needs
        let mut agent_tools = self.base_tools.subset_from_strs(self.agent.tool_names());

        // If not at max depth, add agent tools so this agent can call other agents
        let next_depth = self.current_depth + 1;
        if next_depth < self.max_depth {
            let nested_agent_tools = create_agent_tools(
                &self.base_tools,
                Arc::clone(&self.provider),
                &self.external_agents,
                &self.enabled_agents,
                next_depth,
                self.max_depth,
                self.execution_context.clone(),
                self.event_bus.clone(),
            );
            for tool in nested_agent_tools {
                agent_tools.register(tool);
            }
        }

        let agent_tools = Arc::new(agent_tools);

        let config = AgentConfig::new(self.agent.name())
            .with_system_prompt(self.agent.system_prompt())
            .with_max_iterations(self.agent.max_iterations());

        // Context is ONLY the task - no chat history, no chat system prompt
        let context = vec![qq_core::Message::user(args.task.as_str())];

        // Create progress handler if event bus is available
        let progress = self.event_bus.as_ref().map(|bus| bus.create_handler());

        let result = match Agent::run_once_with_progress(
            Arc::clone(&self.provider),
            agent_tools,
            config,
            context,
            progress,
        ).await {
            Ok(result) => Ok(ToolOutput::success(result)),
            Err(e) => Ok(ToolOutput::error(format!("Agent error: {}", e))),
        };

        // Pop agent context
        if let Some(ref ctx) = self.execution_context {
            ctx.pop().await;
        }

        result
    }
}

/// A tool that wraps an external (config-defined) agent.
pub struct ExternalAgentTool {
    /// Tool name (e.g., "Agent[doc_researcher]")
    tool_name: String,
    /// Agent name
    agent_name: String,
    /// Agent definition from config
    definition: AgentDefinition,
    /// Base tools (filesystem, memory, web) - used to build agent's tool set
    base_tools: Arc<ToolRegistry>,
    /// Provider for LLM calls
    provider: Arc<dyn Provider>,
    /// External agents config (for creating nested agent tools)
    external_agents: AgentsConfig,
    /// Enabled agents filter
    enabled_agents: Option<Vec<String>>,
    /// Current nesting depth
    current_depth: u32,
    /// Maximum allowed depth
    max_depth: u32,
    /// Execution context for tracking the call stack
    execution_context: Option<ExecutionContext>,
    /// Event bus for progress reporting
    event_bus: Option<AgentEventBus>,
}

impl ExternalAgentTool {
    pub fn new(
        name: &str,
        definition: AgentDefinition,
        base_tools: &ToolRegistry,
        provider: Arc<dyn Provider>,
        external_agents: AgentsConfig,
        enabled_agents: Option<Vec<String>>,
        current_depth: u32,
        max_depth: u32,
        execution_context: Option<ExecutionContext>,
        event_bus: Option<AgentEventBus>,
    ) -> Self {
        let tool_name = format!("Agent[{}]", name);

        Self {
            tool_name,
            agent_name: name.to_string(),
            definition,
            base_tools: Arc::new(base_tools.clone()),
            provider,
            external_agents,
            enabled_agents,
            current_depth,
            max_depth,
            execution_context,
            event_bus,
        }
    }

    /// Get the agent name without the wrapper (for display purposes)
    pub fn agent_name(&self) -> &str {
        &self.agent_name
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

        // Push agent context
        if let Some(ref ctx) = self.execution_context {
            ctx.push_agent(&self.agent_name).await;
        }

        // Build tools for this agent: start with base tools it needs
        let mut agent_tools = self.base_tools.subset(&self.definition.tools);

        // If not at max depth, add agent tools so this agent can call other agents
        let next_depth = self.current_depth + 1;
        if next_depth < self.max_depth {
            let nested_agent_tools = create_agent_tools(
                &self.base_tools,
                Arc::clone(&self.provider),
                &self.external_agents,
                &self.enabled_agents,
                next_depth,
                self.max_depth,
                self.execution_context.clone(),
                self.event_bus.clone(),
            );
            for tool in nested_agent_tools {
                agent_tools.register(tool);
            }
        }

        let agent_tools = Arc::new(agent_tools);

        let config = AgentConfig::new(self.agent_name.as_str())
            .with_system_prompt(&self.definition.system_prompt)
            .with_max_iterations(self.definition.max_iterations);

        // Context is ONLY the task - no chat history, no chat system prompt
        let context = vec![qq_core::Message::user(args.task.as_str())];

        // Create progress handler if event bus is available
        let progress = self.event_bus.as_ref().map(|bus| bus.create_handler());

        let result = match Agent::run_once_with_progress(
            Arc::clone(&self.provider),
            agent_tools,
            config,
            context,
            progress,
        ).await {
            Ok(result) => Ok(ToolOutput::success(result)),
            Err(e) => Ok(ToolOutput::error(format!("Agent error: {}", e))),
        };

        // Pop agent context
        if let Some(ref ctx) = self.execution_context {
            ctx.pop().await;
        }

        result
    }
}

/// Create agent tools for all enabled agents.
///
/// Returns a vector of tools that can be registered with the main tool registry.
///
/// # Arguments
/// * `base_tools` - Base tools (filesystem, memory, web) available to agents
/// * `provider` - LLM provider for agent calls
/// * `external_agents` - External agent definitions from config
/// * `enabled_agents` - Filter for which agents are enabled (None = all)
/// * `current_depth` - Current nesting depth (0 = top level)
/// * `max_depth` - Maximum allowed nesting depth
/// * `execution_context` - Optional context for tracking execution stack
/// * `event_bus` - Optional event bus for progress reporting
pub fn create_agent_tools(
    base_tools: &ToolRegistry,
    provider: Arc<dyn Provider>,
    external_agents: &AgentsConfig,
    enabled_agents: &Option<Vec<String>>,
    current_depth: u32,
    max_depth: u32,
    execution_context: Option<ExecutionContext>,
    event_bus: Option<AgentEventBus>,
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
                external_agents.clone(),
                enabled_agents.clone(),
                current_depth,
                max_depth,
                execution_context.clone(),
                event_bus.clone(),
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
                external_agents.clone(),
                enabled_agents.clone(),
                current_depth,
                max_depth,
                execution_context.clone(),
                event_bus.clone(),
            )));
        }
    }

    tools
}
