//! Agent tools that expose agents as callable tools for the LLM.
//!
//! Each agent becomes a tool (e.g., `Agent[researcher]`, `Agent[explore]`) that
//! the LLM can invoke. Agents can call other agents up to a maximum depth,
//! after which they only have access to base tools.

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use serde::Deserialize;

use qq_core::{AgentConfig, AgentMemory, CompletionRequest, Error, Message, PropertySchema, Provider, Role, Tool, ToolDefinition, ToolOutput, ToolParameters, ToolRegistry};

use qq_agents::{AgentDefinition, AgentsConfig, InternalAgent, InternalAgentType, DEFAULT_COMPACT_PROMPT};
use crate::agents::continuation::{execute_with_continuation, AgentExecutionResult, ContinuationConfig};
use crate::agents::InformUserTool;
use crate::event_bus::AgentEventBus;
use crate::ExecutionContext;

/// Maximum nesting depth for agent calls.
/// At depth 0, agents can call other agents. At max_depth, they only get base tools.
pub const DEFAULT_MAX_AGENT_DEPTH: u32 = 5;

// =============================================================================
// Shared agent tool helpers
// =============================================================================

/// Resolved configuration for executing an agent as a tool.
struct AgentToolConfig {
    agent_name: String,
    system_prompt: String,
    tool_names: Vec<String>,
    max_turns: usize,
    tool_limits: Option<HashMap<String, usize>>,
    compact_prompt: String,
    is_read_only: bool,
}

/// Build the standard agent tool definition.
///
/// Both `InternalAgentTool` and `ExternalAgentTool` use identical parameter schemas;
/// only the name and description differ.
fn agent_tool_definition(name: &str, description: &str) -> ToolDefinition {
    let task_description = "A high-level goal or question for the agent. \
        Describe WHAT you want to achieve, not HOW to do it. \
        The agent autonomously decides which tools to use and how to accomplish the task.";

    ToolDefinition::new(name, description).with_parameters(
        ToolParameters::new()
            .add_property("task", PropertySchema::string(task_description), true)
            .add_property(
                "new_instance",
                PropertySchema::boolean(
                    "Start with a fresh context, discarding previous conversation history. \
                     Default: false (reuses history from prior calls)."
                ).with_default(serde_json::Value::Bool(false)),
                false,
            ),
    )
}

