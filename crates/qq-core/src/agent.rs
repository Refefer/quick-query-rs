//! Agent framework for multi-agent communication and coordination.
//!
//! This module provides:
//! - `Agent` for running LLM-powered agents (stateful or stateless)
//! - `AgentChannel` and `AgentSender` for inter-agent communication
//! - `AgentRegistry` for managing multiple agents
//! - Streaming support for real-time agent-to-agent communication

use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::sync::Arc;

use async_trait::async_trait;
use futures::StreamExt;
use tokio::sync::{mpsc, RwLock};

use crate::error::Error;
use crate::message::{FinishReason, Message, Role, StreamChunk, Usage};
use crate::observation::{ContextCompactor, ObservationConfig, ObservationalMemory};
use crate::provider::{CompletionRequest, Provider};
use crate::tool::ToolRegistry;

/// Result of a single agent execution.
///
/// Distinguishes between successful completion, hitting limits, and repetition detection.
/// History-carrying variants allow callers (e.g. continuation logic) to generate summaries.
#[derive(Debug)]
pub enum AgentRunResult {
    /// Agent completed successfully with a final response and conversation messages.
    Success {
        content: String,
        messages: Vec<Message>,
        /// Observation log from observational memory (empty if not used).
        observation_log: String,
    },
    /// Agent hit the max iterations safety ceiling. Contains the full message history.
    MaxIterationsExceeded {
        messages: Vec<Message>,
        /// Observation log from observational memory (empty if not used).
        observation_log: String,
    },
    /// Agent hit the max observations limit. Contains the last response and history.
    ObservationLimitReached {
        content: String,
        messages: Vec<Message>,
        observation_log: String,
    },
    /// Agent was stuck calling the same tool(s) with the same arguments repeatedly.
    RepetitionDetected {
        messages: Vec<Message>,
        observation_log: String,
    },
    /// The model's response was cut off by the provider's max-tokens limit before
    /// completion. The partial content (if any) is preserved for the caller, but
    /// this MUST NOT be treated as success — the agent did not finish its task.
    /// Surfacing this distinctly lets parent agents stop dispatching new sub-agents
    /// for the same purpose, which would otherwise loop indefinitely.
    ///
    /// `was_context_full` distinguishes "ran out of context window" (recoverable
    /// by splitting the task into smaller subtasks) from "hit the request's
    /// per-call max_tokens cap" (recoverable by raising the cap or shortening
    /// the expected output). When true, the loop already attempted at least
    /// one emergency compaction pass and it didn't free enough space.
    TruncatedByLength {
        partial_content: String,
        messages: Vec<Message>,
        observation_log: String,
        was_context_full: bool,
    },
}

/// Default repetition threshold: block after this many identical calls.
const REPETITION_THRESHOLD: usize = 3;

/// Default tool fatigue threshold: warn after this many calls to the same tool.
const TOOL_FATIGUE_THRESHOLD: usize = 5;

/// Detects when an agent is stuck calling the same tool with similar arguments.
///
/// Two detection modes:
/// 1. **Exact match** — blocks repeated `(tool_name, canonical_args)` hashes.
/// 2. **Tool fatigue** — warns (then blocks) when a single tool has been called
///    N+ times across iterations, even with different arguments.
struct RepetitionDetector {
    /// hash(tool_name, canonical_args) -> total call count
    call_counts: HashMap<u64, usize>,
    /// tool_name -> total calls across all iterations
    tool_call_counts: HashMap<String, usize>,
    threshold: usize,
    fatigue_threshold: usize,
}

impl RepetitionDetector {
    fn new() -> Self {
        Self {
            call_counts: HashMap::new(),
            tool_call_counts: HashMap::new(),
            threshold: REPETITION_THRESHOLD,
            fatigue_threshold: TOOL_FATIGUE_THRESHOLD,
        }
    }

    /// Check whether this tool call is a repetition or shows tool fatigue.
    /// Returns `Some(result)` if blocked/warned, `None` if allowed.
    fn check(&mut self, tool_name: &str, arguments: &serde_json::Value) -> Option<String> {
        // Track total calls per tool name (cross-iteration)
        let total_for_tool = self.tool_call_counts.entry(tool_name.to_string()).or_insert(0);
        *total_for_tool += 1;

        // Exact match detection
        let hash = Self::canonical_hash(tool_name, arguments);
        let count = self.call_counts.entry(hash).or_insert(0);
        *count += 1;
        if *count >= self.threshold {
            return Some(format!(
                "Error: You have called '{}' with identical arguments {} times. \n\
                 The result will not change. Try a different approach or provide your final response.",
                tool_name, *count
            ));
        }

        // Tool fatigue detection: same tool called many times with different args
        if *total_for_tool >= self.fatigue_threshold {
            return Some(format!(
                "Warning: You have called '{}' {} times across your investigation. \n\
                 If these calls are not producing new information, consider a different strategy \n\
                 or synthesize what you've already found.",
                tool_name, *total_for_tool
            ));
        }

        None
    }

    /// Compute a deterministic hash of (tool_name, arguments) with sorted JSON keys.
    fn canonical_hash(tool_name: &str, arguments: &serde_json::Value) -> u64 {
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        tool_name.hash(&mut hasher);
        Self::hash_value(arguments, &mut hasher);
        hasher.finish()
    }

