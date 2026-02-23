//! Event bus for agent progress events.
//!
//! Provides a decoupled way for agent tools to emit progress events
//! that the TUI can subscribe to.

use std::sync::Arc;

use async_trait::async_trait;
use tokio::sync::broadcast;

use qq_core::{AgentProgressEvent, AgentProgressHandler, Usage};

use crate::debug_log::DebugLogger;

/// Events emitted by agents for TUI consumption.
#[derive(Debug, Clone)]
pub enum AgentEvent {
    /// An agent iteration has started.
    IterationStart {
        agent_name: String,
        iteration: u32,
        max_turns: u32,
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
        arguments: String,
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
    /// Byte count update from an agent.
    ByteCount {
        agent_name: String,
        input_bytes: usize,
        output_bytes: usize,
    },
    /// Non-blocking notification to display to user.
    UserNotification {
        agent_name: String,
        message: String,
    },
    /// Agent is continuing after hitting turn limit.
    ContinuationStarted {
        agent_name: String,
        continuation_number: u32,
        max_continuations: u32,
    },
    /// A transient error occurred and the LLM call is being retried.
    Retry {
        agent_name: String,
        attempt: u32,
        max_retries: u32,
        error: String,
    },
    /// An observation pass completed during agent execution.
    ObservationComplete {
        agent_name: String,
        observation_count: u32,
        log_bytes: usize,
    },
}

impl From<AgentProgressEvent> for AgentEvent {
    fn from(event: AgentProgressEvent) -> Self {
        match event {
            AgentProgressEvent::IterationStart {
                agent_name,
                iteration,
                max_turns,
            } => AgentEvent::IterationStart {
                agent_name,
                iteration,
                max_turns,
            },
            AgentProgressEvent::ThinkingDelta { agent_name, content } => {
                AgentEvent::ThinkingDelta { agent_name, content }
            }
            AgentProgressEvent::ToolStart {
                agent_name,
                tool_name,
                arguments,
            } => AgentEvent::ToolStart {
                agent_name,
                tool_name,
                arguments,
            },
            AgentProgressEvent::ToolComplete {
                agent_name,
                tool_name,
                is_error,
                ..
            } => AgentEvent::ToolComplete {
                agent_name,
                tool_name,
                is_error,
            },
            AgentProgressEvent::UsageUpdate { agent_name, usage } => {
                AgentEvent::UsageUpdate { agent_name, usage }
            }
            AgentProgressEvent::ByteCount {
                agent_name,
                input_bytes,
                output_bytes,
            } => AgentEvent::ByteCount {
                agent_name,
                input_bytes,
                output_bytes,
            },
            AgentProgressEvent::Retry {
                agent_name,
                attempt,
                max_retries,
                error,
            } => AgentEvent::Retry {
                agent_name,
                attempt,
                max_retries,
                error,
            },
            AgentProgressEvent::ObservationComplete {
                agent_name,
                observation_count,
                log_bytes,
            } => AgentEvent::ObservationComplete {
                agent_name,
                observation_count,
                log_bytes,
            },
            // AssistantResponse is only used for debug logging; never broadcast
            AgentProgressEvent::AssistantResponse { .. } => {
                unreachable!("AssistantResponse is filtered before broadcast")
            }
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
    debug_logger: Option<Arc<DebugLogger>>,
}

impl AgentEventBus {
    /// Create a new event bus with the specified channel capacity.
    pub fn new(capacity: usize) -> Self {
        let (tx, _) = broadcast::channel(capacity);
        Self { tx, debug_logger: None }
    }

    /// Attach a debug logger for trace logging of agent tool calls and responses.
    pub fn with_debug_logger(mut self, logger: Arc<DebugLogger>) -> Self {
        self.debug_logger = Some(logger);
        self
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

/// Progress handler that publishes events to an event bus and optionally logs to DebugLogger.
struct EventBusProgressHandler {
    bus: AgentEventBus,
}

#[async_trait]
impl AgentProgressHandler for EventBusProgressHandler {
    async fn on_progress(&self, event: AgentProgressEvent) {
        // Log to DebugLogger before broadcasting (captures full content)
        if let Some(ref logger) = self.bus.debug_logger {
            match &event {
                AgentProgressEvent::ToolStart {
                    tool_name,
                    arguments,
                    ..
                } => {
                    // Parse arguments string back to JSON for structured logging
                    let args_value = serde_json::from_str(arguments)
                        .unwrap_or(serde_json::Value::String(arguments.clone()));
                    logger.log_tool_call_full("", tool_name, &args_value);
                }
                AgentProgressEvent::ToolComplete {
                    tool_name,
                    tool_call_id,
                    result,
                    is_error,
                    ..
                } => {
                    logger.log_tool_result_full(tool_call_id, tool_name, result, *is_error);
                }
                AgentProgressEvent::AssistantResponse {
                    content,
                    tool_call_count,
                    ..
                } => {
                    logger.log_assistant_response(content, None, *tool_call_count);
                }
                _ => {}
            }
        }

        // Broadcast to TUI/subscribers (skipping events that are only for logging)
        if !matches!(event, AgentProgressEvent::AssistantResponse { .. }) {
            self.bus.publish(event.into());
        }
    }
}