/// Shared execution logic for both internal and external agent tools.
///
/// Handles scope management, tool setup, agent execution, memory compaction,
/// and result formatting.
#[allow(clippy::too_many_arguments)]
async fn execute_agent(
    config: AgentToolConfig,
    task: String,
    new_instance: bool,
    base_tools: &Arc<ToolRegistry>,
    provider: &Arc<dyn Provider>,
    external_agents: &AgentsConfig,
    enabled_agents: &Option<Vec<String>>,
    current_depth: u32,
    max_depth: u32,
    execution_context: &Option<ExecutionContext>,
    event_bus: &Option<AgentEventBus>,
    agent_memory: &Option<AgentMemory>,
    scope: &str,
    task_store: &Option<Arc<qq_tools::TaskStore>>,
) -> Result<ToolOutput, Error> {
    let child_scope = format!("{}/{}", scope, config.agent_name);

    // Clear scope if fresh instance requested
    if new_instance {
        if let Some(ref memory) = agent_memory {
            memory.clear_scope(&child_scope).await;
        }
    }

    // Load prior conversation history
    let prior_history = if let Some(ref memory) = agent_memory {
        memory.get_messages(&child_scope).await
    } else {
        Vec::new()
    };

    // Build tools for this agent: start with base tools it needs
    let mut agent_tools = base_tools.subset(&config.tool_names);

    // If not at max depth, add agent tools so this agent can call other agents
    let next_depth = current_depth + 1;
    if next_depth < max_depth {
        let nested_agent_tools = create_agent_tools(
            base_tools,
            Arc::clone(provider),
            external_agents,
            enabled_agents,
            next_depth,
            max_depth,
            execution_context.clone(),
            event_bus.clone(),
            agent_memory.clone(),
            child_scope.clone(),
            task_store.clone(),
        );
        for tool in nested_agent_tools {
            agent_tools.register(tool);
        }
    }

    // Add inform_user tool if event bus is available
    if let Some(ref event_bus) = event_bus {
        agent_tools.register(Arc::new(InformUserTool::new(
            event_bus.clone(),
            &config.agent_name,
        )));
    }

    let agent_tools = Arc::new(agent_tools);

    let has_sub_agents = next_depth < max_depth;
    let has_tools = !config.tool_names.is_empty();
    let has_inform_user = event_bus.is_some();
    let has_task_tracking = config.tool_names.iter().any(|n| n == "update_my_task");
    let has_bash = config.tool_names.iter().any(|n| n == "bash");
    let has_preferences = config.tool_names.iter().any(|n| n == "read_preference" || n == "update_preference");

    let preamble = qq_agents::generate_preamble(&qq_agents::PreambleContext {
        has_tools,
        has_sub_agents,
        has_inform_user,
        has_task_tracking,
        has_preferences,
        has_bash,
        is_read_only: config.is_read_only,
    });
    let full_prompt = format!("{}\n\n---\n\n{}", preamble, config.system_prompt);

    // Prepend task board for non-PM agents
    let augmented_task = if config.agent_name != "pm" {
        if let Some(ref store) = task_store {
            if let Some(board) = store.format_board() {
                format!("{}\n\n---\n\n{}", board, task)
            } else {
                task
            }
        } else {
            task
        }
    } else {
        task
    };

    let mut agent_cfg = AgentConfig::new(config.agent_name.as_str())
        .with_system_prompt(&full_prompt)
        .with_max_turns(config.max_turns);

    if let Some(limits) = config.tool_limits {
        agent_cfg = agent_cfg.with_tool_limits(limits);
    }

    // Create progress handler if event bus is available
    let progress = event_bus.as_ref().map(|bus| bus.create_handler());

    // Use continuation wrapper for execution
    let continuation_config = ContinuationConfig::default();
    let result = execute_with_continuation(
        Arc::clone(provider),
        agent_tools,
        agent_cfg,
        augmented_task,
        progress,
        continuation_config,
        event_bus.as_ref(),
        prior_history,
    )
    .await;

    // Store results back to memory, with LLM compaction
    match &result {
        AgentExecutionResult::Success { messages, .. }
        | AgentExecutionResult::MaxContinuationsReached { messages, .. } => {
            if let Some(ref memory) = agent_memory {
                let tool_calls: u32 = messages
                    .iter()
                    .map(|m| m.tool_calls.len() as u32)
                    .sum();

                let compacted =
                    compact_agent_messages(provider, messages.clone(), &config.compact_prompt).await;

                memory
                    .store_messages(&child_scope, compacted, tool_calls)
                    .await;
            }
        }
        AgentExecutionResult::Error(_) => {} // don't store on error
    }

    match result {
        AgentExecutionResult::Success { content, .. } => Ok(ToolOutput::success(content)),
        AgentExecutionResult::MaxContinuationsReached {
            partial_result,
            continuations,
            ..
        } => Ok(ToolOutput::success(format!(
            "Task partially completed after {} continuations.\n\n{}",
            continuations, partial_result
        ))),
        AgentExecutionResult::Error(e) => {
            Ok(ToolOutput::error(format!("Agent error: {}", e)))
        }
    }
}

// =============================================================================
// InternalAgentTool
// =============================================================================

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
    /// Scoped agent memory for persistent instance state
    agent_memory: Option<AgentMemory>,
    /// Scope path for this tool (e.g., "pm" at depth 0)
    scope: String,
    /// Task store for task board injection into sub-agent context
    task_store: Option<Arc<qq_tools::TaskStore>>,
}

