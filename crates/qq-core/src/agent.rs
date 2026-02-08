//! Agent framework for multi-agent communication and coordination.
//!
//! This module provides:
//! - `Agent` for running LLM-powered agents (stateful or stateless)
//! - `AgentChannel` and `AgentSender` for inter-agent communication
//! - `AgentRegistry` for managing multiple agents
//! - Streaming support for real-time agent-to-agent communication

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use futures::StreamExt;
use tokio::sync::{mpsc, RwLock};

use crate::error::Error;
use crate::message::{Message, Role, StreamChunk, Usage};
use crate::provider::{CompletionRequest, Provider};
use crate::tool::ToolRegistry;

/// Result of a single agent execution.
///
/// Distinguishes between successful completion and hitting the max iterations limit.
/// `MaxIterationsExceeded` carries the full conversation history so callers (e.g.
/// continuation logic) can generate meaningful summaries.
#[derive(Debug)]
pub enum AgentRunResult {
    /// Agent completed successfully with a final response and conversation messages.
    Success {
        content: String,
        messages: Vec<Message>,
    },
    /// Agent hit the max iterations limit. Contains the full message history.
    MaxIterationsExceeded { messages: Vec<Message> },
}

/// Metadata about an agent instance's execution history.
#[derive(Debug, Clone, Default)]
pub struct AgentInstanceMetadata {
    pub call_count: u32,
    pub total_tool_calls: u32,
}

/// Stored state for a single agent instance (keyed by scope path).
#[derive(Debug, Clone)]
pub struct AgentInstanceState {
    pub messages: Vec<Message>,
    pub metadata: AgentInstanceMetadata,
}

impl AgentInstanceState {
    pub fn new() -> Self {
        Self {
            messages: Vec::new(),
            metadata: AgentInstanceMetadata::default(),
        }
    }

    pub fn total_bytes(&self) -> usize {
        self.messages.iter().map(|m| m.byte_count()).sum()
    }

    /// Trim oldest messages to fit within byte budget.
    /// Uses find_safe_trim_point to avoid orphaning tool results.
    pub fn trim_to_budget(&mut self, max_bytes: usize) {
        while self.total_bytes() > max_bytes && !self.messages.is_empty() {
            let remove_count = find_safe_trim_point(&self.messages);
            if remove_count == 0 {
                break;
            }
            self.messages.drain(..remove_count);
        }
    }
}

impl Default for AgentInstanceState {
    fn default() -> Self {
        Self::new()
    }
}

/// Default byte budget per agent instance (200KB).
pub const DEFAULT_MAX_INSTANCE_BYTES: usize = 200_000;

/// Central memory store for all scoped agent instances.
/// Keyed by scope path strings like "chat/explore", "chat/coder/explore".
#[derive(Debug, Clone)]
pub struct AgentMemory {
    instances: Arc<RwLock<HashMap<String, AgentInstanceState>>>,
    max_instance_bytes: usize,
}

impl AgentMemory {
    pub fn new() -> Self {
        Self {
            instances: Arc::new(RwLock::new(HashMap::new())),
            max_instance_bytes: DEFAULT_MAX_INSTANCE_BYTES,
        }
    }

    pub fn with_max_instance_bytes(max_bytes: usize) -> Self {
        Self {
            instances: Arc::new(RwLock::new(HashMap::new())),
            max_instance_bytes: max_bytes,
        }
    }

    /// Clone stored messages for a scope (or empty if none).
    pub async fn get_messages(&self, scope: &str) -> Vec<Message> {
        let instances = self.instances.read().await;
        instances
            .get(scope)
            .map(|s| s.messages.clone())
            .unwrap_or_default()
    }

    /// Get metadata for a scope.
    pub async fn get_metadata(&self, scope: &str) -> AgentInstanceMetadata {
        let instances = self.instances.read().await;
        instances
            .get(scope)
            .map(|s| s.metadata.clone())
            .unwrap_or_default()
    }

    /// Store messages for a scope, incrementing metadata and trimming to budget.
    pub async fn store_messages(&self, scope: &str, messages: Vec<Message>, tool_calls: u32) {
        let mut instances = self.instances.write().await;
        let state = instances
            .entry(scope.to_string())
            .or_insert_with(AgentInstanceState::new);
        state.messages = messages;
        state.metadata.call_count += 1;
        state.metadata.total_tool_calls += tool_calls;
        state.trim_to_budget(self.max_instance_bytes);
    }

    /// Remove a single scope's instance.
    pub async fn clear_scope(&self, scope: &str) {
        let mut instances = self.instances.write().await;
        instances.remove(scope);
    }

    /// Remove all instances (for /reset).
    pub async fn clear_all(&self) {
        let mut instances = self.instances.write().await;
        instances.clear();
    }

    /// Get diagnostics: (scope, bytes, call_count) for each instance.
    pub async fn diagnostics(&self) -> Vec<(String, usize, u32)> {
        let instances = self.instances.read().await;
        let mut result: Vec<_> = instances
            .iter()
            .map(|(scope, state)| {
                (scope.clone(), state.total_bytes(), state.metadata.call_count)
            })
            .collect();
        result.sort_by(|a, b| a.0.cmp(&b.0));
        result
    }
}

