//! Tool for requesting internet access by promoting network commands to session tier.

use async_trait::async_trait;
use std::sync::Arc;

use qq_core::{Error, PropertySchema, Tool, ToolDefinition, ToolOutput, ToolParameters};

use super::permissions::{ApprovalChannel, ApprovalResponse, PermissionStore, Tier};
pub use super::permissions::PROMOTABLE_NETWORK_COMMANDS;

/// LLM-callable tool for requesting user approval to use network commands.
pub struct RequestInternetAccessTool {
    permissions: Arc<PermissionStore>,
    approval: ApprovalChannel,
}

#[derive(serde::Deserialize)]
struct RequestInternetAccessArgs {
    reason: String,
}

impl RequestInternetAccessTool {
    pub fn new(permissions: Arc<PermissionStore>, approval: ApprovalChannel) -> Self {
        Self { permissions, approval }
    }
}

const INTERNET_ACCESS_TOOL_DESC: &str = "\
Request permission to use network commands (curl, wget, ssh, etc.).

Network commands are blocked by default. Call this tool to request user approval \
before attempting to use network transfer commands. If approved, the commands \
become available for the rest of the session.

Parameters:
  - reason: Brief explanation of why internet access is needed

Commands enabled upon approval: curl, wget, nc, ncat, socat, ssh, scp, rsync, ftp";

#[async_trait]
impl Tool for RequestInternetAccessTool {
    fn name(&self) -> &str {
        "request_internet_access"
    }

    fn description(&self) -> &str {
        "Request user approval to use network commands (curl, wget, ssh, etc.)"
    }

    fn tool_description(&self) -> &str {
        INTERNET_ACCESS_TOOL_DESC
    }

    fn definition(&self) -> ToolDefinition {
        ToolDefinition::new(self.name(), self.tool_description()).with_parameters(
            ToolParameters::new().add_property(
                "reason",
                PropertySchema::string("Brief explanation of why internet access is needed"),
                true,
            ),
        )
    }