impl InternalAgentTool {
    #[allow(clippy::too_many_arguments)]
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
        agent_memory: Option<AgentMemory>,
        scope: String,
        task_store: Option<Arc<qq_tools::TaskStore>>,
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
            agent_memory,
            scope,
            task_store,
        }
    }
}

#[derive(Deserialize)]
struct AgentArgs {
    task: String,
    #[serde(default)]
    new_instance: bool,
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
        agent_tool_definition(self.name(), self.agent.tool_description())
    }

    async fn execute(&self, arguments: serde_json::Value) -> Result<ToolOutput, Error> {
        let args: AgentArgs = serde_json::from_value(arguments)
            .map_err(|e| Error::tool(&self.tool_name, format!("Invalid arguments: {}", e)))?;

        // Push agent context
        if let Some(ref ctx) = self.execution_context {
            ctx.push_agent(self.agent.name()).await;
        }

        // Resolve config: builtin overrides > agent defaults
        let max_turns = self
            .external_agents
            .get_builtin_max_turns(self.agent.name())
            .unwrap_or_else(|| self.agent.max_turns());

        let tool_limits = if let Some(limits) = self.external_agents.get_builtin_tool_limits(self.agent.name()) {
            Some(limits.clone())
        } else {
            self.agent.tool_limits()
        };

        let compact_prompt = self
            .external_agents
            .get_builtin_compact_prompt(self.agent.name())
            .unwrap_or_else(|| self.agent.compact_prompt())
            .to_string();

        let mut tool_names: Vec<String> = self.agent.tool_names().iter().map(|s| s.to_string()).collect();

        // Auto-inject bash + mount_external unless disabled via config
        let no_bash = self.external_agents.get_builtin_no_bash(self.agent.name());
        if !no_bash && !tool_names.is_empty() {
            if !tool_names.iter().any(|n| n == "bash") {
                tool_names.push("bash".to_string());
            }
            if !tool_names.iter().any(|n| n == "mount_external") {
                tool_names.push("mount_external".to_string());
            }
        }

        let config = AgentToolConfig {
            agent_name: self.agent.name().to_string(),
            system_prompt: self.agent.system_prompt().to_string(),
            tool_names,
            max_turns,
            tool_limits,
            compact_prompt,
            is_read_only: self.agent.is_read_only(),
        };

        let output = execute_agent(
            config,
            args.task,
            args.new_instance,
            &self.base_tools,
            &self.provider,
            &self.external_agents,
            &self.enabled_agents,
            self.current_depth,
            self.max_depth,
            &self.execution_context,
            &self.event_bus,
            &self.agent_memory,
            &self.scope,
            &self.task_store,
        )
        .await;

        // Pop agent context
        if let Some(ref ctx) = self.execution_context {
            ctx.pop().await;
        }

        output
    }
}

// =============================================================================
// ExternalAgentTool
// =============================================================================

/// A tool that wraps an external (config-defined) agent.
pub struct ExternalAgentTool {
    /// Tool name (e.g., "Agent[doc_researcher]")
    tool_name: String,
    /// Agent name
    agent_name: String,
    /// Agent definition from config
    agent_def: AgentDefinition,
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
    /// Scoped agent memory for persistent instance state
    agent_memory: Option<AgentMemory>,
    /// Scope path for this tool
    scope: String,
    /// Task store for task board injection into sub-agent context
    task_store: Option<Arc<qq_tools::TaskStore>>,
}

impl ExternalAgentTool {
    #[allow(clippy::too_many_arguments)]
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
        agent_memory: Option<AgentMemory>,
        scope: String,
        task_store: Option<Arc<qq_tools::TaskStore>>,
    ) -> Self {
        let tool_name = format!("Agent[{}]", name);

        Self {
            tool_name,
            agent_name: name.to_string(),
            agent_def: definition,
            base_tools: Arc::new(base_tools.clone()),
            provider,
            external_agents,
            enabled_agents,
            current_depth,
            max_depth,
            execution_context,
            event_bus,
            agent_memory,
            scope,
            task_store,
        }
    }
}

#[async_trait]
impl Tool for ExternalAgentTool {
    fn name(&self) -> &str {
        &self.tool_name
    }

