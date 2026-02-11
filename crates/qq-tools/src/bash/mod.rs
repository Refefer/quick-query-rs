//! Sandboxed bash tool for executing shell commands.
//!
//! Provides kernel-level process isolation via hakoniwa (Linux) with
//! graceful fallback to app-level sandboxing on other platforms.

pub mod mounts;
pub mod parse;
pub mod permissions;
pub mod sandbox;

use async_trait::async_trait;
use serde::Deserialize;
use std::sync::Arc;

use qq_core::{Error, PropertySchema, Tool, ToolDefinition, ToolOutput, ToolParameters};

pub use mounts::{MountExternalTool, MountPoint, SandboxMounts};
pub use permissions::{
    create_approval_channel, ApprovalChannel, ApprovalRequest, ApprovalResponse,
    PermissionStore, Tier,
};
pub use sandbox::SandboxExecutor;

/// Default command timeout in seconds.
const DEFAULT_TIMEOUT_SECS: u64 = 30;

/// Maximum output size in bytes before truncation.
const MAX_OUTPUT_BYTES: usize = 512 * 1024; // 512KB

/// Sandboxed bash tool for executing shell commands.
pub struct BashTool {
    mounts: Arc<SandboxMounts>,
    permissions: Arc<PermissionStore>,
    approval: ApprovalChannel,
    executor: SandboxExecutor,
    tool_desc: String,
    timeout_secs: u64,
    max_output_bytes: usize,
}

#[derive(Deserialize)]
struct BashArgs {
    command: String,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    timeout: Option<u64>,
}

impl BashTool {
    pub fn new(
        mounts: Arc<SandboxMounts>,
        permissions: Arc<PermissionStore>,
        approval: ApprovalChannel,
    ) -> Self {
        let executor = SandboxExecutor::detect();
        let tool_desc = build_tool_description(&mounts, &executor);
        Self {
            mounts,
            permissions,
            approval,
            executor,
            tool_desc,
            timeout_secs: DEFAULT_TIMEOUT_SECS,
            max_output_bytes: MAX_OUTPUT_BYTES,
        }
    }

    pub fn with_timeout(mut self, timeout_secs: u64) -> Self {
        self.timeout_secs = timeout_secs;
        self
    }
}

#[async_trait]
impl Tool for BashTool {
    fn name(&self) -> &str {
        "bash"
    }

    fn description(&self) -> &str {
        "Execute shell commands in a sandboxed environment"
    }

    fn tool_description(&self) -> &str {
        &self.tool_desc
    }

    fn definition(&self) -> ToolDefinition {
        ToolDefinition::new(self.name(), self.tool_description()).with_parameters(
            ToolParameters::new()
                .add_property(
                    "command",
                    PropertySchema::string(
                        "The shell command to execute. Supports pipes, redirects, and globs in kernel sandbox mode.",
                    ),
                    true,
                )
                .add_property(
                    "description",
                    PropertySchema::string(
                        "Brief description of what this command does (for audit logging).",
                    ),
                    false,
                )
                .add_property(
                    "timeout",
                    PropertySchema::integer(
                        "Timeout in seconds (default: 30, max: 300).",
                    ),
                    false,
                ),
        )
    }

    fn is_blocking(&self) -> bool {
        false // Async: approval await + spawn_blocking for hakoniwa
    }

