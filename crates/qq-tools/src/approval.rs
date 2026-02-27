//! Shared approval channel for interactive user consent.
//!
//! Used by both the bash tool (command-level approval) and filesystem
//! write tools (file operation approval).

use tokio::sync::{mpsc, oneshot};

/// Request sent to the UI for user approval.
pub struct ApprovalRequest {
    /// The full command/description the agent wants to execute.
    pub full_command: String,
    /// Which specific items triggered the approval requirement.
    pub trigger_commands: Vec<String>,
    /// Channel to send the user's response back.
    pub response_tx: oneshot::Sender<ApprovalResponse>,
    /// UI category label (e.g. "Bash Command", "File Operation", "Mount").
    pub category: String,
}

/// User's response to an approval request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ApprovalResponse {
    /// Allow this single execution.
    Allow,
    /// Allow and promote trigger commands to session tier.
    AllowForSession,
    /// Deny execution.
    Deny,
}

/// Sender side of the approval channel, held by tools that need user consent.
#[derive(Clone)]
pub struct ApprovalChannel {
    pub(crate) request_tx: mpsc::Sender<ApprovalRequest>,
}

impl ApprovalChannel {
    /// Send an approval request and wait for the response.
    pub async fn request_approval(
        &self,
        full_command: String,
        trigger_commands: Vec<String>,
        category: &str,
    ) -> Result<ApprovalResponse, String> {
        let (response_tx, response_rx) = oneshot::channel();

        self.request_tx
            .send(ApprovalRequest {
                full_command,
                trigger_commands,
                response_tx,
                category: category.to_string(),
            })
            .await
            .map_err(|_| "Approval channel closed".to_string())?;

        response_rx
            .await
            .map_err(|_| "Approval cancelled".to_string())
    }
}

/// Create an approval channel pair.
///
/// Returns the sender (for tools) and receiver (for TUI/CLI).
pub fn create_approval_channel() -> (ApprovalChannel, mpsc::Receiver<ApprovalRequest>) {
    let (tx, rx) = mpsc::channel(8);
    (ApprovalChannel { request_tx: tx }, rx)
}