    fn description(&self) -> &str {
        &self.agent_def.description
    }

    fn definition(&self) -> ToolDefinition {
        let desc = self.agent_def.tool_description
            .as_deref()
            .unwrap_or(&self.agent_def.description);
        agent_tool_definition(self.name(), desc)
    }

    async fn execute(&self, arguments: serde_json::Value) -> Result<ToolOutput, Error> {
        let args: AgentArgs = serde_json::from_value(arguments)
            .map_err(|e| Error::tool(&self.tool_name, format!("Invalid arguments: {}", e)))?;

        // Push agent context
        if let Some(ref ctx) = self.execution_context {
            ctx.push_agent(&self.agent_name).await;
        }

        let tool_limits = if self.agent_def.tool_limits.is_empty() {
            None
        } else {
            Some(self.agent_def.tool_limits.clone())
        };

        let mut tool_names = self.agent_def.tools.clone();

        // Auto-inject bash + mount_external unless disabled via config
        if !self.agent_def.no_bash && !tool_names.is_empty() {
            if !tool_names.iter().any(|n| n == "bash") {
                tool_names.push("bash".to_string());
            }
            if !tool_names.iter().any(|n| n == "mount_external") {
                tool_names.push("mount_external".to_string());
            }
        }

        let config = AgentToolConfig {
            agent_name: self.agent_name.clone(),
            system_prompt: self.agent_def.system_prompt.clone(),
            tool_names,
            max_turns: self.agent_def.max_turns,
            tool_limits,
            compact_prompt: self.agent_def.compact_prompt
                .as_deref()
                .unwrap_or(DEFAULT_COMPACT_PROMPT)
                .to_string(),
            is_read_only: self.agent_def.read_only,
        };

        let output = execute_agent(
            config,
            args.task,
            args.new_instance,
            &self.base_tools,
            &self.provider,
            &self.external_agents,
            &self.enabled_agents,
            self.current_depth,
            self.max_depth,
            &self.execution_context,
            &self.event_bus,
            &self.agent_memory,
            &self.scope,
            &self.task_store,
        )
        .await;

        // Pop agent context
        if let Some(ref ctx) = self.execution_context {
            ctx.pop().await;
        }

        output
    }
}

// =============================================================================
// create_agent_tools
// =============================================================================

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
/// * `agent_memory` - Optional scoped agent memory for persistent instance state
/// * `scope` - Current scope path (e.g., "pm", "pm/coder")
#[allow(clippy::too_many_arguments)]
pub fn create_agent_tools(
    base_tools: &ToolRegistry,
    provider: Arc<dyn Provider>,
    external_agents: &AgentsConfig,
    enabled_agents: &Option<Vec<String>>,
    current_depth: u32,
    max_depth: u32,
    execution_context: Option<ExecutionContext>,
    event_bus: Option<AgentEventBus>,
    agent_memory: Option<AgentMemory>,
    scope: String,
    task_store: Option<Arc<qq_tools::TaskStore>>,
) -> Vec<Arc<dyn Tool>> {
    let mut tools: Vec<Arc<dyn Tool>> = Vec::new();

    let is_enabled = |name: &str| -> bool {
        match enabled_agents {
            None => true,
            Some(list) => list.iter().any(|n| n == name),
        }
    };

    // Create tools for internal agents
    for agent_type in InternalAgentType::all() {
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
                agent_memory.clone(),
                scope.clone(),
                task_store.clone(),
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
                agent_memory.clone(),
                scope.clone(),
                task_store.clone(),
            )));
        }
    }

    tools
}

// =============================================================================
// Agent memory compaction
// =============================================================================

/// Threshold in bytes at which agent memory compaction is triggered.
/// Set below the 200KB trim_to_budget safety net to allow summarization before truncation.
const AGENT_COMPACT_THRESHOLD_BYTES: usize = 150_000;