    /// Recursively hash a JSON value with sorted object keys for determinism.
    fn hash_value(value: &serde_json::Value, hasher: &mut impl Hasher) {
        match value {
            serde_json::Value::Null => 0u8.hash(hasher),
            serde_json::Value::Bool(b) => { 1u8.hash(hasher); b.hash(hasher); }
            serde_json::Value::Number(n) => { 2u8.hash(hasher); n.to_string().hash(hasher); }
            serde_json::Value::String(s) => { 3u8.hash(hasher); s.hash(hasher); }
            serde_json::Value::Array(arr) => {
                4u8.hash(hasher);
                arr.len().hash(hasher);
                for v in arr {
                    Self::hash_value(v, hasher);
                }
            }
            serde_json::Value::Object(map) => {
                5u8.hash(hasher);
                map.len().hash(hasher);
                let mut keys: Vec<&String> = map.keys().collect();
                keys.sort();
                for key in keys {
                    key.hash(hasher);
                    Self::hash_value(&map[key], hasher);
                }
            }
        }
    }
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
    /// Observation log from observational memory (empty if using compaction strategy).
    pub observation_log: String,
}

impl AgentInstanceState {
    pub fn new() -> Self {
        Self {
            messages: Vec::new(),
            metadata: AgentInstanceMetadata::default(),
            observation_log: String::new(),
        }
    }

    pub fn total_bytes(&self) -> usize {
        let msg_bytes: usize = self.messages.iter().map(|m| m.byte_count()).sum();
        msg_bytes + self.observation_log.len()
    }

    /// Trim oldest messages to fit within byte budget.
    /// Uses find_safe_trim_point to avoid orphaning tool results.
    /// Only trims messages, never the observation log (the log IS the compressed form).
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

    /// Store messages and observation log for a scope (obs-memory strategy).
    pub async fn store_state(
        &self,
        scope: &str,
        messages: Vec<Message>,
        observation_log: String,
        tool_calls: u32,
    ) {
        let mut instances = self.instances.write().await;
        let state = instances
            .entry(scope.to_string())
            .or_insert_with(AgentInstanceState::new);
        state.messages = messages;
        state.observation_log = observation_log;
        state.metadata.call_count += 1;
        state.metadata.total_tool_calls += tool_calls;
        state.trim_to_budget(self.max_instance_bytes);
    }

    /// Get messages and observation log for a scope.
    pub async fn get_state(&self, scope: &str) -> (Vec<Message>, String) {
        let instances = self.instances.read().await;
        instances
            .get(scope)
            .map(|s| (s.messages.clone(), s.observation_log.clone()))
            .unwrap_or_default()
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

/// Permission envelope inherited across agent delegation boundaries.
///
/// Invariant: a delegated agent's effective permissions are the *minimum*
/// (strictest) of the caller's inherited permissions and the callee's own
/// declared permissions. Permissions never widen as the call chain deepens.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct DelegationPermissions {
    /// When true, this agent and everything it delegates to must be read-only.
    pub read_only: bool,
}

impl DelegationPermissions {
    /// Combine inherited permissions with a child's declared permissions,
    /// taking the stricter of the two on every axis.
    pub fn restrict_with(self, declared: DelegationPermissions) -> Self {
        Self {
            read_only: self.read_only || declared.read_only,
        }
    }
}

/// Configuration for an agent.
#[derive(Clone)]
pub struct AgentConfig {
    /// Unique agent identifier.
    pub id: AgentId,
    /// System prompt for the agent.
    pub system_prompt: Option<String>,
    /// Maximum agentic loop iterations (safety ceiling).
    pub max_turns: usize,
    /// Whether to maintain message history (stateful mode).
    pub stateful: bool,
    /// Per-tool call limits (tool_name -> max_calls).
    /// When a tool reaches its limit, the agent receives an error instead.
    pub tool_limits: Option<HashMap<String, usize>>,
    /// Optional context compactor for observational memory in the loop.
    pub compactor: Option<Arc<dyn ContextCompactor>>,
    /// Observation config (thresholds). Used only when compactor is Some.
    pub observation_config: Option<ObservationConfig>,
    /// Maximum observations before requesting wrap-up. None = no limit.
    pub max_observations: Option<u32>,
    /// Prior observation log to restore (for resuming stateful agents).
    pub prior_observation_log: Option<String>,
}

impl AgentConfig {
    /// Create a new agent configuration.
    pub fn new(id: impl Into<AgentId>) -> Self {
        Self {
            id: id.into(),
            system_prompt: None,
            max_turns: 10_000,
            stateful: false,
            tool_limits: None,
            compactor: None,
            observation_config: None,
            max_observations: None,
            prior_observation_log: None,
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

    /// Set the context compactor for observational memory.
    pub fn with_compactor(mut self, compactor: Arc<dyn ContextCompactor>) -> Self {
        self.compactor = Some(compactor);
        self
    }

    /// Set the observation config (thresholds).
    pub fn with_observation_config(mut self, config: ObservationConfig) -> Self {
        self.observation_config = Some(config);
        self
    }

    /// Set the maximum observations before requesting wrap-up.
    pub fn with_max_observations(mut self, max: u32) -> Self {
        self.max_observations = Some(max);
        self
    }

    /// Set a prior observation log to restore when resuming.
    pub fn with_prior_observation_log(mut self, log: String) -> Self {
        if !log.is_empty() {
            self.prior_observation_log = Some(log);
        }
        self
    }
}

impl std::fmt::Debug for AgentConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AgentConfig")
            .field("id", &self.id)
            .field("system_prompt", &self.system_prompt.as_deref().map(|s| {
                if s.len() > 50 { &s[..50] } else { s }
            }))
            .field("max_turns", &self.max_turns)
            .field("stateful", &self.stateful)
            .field("tool_limits", &self.tool_limits)
            .field("has_compactor", &self.compactor.is_some())
            .field("observation_config", &self.observation_config)
            .field("max_observations", &self.max_observations)
            .field("has_prior_obs_log", &self.prior_observation_log.is_some())
            .finish()
    }
}

/// Events emitted during agent execution for progress reporting.
#[derive(Debug, Clone)]
pub enum AgentProgressEvent {
    /// An iteration of the agent loop has started.
    IterationStart {
        agent_name: String,
        iteration: u32,
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
    /// An observation pass completed during the agent loop.
    ObservationComplete {
        agent_name: String,
        observation_count: u32,
        log_bytes: usize,
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
            AgentRunResult::ObservationLimitReached { content, .. } => Ok(content),
            AgentRunResult::MaxIterationsExceeded { .. } => {
                Err(Error::Unknown("Agent exceeded max iterations".into()))
            }
            AgentRunResult::RepetitionDetected { .. } => {
                Err(Error::Unknown("Agent stuck in repetitive loop".into()))
            }
            AgentRunResult::TruncatedByLength { partial_content, .. } => {
                Err(Error::Unknown(format!(
                    "Agent response truncated by provider's max-tokens limit. \
                     Partial output: {}",
                    partial_content
                )))
            }
        }
    }

