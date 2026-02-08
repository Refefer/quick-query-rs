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

    fn tool_description(&self) -> &str {
        "Send a status message to the user without ending your turn.\n\n\
         Use this to keep the user informed about what you're doing \
         (e.g., 'Delegating to researcher...', 'Analyzing results...').\n\n\
         When to use:\n\
         - Before starting significant work: what you are about to do\n\
         - When you discover something notable: key findings or unexpected issues\n\
         - When completing phases of a multi-step task: progress updates\n\
         - When plans change: why you are adjusting your approach\n\n\
         This is fire-and-forget: does not pause execution or wait for a response.\n\
         When executing multi-step plans, report completion of each step, then keep going.\n\
         DO NOT stop between steps to wait for confirmation."
    }

    fn definition(&self) -> ToolDefinition {
        ToolDefinition::new("inform_user", self.tool_description())
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
