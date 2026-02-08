//! Agent tools that expose agents as callable tools for the LLM.
//!
//! Each agent becomes a tool (e.g., `Agent[researcher]`, `Agent[explore]`) that
//! the LLM can invoke. Agents can call other agents up to a maximum depth,
//! after which they only have access to base tools.

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
    /// Scope path for this tool (e.g., "chat" at depth 0)
    scope: String,
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
        agent_memory: Option<AgentMemory>,
        scope: String,
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
        let description = self.agent.tool_description().to_string();

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
                )
                .add_property(
                    "new_instance",
                    PropertySchema::boolean(
                        "Start with a fresh context, discarding previous conversation history. Default: false (reuses history from prior calls)."
                    ).with_default(serde_json::Value::Bool(false)),
                    false,
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

        let child_scope = format!("{}/{}", self.scope, self.agent.name());

        // Clear scope if fresh instance requested
        if args.new_instance {
            if let Some(ref memory) = self.agent_memory {
                memory.clear_scope(&child_scope).await;
            }
        }

        // Load prior conversation history
        let prior_history = if let Some(ref memory) = self.agent_memory {
            memory.get_messages(&child_scope).await
        } else {
            Vec::new()
        };

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
                self.agent_memory.clone(),
                child_scope.clone(),
            );
            for tool in nested_agent_tools {
                agent_tools.register(tool);
            }
        }

        // Add inform_user tool if event bus is available
        if let Some(ref event_bus) = self.event_bus {
            agent_tools.register(Arc::new(InformUserTool::new(
                event_bus.clone(),
                self.agent.name(),
            )));
        }

        let agent_tools = Arc::new(agent_tools);

        // Get max_turns: config override takes precedence over hardcoded default
        let max_turns = self
            .external_agents
            .get_builtin_max_turns(self.agent.name())
            .unwrap_or_else(|| self.agent.max_turns());

        let has_sub_agents = next_depth < self.max_depth;
        let has_tools = !self.agent.tool_names().is_empty();
        let has_inform_user = self.event_bus.is_some();

        let preamble = qq_agents::generate_preamble(&qq_agents::PreambleContext {
            has_tools,
            has_sub_agents,
            has_inform_user,
        });
        let full_prompt = format!("{}\n\n---\n\n{}", preamble, self.agent.system_prompt());

        let mut config = AgentConfig::new(self.agent.name())
            .with_system_prompt(&full_prompt)
            .with_max_turns(max_turns);

        // Apply tool limits: config overrides take precedence over hardcoded defaults
        if let Some(limits) = self.external_agents.get_builtin_tool_limits(self.agent.name()) {
            // Use config override
            config = config.with_tool_limits(limits.clone());
        } else if let Some(limits) = self.agent.tool_limits() {
            // Fall back to hardcoded defaults
            config = config.with_tool_limits(limits);
        }

        // Create progress handler if event bus is available
        let progress = self.event_bus.as_ref().map(|bus| bus.create_handler());

        // Use continuation wrapper for execution
        let continuation_config = ContinuationConfig::default();
        let result = execute_with_continuation(
            Arc::clone(&self.provider),
            agent_tools,
            config,
            args.task.clone(),
            progress,
            continuation_config,
            self.event_bus.as_ref(),
            prior_history,
        )
        .await;

        // Store results back to memory, with LLM compaction
        match &result {
            AgentExecutionResult::Success { messages, .. }
            | AgentExecutionResult::MaxContinuationsReached { messages, .. } => {
                if let Some(ref memory) = self.agent_memory {
                    let tool_calls: u32 = messages
                        .iter()
                        .map(|m| m.tool_calls.len() as u32)
                        .sum();

                    // Resolve compaction prompt: config override > agent default
                    let prompt = self
                        .external_agents
                        .get_builtin_compact_prompt(self.agent.name())
                        .unwrap_or_else(|| self.agent.compact_prompt());

                    let compacted =
                        compact_agent_messages(&self.provider, messages.clone(), prompt).await;

                    memory
                        .store_messages(&child_scope, compacted, tool_calls)
                        .await;
                }
            }
            AgentExecutionResult::Error(_) => {} // don't store on error
        }

        let output = match result {
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
        };

        // Pop agent context
        if let Some(ref ctx) = self.execution_context {
            ctx.pop().await;
        }

        output
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
    /// Scoped agent memory for persistent instance state
    agent_memory: Option<AgentMemory>,
    /// Scope path for this tool
    scope: String,
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
        agent_memory: Option<AgentMemory>,
        scope: String,
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
            agent_memory,
            scope,
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
                )
                .add_property(
                    "new_instance",
                    PropertySchema::boolean(
                        "Start with a fresh context, discarding previous conversation history. Default: false (reuses history from prior calls)."
                    ).with_default(serde_json::Value::Bool(false)),
                    false,
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

        let child_scope = format!("{}/{}", self.scope, self.agent_name);

        // Clear scope if fresh instance requested
        if args.new_instance {
            if let Some(ref memory) = self.agent_memory {
                memory.clear_scope(&child_scope).await;
            }
        }

        // Load prior conversation history
        let prior_history = if let Some(ref memory) = self.agent_memory {
            memory.get_messages(&child_scope).await
        } else {
            Vec::new()
        };

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
                self.agent_memory.clone(),
                child_scope.clone(),
            );
            for tool in nested_agent_tools {
                agent_tools.register(tool);
            }
        }

        // Add inform_user tool if event bus is available
        if let Some(ref event_bus) = self.event_bus {
            agent_tools.register(Arc::new(InformUserTool::new(
                event_bus.clone(),
                &self.agent_name,
            )));
        }

        let agent_tools = Arc::new(agent_tools);

        let has_sub_agents = next_depth < self.max_depth;
        let has_tools = !self.definition.tools.is_empty();
        let has_inform_user = self.event_bus.is_some();

        let preamble = qq_agents::generate_preamble(&qq_agents::PreambleContext {
            has_tools,
            has_sub_agents,
            has_inform_user,
        });
        let full_prompt = format!("{}\n\n---\n\n{}", preamble, self.definition.system_prompt);

        let mut config = AgentConfig::new(self.agent_name.as_str())
            .with_system_prompt(&full_prompt)
            .with_max_turns(self.definition.max_turns);

        // Apply tool limits if configured for this external agent
        if !self.definition.tool_limits.is_empty() {
            config = config.with_tool_limits(self.definition.tool_limits.clone());
        }

        // Create progress handler if event bus is available
        let progress = self.event_bus.as_ref().map(|bus| bus.create_handler());

        // Use continuation wrapper for execution
        let continuation_config = ContinuationConfig::default();
        let result = execute_with_continuation(
            Arc::clone(&self.provider),
            agent_tools,
            config,
            args.task.clone(),
            progress,
            continuation_config,
            self.event_bus.as_ref(),
            prior_history,
        )
        .await;

        // Store results back to memory, with LLM compaction
        match &result {
            AgentExecutionResult::Success { messages, .. }
            | AgentExecutionResult::MaxContinuationsReached { messages, .. } => {
                if let Some(ref memory) = self.agent_memory {
                    let tool_calls: u32 = messages
                        .iter()
                        .map(|m| m.tool_calls.len() as u32)
                        .sum();

                    // Resolve compaction prompt: definition > default
                    let prompt = self
                        .definition
                        .compact_prompt
                        .as_deref()
                        .unwrap_or(DEFAULT_COMPACT_PROMPT);

                    let compacted =
                        compact_agent_messages(&self.provider, messages.clone(), prompt).await;

                    memory
                        .store_messages(&child_scope, compacted, tool_calls)
                        .await;
                }
            }
            AgentExecutionResult::Error(_) => {}
        }

        let output = match result {
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
        };

        // Pop agent context
        if let Some(ref ctx) = self.execution_context {
            ctx.pop().await;
        }

        output
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
/// * `agent_memory` - Optional scoped agent memory for persistent instance state
/// * `scope` - Current scope path (e.g., "chat", "chat/coder")
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
            )));
        }
    }

    tools
}

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