    /// Run a one-shot task with progress reporting.
    ///
    /// When a progress handler is provided, this uses streaming for LLM calls
    /// and emits progress events for thinking content, tool execution, and usage.
    ///
    /// If `config.compactor` is provided, observational memory runs inside the loop:
    /// after each tool execution round, old messages are observed and drained,
    /// with the observation log injected as a system message. The `max_observations`
    /// config controls when the agent is asked to wrap up.
    ///
    /// Returns `AgentRunResult::Success` on completion,
    /// `AgentRunResult::ObservationLimitReached` when max observations is hit, or
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
            has_compactor = config.compactor.is_some(),
            "Agent run_once starting"
        );

        // Hold system prompt separately — prepended to request each iteration.
        let system_prompt = config.system_prompt.clone();

        // All conversation messages (context + accumulated loop messages).
        // ObservationalMemory drains from this vec when it compacts.
        let mut messages: Vec<Message> = context;

        // Initialize observational memory if compactor is provided
        let mut obs_memory = if config.compactor.is_some() {
            let obs_config = config
                .observation_config
                .clone()
                .unwrap_or_else(ObservationConfig::for_agents);
            let om = if let Some(ref prior_log) = config.prior_observation_log {
                ObservationalMemory::with_observation_log(obs_config, prior_log.clone())
            } else {
                ObservationalMemory::new(obs_config)
            };
            Some(om)
        } else {
            None
        };

        // Track tool call counts for enforcing limits
        let mut tool_call_counts: HashMap<String, usize> = HashMap::new();

        // Consecutive emergency-compaction-then-retry attempts. Reset whenever we
        // get a non-truncated response. Capped to prevent infinite loops when
        // compaction can't actually free meaningful space (e.g., long
        // user-recent messages already protected by `preserve_recent`).
        let mut consecutive_emergency_compactions: u32 = 0;

        // Repetition detector: catches agents stuck calling the same tool with same args
        let mut repetition_detector = RepetitionDetector::new();

        // Wrap-up tracking for obs memory
        let mut wrap_up_injected = false;
        let mut wrap_up_iteration: usize = 0;

