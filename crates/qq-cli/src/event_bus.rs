//! Event bus for agent progress events.
//!
//! Provides a decoupled way for agent tools to emit progress events
//! that the TUI can subscribe to.

use std::sync::Arc;

use async_trait::async_trait;
use tokio::sync::broadcast;

use qq_core::{AgentProgressEvent, AgentProgressHandler, Usage};

/// Events emitted by agents for TUI consumption.
#[derive(Debug, Clone)]
pub enum AgentEvent {
    /// An agent iteration has started.
    IterationStart {
        agent_name: String,
        iteration: u32,
        max_iterations: u32,
    },
    /// Thinking content from an agent.
    ThinkingDelta {
        agent_name: String,
        content: String,
    },
    /// An agent has started executing a tool.
    ToolStart {
        agent_name: String,
        tool_name: String,
    },
    /// An agent has finished executing a tool.
    ToolComplete {
        agent_name: String,
        tool_name: String,
        is_error: bool,
    },
    /// Usage update from an agent.
    UsageUpdate {
        agent_name: String,
        usage: Usage,
    },
    /// Character count update from an agent.
    CharacterCount {
        agent_name: String,
        input_chars: usize,
        output_chars: usize,
    },
}

impl From<AgentProgressEvent> for AgentEvent {
    fn from(event: AgentProgressEvent) -> Self {
        match event {
            AgentProgressEvent::IterationStart {
                agent_name,
                iteration,
                max_iterations,
            } => AgentEvent::IterationStart {
                agent_name,
                iteration,
                max_iterations,
            },
            AgentProgressEvent::ThinkingDelta { agent_name, content } => {
                AgentEvent::ThinkingDelta { agent_name, content }
            }
            AgentProgressEvent::ToolStart {
                agent_name,
                tool_name,
            } => AgentEvent::ToolStart {
                agent_name,
                tool_name,
            },
            AgentProgressEvent::ToolComplete {
                agent_name,
                tool_name,
                is_error,
            } => AgentEvent::ToolComplete {
                agent_name,
                tool_name,
                is_error,
            },
            AgentProgressEvent::UsageUpdate { agent_name, usage } => {
                AgentEvent::UsageUpdate { agent_name, usage }
            }
            AgentProgressEvent::CharacterCount {
                agent_name,
                input_chars,
                output_chars,
            } => AgentEvent::CharacterCount {
                agent_name,
                input_chars,
                output_chars,
            },
        }
    }
}

/// Event bus for broadcasting agent progress events.
///
/// Clone this to share across agent tools. Each clone shares the same
/// underlying broadcast channel.
#[derive(Clone)]
pub struct AgentEventBus {
    tx: broadcast::Sender<AgentEvent>,
}

impl AgentEventBus {
    /// Create a new event bus with the specified channel capacity.
    pub fn new(capacity: usize) -> Self {
        let (tx, _) = broadcast::channel(capacity);
        Self { tx }
    }

    /// Subscribe to events from this bus.
    pub fn subscribe(&self) -> broadcast::Receiver<AgentEvent> {
        self.tx.subscribe()
    }

    /// Publish an event to all subscribers.
    pub fn publish(&self, event: AgentEvent) {
        // Ignore send errors (no subscribers)
        let _ = self.tx.send(event);
    }

    /// Create a progress handler that publishes to this bus.
    pub fn create_handler(&self) -> Arc<dyn AgentProgressHandler> {
        Arc::new(EventBusProgressHandler {
            bus: self.clone(),
        })
    }
}

/// Progress handler that publishes events to an event bus.
struct EventBusProgressHandler {
    bus: AgentEventBus,
}

#[async_trait]
impl AgentProgressHandler for EventBusProgressHandler {
    async fn on_progress(&self, event: AgentProgressEvent) {
        self.bus.publish(event.into());
    }
}
