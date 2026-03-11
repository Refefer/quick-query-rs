//! Tool for requesting access to sensitive home directories hidden by tmpfs.

use async_trait::async_trait;
use std::path::PathBuf;
use std::sync::{Arc, RwLock};

use qq_core::{Error, PropertySchema, Tool, ToolDefinition, ToolOutput, ToolParameters};

use super::permissions::{ApprovalChannel, ApprovalResponse};
use super::sandbox::{SandboxPathPolicy, SENSITIVE_DIR_NAMES};

/// LLM-callable tool for requesting read access to sensitive directories.
pub struct RequestSensitiveAccessTool {
    path_policy: Arc<RwLock<SandboxPathPolicy>>,
    approval: ApprovalChannel,
}

#[derive(serde::Deserialize)]
struct RequestSensitiveAccessArgs {
    directory: String,
    reason: String,
}

impl RequestSensitiveAccessTool {
    pub fn new(path_policy: Arc<RwLock<SandboxPathPolicy>>, approval: ApprovalChannel) -> Self {
        Self { path_policy, approval }
    }
}

const SENSITIVE_ACCESS_TOOL_DESC: &str = "\
Request read access to a sensitive home directory (.ssh, .aws, .kube, etc.).

Sensitive directories under $HOME are hidden by default in the sandbox. Call this \
tool to request user approval before accessing them. If approved, the directory \
becomes readable in subsequent bash commands.