        // Run agentic loop (safety ceiling only; repetition detector is primary stop)
        for iteration in 0..config.max_turns {
            // Emit iteration start event
            if let Some(ref handler) = progress {
                handler
                    .on_progress(AgentProgressEvent::IterationStart {
                        agent_name: agent_name.clone(),
                        iteration: iteration as u32 + 1,
                    })
                    .await;
            }

            debug!(
                agent = %config.id,
                iteration = iteration,
                message_count = messages.len(),
                "Agent iteration starting"
            );

            // Build request messages: single system prompt (with obs log merged) + conversation messages
            let mut request_messages = Vec::new();
            let obs_log = obs_memory.as_ref().map(|om| om.observation_log()).unwrap_or_default();
            let has_system = system_prompt.is_some();
            let has_log = !obs_log.is_empty();

            if has_system || has_log {
                let mut system_content = system_prompt.clone().unwrap_or_default();
                if has_log {
                    if !system_content.is_empty() {
                        system_content.push_str("\n\n");
                    }
                    system_content.push_str(&format!(
                        "## Investigation Summary\n\n\
                     The following are findings from investigation steps that have already been completed.\n\
                     Do NOT re-investigate these topics or call tools with similar arguments.\n\
                     Build on these findings instead of repeating them.\n\n{}",
                        obs_log
                    ));
                }
                request_messages.push(Message::system(system_content.as_str()));
            }
            request_messages.extend(messages.iter().cloned());

            // Count input bytes (messages being sent)
            let input_bytes: usize = request_messages.iter()
                .map(|m| m.byte_count())
                .sum();

            // Use streaming if we have a progress handler, otherwise use complete()
            // Wrap with retry logic for transient transport/stream errors
            let (content, tool_calls, usage, thinking, finish_reason) = {
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
                        request_messages.clone(),
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
                        usage: usage.clone(),
                    })
                    .await;
            }

            // Detect truncation: the provider cut the response off at max-tokens.
            // Distinct from "model has nothing more to say" — never treat as success.
            // If truncated AND there are tool_calls, those calls were mid-stream
            // when the model was cut off, so their JSON args are likely malformed
            // (e.g. `{"command":"sed -n '958,970p` with no closing quote). Don't
            // execute them.
            //
            // If the cause was the model's context window (not a per-request
            // max_tokens cap), we have a real recovery path: run an emergency
            // compaction pass and retry the same iteration with smaller context.
            // Capped at MAX_CONSECUTIVE_EMERGENCY_COMPACTIONS to prevent infinite
            // loops if compaction can't free meaningful space.
            if matches!(finish_reason, Some(FinishReason::Length)) {
                let total_tokens = (usage.prompt_tokens as u64) + (usage.completion_tokens as u64);
                let context_full = match provider.context_window() {
                    Some(window) => {
                        total_tokens >= (window as u64).saturating_sub(CONTEXT_WINDOW_BUFFER as u64)
                    }
                    None => false,
                };

                tracing::warn!(
                    agent = %config.id,
                    iterations = iteration + 1,
                    response_len = content.len(),
                    pending_tool_calls = tool_calls.len(),
                    total_tokens = total_tokens,
                    context_window = ?provider.context_window(),
                    context_full = context_full,
                    consecutive_emergency_compactions = consecutive_emergency_compactions,
                    "Agent response truncated by max-tokens limit"
                );

                // Try emergency compaction if: context window was the cause,
                // we have a compactor, and we haven't exhausted retries.
                if context_full
                    && consecutive_emergency_compactions < MAX_CONSECUTIVE_EMERGENCY_COMPACTIONS
                {
                    if let (Some(ref mut om), Some(ref compactor)) =
                        (&mut obs_memory, &config.compactor)
                    {
                        let bytes_before: usize =
                            messages.iter().map(|m| m.byte_count()).sum();
                        match om.compact_force(&mut messages, compactor.as_ref()).await {
                            Ok(()) => {
                                let bytes_after: usize =
                                    messages.iter().map(|m| m.byte_count()).sum();
                                if bytes_after < bytes_before {
                                    tracing::info!(
                                        agent = %config.id,
                                        bytes_before = bytes_before,
                                        bytes_after = bytes_after,
                                        bytes_freed = bytes_before - bytes_after,
                                        "Emergency compaction freed space — retrying iteration"
                                    );
                                    consecutive_emergency_compactions += 1;
                                    if let Some(ref handler) = progress {
                                        handler
                                            .on_progress(AgentProgressEvent::ObservationComplete {
                                                agent_name: agent_name.clone(),
                                                observation_count: om.observation_count(),
                                                log_bytes: om.observation_log().len(),
                                            })
                                            .await;
                                    }
                                    continue;
                                }
                                tracing::warn!(
                                    agent = %config.id,
                                    "Emergency compaction freed no space — surfacing as truncation"
                                );
                            }
                            Err(e) => {
                                tracing::warn!(
                                    agent = %config.id,
                                    error = %e,
                                    "Emergency compaction failed — surfacing as truncation"
                                );
                            }
                        }
                    }
                }

                let obs_log = obs_memory
                    .map(|om| om.into_parts().0)
                    .unwrap_or_default();
                return Ok(AgentRunResult::TruncatedByLength {
                    partial_content: content,
                    messages,
                    observation_log: obs_log,
                    was_context_full: context_full,
                });
            }

            // Non-truncated response — reset the emergency-compaction counter so
            // future truncations later in the same run get a fresh budget.
            consecutive_emergency_compactions = 0;

            // Check for tool calls
            if !tool_calls.is_empty() {
                debug!(
                    agent = %config.id,
                    tool_count = tool_calls.len(),
                    "Agent executing tools"
                );

                // Store message with tool calls; attach reasoning if configured
                let reasoning = if provider.include_tool_reasoning() { thinking } else { None };
                let msg = Message::assistant_with_tool_calls("", tool_calls.clone())
                    .with_reasoning(reasoning);
                messages.push(msg);

                // Check tool limits and repetition, partition into executable vs blocked
                let mut executable_calls = Vec::new();
                let mut blocked_count = 0usize;
                for tool_call in &tool_calls {
                    // Check per-tool call limits
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

                        messages.push(Message::tool_result(&tool_call.id, result));
                        blocked_count += 1;
                        continue;
                    }

                    // Check repetition detector (exact match + tool fatigue)
                    if let Some(rep_message) = repetition_detector.check(&tool_call.name, &tool_call.arguments) {
                        debug!(
                            agent = %config.id,
                            tool = %tool_call.name,
                            message = %rep_message,
                            "Repetitive tool call detected"
                        );

                        if let Some(ref handler) = progress {
                            handler
                                .on_progress(AgentProgressEvent::ToolStart {
                                    agent_name: agent_name.clone(),
                                    tool_name: tool_call.name.clone(),
                                    arguments: tool_call.arguments.to_string(),
                                })
                                .await;
                        }

                        let result = truncate_tool_result(rep_message, MAX_AGENT_TOOL_RESULT_BYTES);

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

                        messages.push(Message::tool_result(&tool_call.id, result));
                        blocked_count += 1;
                        continue;
                    }

                    *tool_call_counts.entry(tool_call.name.clone()).or_insert(0) += 1;
                    executable_calls.push(tool_call);
                }

                // If ALL tool calls were blocked, the agent is stuck — hard terminate
                if blocked_count == tool_calls.len() {
                    let obs_log = obs_memory
                        .map(|om| om.into_parts().0)
                        .unwrap_or_default();
                    return Ok(AgentRunResult::RepetitionDetected {
                        messages,
                        observation_log: obs_log,
                    });
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

                    messages.push(Message::tool_result(&tool_call.id, result));
                }

                // Run observational memory compaction after tool execution
                if let (Some(ref mut om), Some(ref compactor)) =
                    (&mut obs_memory, &config.compactor)
                {
                    if let Err(e) = om.compact(&mut messages, compactor.as_ref()).await {
                        tracing::warn!(
                            agent = %config.id,
                            error = %e,
                            "Observation compaction error (continuing)"
                        );
                    }

                    let obs_count = om.observation_count();

                    // Emit observation event if an observation just happened
                    if obs_count > 0 {
                        if let Some(ref handler) = progress {
                            handler
                                .on_progress(AgentProgressEvent::ObservationComplete {
                                    agent_name: agent_name.clone(),
                                    observation_count: obs_count,
                                    log_bytes: om.log_bytes(),
                                })
                                .await;
                        }
                    }

                    // Check max_observations limit
                    if let Some(max_obs) = config.max_observations {
                        if obs_count >= max_obs && !wrap_up_injected {
                            debug!(
                                agent = %config.id,
                                obs_count = obs_count,
                                max_obs = max_obs,
                                "Max observations reached, injecting wrap-up"
                            );
                            messages.push(Message::user(
                                "You are approaching the context limit for this session. \
                                 Please wrap up your current work and provide a final response \
                                 with your findings and any remaining recommendations."
                            ));
                            wrap_up_injected = true;
                            wrap_up_iteration = iteration;
                        }

                        // Hard stop after grace period
                        if wrap_up_injected && iteration >= wrap_up_iteration + 3 {
                            let obs_log = om.observation_log().to_string();
                            return Ok(AgentRunResult::ObservationLimitReached {
                                content: String::new(),
                                messages,
                                observation_log: obs_log,
                            });
                        }
                    }
                }

                continue;
            }

            // No tool calls - strip reasoning from history and return final response
            crate::message::strip_reasoning_from_history(&mut messages);
            debug!(
                agent = %config.id,
                iterations = iteration + 1,
                response_len = content.len(),
                "Agent completed successfully"
            );
            let obs_log = obs_memory
                .map(|om| om.into_parts().0)
                .unwrap_or_default();
            return Ok(AgentRunResult::Success {
                content,
                messages,
                observation_log: obs_log,
            });
        }

        let obs_log = obs_memory
            .map(|om| om.into_parts().0)
            .unwrap_or_default();
        Ok(AgentRunResult::MaxIterationsExceeded {
            messages,
            observation_log: obs_log,
        })
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
                // Add assistant message with tool calls; attach reasoning if configured
                let reasoning = if self.provider.include_tool_reasoning() {
                    response.thinking
                } else {
                    None
                };
                let msg = Message::assistant_with_tool_calls("", response.message.tool_calls.clone())
                    .with_reasoning(reasoning);
                self.messages.push(msg);

                // Execute tools
                for tool_call in &response.message.tool_calls {
                    let result = execute_tool(&self.tools, tool_call).await;
                    self.messages.push(Message::tool_result(&tool_call.id, result));
                }

                continue;
            }

            // No tool calls - strip reasoning from history, save and return response
            crate::message::strip_reasoning_from_history(&mut self.messages);
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