impl Default for AgentMemory {
    fn default() -> Self {
        Self::new()
    }
}

/// Find the number of leading messages that can be safely removed
/// without breaking an assistant-with-tool-calls / tool-result sequence.
fn find_safe_trim_point(messages: &[Message]) -> usize {
    if messages.is_empty() {
        return 0;
    }

    // Start at index 1 and walk forward to find the first safe boundary
    for (i, msg) in messages.iter().enumerate().skip(1) {
        // If this message is a tool result, we're inside a sequence — keep scanning
        if msg.tool_call_id.is_some() {
            continue;
        }

        // If this message is an assistant with tool_calls, the results follow it.
        // This is the start of a new sequence — safe to trim before it.
        if msg.role == Role::Assistant && !msg.tool_calls.is_empty() {
            return i;
        }

        // Any other message type (user, plain assistant) is a safe boundary.
        return i;
    }

    // Couldn't find a safe point (entire history is one tool sequence)
    0
}

/// Unique identifier for an agent.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct AgentId(pub String);

impl AgentId {
    /// Create a new agent ID.
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }
}

impl std::fmt::Display for AgentId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl From<&str> for AgentId {
    fn from(s: &str) -> Self {
        Self(s.to_string())
    }
}

impl From<String> for AgentId {
    fn from(s: String) -> Self {
        Self(s)
    }
}

/// Messages exchanged between agents.
#[derive(Debug, Clone)]
pub enum AgentMessage {
    /// A request from one agent to another.
    Request {
        /// The sender agent's ID.
        from: AgentId,
        /// The request content/prompt.
        content: String,
        /// Optional context messages to include.
        context: Vec<Message>,
    },

    /// A complete response from an agent.
    Response {
        /// The sender agent's ID.
        from: AgentId,
        /// The response content.
        content: String,
        /// Whether the operation succeeded.
        success: bool,
    },

    /// Signal that a streaming response is starting.
    StreamStart {
        /// The sender agent's ID.
        from: AgentId,
    },

    /// A chunk of streaming content.
    StreamDelta {
        /// The sender agent's ID.
        from: AgentId,
        /// The content chunk.
        content: String,
    },

    /// Signal that a streaming response has ended.
    StreamEnd {
        /// The sender agent's ID.
        from: AgentId,
        /// Whether the operation succeeded.
        success: bool,
    },

    /// A notification message (fire-and-forget).
    Notification {
        /// The sender agent's ID.
        from: AgentId,
        /// The notification content.
        content: String,
    },

    /// Signal to shut down the agent.
    Shutdown,
}

/// A bidirectional communication channel for an agent.
pub struct AgentChannel {
    /// The agent's ID.
    pub id: AgentId,
    /// Sender for outgoing messages.
    tx: mpsc::Sender<AgentMessage>,
    /// Receiver for incoming messages.
    rx: mpsc::Receiver<AgentMessage>,
}

impl AgentChannel {
    /// Create a new channel pair for an agent.
    ///
    /// Returns the channel and a sender that can be shared with other agents.
    pub fn new(id: impl Into<AgentId>, buffer_size: usize) -> (Self, AgentSender) {
        let id = id.into();
        let (tx, rx) = mpsc::channel(buffer_size);

        let channel = Self {
            id: id.clone(),
            tx: tx.clone(),
            rx,
        };

        let sender = AgentSender { id, tx };

        (channel, sender)
    }

    /// Send a message to this agent's outbox.
    pub async fn send(&self, msg: AgentMessage) -> Result<(), Error> {
        self.tx
            .send(msg)
            .await
            .map_err(|_| Error::stream("Agent channel closed"))
    }

    /// Receive the next message from the inbox.
    pub async fn recv(&mut self) -> Option<AgentMessage> {
        self.rx.recv().await
    }

    /// Get a clonable sender for this channel.
    pub fn sender(&self) -> AgentSender {
        AgentSender {
            id: self.id.clone(),
            tx: self.tx.clone(),
        }
    }
}

/// A clonable sender for sending messages to an agent.
#[derive(Clone)]
pub struct AgentSender {
    /// The target agent's ID.
    pub id: AgentId,
    /// The underlying sender.
    tx: mpsc::Sender<AgentMessage>,
}

impl AgentSender {
    /// Send a message to the agent.
    pub async fn send(&self, msg: AgentMessage) -> Result<(), Error> {
        self.tx
            .send(msg)
            .await
            .map_err(|_| Error::stream("Agent channel closed"))
    }

    /// Send a request to the agent.
    pub async fn request(
        &self,
        from: AgentId,
        content: impl Into<String>,
        context: Vec<Message>,
    ) -> Result<(), Error> {
        self.send(AgentMessage::Request {
            from,
            content: content.into(),
            context,
        })
        .await
    }

    /// Send a response to the agent.
    pub async fn respond(
        &self,
        from: AgentId,
        content: impl Into<String>,
        success: bool,
    ) -> Result<(), Error> {
        self.send(AgentMessage::Response {
            from,
            content: content.into(),
            success,
        })
        .await
    }