    async fn execute(&self, arguments: serde_json::Value) -> Result<ToolOutput, Error> {
        let args: BashArgs = serde_json::from_value(arguments)
            .map_err(|e| Error::tool("bash", format!("Invalid arguments: {}", e)))?;

        let command = args.command.trim();
        if command.is_empty() {
            return Ok(ToolOutput::error("Command cannot be empty."));
        }

        let timeout = args
            .timeout
            .map(|t| t.min(300))
            .unwrap_or(self.timeout_secs);

        // 1. Extract commands from pipeline
        let commands = match parse::extract_commands(command) {
            Ok(cmds) => cmds,
            Err(e) => return Ok(ToolOutput::error(format!("Failed to parse command: {}", e))),
        };

        // 2. Check permissions
        match self.permissions.check_pipeline(&commands) {
            permissions::PipelinePermission::Restricted(cmds) => {
                return Ok(ToolOutput::error(format!(
                    "Restricted commands cannot be executed: {}. \
                     These commands are blocked for safety.",
                    cmds.join(", ")
                )));
            }
            permissions::PipelinePermission::NeedsApproval(trigger_cmds) => {
                // In app-level mode, per-call commands are not allowed
                if !self.executor.supports_shell() {
                    return Ok(ToolOutput::error(format!(
                        "Commands requiring approval ({}) are not available in app-level sandbox mode. \
                         Kernel sandbox (Linux with user namespaces) is required.",
                        trigger_cmds.join(", ")
                    )));
                }

                // 3. Request user approval via channel
                match self
                    .approval
                    .request_approval(command.to_string(), trigger_cmds.clone())
                    .await
                {
                    Ok(permissions::ApprovalResponse::Allow) => { /* proceed */ }
                    Ok(permissions::ApprovalResponse::AllowForSession) => {
                        for cmd in &trigger_cmds {
                            self.permissions.promote_to_session(cmd);
                        }
                    }
                    Ok(permissions::ApprovalResponse::Deny) => {
                        return Ok(ToolOutput::error("Command denied by user."));
                    }
                    Err(e) => {
                        return Ok(ToolOutput::error(format!(
                            "Approval system unavailable: {}",
                            e
                        )));
                    }
                }
            }
            permissions::PipelinePermission::Allowed => { /* proceed */ }
        }

        // Log the execution
        if let Some(desc) = &args.description {
            tracing::info!(command = %command, description = %desc, "Executing bash command");
        } else {
            tracing::info!(command = %command, "Executing bash command");
        }

        // 4. Execute in sandbox
        let result = match self.executor.execute(command, &self.mounts, timeout).await {
            Ok(result) => result,
            Err(e) => return Ok(ToolOutput::error(format!("Execution failed: {}", e))),
        };

        // 5. Format output
        Ok(format_output(result, self.max_output_bytes))
    }
}

/// Format a CommandResult into a ToolOutput.
fn format_output(result: sandbox::CommandResult, max_bytes: usize) -> ToolOutput {
    let mut output = String::new();

    if result.timed_out {
        output.push_str("[Command timed out]\n\n");
    }

    // Combine stdout and stderr
    let stdout = result.stdout.trim();
    let stderr = result.stderr.trim();

    if !stdout.is_empty() {
        output.push_str(stdout);
    }

    if !stderr.is_empty() {
        if !output.is_empty() {
            output.push_str("\n\n");
        }
        output.push_str("[stderr]\n");
        output.push_str(stderr);
    }

    if output.is_empty() {
        if result.exit_code == 0 {
            output.push_str("(no output)");
        } else {
            output.push_str(&format!("(no output, exit code {})", result.exit_code));
        }
    } else if result.exit_code != 0 && !result.timed_out {
        output.push_str(&format!("\n\n[exit code {}]", result.exit_code));
    }

    // Truncate if needed
    if output.len() > max_bytes {
        let truncated = &output[..max_bytes];
        // Find last newline to avoid cutting mid-line
        let cut_point = truncated.rfind('\n').unwrap_or(max_bytes);
        output = format!(
            "{}\n\n[output truncated at {} bytes, total was {} bytes]",
            &output[..cut_point],
            cut_point,
            output.len()
        );
    }

    if result.exit_code != 0 || result.timed_out {
        ToolOutput::error(output)
    } else {
        ToolOutput::success(output)
    }
}