/// Tokens reserved when comparing usage against `Provider::context_window()`.
/// Providers often stop a few hundred tokens shy of the hard ceiling, and a
/// strict equality check would miss obvious context-window-full cases.
const CONTEXT_WINDOW_BUFFER: u32 = 1024;

/// Maximum consecutive emergency-compaction retries within a single agent run.
/// If we hit truncation twice in a row even after compacting, compaction can't
/// free enough space — surface a hard error rather than burn through max_turns.
const MAX_CONSECUTIVE_EMERGENCY_COMPACTIONS: u32 = 2;

/// Run a single iteration using streaming (for progress reporting).
/// Returns (content, tool_calls, usage, thinking, finish_reason).
async fn run_streaming_iteration(
    provider: &Arc<dyn Provider>,
    agent_name: &str,
    request: CompletionRequest,
    progress: Option<&Arc<dyn AgentProgressHandler>>,
) -> Result<(String, Vec<crate::message::ToolCall>, Usage, Option<String>, Option<FinishReason>), Error> {
    use tracing::debug;

    let mut stream = provider.stream(request).await?;

    let mut content = String::new();
    let mut thinking_content = String::new();
    let mut tool_calls: Vec<crate::message::ToolCall> = Vec::new();
    let mut current_tool_call: Option<(String, String, String)> = None;
    let mut usage = Usage::default();
    let mut finish_reason: Option<FinishReason> = None;

    loop {
        match tokio::time::timeout(STREAM_CHUNK_TIMEOUT, stream.next()).await {
            Ok(Some(chunk)) => {
                match chunk {
                    Ok(StreamChunk::Start { model: _ }) => {
                        // Model started
                    }
                    Ok(StreamChunk::ThinkingDelta { content: delta }) => {
                        // Accumulate thinking content for potential round-tripping
                        thinking_content.push_str(&delta);
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
                    Ok(StreamChunk::Done { usage: u, finish_reason: fr }) => {
                        // Finish pending tool call
                        if let Some((tc_id, tc_name, tc_args)) = current_tool_call.take() {
                            let args: serde_json::Value =
                                serde_json::from_str(&tc_args).unwrap_or(serde_json::Value::Null);
                            tool_calls.push(crate::message::ToolCall::new(tc_id, tc_name, args));
                        }
                        if let Some(u) = u {
                            usage = u;
                        }
                        if fr.is_some() {
                            finish_reason = fr;
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

    let thinking = if thinking_content.is_empty() {
        None
    } else {
        Some(thinking_content)
    };
    Ok((content, tool_calls, usage, thinking, finish_reason))
}

/// Run a single iteration using non-streaming complete() (when no progress handler).
/// Returns (content, tool_calls, usage, thinking, finish_reason).
async fn run_complete_iteration(
    provider: &Arc<dyn Provider>,
    config: &AgentConfig,
    mut request: CompletionRequest,
) -> Result<(String, Vec<crate::message::ToolCall>, Usage, Option<String>, Option<FinishReason>), Error> {
    use tracing::debug;

    request.stream = false;

    let response = provider.complete(request).await?;

    // Log if thinking was extracted
    if let Some(ref thinking) = response.thinking {
        debug!(
            agent = %config.id,
            thinking_len = thinking.len(),
            "Extracted thinking content"
        );
    }

    let content = response.message.content.to_string_lossy();
    let tool_calls = response.message.tool_calls;
    let usage = response.usage;
    let thinking = response.thinking;
    let finish_reason = Some(response.finish_reason);

    Ok((content, tool_calls, usage, thinking, finish_reason))
}

/// Maximum bytes for a tool result in agent context.
/// Matches `ChunkerConfig::default_threshold_bytes` (50KB).
const MAX_AGENT_TOOL_RESULT_BYTES: usize = 50_000;

/// Truncate a tool result string to fit within a byte budget.
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

/// Execute a single tool call, returning text-only content.
///
/// Agents use observation memory with limited context budgets. Passing raw
/// image data would bloat messages, get stripped during compaction, and cause
/// the agent to lose track of results and retry in a loop. Instead, agents
/// receive only the text portions of tool output (which includes metadata
/// like dimensions and format for images).
async fn execute_tool(
    registry: &ToolRegistry,
    tool_call: &crate::message::ToolCall,
) -> String {
    let Some(tool) = registry.get_arc(&tool_call.name) else {
        return format!("Error: Unknown tool '{}'", tool_call.name);
    };

    match crate::tool::execute_tool_dispatch(tool, tool_call.arguments.clone()).await {
        Ok(output) => {
            let text = output.text_content();
            let content = if output.is_error {
                format!("Error: {}", text)
            } else {
                text
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
    fn delegation_permissions_restrict_with_truth_table() {
        let ro = DelegationPermissions { read_only: true };
        let rw = DelegationPermissions { read_only: false };

        assert_eq!(ro.restrict_with(rw), ro, "inherited RO ∘ declared RW -> RO");
        assert_eq!(rw.restrict_with(ro), ro, "inherited RW ∘ declared RO -> RO");
        assert_eq!(ro.restrict_with(ro), ro, "RO ∘ RO -> RO");
        assert_eq!(rw.restrict_with(rw), rw, "RW ∘ RW -> RW");
    }

    #[test]
    fn delegation_permissions_default_is_permissive() {
        assert_eq!(DelegationPermissions::default().read_only, false);
    }

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

    #[test]
    fn test_repetition_detector_allows_below_threshold() {
        let mut detector = RepetitionDetector::new();
        let args = serde_json::json!({"query": "test"});
        assert!(detector.check("web_search", &args).is_none());
        assert!(detector.check("web_search", &args).is_none());
    }

    #[test]
    fn test_repetition_detector_blocks_at_threshold() {
        let mut detector = RepetitionDetector::new();
        let args = serde_json::json!({"query": "test"});
        detector.check("web_search", &args); // 1
        detector.check("web_search", &args); // 2
        let result = detector.check("web_search", &args); // 3
        assert!(result.is_some());
        let msg = result.unwrap();
        assert!(msg.contains("identical arguments 3 times"));
    }

    #[test]
    fn test_repetition_detector_different_args_independent() {
        let mut detector = RepetitionDetector::new();
        let args1 = serde_json::json!({"query": "hello"});
        let args2 = serde_json::json!({"query": "world"});
        detector.check("web_search", &args1);
        detector.check("web_search", &args2);
        detector.check("web_search", &args1);
        // Still below threshold for args1 (2 calls) and args2 (1 call)
        assert!(detector.check("web_search", &args2).is_none());
    }

    #[test]
    fn test_repetition_detector_different_tools_independent() {
        let mut detector = RepetitionDetector::new();
        let args = serde_json::json!({"path": "foo.rs"});
        detector.check("read_file", &args);
        detector.check("read_file", &args);
        detector.check("write_file", &args);
        // read_file at 2, write_file at 1 — neither at threshold
        assert!(detector.check("write_file", &args).is_none());
    }

    #[test]
    fn test_repetition_detector_tool_fatigue() {
        let mut detector = RepetitionDetector::new();
        // Call the same tool 5 times with different arguments — should trigger fatigue warning
        for i in 0..4 {
            let args = serde_json::json!({"query": format!("search variant {}", i)});
            assert!(detector.check("web_search", &args).is_none(), "iteration {}", i);
        }
        // 5th call triggers fatigue warning
        let args = serde_json::json!({"query": "search variant 4"});
        let result = detector.check("web_search", &args);
        assert!(result.is_some());
        let msg = result.unwrap();
        assert!(msg.contains("web_search"));
        assert!(msg.contains("5 times"));
    }

    #[test]
    fn test_repetition_detector_fatigue_per_tool() {
        let mut detector = RepetitionDetector::new();
        // web_search called 5 times, read_file called 2 times — only web_search fatigued
        for i in 0..5 {
            let args = serde_json::json!({"query": format!("q{}", i)});
            detector.check("web_search", &args);
        }
        // read_file should still be fine
        let args = serde_json::json!({"path": "foo.rs"});
        assert!(detector.check("read_file", &args).is_none());
    }

    #[test]
    fn test_canonical_hash_key_order_independent() {
        let args1 = serde_json::json!({"a": 1, "b": 2});
        let args2 = serde_json::json!({"b": 2, "a": 1});
        assert_eq!(
            RepetitionDetector::canonical_hash("tool", &args1),
            RepetitionDetector::canonical_hash("tool", &args2),
        );
    }

    #[test]
    fn test_canonical_hash_nested_key_order() {
        let args1 = serde_json::json!({"outer": {"z": 1, "a": 2}});
        let args2 = serde_json::json!({"outer": {"a": 2, "z": 1}});
        assert_eq!(
            RepetitionDetector::canonical_hash("tool", &args1),
            RepetitionDetector::canonical_hash("tool", &args2),
        );
    }

    #[test]
    fn test_canonical_hash_different_values_differ() {
        let args1 = serde_json::json!({"query": "hello"});
        let args2 = serde_json::json!({"query": "world"});
        assert_ne!(
            RepetitionDetector::canonical_hash("tool", &args1),
            RepetitionDetector::canonical_hash("tool", &args2),
        );
    }

    // -------------------------------------------------------------------
    // Truncation handling tests
    //
    // Regression tests for the bug where qwen (or any provider) ending a
    // stream with finish_reason=length silently produced AgentRunResult::
    // Success { content: "" }. Parent agents had no signal, dispatched
    // another sub-agent, looped indefinitely.
    // -------------------------------------------------------------------

    use crate::testing::MockProvider;
    use crate::tool::ToolRegistry;

    fn empty_tools() -> Arc<ToolRegistry> {
        Arc::new(ToolRegistry::new())
    }

    #[tokio::test]
    async fn truncation_with_empty_response_returns_truncated_by_length() {
        let provider = Arc::new(MockProvider::new());
        provider.queue_response_with_finish("", FinishReason::Length);
        let provider: Arc<dyn Provider> = provider;

        let config = AgentConfig::new("test-agent");
        let result = Agent::run_once_with_progress(
            provider,
            empty_tools(),
            config,
            vec![Message::user("do a thing")],
            None,
        )
        .await
        .expect("agent run should not error");

        match result {
            AgentRunResult::TruncatedByLength { partial_content, .. } => {
                assert_eq!(partial_content, "");
            }
            other => panic!("expected TruncatedByLength, got {:?}", other),
        }
    }

    #[tokio::test]
    async fn truncation_with_partial_response_preserves_partial_content() {
        let provider = Arc::new(MockProvider::new());
        provider.queue_response_with_finish(
            "I was about to fix the bug when I got cut off",
            FinishReason::Length,
        );
        let provider: Arc<dyn Provider> = provider;

        let config = AgentConfig::new("test-agent");
        let result = Agent::run_once_with_progress(
            provider,
            empty_tools(),
            config,
            vec![Message::user("fix the bug")],
            None,
        )
        .await
        .expect("agent run should not error");

        match result {
            AgentRunResult::TruncatedByLength { partial_content, .. } => {
                assert!(
                    partial_content.contains("cut off"),
                    "partial_content should preserve what the model managed to say"
                );
            }
            other => panic!("expected TruncatedByLength, got {:?}", other),
        }
    }

    #[tokio::test]
    async fn stop_finish_reason_yields_normal_success() {
        // Sanity check: the truncation path must not fire on normal completion.
        let provider = Arc::new(MockProvider::new());
        provider.queue_response_with_finish("done", FinishReason::Stop);
        let provider: Arc<dyn Provider> = provider;

        let config = AgentConfig::new("test-agent");
        let result = Agent::run_once_with_progress(
            provider,
            empty_tools(),
            config,
            vec![Message::user("do a thing")],
            None,
        )
        .await
        .expect("agent run should not error");

        match result {
            AgentRunResult::Success { content, .. } => assert_eq!(content, "done"),
            other => panic!("expected Success, got {:?}", other),
        }
    }

    #[tokio::test]
    async fn truncation_via_streaming_path_returns_truncated_by_length() {
        // Verify the streaming code path (run_streaming_iteration) propagates
        // finish_reason from StreamChunk::Done — distinct from the
        // non-streaming path which reads CompletionResponse.finish_reason.
        use crate::message::StreamChunk;

        let provider = Arc::new(MockProvider::new());
        provider.queue_stream(vec![
            StreamChunk::Start { model: "mock".into() },
            StreamChunk::Delta { content: "Let me try the urlparse approach... actually wait,".into() },
            StreamChunk::Done {
                usage: None,
                finish_reason: Some(FinishReason::Length),
            },
        ]);
        let provider: Arc<dyn Provider> = provider;

        // Streaming path requires a progress handler. Use a no-op handler.
        struct NoopHandler;
        #[async_trait]
        impl AgentProgressHandler for NoopHandler {
            async fn on_progress(&self, _event: AgentProgressEvent) {}
        }
        let progress: Arc<dyn AgentProgressHandler> = Arc::new(NoopHandler);

        let config = AgentConfig::new("test-agent");
        let result = Agent::run_once_with_progress(
            provider,
            empty_tools(),
            config,
            vec![Message::user("fix the bug")],
            Some(progress),
        )
        .await
        .expect("agent run should not error");

        match result {
            AgentRunResult::TruncatedByLength { partial_content, .. } => {
                assert!(partial_content.contains("urlparse"));
            }
            other => panic!("expected TruncatedByLength, got {:?}", other),
        }
    }

    // -------------------------------------------------------------------
    // Context-window-full recovery: emergency compaction + retry
    // -------------------------------------------------------------------

    use crate::observation::ObservationConfig;
    use crate::provider::CompletionResponse;
    use crate::testing::MockCompactor;

    /// Build a CompletionResponse with the given content, finish_reason, and
    /// usage so tests can simulate "context window full" responses.
    fn truncated_response(
        content: &str,
        prompt_tokens: u32,
        completion_tokens: u32,
    ) -> CompletionResponse {
        CompletionResponse {
            message: Message::assistant(content),
            thinking: None,
            usage: Usage::new(prompt_tokens, completion_tokens),
            model: "mock".to_string(),
            finish_reason: FinishReason::Length,
        }
    }

    fn ok_response(content: &str) -> CompletionResponse {
        CompletionResponse {
            message: Message::assistant(content),
            thinking: None,
            usage: Usage::new(0, 0),
            model: "mock".to_string(),
            finish_reason: FinishReason::Stop,
        }
    }

    fn long_messages(count: usize) -> Vec<Message> {
        // Each message is large enough to exceed the message_threshold so the
        // emergency compactor has something to drain.
        (0..count)
            .map(|i| Message::user(&format!("message {} {}", i, "x".repeat(2_000))))
            .collect()
    }

    #[tokio::test]
    async fn context_full_with_compactor_runs_emergency_compaction_and_retries() {
        // Provider returns: 1) truncated response with usage at context window,
        // 2) successful response. Agent should detect context-full, force-compact,
        // retry, and return Success.
        let provider = Arc::new(
            crate::testing::MockProvider::new().with_context_window(4_000),
        );
        // FIFO queueing: queue_raw_response inserts at index 0, so the most
        // recently queued response is popped first. Order matters — we want
        // the truncated response to come out FIRST.
        provider.queue_raw_response(ok_response("done after compaction"));
        provider.queue_raw_response(truncated_response("partial...", 3_500, 500));
        let provider: Arc<dyn Provider> = provider;

        let compactor = Arc::new(MockCompactor::new());
        compactor.queue_observe(Ok("- saw a thing".to_string()));
        let compactor: Arc<dyn ContextCompactor> = compactor;

        let config = AgentConfig::new("test-agent")
            .with_compactor(Arc::clone(&compactor))
            .with_observation_config(ObservationConfig {
                message_threshold_bytes: 1_000,
                observation_threshold_bytes: 1_000_000,
                preserve_recent: 2,
                hysteresis: 1.0,
                context_budget_bytes: None,
            });

        // Pre-load enough messages that compact_force has something meaningful
        // to drain (preserve_recent=2 keeps the last two; everything before is
        // observable).
        let context = long_messages(10);

        let result = Agent::run_once(provider, empty_tools(), config, context)
            .await
            .expect("agent run should not error");

        assert_eq!(result, "done after compaction");
    }

    #[tokio::test]
    async fn context_full_without_compactor_surfaces_truncation() {
        // No compactor configured — agent should surface TruncatedByLength
        // immediately with `was_context_full=true` so the parent knows the
        // task hit the context wall (not a per-request max_tokens cap).
        let provider = Arc::new(
            crate::testing::MockProvider::new().with_context_window(4_000),
        );
        provider.queue_raw_response(truncated_response("", 3_500, 500));
        let provider: Arc<dyn Provider> = provider;

        let config = AgentConfig::new("test-agent");
        let result = Agent::run_once_with_progress(
            provider,
            empty_tools(),
            config,
            vec![Message::user("do a thing")],
            None,
        )
        .await
        .expect("agent run should not error");

        match result {
            AgentRunResult::TruncatedByLength { was_context_full, .. } => {
                assert!(
                    was_context_full,
                    "should detect context window exhaustion via usage vs context_window()"
                );
            }
            other => panic!("expected TruncatedByLength, got {:?}", other),
        }
    }

    #[tokio::test]
    async fn max_tokens_cap_hit_does_not_trigger_compaction() {
        // total_tokens (1000) is well below context_window (128_000), so this
        // is a per-request max_tokens cap, not context-window exhaustion.
        // Compaction should NOT run and the variant must report
        // `was_context_full=false` so the parent gets the right advice.
        let provider = Arc::new(
            crate::testing::MockProvider::new().with_context_window(128_000),
        );
        provider.queue_raw_response(truncated_response("partial", 500, 500));
        let provider: Arc<dyn Provider> = provider;

        // Even with a compactor available, a non-context-full truncation must
        // not invoke it.
        let compactor = Arc::new(MockCompactor::new());
        let compactor: Arc<dyn ContextCompactor> = compactor.clone();
        let config = AgentConfig::new("test-agent").with_compactor(Arc::clone(&compactor));

        let result = Agent::run_once_with_progress(
            provider,
            empty_tools(),
            config,
            vec![Message::user("be brief")],
            None,
        )
        .await
        .expect("agent run should not error");

        match result {
            AgentRunResult::TruncatedByLength { was_context_full, .. } => {
                assert!(
                    !was_context_full,
                    "max_tokens cap should report was_context_full=false"
                );
            }
            other => panic!("expected TruncatedByLength, got {:?}", other),
        }
    }

    #[tokio::test]
    async fn consecutive_emergency_compactions_capped() {
        // Every response truncates with context-window-full usage. After
        // MAX_CONSECUTIVE_EMERGENCY_COMPACTIONS retries, the loop must give up
        // and surface TruncatedByLength rather than burn through max_turns.
        let provider = Arc::new(
            crate::testing::MockProvider::new().with_context_window(4_000),
        );
        // Queue many — agent will pop until cap, then surface error.
        for _ in 0..5 {
            provider.queue_raw_response(truncated_response("...", 3_500, 500));
        }
        let provider: Arc<dyn Provider> = provider;

        let compactor = Arc::new(MockCompactor::new());
        // Each compaction returns a non-empty observation so compact_force
        // actually drains and bytes_after < bytes_before.
        for _ in 0..5 {
            compactor.queue_observe(Ok("- drain".to_string()));
        }
        let compactor: Arc<dyn ContextCompactor> = compactor;

        let config = AgentConfig::new("test-agent")
            .with_compactor(Arc::clone(&compactor))
            .with_observation_config(ObservationConfig {
                message_threshold_bytes: 1_000,
                observation_threshold_bytes: 1_000_000,
                preserve_recent: 2,
                hysteresis: 1.0,
                context_budget_bytes: None,
            });

        let result = Agent::run_once_with_progress(
            provider,
            empty_tools(),
            config,
            long_messages(20),
            None,
        )
        .await
        .expect("agent run should not error");

        match result {
            AgentRunResult::TruncatedByLength { was_context_full, .. } => {
                assert!(was_context_full);
            }
            other => panic!("expected TruncatedByLength after cap, got {:?}", other),
        }
    }
}