    /// Send a notification to the agent.
    pub async fn notify(&self, from: AgentId, content: impl Into<String>) -> Result<(), Error> {
        self.send(AgentMessage::Notification {
            from,
            content: content.into(),
        })
        .await
    }

    /// Signal shutdown to the agent.
    pub async fn shutdown(&self) -> Result<(), Error> {
        self.send(AgentMessage::Shutdown).await
    }
}

/// Registry for managing multiple agents.
pub struct AgentRegistry {
    /// Map of agent IDs to their senders.
    agents: HashMap<AgentId, AgentSender>,
}

impl Default for AgentRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl AgentRegistry {
    /// Create a new empty registry.
    pub fn new() -> Self {
        Self {
            agents: HashMap::new(),
        }
    }

    /// Register an agent's sender.
    pub fn register(&mut self, sender: AgentSender) {
        self.agents.insert(sender.id.clone(), sender);
    }

    /// Unregister an agent.
    pub fn unregister(&mut self, id: &AgentId) -> Option<AgentSender> {
        self.agents.remove(id)
    }

    /// Get a sender for an agent.
    pub fn get(&self, id: &AgentId) -> Option<&AgentSender> {
        self.agents.get(id)
    }

    /// Check if an agent is registered.
    pub fn contains(&self, id: &AgentId) -> bool {
        self.agents.contains_key(id)
    }

    /// Get all registered agent IDs.
    pub fn agent_ids(&self) -> Vec<&AgentId> {
        self.agents.keys().collect()
    }

    /// Broadcast a message to all agents.
    pub async fn broadcast(&self, msg: AgentMessage) -> Vec<Result<(), Error>> {
        let mut results = Vec::new();
        for sender in self.agents.values() {
            results.push(sender.send(msg.clone()).await);
        }
        results
    }

    /// Shutdown all agents.
    pub async fn shutdown_all(&self) -> Vec<Result<(), Error>> {
        self.broadcast(AgentMessage::Shutdown).await
    }
}

/// Configuration for an agent.
#[derive(Debug, Clone)]
pub struct AgentConfig {
    /// Unique agent identifier.
    pub id: AgentId,
    /// System prompt for the agent.
    pub system_prompt: Option<String>,
    /// Maximum agentic loop iterations.
    pub max_turns: usize,
    /// Whether to maintain message history (stateful mode).
    pub stateful: bool,
    /// Per-tool call limits (tool_name -> max_calls).
    /// When a tool reaches its limit, the agent receives an error instead.
    pub tool_limits: Option<HashMap<String, usize>>,
}

impl AgentConfig {
    /// Create a new agent configuration.
    pub fn new(id: impl Into<AgentId>) -> Self {
        Self {
            id: id.into(),
            system_prompt: None,
            max_turns: 20,
            stateful: false,
            tool_limits: None,
        }
    }

    /// Set the system prompt.
    pub fn with_system_prompt(mut self, prompt: impl Into<String>) -> Self {
        self.system_prompt = Some(prompt.into());
        self
    }

    /// Set the maximum iterations.
    pub fn with_max_turns(mut self, max: usize) -> Self {
        self.max_turns = max;
        self
    }

    /// Enable stateful mode (maintains message history).
    pub fn stateful(mut self) -> Self {
        self.stateful = true;
        self
    }

    /// Set per-tool call limits.
    ///
    /// When a tool reaches its limit, the agent receives an error message
    /// instead of executing the tool.
    pub fn with_tool_limits(mut self, limits: HashMap<String, usize>) -> Self {
        self.tool_limits = Some(limits);
        self
    }
}

/// Events emitted during agent execution for progress reporting.
#[derive(Debug, Clone)]
pub enum AgentProgressEvent {
    /// An iteration of the agent loop has started.
    IterationStart {
        agent_name: String,
        iteration: u32,
        max_turns: u32,
    },
    /// A chunk of thinking/reasoning content.
    ThinkingDelta {
        agent_name: String,
        content: String,
    },
    /// A tool execution has started.
    ToolStart {
        agent_name: String,
        tool_name: String,
        arguments: String,
    },
    /// A tool execution has completed.
    ToolComplete {
        agent_name: String,
        tool_name: String,
        tool_call_id: String,
        result: String,
        is_error: bool,
    },
    /// Assistant response received from LLM.
    AssistantResponse {
        agent_name: String,
        content: String,
        tool_call_count: usize,
    },
    /// Usage statistics update after an LLM call.
    UsageUpdate {
        agent_name: String,
        usage: Usage,
    },
    /// Byte count update (bytes sent/received to LLM).
    ByteCount {
        agent_name: String,
        /// Bytes sent to the LLM (prompt/context)
        input_bytes: usize,
        /// Bytes received from the LLM (response)
        output_bytes: usize,
    },
    /// A transient error occurred and the LLM call is being retried.
    Retry {
        agent_name: String,
        attempt: u32,
        max_retries: u32,
        error: String,
    },
}

/// Handler for receiving agent progress events.
///
/// Implement this trait to receive real-time updates during agent execution.
/// The TUI uses this to display thinking content, token counts, and iteration progress.
#[async_trait]
pub trait AgentProgressHandler: Send + Sync {
    /// Called when a progress event occurs during agent execution.
    async fn on_progress(&self, event: AgentProgressEvent);
}