/// Build the dynamic tool description based on current mounts and sandbox mode.
fn build_tool_description(mounts: &SandboxMounts, executor: &SandboxExecutor) -> String {
    let mode = executor.mode_name();
    let supports_shell = executor.supports_shell();
    let root = mounts.project_root().display();

    let mut desc = format!(
        "Execute shell commands in a sandboxed environment.\n\n\
         Sandbox mode: {mode}\n\
         Project root: {root} (read-write)\n\
         /tmp: writable scratch space (persists across commands, session-scoped)\n"
    );

    let extra = mounts.list_extra();
    if !extra.is_empty() {
        desc.push_str("Extra mounts (read-only):\n");
        for m in &extra {
            desc.push_str(&format!("  - {}\n", m.host_path.display()));
        }
    }

    if supports_shell {
        desc.push_str("\nCapabilities:\n\
            - Pipes: grep pattern file | sort | uniq -c\n\
            - Redirects: cargo build 2>&1 | head -50\n\
            - Globs: ls *.rs\n\
            - Environment variables: echo $PWD\n\
            - Subshells: echo $(git rev-parse HEAD)\n");
    } else {
        desc.push_str("\nLimitations (app-level sandbox):\n\
            - No pipes, redirects, or shell operators\n\
            - Only read-only commands available\n\
            - Simple commands only (e.g., 'ls -la', 'grep pattern file')\n");
    }

    desc.push_str("\nScratch space (/tmp):\n\
        Use /tmp for scripts, intermediate results, and working notes — it persists across commands.\n\
        Prefer writing scripts to /tmp over inlining them: echo 'script' > /tmp/check.sh && sh /tmp/check.sh\n\n\
        Permission tiers:\n\
        - Session (run immediately): ls, cat, grep, find, git log, git diff, cargo build, cargo test, npm test, etc.\n\
        - Per-call (requires user approval): cargo run, npm install, git commit, rm, mv, python, etc.\n\
        - Restricted (always blocked): sudo, curl, wget, ssh, dd, kill, etc.\n\n\
        Network access is blocked. Commands execute with a 30-second default timeout.\n\n\
        Examples:\n\
        - List files: ls -la src/\n\
        - Search code: grep -rn 'TODO' src/ | sort\n\
        - Git history: git log --oneline -10\n\
        - Build project: cargo build\n\
        - Run tests: cargo test && cargo clippy\n\
        - Run binary: cargo run --bin myapp (requires approval)\n\
        - Install deps: npm install (requires approval)");

    desc
}

/// Create bash tools for registration in a tool registry.
pub fn create_bash_tools(
    mounts: Arc<SandboxMounts>,
    permissions: Arc<PermissionStore>,
    approval: ApprovalChannel,
) -> Vec<Arc<dyn Tool>> {
    let bash = Arc::new(BashTool::new(
        Arc::clone(&mounts),
        Arc::clone(&permissions),
        approval.clone(),
    ));
    let mount_ext = Arc::new(MountExternalTool::new(mounts, approval));
    vec![bash, mount_ext]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_output_success() {
        let result = sandbox::CommandResult {
            stdout: "hello world\n".to_string(),
            stderr: String::new(),
            exit_code: 0,
            timed_out: false,
        };
        let output = format_output(result, MAX_OUTPUT_BYTES);
        assert!(!output.is_error);
        assert_eq!(output.content, "hello world");
    }

    #[test]
    fn test_format_output_with_stderr() {
        let result = sandbox::CommandResult {
            stdout: "output\n".to_string(),
            stderr: "warning: something\n".to_string(),
            exit_code: 0,
            timed_out: false,
        };
        let output = format_output(result, MAX_OUTPUT_BYTES);
        assert!(output.content.contains("output"));
        assert!(output.content.contains("[stderr]"));
        assert!(output.content.contains("warning: something"));
    }

    #[test]
    fn test_format_output_error() {
        let result = sandbox::CommandResult {
            stdout: String::new(),
            stderr: "error: not found\n".to_string(),
            exit_code: 1,
            timed_out: false,
        };
        let output = format_output(result, MAX_OUTPUT_BYTES);
        assert!(output.is_error);
        assert!(output.content.contains("exit code 1"));
    }

    #[test]
    fn test_format_output_timeout() {
        let result = sandbox::CommandResult {
            stdout: "partial\n".to_string(),
            stderr: String::new(),
            exit_code: -1,
            timed_out: true,
        };
        let output = format_output(result, MAX_OUTPUT_BYTES);
        assert!(output.is_error);
        assert!(output.content.contains("timed out"));
    }

    #[test]
    fn test_format_output_truncation() {
        let long_output = "x".repeat(1000);
        let result = sandbox::CommandResult {
            stdout: long_output,
            stderr: String::new(),
            exit_code: 0,
            timed_out: false,
        };
        let output = format_output(result, 500);
        assert!(output.content.contains("truncated"));
        assert!(output.content.len() < 600); // Some overhead for the truncation message
    }

    #[test]
    fn test_format_output_empty() {
        let result = sandbox::CommandResult {
            stdout: String::new(),
            stderr: String::new(),
            exit_code: 0,
            timed_out: false,
        };
        let output = format_output(result, MAX_OUTPUT_BYTES);
        assert_eq!(output.content, "(no output)");
    }
}
