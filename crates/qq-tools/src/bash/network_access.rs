//! Tool for requesting user consent to use the network.

use async_trait::async_trait;
use std::sync::RwLock;

use qq_core::{Error, PropertySchema, Tool, ToolDefinition, ToolOutput, ToolParameters};

use super::permissions::{ApprovalChannel, ApprovalResponse};

/// LLM-callable tool for requesting user consent to use the network.
pub struct RequestNetworkAccessTool {
    approval: ApprovalChannel,
    approved: RwLock<bool>,
}

#[derive(serde::Deserialize)]
struct RequestNetworkAccessArgs {
    justification: String,
}

impl RequestNetworkAccessTool {
    pub fn new(approval: ApprovalChannel) -> Self {
        Self {
            approval,
            approved: RwLock::new(false),
        }
    }
}

const NETWORK_ACCESS_TOOL_DESC: &str = "\
Request permission to use the network.

Network access is not available by default. Call this tool to request user approval \
before running commands or tools that require internet connectivity.

Parameters:
  - justification: Explain what you need network access for and why";

#[async_trait]
impl Tool for RequestNetworkAccessTool {
    fn name(&self) -> &str {
        "request_network_access"
    }

    fn description(&self) -> &str {
        "Request user approval before using the network"
    }

    fn tool_description(&self) -> &str {
        NETWORK_ACCESS_TOOL_DESC
    }

    fn definition(&self) -> ToolDefinition {
        ToolDefinition::new(self.name(), self.tool_description()).with_parameters(
            ToolParameters::new().add_property(
                "justification",
                PropertySchema::string("Explain what you need network access for and why"),
                true,
            ),
        )
    }

    async fn execute(&self, arguments: serde_json::Value) -> Result<ToolOutput, Error> {
        let args: RequestNetworkAccessArgs = serde_json::from_value(arguments).map_err(|e| {
            Error::tool(
                "request_network_access",
                format!("Invalid arguments: {}", e),
            )
        })?;

        // Return early if already approved this session
        if let Ok(approved) = self.approved.read() {
            if *approved {
                return Ok(ToolOutput::success("Network access already approved."));
            }
        }

        match self
            .approval
            .request_approval(
                format!("Network access requested: {}", args.justification),
                vec!["Network Access".to_string()],
                "Network Access",
            )
            .await
        {
            Ok(ApprovalResponse::Allow) | Ok(ApprovalResponse::AllowForSession) => {
                if let Ok(mut approved) = self.approved.write() {
                    *approved = true;
                }
                Ok(ToolOutput::success("Network access approved."))
            }
            Ok(ApprovalResponse::Deny(reason)) => {
                let msg = match reason {
                    Some(r) => format!("Network access denied by user: {r}"),
                    None => "Network access denied by user.".to_string(),
                };
                Ok(ToolOutput::error(msg))
            }
            Err(e) => Ok(ToolOutput::error(format!(
                "Approval system unavailable: {}",
                e
            ))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::approval::create_approval_channel;

    #[tokio::test]
    async fn test_already_approved_skips_dialog() {
        let (approval, mut rx) = create_approval_channel();
        let tool = RequestNetworkAccessTool::new(approval);
        // Pre-approve
        *tool.approved.write().unwrap() = true;

        let result = tool
            .execute(serde_json::json!({ "justification": "test" }))
            .await
            .unwrap();

        assert!(!result.is_error, "Should succeed");
        assert!(result.text_content().contains("already approved"));
        assert!(rx.try_recv().is_err(), "No approval request expected");
    }

    #[tokio::test]
    async fn test_deny_returns_error() {
        let (approval, mut rx) = create_approval_channel();
        let tool = RequestNetworkAccessTool::new(approval);

        tokio::spawn(async move {
            if let Some(req) = rx.recv().await {
                let _ = req.response_tx.send(ApprovalResponse::Deny(None));
            }
        });

        let result = tool
            .execute(serde_json::json!({ "justification": "fetch latest docs" }))
            .await
            .unwrap();

        assert!(result.is_error, "Deny → error");
        assert!(result.text_content().contains("denied"));
    }

    #[tokio::test]
    async fn test_allow_sets_approved() {
        let (approval, mut rx) = create_approval_channel();
        let tool = RequestNetworkAccessTool::new(approval);

        tokio::spawn(async move {
            if let Some(req) = rx.recv().await {
                let _ = req.response_tx.send(ApprovalResponse::Allow);
            }
        });

        let result = tool
            .execute(serde_json::json!({ "justification": "run curl" }))
            .await
            .unwrap();

        assert!(
            !result.is_error,
            "Should succeed: {}",
            result.text_content()
        );
        assert!(*tool.approved.read().unwrap(), "Should be marked approved");
    }

    #[tokio::test]
    async fn test_allow_for_session_sets_approved() {
        let (approval, mut rx) = create_approval_channel();
        let tool = RequestNetworkAccessTool::new(approval);

        tokio::spawn(async move {
            if let Some(req) = rx.recv().await {
                let _ = req.response_tx.send(ApprovalResponse::AllowForSession);
            }
        });

        let result = tool
            .execute(serde_json::json!({ "justification": "need ssh" }))
            .await
            .unwrap();

        assert!(
            !result.is_error,
            "Should succeed: {}",
            result.text_content()
        );
        assert!(*tool.approved.read().unwrap(), "Should be marked approved");
    }
}