Parameters:
  - directory: Name of the directory (e.g., \".ssh\", \".aws\", \".kube\")
  - reason: Brief explanation of why access is needed

Valid directories: .ssh, .gnupg, .gpg, .aws, .kube, .docker, .password-store, .netrc";

#[async_trait]
impl Tool for RequestSensitiveAccessTool {
    fn name(&self) -> &str {
        "request_sensitive_access"
    }

    fn description(&self) -> &str {
        "Request user approval to access a sensitive home directory (.ssh, .aws, .kube, etc.)"
    }

    fn tool_description(&self) -> &str {
        SENSITIVE_ACCESS_TOOL_DESC
    }

    fn definition(&self) -> ToolDefinition {
        ToolDefinition::new(self.name(), self.tool_description()).with_parameters(
            ToolParameters::new()
                .add_property(
                    "directory",
                    PropertySchema::string(
                        "Name of the sensitive directory to access (e.g., \".ssh\", \".aws\", \".kube\")",
                    ),
                    true,
                )
                .add_property(
                    "reason",
                    PropertySchema::string("Brief explanation of why access is needed"),
                    true,
                ),
        )
    }

    async fn execute(&self, arguments: serde_json::Value) -> Result<ToolOutput, Error> {
        let args: RequestSensitiveAccessArgs = serde_json::from_value(arguments)
            .map_err(|e| {
                Error::tool("request_sensitive_access", format!("Invalid arguments: {}", e))
            })?;

        // Strip any leading slash/dot-slash — accept ".ssh" or "ssh" or "/.ssh"
        let dir_name = args.directory.trim_start_matches('/');
        // Normalize: ensure it starts with '.' if the user omitted it
        let dir_name = if SENSITIVE_DIR_NAMES.contains(&dir_name) {
            dir_name
        } else {
            // Try with leading '.' in case user passed "ssh" instead of ".ssh"
            let with_dot = format!(".{}", dir_name);
            if SENSITIVE_DIR_NAMES.iter().any(|d| *d == with_dot.as_str()) {
                // We need to return a reference that lives long enough; fall through to the error
                // instead of borrowing a temporary. Just validate below.
                dir_name
            } else {
                dir_name
            }
        };

        // Validate directory name
        if !SENSITIVE_DIR_NAMES.contains(&dir_name) {
            let valid = SENSITIVE_DIR_NAMES.join(", ");
            return Ok(ToolOutput::error(format!(
                "Invalid directory '{}'. Valid sensitive directories: {}",
                dir_name, valid
            )));
        }

        // Check policy state first — agent mode detection happens before path resolution
        // so we don't need $HOME or an existing directory to report the right error.
        {
            let policy = match self.path_policy.read() {
                Ok(p) => p,
                Err(_) => return Ok(ToolOutput::error("Path policy lock poisoned.")),
            };

            // Detect agent mode (system_only): both lists are empty — tmpfs overlays don't apply
            if policy.ro_mounts.is_empty() && policy.tmpfs_mounts.is_empty() {
                return Ok(ToolOutput::error(
                    "Sensitive directory access is not available in agent mode.",
                ));
            }
        } // read lock released here

        // Resolve full path via $HOME
        let home = match std::env::var("HOME") {
            Ok(h) if !h.is_empty() => PathBuf::from(h),
            _ => return Ok(ToolOutput::error("$HOME is not set.")),
        };

        let full_path = home.join(dir_name);

        if !full_path.exists() {
            return Ok(ToolOutput::error(format!(
                "Directory does not exist: {}",
                full_path.display()
            )));
        }

        // Check if already approved (directory no longer in tmpfs_mounts)
        {
            let policy = match self.path_policy.read() {
                Ok(p) => p,
                Err(_) => return Ok(ToolOutput::error("Path policy lock poisoned.")),
            };

            if !policy.tmpfs_mounts.contains(&full_path) {
                return Ok(ToolOutput::success(format!(
                    "Access to {} is already available.",
                    full_path.display()
                )));
            }
        } // read lock released here

        // Request user approval
        match self
            .approval
            .request_approval(
                format!("Access {} (reason: {})", full_path.display(), args.reason),
                vec![format!("Sensitive Directory: {}", full_path.display())],
                "Sensitive Directory",
            )
            .await
        {
            Ok(ApprovalResponse::Allow) | Ok(ApprovalResponse::AllowForSession) => {
                match self.path_policy.write() {
                    Ok(mut policy) => {
                        policy.tmpfs_mounts.retain(|p| p != &full_path);
                        Ok(ToolOutput::success(format!(
                            "Access to {} approved. The directory is now readable in bash commands.",
                            full_path.display()
                        )))
                    }
                    Err(_) => Ok(ToolOutput::error(
                        "Path policy lock poisoned after approval.",
                    )),
                }
            }
            Ok(ApprovalResponse::Deny) => Ok(ToolOutput::error("Access request denied by user.")),
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
    use crate::bash::sandbox::SandboxPathPolicy;

    fn policy_with_ssh(home: &std::path::Path) -> Arc<RwLock<SandboxPathPolicy>> {
        let policy = SandboxPathPolicy {
            path_value: "/usr/bin:/bin".into(),
            ro_mounts: vec![home.to_path_buf()],
            tmpfs_mounts: vec![home.join(".ssh")],
            env_vars: vec![],
        };
        Arc::new(RwLock::new(policy))
    }

    #[tokio::test]
    async fn test_invalid_directory_returns_error() {
        let (approval, _rx) = create_approval_channel();
        let policy = Arc::new(RwLock::new(SandboxPathPolicy::system_only()));
        let tool = RequestSensitiveAccessTool::new(policy, approval);

        let result = tool
            .execute(serde_json::json!({
                "directory": ".invalid_dir",
                "reason": "test"
            }))
            .await
            .unwrap();

        assert!(result.is_error);
        assert!(result.text_content().contains("Invalid directory"));
        assert!(result.text_content().contains(".ssh"));
    }

    #[tokio::test]
    async fn test_agent_mode_returns_error() {
        let (approval, _rx) = create_approval_channel();
        // system_only has empty ro_mounts and tmpfs_mounts
        let policy = Arc::new(RwLock::new(SandboxPathPolicy::system_only()));
        let tool = RequestSensitiveAccessTool::new(policy, approval);

        let result = tool
            .execute(serde_json::json!({
                "directory": ".ssh",
                "reason": "test"
            }))
            .await
            .unwrap();

        assert!(result.is_error);
        assert!(result.text_content().contains("agent mode"));
    }

    #[tokio::test]
    async fn test_already_approved_returns_success_without_approval() {
        // Use the real HOME to avoid env mutation races between concurrent tests.
        let home_str = match std::env::var("HOME") {
            Ok(h) if !h.is_empty() => h,
            _ => {
                eprintln!("Skipping: $HOME not set");
                return;
            }
        };
        let home_path = std::path::PathBuf::from(&home_str);
        let ssh_dir = home_path.join(".ssh");
        if !ssh_dir.exists() {
            eprintln!("Skipping: ~/.ssh does not exist");
            return;
        }

        // Policy where .ssh is NOT in tmpfs_mounts (already approved)
        let policy = SandboxPathPolicy {
            path_value: "/usr/bin:/bin".into(),
            ro_mounts: vec![home_path.clone()],
            tmpfs_mounts: vec![], // .ssh not hidden
            env_vars: vec![],
        };
        let policy = Arc::new(RwLock::new(policy));
        let (approval, mut rx) = create_approval_channel();
        let tool = RequestSensitiveAccessTool::new(policy, approval);

        let result = tool
            .execute(serde_json::json!({
                "directory": ".ssh",
                "reason": "test"
            }))
            .await
            .unwrap();

        assert!(!result.is_error, "Should succeed: {}", result.text_content());
        assert!(
            result.text_content().contains("already available"),
            "Expected 'already available', got: {}",
            result.text_content()
        );
        assert!(rx.try_recv().is_err(), "No approval request expected");
    }

    #[tokio::test]
    async fn test_approved_removes_from_tmpfs_mounts() {
        // Use the real HOME to avoid env mutation races between concurrent tests.
        let home_str = match std::env::var("HOME") {
            Ok(h) if !h.is_empty() => h,
            _ => {
                eprintln!("Skipping: $HOME not set");
                return;
            }
        };
        let home_path = std::path::PathBuf::from(&home_str);
        let ssh_dir = home_path.join(".ssh");
        if !ssh_dir.exists() {
            eprintln!("Skipping: ~/.ssh does not exist");
            return;
        }

        let policy = policy_with_ssh(&home_path);
        let (approval, mut rx) = create_approval_channel();
        let tool = RequestSensitiveAccessTool::new(Arc::clone(&policy), approval.clone());

        // Spawn a task that auto-approves the request
        tokio::spawn(async move {
            if let Some(req) = rx.recv().await {
                let _ = req.response_tx.send(ApprovalResponse::Allow);
            }
        });

        let result = tool
            .execute(serde_json::json!({
                "directory": ".ssh",
                "reason": "need SSH key"
            }))
            .await
            .unwrap();

        assert!(!result.is_error, "Should succeed: {}", result.text_content());
        assert!(
            result.text_content().contains("approved"),
            "Expected 'approved', got: {}",
            result.text_content()
        );

        // .ssh should no longer be in tmpfs_mounts
        let p = policy.read().unwrap();
        assert!(
            !p.tmpfs_mounts.contains(&ssh_dir),
            ".ssh should be removed from tmpfs_mounts"
        );
    }
}