/// An LLM-powered agent that can run tasks and communicate with other agents.
pub struct Agent {
    /// Agent configuration.
    pub config: AgentConfig,
    /// The LLM provider.
    provider: Arc<dyn Provider>,
    /// Available tools.
    tools: Arc<ToolRegistry>,
    /// Message history (only used in stateful mode).
    messages: Vec<Message>,
}

impl Agent {
    /// Create a new stateful agent.
    ///
    /// Stateful agents maintain message history across calls to `process()`.
    pub fn new_stateful(
        provider: Arc<dyn Provider>,
        tools: Arc<ToolRegistry>,
        config: AgentConfig,
    ) -> Self {
        let mut config = config;
        config.stateful = true;

        Self {
            config,
            provider,
            tools,
            messages: Vec::new(),
        }
    }

    /// Create a new stateless agent.
    ///
    /// Stateless agents don't maintain history; use `run_once()` for one-shot tasks.
    pub fn new_stateless(
        provider: Arc<dyn Provider>,
        tools: Arc<ToolRegistry>,
        config: AgentConfig,
    ) -> Self {
        let mut config = config;
        config.stateful = false;

        Self {
            config,
            provider,
            tools,
            messages: Vec::new(),
        }
    }

    /// Get the agent's ID.
    pub fn id(&self) -> &AgentId {
        &self.config.id
    }

    /// Run a one-shot task with the given context.
    ///
    /// This is the preferred method for stateless agent execution.
    /// The agent runs until it produces a final response (no tool calls).
    /// Returns an error if max iterations is exceeded (use `run_once_with_progress`
    /// directly if you need the conversation history on max iterations).
    pub async fn run_once(
        provider: Arc<dyn Provider>,
        tools: Arc<ToolRegistry>,
        config: AgentConfig,
        context: Vec<Message>,
    ) -> Result<String, Error> {
        match Self::run_once_with_progress(provider, tools, config, context, None).await? {
            AgentRunResult::Success { content, .. } => Ok(content),
            AgentRunResult::MaxIterationsExceeded { .. } => {
                Err(Error::Unknown("Agent exceeded max iterations".into()))
            }
        }
    }

