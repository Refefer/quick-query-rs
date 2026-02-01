//! Agent framework for multi-agent communication and coordination.
//!
//! This module provides:
//! - `Agent` for running LLM-powered agents (stateful or stateless)
//! - `AgentChannel` and `AgentSender` for inter-agent communication
//! - `AgentRegistry` for managing multiple agents
//! - Streaming support for real-time agent-to-agent communication

use std::collections::HashMap;
use std::sync::Arc;

use tokio::sync::mpsc;

use crate::error::Error;
use crate::message::Message;
use crate::provider::{CompletionRequest, Provider};
use crate::tool::ToolRegistry;

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
    pub max_iterations: usize,
    /// Whether to maintain message history (stateful mode).
    pub stateful: bool,
}

impl AgentConfig {
    /// Create a new agent configuration.
    pub fn new(id: impl Into<AgentId>) -> Self {
        Self {
            id: id.into(),
            system_prompt: None,
            max_iterations: 20,
            stateful: false,
        }
    }

    /// Set the system prompt.
    pub fn with_system_prompt(mut self, prompt: impl Into<String>) -> Self {
        self.system_prompt = Some(prompt.into());
        self
    }

    /// Set the maximum iterations.
    pub fn with_max_iterations(mut self, max: usize) -> Self {
        self.max_iterations = max;
        self
    }

    /// Enable stateful mode (maintains message history).
    pub fn stateful(mut self) -> Self {
        self.stateful = true;
        self
    }
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
    pub async fn run_once(
        provider: Arc<dyn Provider>,
        tools: Arc<ToolRegistry>,
        config: AgentConfig,
        context: Vec<Message>,
    ) -> Result<String, Error> {
        use tracing::debug;

        debug!(
            agent = %config.id,
            context_messages = context.len(),
            tools_available = tools.definitions().len(),
            "Agent run_once starting"
        );

        // Assertion: agent context should be minimal (typically just the task)
        debug_assert!(
            context.len() <= 2,
            "Agent context should be minimal (task only), got {} messages",
            context.len()
        );

        let mut messages = Vec::new();

        // Add system prompt if configured
        if let Some(system) = &config.system_prompt {
            messages.push(Message::system(system.as_str()));
        }

        // Add provided context
        messages.extend(context);

        // Run agentic loop
        for iteration in 0..config.max_iterations {
            debug!(
                agent = %config.id,
                iteration = iteration,
                message_count = messages.len(),
                "Agent iteration starting"
            );

            let mut request = CompletionRequest::new(messages.clone())
                .with_tools(tools.definitions());

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

            // Check for tool calls
            if !response.message.tool_calls.is_empty() {
                debug!(
                    agent = %config.id,
                    tool_count = response.message.tool_calls.len(),
                    "Agent executing tools"
                );

                // Store message with tool calls but NO content (don't store thinking)
                let msg = Message::assistant_with_tool_calls("", response.message.tool_calls.clone());
                messages.push(msg);

                // Execute tools
                for tool_call in &response.message.tool_calls {
                    debug!(
                        agent = %config.id,
                        tool = %tool_call.name,
                        "Executing tool"
                    );
                    let result = execute_tool(&tools, tool_call).await;
                    messages.push(Message::tool_result(&tool_call.id, result));
                }

                continue;
            }

            // No tool calls - return final response
            let content = response.message.content.to_string_lossy();
            debug!(
                agent = %config.id,
                iterations = iteration + 1,
                response_len = content.len(),
                "Agent completed successfully"
            );
            return Ok(content);
        }

        Err(Error::Unknown(format!(
            "Agent {} exceeded max iterations ({})",
            config.id, config.max_iterations
        )))
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

        // Build messages with system prompt
        let mut messages = Vec::new();
        if let Some(system) = &self.config.system_prompt {
            messages.push(Message::system(system.as_str()));
        }
        messages.extend(self.messages.clone());

        // Run agentic loop
        for _iteration in 0..self.config.max_iterations {
            let mut request = CompletionRequest::new(messages.clone())
                .with_tools(self.tools.definitions());

            request.stream = false;

            let response = self.provider.complete(request).await?;

            // Check for tool calls
            if !response.message.tool_calls.is_empty() {
                // Add assistant message with tool calls but NO content (don't store thinking)
                // Note: response.message.content might contain thinking from some providers
                let msg = Message::assistant_with_tool_calls("", response.message.tool_calls.clone());
                messages.push(msg.clone());
                self.messages.push(msg);

                // Execute tools
                for tool_call in &response.message.tool_calls {
                    let result = execute_tool(&self.tools, tool_call).await;
                    let tool_msg = Message::tool_result(&tool_call.id, &result);
                    messages.push(tool_msg.clone());
                    self.messages.push(tool_msg);
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
            self.config.id, self.config.max_iterations
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

/// Execute a single tool call.
async fn execute_tool(
    registry: &ToolRegistry,
    tool_call: &crate::message::ToolCall,
) -> String {
    let Some(tool) = registry.get(&tool_call.name) else {
        return format!("Error: Unknown tool '{}'", tool_call.name);
    };

    match tool.execute(tool_call.arguments.clone()).await {
        Ok(output) => {
            if output.is_error {
                format!("Error: {}", output.content)
            } else {
                output.content
            }
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
            .with_max_iterations(10)
            .stateful();

        assert_eq!(config.id.0, "my-agent");
        assert_eq!(
            config.system_prompt,
            Some("You are a helpful assistant".to_string())
        );
        assert_eq!(config.max_iterations, 10);
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
}