    async fn execute(&self, arguments: serde_json::Value) -> Result<ToolOutput, Error> {
        let args: RequestInternetAccessArgs = serde_json::from_value(arguments)
            .map_err(|e| {
                Error::tool("request_internet_access", format!("Invalid arguments: {}", e))
            })?;

        // Only skip the dialog if commands are already Session-tier (fully automatic).
        // PerCall-promoted commands still show the dialog so the user can upgrade to session.
        let already_session = PROMOTABLE_NETWORK_COMMANDS
            .iter()
            .all(|cmd| self.permissions.check_tier(cmd) == Tier::Session);

        if already_session {
            return Ok(ToolOutput::success(
                "Internet access already enabled for session. \
                 Network commands (curl, wget, ssh, etc.) run automatically.",
            ));
        }

        // Request user approval
        let cmds_list = PROMOTABLE_NETWORK_COMMANDS.join(", ");
        match self
            .approval
            .request_approval(
                format!("Enable internet access (reason: {})", args.reason),
                vec![format!("Internet Access: {}", cmds_list)],
                "Internet Access",
            )
            .await
        {
            Ok(ApprovalResponse::Allow) => {
                // Lift from Restricted → PerCall: unblocked but each bash command still
                // needs individual approval (consistent with bash's own "Allow" semantics).
                for cmd in PROMOTABLE_NETWORK_COMMANDS {
                    self.permissions.promote_to_per_call(cmd);
                }
                Ok(ToolOutput::success(format!(
                    "Internet access unblocked. The following commands are now available \
                     with per-command approval: {}",
                    cmds_list
                )))
            }
            Ok(ApprovalResponse::AllowForSession) => {
                // Promote to Session: commands run automatically without further approval.
                for cmd in PROMOTABLE_NETWORK_COMMANDS {
                    self.permissions.promote_to_session(cmd);
                }
                Ok(ToolOutput::success(format!(
                    "Internet access enabled for session. \
                     The following commands will run automatically: {}",
                    cmds_list
                )))
            }
            Ok(ApprovalResponse::Deny(reason)) => {
                let msg = match reason {
                    Some(r) => format!("Internet access denied by user: {r}"),
                    None => "Internet access denied by user.".to_string(),
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
    use std::collections::HashMap;

    #[test]
    fn test_network_commands_start_restricted() {
        let permissions = Arc::new(PermissionStore::new(HashMap::new()));
        for cmd in PROMOTABLE_NETWORK_COMMANDS {
            assert_eq!(
                permissions.check_tier(cmd),
                Tier::Restricted,
                "{} should start restricted",
                cmd
            );
        }
    }

    #[test]
    fn test_promote_network_commands_to_session() {
        let permissions = Arc::new(PermissionStore::new(HashMap::new()));
        for cmd in PROMOTABLE_NETWORK_COMMANDS {
            permissions.promote_to_session(cmd);
        }
        for cmd in PROMOTABLE_NETWORK_COMMANDS {
            assert_eq!(
                permissions.check_tier(cmd),
                Tier::Session,
                "{} should be session after session promotion",
                cmd
            );
        }
    }

    #[test]
    fn test_promote_network_commands_to_per_call() {
        let permissions = Arc::new(PermissionStore::new(HashMap::new()));
        for cmd in PROMOTABLE_NETWORK_COMMANDS {
            permissions.promote_to_per_call(cmd);
        }
        for cmd in PROMOTABLE_NETWORK_COMMANDS {
            assert_eq!(
                permissions.check_tier(cmd),
                Tier::PerCall,
                "{} should be per-call after per-call promotion",
                cmd
            );
        }
    }

    #[test]
    fn test_session_wins_over_per_call() {
        let permissions = Arc::new(PermissionStore::new(HashMap::new()));
        permissions.promote_to_per_call("curl");
        permissions.promote_to_session("curl");
        assert_eq!(permissions.check_tier("curl"), Tier::Session);
    }

    #[test]
    fn test_non_network_restricted_unaffected() {
        let permissions = Arc::new(PermissionStore::new(HashMap::new()));
        for cmd in PROMOTABLE_NETWORK_COMMANDS {
            permissions.promote_to_session(cmd);
        }
        for cmd in &["sudo", "dd", "kill", "shutdown", "mount"] {
            assert_eq!(
                permissions.check_tier(cmd),
                Tier::Restricted,
                "{} should remain restricted",
                cmd
            );
        }
    }

    #[tokio::test]
    async fn test_per_call_promoted_still_shows_dialog() {
        // PerCall-promoted commands should NOT trigger the early return — the user
        // can still call request_internet_access to upgrade to session-wide access.
        let permissions = Arc::new(PermissionStore::new(HashMap::new()));
        for cmd in PROMOTABLE_NETWORK_COMMANDS {
            permissions.promote_to_per_call(cmd);
        }
        let (approval, mut rx) = create_approval_channel();
        let tool = RequestInternetAccessTool::new(permissions, approval);

        // Spawn a task that auto-denies so execute() returns
        tokio::spawn(async move {
            if let Some(req) = rx.recv().await {
                let _ = req.response_tx.send(ApprovalResponse::Deny(None));
            }
        });

        let result = tool
            .execute(serde_json::json!({ "reason": "test" }))
            .await
            .unwrap();

        // Should have sent an approval request (not returned early)
        assert!(result.is_error, "Denied → error");
        assert!(result.text_content().contains("denied"));
    }

    #[tokio::test]
    async fn test_session_promoted_skips_dialog() {
        let permissions = Arc::new(PermissionStore::new(HashMap::new()));
        for cmd in PROMOTABLE_NETWORK_COMMANDS {
            permissions.promote_to_session(cmd);
        }
        let (approval, mut rx) = create_approval_channel();
        let tool = RequestInternetAccessTool::new(permissions, approval);

        let result = tool
            .execute(serde_json::json!({ "reason": "test" }))
            .await
            .unwrap();

        assert!(!result.is_error, "Should succeed");
        assert!(
            result.text_content().contains("already enabled"),
            "Should report already enabled, got: {}",
            result.text_content()
        );
        assert!(rx.try_recv().is_err(), "No approval request expected");
    }

    #[tokio::test]
    async fn test_allow_once_promotes_to_per_call() {
        let permissions = Arc::new(PermissionStore::new(HashMap::new()));
        let (approval, mut rx) = create_approval_channel();
        let tool = RequestInternetAccessTool::new(Arc::clone(&permissions), approval);

        tokio::spawn(async move {
            if let Some(req) = rx.recv().await {
                let _ = req.response_tx.send(ApprovalResponse::Allow);
            }
        });

        let result = tool
            .execute(serde_json::json!({ "reason": "test" }))
            .await
            .unwrap();

        assert!(!result.is_error, "Should succeed: {}", result.text_content());
        assert!(
            result.text_content().contains("per-command approval"),
            "Should mention per-command approval, got: {}",
            result.text_content()
        );
        for cmd in PROMOTABLE_NETWORK_COMMANDS {
            assert_eq!(
                permissions.check_tier(cmd),
                Tier::PerCall,
                "{} should be PerCall after Allow",
                cmd
            );
        }
    }

    #[tokio::test]
    async fn test_allow_for_session_promotes_to_session() {
        let permissions = Arc::new(PermissionStore::new(HashMap::new()));
        let (approval, mut rx) = create_approval_channel();
        let tool = RequestInternetAccessTool::new(Arc::clone(&permissions), approval);

        tokio::spawn(async move {
            if let Some(req) = rx.recv().await {
                let _ = req.response_tx.send(ApprovalResponse::AllowForSession);
            }
        });

        let result = tool
            .execute(serde_json::json!({ "reason": "test" }))
            .await
            .unwrap();

        assert!(!result.is_error, "Should succeed: {}", result.text_content());
        for cmd in PROMOTABLE_NETWORK_COMMANDS {
            assert_eq!(
                permissions.check_tier(cmd),
                Tier::Session,
                "{} should be Session after AllowForSession",
                cmd
            );
        }
    }
}