    /// Run a one-shot task with progress reporting.
    ///
    /// When a progress handler is provided, this uses streaming for LLM calls
    /// and emits progress events for thinking content, tool execution, and usage.
    ///
    /// Returns `AgentRunResult::Success` on completion or
    /// `AgentRunResult::MaxIterationsExceeded` with the full conversation history
    /// when the iteration limit is reached.
    pub async fn run_once_with_progress(
        provider: Arc<dyn Provider>,
        tools: Arc<ToolRegistry>,
        config: AgentConfig,
        context: Vec<Message>,
        progress: Option<Arc<dyn AgentProgressHandler>>,
    ) -> Result<AgentRunResult, Error> {
        use tracing::debug;

        let agent_name = config.id.0.clone();

        debug!(
            agent = %config.id,
            context_messages = context.len(),
            tools_available = tools.definitions().len(),
            has_progress_handler = progress.is_some(),
            "Agent run_once starting"
        );

        tracing::trace!(agent = %config.id, context_messages = context.len(), "Agent context");

        // Split into base (system prompt + initial context) and loop-accumulated messages.
        // This avoids cloning the growing loop_messages into base on every iteration.
        let mut base_messages = Vec::new();

        // Add system prompt if configured
        if let Some(system) = &config.system_prompt {
            base_messages.push(Message::system(system.as_str()));
        }

        // Add provided context
        base_messages.extend(context);

        // Messages accumulated during the agentic loop (assistant + tool_result)
        let mut loop_messages: Vec<Message> = Vec::new();

        let max_turns = config.max_turns as u32;

        // Track tool call counts for enforcing limits
        let mut tool_call_counts: HashMap<String, usize> = HashMap::new();

        // Run agentic loop
        for iteration in 0..config.max_turns {
            // Emit iteration start event
            if let Some(ref handler) = progress {
                handler
                    .on_progress(AgentProgressEvent::IterationStart {
                        agent_name: agent_name.clone(),
                        iteration: iteration as u32 + 1,
                        max_turns,
                    })
                    .await;
            }

            debug!(
                agent = %config.id,
                iteration = iteration,
                message_count = base_messages.len() + loop_messages.len(),
                "Agent iteration starting"
            );

            // Count input bytes (messages being sent)
            let input_bytes: usize = base_messages.iter()
                .chain(loop_messages.iter())
                .map(|m| m.byte_count())
                .sum();

            // Use streaming if we have a progress handler, otherwise use complete()
            // Wrap with retry logic for transient transport/stream errors
            let (content, tool_calls, usage) = {
                let mut last_error = None;
                let mut result = None;
                for attempt in 0..=MAX_STREAM_RETRIES {
                    if attempt > 0 {
                        let delay = INITIAL_RETRY_DELAY * 2u32.pow(attempt - 1);
                        tracing::warn!(
                            agent = %config.id,
                            attempt = attempt,
                            max_retries = MAX_STREAM_RETRIES,
                            delay_secs = delay.as_secs(),
                            "Retrying after transient error"
                        );
                        if let Some(ref handler) = progress {
                            handler
                                .on_progress(AgentProgressEvent::Retry {
                                    agent_name: agent_name.clone(),
                                    attempt,
                                    max_retries: MAX_STREAM_RETRIES,
                                    error: last_error
                                        .as_ref()
                                        .map(|e: &Error| e.to_string())
                                        .unwrap_or_default(),
                                })
                                .await;
                        }
                        tokio::time::sleep(delay).await;
                    }

                    // Re-build the request for each attempt (need a fresh clone)
                    let attempt_request = CompletionRequest::new(
                        base_messages
                            .iter()
                            .chain(loop_messages.iter())
                            .cloned()
                            .collect(),
                    )
                    .with_tools(tools.definitions());

                    let iter_result = if progress.is_some() {
                        run_streaming_iteration(
                            &provider,
                            &agent_name,
                            attempt_request,
                            progress.as_ref(),
                        )
                        .await
                    } else {
                        run_complete_iteration(&provider, &config, attempt_request).await
                    };

                    match iter_result {
                        Ok(val) => {
                            result = Some(val);
                            break;
                        }
                        Err(e) if e.is_retryable() && attempt < MAX_STREAM_RETRIES => {
                            tracing::warn!(
                                agent = %config.id,
                                error = %e,
                                "Transient error, will retry"
                            );
                            last_error = Some(e);
                            continue;
                        }
                        Err(e) => return Err(e),
                    }
                }
                // Safe: loop either sets result or returns Err
                result.unwrap()
            };

            // Count output bytes (response received)
            let output_bytes = content.len();

            // Emit assistant response event
            if let Some(ref handler) = progress {
                handler
                    .on_progress(AgentProgressEvent::AssistantResponse {
                        agent_name: agent_name.clone(),
                        content: content.clone(),
                        tool_call_count: tool_calls.len(),
                    })
                    .await;
            }

            // Emit byte count update
            if let Some(ref handler) = progress {
                handler
                    .on_progress(AgentProgressEvent::ByteCount {
                        agent_name: agent_name.clone(),
                        input_bytes,
                        output_bytes,
                    })
                    .await;
            }

            // Emit usage update
            if let Some(ref handler) = progress {
                handler
                    .on_progress(AgentProgressEvent::UsageUpdate {
                        agent_name: agent_name.clone(),
                        usage,
                    })
                    .await;
            }

            // Check for tool calls
            if !tool_calls.is_empty() {
                debug!(
                    agent = %config.id,
                    tool_count = tool_calls.len(),
                    "Agent executing tools"
                );

                // Store message with tool calls but NO content (don't store thinking)
                let msg = Message::assistant_with_tool_calls("", tool_calls.clone());
                loop_messages.push(msg);

                // Check tool limits and partition into executable vs limit-exceeded
                let mut executable_calls = Vec::new();
                for tool_call in &tool_calls {
                    let limit_exceeded = if let Some(ref limits) = config.tool_limits {
                        if let Some(&limit) = limits.get(&tool_call.name) {
                            let count = tool_call_counts.get(&tool_call.name).copied().unwrap_or(0);
                            count >= limit
                        } else {
                            false
                        }
                    } else {
                        false
                    };

                    if limit_exceeded {
                        let limit = config.tool_limits.as_ref()
                            .and_then(|l| l.get(&tool_call.name))
                            .copied()
                            .unwrap_or(0);

                        debug!(
                            agent = %config.id,
                            tool = %tool_call.name,
                            limit = limit,
                            "Tool call limit exceeded"
                        );

                        // Emit tool start event (still shows the attempt)
                        if let Some(ref handler) = progress {
                            handler
                                .on_progress(AgentProgressEvent::ToolStart {
                                    agent_name: agent_name.clone(),
                                    tool_name: tool_call.name.clone(),
                                    arguments: tool_call.arguments.to_string(),
                                })
                                .await;
                        }

                        let result = format!(
                            "Error: Tool '{}' call limit exceeded (limit: {}). \
                            You have already used this tool the maximum number of times allowed. \
                            Please complete your task with the information you have gathered.",
                            tool_call.name, limit
                        );
                        let result = truncate_tool_result(result, MAX_AGENT_TOOL_RESULT_BYTES);

                        // Emit tool complete event (as error)
                        if let Some(ref handler) = progress {
                            handler
                                .on_progress(AgentProgressEvent::ToolComplete {
                                    agent_name: agent_name.clone(),
                                    tool_name: tool_call.name.clone(),
                                    tool_call_id: tool_call.id.clone(),
                                    result: result.clone(),
                                    is_error: true,
                                })
                                .await;
                        }

                        loop_messages.push(Message::tool_result(&tool_call.id, result));
                    } else {
                        *tool_call_counts.entry(tool_call.name.clone()).or_insert(0) += 1;
                        executable_calls.push(tool_call);
                    }
                }

                // Emit tool start events for all executable calls
                for tool_call in &executable_calls {
                    if let Some(ref handler) = progress {
                        handler
                            .on_progress(AgentProgressEvent::ToolStart {
                                agent_name: agent_name.clone(),
                                tool_name: tool_call.name.clone(),
                                arguments: tool_call.arguments.to_string(),
                            })
                            .await;
                    }

                    debug!(
                        agent = %config.id,
                        tool = %tool_call.name,
                        "Executing tool"
                    );
                }

                // Execute all tools concurrently
                let futures: Vec<_> = executable_calls
                    .iter()
                    .map(|tool_call| {
                        let tools_ref = &tools;
                        async move {
                            let result = execute_tool(tools_ref, tool_call).await;
                            let is_error = result.starts_with("Error:");
                            (tool_call, result, is_error)
                        }
                    })
                    .collect();

                let results = futures::future::join_all(futures).await;

                // Process results and emit completion events
                for (tool_call, result, is_error) in results {
                    if let Some(ref handler) = progress {
                        handler
                            .on_progress(AgentProgressEvent::ToolComplete {
                                agent_name: agent_name.clone(),
                                tool_name: tool_call.name.clone(),
                                tool_call_id: tool_call.id.clone(),
                                result: result.clone(),
                                is_error,
                            })
                            .await;
                    }

                    loop_messages.push(Message::tool_result(&tool_call.id, result));
                }

                continue;
            }

            // No tool calls - return final response
            debug!(
                agent = %config.id,
                iterations = iteration + 1,
                response_len = content.len(),
                "Agent completed successfully"
            );
            return Ok(AgentRunResult::Success {
                content,
                messages: loop_messages,
            });
        }

        // Combine base + loop messages for the full conversation history
        base_messages.extend(loop_messages);
        Ok(AgentRunResult::MaxIterationsExceeded { messages: base_messages })
    }

