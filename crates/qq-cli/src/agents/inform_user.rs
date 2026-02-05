//! InformUser tool for non-blocking agent notifications.
//!
//! This tool allows agents to send status messages to the user without
//! ending their turn. The message is published via the event bus and
//! displayed immediately in the TUI or readline interface.

use async_trait::async_trait;
use serde::Deserialize;

use qq_core::{Error, PropertySchema, Tool, ToolDefinition, ToolOutput, ToolParameters};

use crate::event_bus::{AgentEvent, AgentEventBus};

/// Tool that sends non-blocking notifications to the user.
pub struct InformUserTool {
    event_bus: AgentEventBus,
    agent_name: String,
}

impl InformUserTool {
    pub fn new(event_bus: AgentEventBus, agent_name: &str) -> Self {
        Self {
            event_bus,
            agent_name: agent_name.to_string(),
        }
    }
}

#[derive(Deserialize)]
struct InformUserArgs {
    message: String,
}

#[async_trait]
impl Tool for InformUserTool {
    fn name(&self) -> &str {
        "inform_user"
    }

    fn description(&self) -> &str {
        "Send a status message to the user without ending your turn"
    }

    fn definition(&self) -> ToolDefinition {
        ToolDefinition::new(
            "inform_user",
            "Send a status message to the user without ending your turn. \
             Use this to keep the user informed about what you're doing \
             (e.g., 'Delegating to researcher...', 'Analyzing results...'). \
             The message is displayed immediately and you continue executing.",
        )
        .with_parameters(
            ToolParameters::new().add_property(
                "message",
                PropertySchema::string("The message to display to the user"),
                true,
            ),
        )
    }

    async fn execute(&self, arguments: serde_json::Value) -> Result<ToolOutput, Error> {
        let args: InformUserArgs = serde_json::from_value(arguments)
            .map_err(|e| Error::tool("inform_user", format!("Invalid arguments: {}", e)))?;

        self.event_bus.publish(AgentEvent::UserNotification {
            agent_name: self.agent_name.clone(),
            message: args.message,
        });

        Ok(ToolOutput::success("Message sent to user"))
    }
}