/// Minimum number of messages required before compaction is attempted.
/// Must have at least preserve_recent + 2 messages to have anything to compact.
const AGENT_COMPACT_PRESERVE_RECENT: usize = 6;

/// Find a safe point to split agent messages that doesn't break tool call sequences.
///
/// Walks backwards from `target` to find a position where splitting won't
/// orphan tool results from their corresponding assistant tool call message.
fn find_safe_compact_point(messages: &[Message], target: usize) -> usize {
    let mut end = target;

    while end > 0 {
        let msg = &messages[end - 1];

        // If the message is a tool result, we're inside a sequence - keep walking back
        if msg.tool_call_id.is_some() {
            end -= 1;
            continue;
        }

        // If it's an assistant with tool_calls, the results come after - exclude this too
        if msg.role == Role::Assistant && !msg.tool_calls.is_empty() {
            end -= 1;
            continue;
        }

        // Safe position found
        break;
    }

    end
}

/// Attempt LLM-summarized compaction of agent messages.
///
/// If the total message bytes exceed `AGENT_COMPACT_THRESHOLD_BYTES`, older messages
/// are summarized by the LLM using the provided `compact_prompt`, preserving recent
/// messages. On failure, returns the original messages unchanged (the downstream
/// `trim_to_budget` serves as safety net).
async fn compact_agent_messages(
    provider: &Arc<dyn Provider>,
    messages: Vec<Message>,
    compact_prompt: &str,
) -> Vec<Message> {
    let total_bytes: usize = messages.iter().map(|m| m.byte_count()).sum();

    if total_bytes <= AGENT_COMPACT_THRESHOLD_BYTES {
        return messages;
    }

    if messages.len() <= AGENT_COMPACT_PRESERVE_RECENT + 2 {
        return messages;
    }

    let preserve_count = AGENT_COMPACT_PRESERVE_RECENT.min(messages.len());
    let compact_end = messages.len() - preserve_count;

    let safe_end = find_safe_compact_point(&messages, compact_end);
    if safe_end <= 1 {
        return messages;
    }

    tracing::info!(
        total_bytes = total_bytes,
        threshold = AGENT_COMPACT_THRESHOLD_BYTES,
        messages_to_compact = safe_end,
        messages_to_preserve = messages.len() - safe_end,
        "Agent memory compaction triggered"
    );

    // Build summarization request from older messages
    let mut summary_messages: Vec<Message> = messages[..safe_end].to_vec();
    summary_messages.push(Message::user(compact_prompt));

    let request = CompletionRequest::new(summary_messages);
    match provider.complete(request).await {
        Ok(response) => {
            let summary = response.message.content.to_string_lossy();
            if summary.is_empty() {
                tracing::warn!("Agent compaction returned empty summary, keeping original messages");
                return messages;
            }

            // Build compacted message list: summary + recent messages
            let mut compacted = Vec::new();
            let summary_text = format!("## Prior Session Summary\n\n{}", summary);
            compacted.push(Message::system(summary_text.as_str()));
            compacted.extend_from_slice(&messages[safe_end..]);

            let new_bytes: usize = compacted.iter().map(|m| m.byte_count()).sum();
            tracing::info!(
                old_messages = messages.len(),
                new_messages = compacted.len(),
                old_bytes = total_bytes,
                new_bytes = new_bytes,
                bytes_freed = total_bytes.saturating_sub(new_bytes),
                "Agent memory compaction complete"
            );

            compacted
        }
        Err(e) => {
            tracing::warn!(error = %e, "Agent memory compaction failed, falling back to trim_to_budget");
            messages
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_agent_tool_definition_structure() {
        let def = agent_tool_definition("Agent[test]", "Test agent description");
        assert_eq!(def.name, "Agent[test]");
        assert_eq!(def.description, "Test agent description");
        assert!(def.parameters.required.contains(&"task".to_string()));
        assert!(!def.parameters.required.contains(&"new_instance".to_string()));
        assert!(def.parameters.properties.contains_key("task"));
        assert!(def.parameters.properties.contains_key("new_instance"));
    }
}