    /// Process an input message (stateful mode).
    ///
    /// In stateful mode, the agent maintains history across calls.
    pub async fn process(&mut self, input: &str) -> Result<String, Error> {
        if !self.config.stateful {
            return Err(Error::config(
                "process() requires stateful mode; use run_once() for stateless agents",
            ));
        }

        // Add user input
        self.messages.push(Message::user(input));

        // Run agentic loop
        // Build request messages from self.messages each iteration to avoid double-cloning.
        for _iteration in 0..self.config.max_turns {
            let mut request_messages = Vec::with_capacity(self.messages.len() + 1);
            if let Some(system) = &self.config.system_prompt {
                request_messages.push(Message::system(system.as_str()));
            }
            request_messages.extend(self.messages.iter().cloned());

            let mut request = CompletionRequest::new(request_messages)
                .with_tools(self.tools.definitions());

            request.stream = false;

            let response = self.provider.complete(request).await?;

            // Check for tool calls
            if !response.message.tool_calls.is_empty() {
                // Add assistant message with tool calls but NO content (don't store thinking)
                let msg = Message::assistant_with_tool_calls("", response.message.tool_calls.clone());
                self.messages.push(msg);

                // Execute tools
                for tool_call in &response.message.tool_calls {
                    let result = execute_tool(&self.tools, tool_call).await;
                    self.messages.push(Message::tool_result(&tool_call.id, result));
                }

                continue;
            }

            // No tool calls - save and return response (content only, no thinking)
            let content = response.message.content.to_string_lossy();
            self.messages.push(Message::assistant(content.as_str()));
            return Ok(content);
        }

        Err(Error::Unknown(format!(
            "Agent {} exceeded max iterations ({})",
            self.config.id, self.config.max_turns
        )))
    }

    /// Process an input and stream the response to another agent.
    ///
    /// Sends `StreamStart`, `StreamDelta*`, and `StreamEnd` messages to the target.
    pub async fn process_streaming(
        &mut self,
        input: &str,
        target: &AgentSender,
    ) -> Result<(), Error> {
        // Signal stream start
        target
            .send(AgentMessage::StreamStart {
                from: self.config.id.clone(),
            })
            .await?;

        // Process the input
        match self.process(input).await {
            Ok(content) => {
                // Send the content as a single delta (could be chunked for true streaming)
                target
                    .send(AgentMessage::StreamDelta {
                        from: self.config.id.clone(),
                        content,
                    })
                    .await?;

                // Signal successful completion
                target
                    .send(AgentMessage::StreamEnd {
                        from: self.config.id.clone(),
                        success: true,
                    })
                    .await?;

                Ok(())
            }
            Err(e) => {
                // Signal failure
                target
                    .send(AgentMessage::StreamEnd {
                        from: self.config.id.clone(),
                        success: false,
                    })
                    .await?;

                Err(e)
            }
        }
    }

    /// Clear the message history (stateful mode only).
    pub fn clear_history(&mut self) {
        self.messages.clear();
    }

    /// Get the current message count.
    pub fn message_count(&self) -> usize {
        self.messages.len()
    }
}

/// Per-chunk inactivity timeout for streaming responses.
/// If no data is received for this duration, the stream is considered stalled.
const STREAM_CHUNK_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(120);

/// Maximum number of retries for transient streaming/transport errors.
const MAX_STREAM_RETRIES: u32 = 3;

/// Initial retry delay (doubles each attempt: 1s, 2s, 4s).
const INITIAL_RETRY_DELAY: std::time::Duration = std::time::Duration::from_secs(1);

/// Run a single iteration using streaming (for progress reporting).
async fn run_streaming_iteration(
    provider: &Arc<dyn Provider>,
    agent_name: &str,
    request: CompletionRequest,
    progress: Option<&Arc<dyn AgentProgressHandler>>,
) -> Result<(String, Vec<crate::message::ToolCall>, Usage), Error> {
    use tracing::debug;

    let mut stream = provider.stream(request).await?;

    let mut content = String::new();
    let mut tool_calls: Vec<crate::message::ToolCall> = Vec::new();
    let mut current_tool_call: Option<(String, String, String)> = None;
    let mut usage = Usage::default();

    loop {
        match tokio::time::timeout(STREAM_CHUNK_TIMEOUT, stream.next()).await {
            Ok(Some(chunk)) => {
                match chunk {
                    Ok(StreamChunk::Start { model: _ }) => {
                        // Model started
                    }
                    Ok(StreamChunk::ThinkingDelta { content: delta }) => {
                        // Emit thinking delta event
                        if let Some(handler) = progress {
                            handler
                                .on_progress(AgentProgressEvent::ThinkingDelta {
                                    agent_name: agent_name.to_string(),
                                    content: delta,
                                })
                                .await;
                        }
                    }
                    Ok(StreamChunk::Delta { content: delta }) => {
                        content.push_str(&delta);
                    }
                    Ok(StreamChunk::ToolCallStart { id, name }) => {
                        // Finish pending tool call
                        if let Some((tc_id, tc_name, tc_args)) = current_tool_call.take() {
                            let args: serde_json::Value =
                                serde_json::from_str(&tc_args).unwrap_or(serde_json::Value::Null);
                            tool_calls.push(crate::message::ToolCall::new(tc_id, tc_name, args));
                        }
                        current_tool_call = Some((id, name, String::new()));
                    }
                    Ok(StreamChunk::ToolCallDelta { arguments }) => {
                        if let Some((_, _, ref mut args)) = current_tool_call {
                            args.push_str(&arguments);
                        }
                    }
                    Ok(StreamChunk::Done { usage: u }) => {
                        // Finish pending tool call
                        if let Some((tc_id, tc_name, tc_args)) = current_tool_call.take() {
                            let args: serde_json::Value =
                                serde_json::from_str(&tc_args).unwrap_or(serde_json::Value::Null);
                            tool_calls.push(crate::message::ToolCall::new(tc_id, tc_name, args));
                        }
                        if let Some(u) = u {
                            usage = u;
                        }
                    }
                    Ok(StreamChunk::Error { message }) => {
                        debug!(agent = agent_name, error = %message, "Stream error");
                        return Err(Error::stream(message));
                    }
                    Err(e) => {
                        debug!(agent = agent_name, error = %e, "Stream error");
                        return Err(e);
                    }
                }
            }
            Ok(None) => {
                // Stream ended normally
                break;
            }
            Err(_elapsed) => {
                debug!(agent = agent_name, "Stream chunk timeout after {:?}", STREAM_CHUNK_TIMEOUT);
                return Err(Error::stream(format!(
                    "Stream timed out: no data received for {} seconds",
                    STREAM_CHUNK_TIMEOUT.as_secs()
                )));
            }
        }
    }

    Ok((content, tool_calls, usage))
}

/// Run a single iteration using non-streaming complete() (when no progress handler).
async fn run_complete_iteration(
    provider: &Arc<dyn Provider>,
    config: &AgentConfig,
    mut request: CompletionRequest,
) -> Result<(String, Vec<crate::message::ToolCall>, Usage), Error> {
    use tracing::debug;

    request.stream = false;

    let response = provider.complete(request).await?;

    // Log if thinking was extracted
    if let Some(ref thinking) = response.thinking {
        debug!(
            agent = %config.id,
            thinking_len = thinking.len(),
            "Extracted thinking content (not stored)"
        );
    }

    let content = response.message.content.to_string_lossy();
    let tool_calls = response.message.tool_calls;
    let usage = response.usage;

    Ok((content, tool_calls, usage))
}

/// Maximum bytes for a tool result in agent context.
/// Matches `ChunkerConfig::default_threshold_bytes` (50KB).
const MAX_AGENT_TOOL_RESULT_BYTES: usize = 50_000;

/// Truncate a tool result to fit within a byte budget.
///
/// If the content exceeds `max_bytes`, truncates at the last newline before
/// the limit (respecting UTF-8 char boundaries) and appends a note.
fn truncate_tool_result(content: String, max_bytes: usize) -> String {
    if content.len() <= max_bytes {
        return content;
    }

    // Find a valid UTF-8 char boundary at or before max_bytes
    let mut boundary = max_bytes;
    while boundary > 0 && !content.is_char_boundary(boundary) {
        boundary -= 1;
    }

    // Try to find the last newline before the boundary for a clean cut
    if let Some(newline_pos) = content[..boundary].rfind('\n') {
        let truncated = &content[..newline_pos];
        format!(
            "{}\n\n[Output truncated: showing {}/{} bytes. Request specific sections if needed.]",
            truncated,
            newline_pos,
            content.len()
        )
    } else {
        // No newline found - truncate at char boundary
        let truncated = &content[..boundary];
        format!(
            "{}\n\n[Output truncated: showing {}/{} bytes. Request specific sections if needed.]",
            truncated,
            boundary,
            content.len()
        )
    }
}

/// Execute a single tool call.
async fn execute_tool(
    registry: &ToolRegistry,
    tool_call: &crate::message::ToolCall,
) -> String {
    let Some(tool) = registry.get_arc(&tool_call.name) else {
        return format!("Error: Unknown tool '{}'", tool_call.name);
    };

    match crate::tool::execute_tool_dispatch(tool, tool_call.arguments.clone()).await {
        Ok(output) => {
            let content = if output.is_error {
                format!("Error: {}", output.content)
            } else {
                output.content
            };
            truncate_tool_result(content, MAX_AGENT_TOOL_RESULT_BYTES)
        }
        Err(e) => format!("Error executing tool: {}", e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_agent_id() {
        let id = AgentId::new("test-agent");
        assert_eq!(id.0, "test-agent");
        assert_eq!(format!("{}", id), "test-agent");

        let id2: AgentId = "another".into();
        assert_eq!(id2.0, "another");
    }

    #[test]
    fn test_agent_config() {
        let config = AgentConfig::new("my-agent")
            .with_system_prompt("You are a helpful assistant")
            .with_max_turns(10)
            .stateful();

        assert_eq!(config.id.0, "my-agent");
        assert_eq!(
            config.system_prompt,
            Some("You are a helpful assistant".to_string())
        );
        assert_eq!(config.max_turns, 10);
        assert!(config.stateful);
    }

    #[tokio::test]
    async fn test_agent_channel() {
        let (mut channel, sender) = AgentChannel::new("test", 10);

        // Send a message
        sender
            .notify(AgentId::new("other"), "Hello!")
            .await
            .unwrap();

        // Receive the message
        let msg = channel.recv().await.unwrap();
        match msg {
            AgentMessage::Notification { from, content } => {
                assert_eq!(from.0, "other");
                assert_eq!(content, "Hello!");
            }
            _ => panic!("Expected Notification"),
        }
    }

    #[tokio::test]
    async fn test_agent_registry() {
        let mut registry = AgentRegistry::new();

        let (_channel1, sender1) = AgentChannel::new("agent-1", 10);
        let (_channel2, sender2) = AgentChannel::new("agent-2", 10);

        registry.register(sender1);
        registry.register(sender2);

        assert!(registry.contains(&AgentId::new("agent-1")));
        assert!(registry.contains(&AgentId::new("agent-2")));
        assert!(!registry.contains(&AgentId::new("agent-3")));

        let ids = registry.agent_ids();
        assert_eq!(ids.len(), 2);
    }

    #[test]
    fn test_agent_message_clone() {
        let msg = AgentMessage::Request {
            from: AgentId::new("sender"),
            content: "Hello".to_string(),
            context: vec![],
        };

        let cloned = msg.clone();
        match cloned {
            AgentMessage::Request { from, content, .. } => {
                assert_eq!(from.0, "sender");
                assert_eq!(content, "Hello");
            }
            _ => panic!("Expected Request"),
        }
    }

    #[test]
    fn test_truncate_tool_result_under_limit() {
        let content = "Hello, world!".to_string();
        let result = truncate_tool_result(content.clone(), 100);
        assert_eq!(result, content);
    }

    #[test]
    fn test_truncate_tool_result_over_limit_at_newline() {
        let content = "line1\nline2\nline3\nline4\nline5".to_string();
        // Limit to 15 bytes - should cut at newline before byte 15
        let result = truncate_tool_result(content.clone(), 15);
        assert!(result.starts_with("line1\nline2"));
        assert!(result.contains("[Output truncated:"));
        assert!(result.contains(&format!("{} bytes", content.len())));
    }

    #[test]
    fn test_truncate_tool_result_over_limit_no_newline() {
        let content = "a".repeat(100);
        let result = truncate_tool_result(content.clone(), 50);
        assert!(result.contains("[Output truncated:"));
        // Should have 50 'a's before the truncation note
        assert!(result.starts_with(&"a".repeat(50)));
    }

    #[test]
    fn test_truncate_tool_result_empty() {
        let result = truncate_tool_result(String::new(), 100);
        assert_eq!(result, "");
    }

    #[test]
    fn test_truncate_tool_result_utf8_safety() {
        // Multi-byte UTF-8: each emoji is 4 bytes
        let content = "🎉🎊🎃🎄🎅".to_string(); // 20 bytes
        // Try to cut at 6 bytes (middle of second emoji)
        let result = truncate_tool_result(content.clone(), 6);
        // Should back up to a char boundary (byte 4, end of first emoji)
        assert!(result.starts_with("🎉"));
        assert!(result.contains("[Output truncated:"));
    }

    #[test]
    fn test_truncate_tool_result_exact_limit() {
        let content = "exact".to_string();
        let result = truncate_tool_result(content.clone(), 5);
        assert_eq!(result, "exact");
    }
}
