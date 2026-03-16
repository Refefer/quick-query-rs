//! Sandboxed run tool for executing shell commands.
//!
//! Provides kernel-level process isolation via hakoniwa (Linux) with
//! graceful fallback to app-level sandboxing on other platforms.

pub mod network_access;
pub mod mounts;
pub mod parse;
pub mod permissions;
pub mod sandbox;
pub mod sensitive_access;

use async_trait::async_trait;
use serde::Deserialize;
use std::sync::{Arc, RwLock};

use qq_core::{Error, PropertySchema, Tool, ToolDefinition, ToolOutput, ToolParameters};

pub use network_access::RequestNetworkAccessTool;
pub use mounts::{MountExternalTool, MountPoint, SandboxMounts};
pub use permissions::{
    create_approval_channel, ApprovalChannel, ApprovalRequest, ApprovalResponse,
    PermissionStore, Tier,
};
pub use sandbox::{SandboxExecutor, SandboxPathPolicy};
pub use sensitive_access::RequestSensitiveAccessTool;

/// Default command timeout in seconds.
const DEFAULT_TIMEOUT_SECS: u64 = 30;

/// Maximum output lines before head+tail truncation.
const MAX_OUTPUT_LINES: usize = 200;

/// Maximum output size in bytes before truncation.
const MAX_OUTPUT_BYTES: usize = 64 * 1024; // 64KB

/// Sandboxed run tool for executing shell commands.
pub struct RunTool {
    mounts: Arc<SandboxMounts>,
    permissions: Arc<PermissionStore>,
    approval: ApprovalChannel,
    executor: SandboxExecutor,
    path_policy: Arc<RwLock<SandboxPathPolicy>>,
    tool_desc: String,
    timeout_secs: u64,
}

#[derive(Deserialize)]
struct RunArgs {
    command: String,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    timeout: Option<u64>,
    #[serde(default)]
    stdin: Option<String>,
}

impl RunTool {
    pub fn new(
        mounts: Arc<SandboxMounts>,
        permissions: Arc<PermissionStore>,
        approval: ApprovalChannel,
        path_policy: Arc<RwLock<SandboxPathPolicy>>,
    ) -> Self {
        Self::with_network_mode(mounts, permissions, approval, path_policy, false)
    }

    pub fn with_network_mode(
        mounts: Arc<SandboxMounts>,
        permissions: Arc<PermissionStore>,
        approval: ApprovalChannel,
        path_policy: Arc<RwLock<SandboxPathPolicy>>,
        has_network: bool,
    ) -> Self {
        let executor = SandboxExecutor::detect();
        let tool_desc = build_tool_description(&mounts, &executor, has_network);
        Self {
            mounts,
            permissions,
            approval,
            executor,
            path_policy,
            tool_desc,
            timeout_secs: DEFAULT_TIMEOUT_SECS,
        }
    }

    pub fn with_timeout(mut self, timeout_secs: u64) -> Self {
        self.timeout_secs = timeout_secs;
        self
    }
}

#[async_trait]
impl Tool for RunTool {
    fn name(&self) -> &str {
        "run"
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
                )
                .add_property(
                    "stdin",
                    PropertySchema::string(
                        "Optional data to pipe to the command's standard input.",
                    ),
                    false,
                ),
        )
    }

    fn is_blocking(&self) -> bool {
        false // Async: approval await + spawn_blocking for hakoniwa
    }

    async fn execute(&self, arguments: serde_json::Value) -> Result<ToolOutput, Error> {
        let args: RunArgs = serde_json::from_value(arguments)
            .map_err(|e| Error::tool("run", format!("Invalid arguments: {}", e)))?;

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

        // 1a. Reject Snap binaries up front — they cannot run inside the kernel
        //     sandbox because Snap relies on cgroup/profile management that
        //     breaks inside user namespaces (exit code 46).
        if let Some(first_cmd) = commands.first() {
            if is_snap_binary(first_cmd) {
                return Ok(ToolOutput::error(format!(
                    "'{}' is a Snap package and cannot run inside the qq sandbox due to \
                     Snap daemon restrictions (user namespaces break Snap's cgroup/profile \
                     management). Install it via apt instead: \
                     `sudo apt install {}`, then restart qq.",
                    first_cmd, first_cmd
                )));
            }
        }

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
                    .request_approval(command.to_string(), trigger_cmds.clone(), "Command")
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
            tracing::info!(command = %command, description = %desc, "Executing command");
        } else {
            tracing::info!(command = %command, "Executing command");
        }

        // 4. Execute in sandbox
        let path_policy = match self.path_policy.read() {
            Ok(p) => p.clone(),
            Err(_) => return Ok(ToolOutput::error("Path policy lock poisoned.")),
        };
        let result = match self.executor.execute(
            command,
            &self.mounts,
            timeout,
            &path_policy,
            args.stdin.as_deref(),
        ).await {
            Ok(result) => result,
            Err(e) => return Ok(ToolOutput::error(format!("Execution failed: {}", e))),
        };

        // 5. Format output
        Ok(format_output(result))
    }
}

// =============================================================================
// Binary detection
// =============================================================================

/// Check if output appears to be binary data.
///
/// Checks for null bytes, known magic numbers, and high control character ratio.
fn is_binary_output(data: &str) -> bool {
    let bytes = data.as_bytes();
    if bytes.is_empty() {
        return false;
    }

    let check_len = bytes.len().min(1024);
    let sample = &bytes[..check_len];

    // Check for null bytes (common in binary)
    if sample.contains(&0) {
        return true;
    }

    // Check for UTF-8 replacement character clusters (from_utf8_lossy artifacts)
    if data.contains("\u{FFFD}\u{FFFD}") {
        return true;
    }

    // Check magic numbers
    if bytes.len() >= 4 {
        if bytes.starts_with(b"\x7fELF") { return true; }    // ELF
        if bytes.starts_with(b"\x89PNG") { return true; }    // PNG
        if bytes.starts_with(b"%PDF") { return true; }       // PDF
        if bytes.starts_with(b"PK\x03\x04") { return true; } // ZIP
    }
    if bytes.len() >= 3 && bytes[0] == 0xFF && bytes[1] == 0xD8 && bytes[2] == 0xFF { return true; } // JPEG
    if bytes.len() >= 2 && bytes[0] == 0x1F && bytes[1] == 0x8B { return true; } // gzip

    // Check control character ratio
    let control_count = sample.iter().filter(|&&b| b < 0x20 && b != b'\n' && b != b'\r' && b != b'\t').count();
    let ratio = control_count as f64 / check_len as f64;
    ratio > 0.30
}

// =============================================================================
// Output truncation
// =============================================================================

struct TruncationResult {
    output: String,
    truncated: bool,
    original_lines: usize,
    original_bytes: usize,
}

/// Smart head+tail truncation that preserves context from both ends.
fn truncate_output(output: &str, max_lines: usize, max_bytes: usize) -> TruncationResult {
    let original_bytes = output.len();
    let lines: Vec<&str> = output.lines().collect();
    let original_lines = lines.len();

    // Within limits — return unchanged
    if original_lines <= max_lines && original_bytes <= max_bytes {
        return TruncationResult {
            output: output.to_string(),
            truncated: false,
            original_lines,
            original_bytes,
        };
    }

    // Line-based truncation: head 40%, tail 40%
    let head_count = max_lines * 40 / 100;
    let tail_count = max_lines * 40 / 100;
    let omitted = original_lines.saturating_sub(head_count + tail_count);

    let mut result = String::new();
    for line in &lines[..head_count.min(original_lines)] {
        result.push_str(line);
        result.push('\n');
    }
    result.push_str(&format!("\n[... {} lines omitted ...]\n\n", omitted));
    let tail_start = original_lines.saturating_sub(tail_count);
    for line in &lines[tail_start..] {
        result.push_str(line);
        result.push('\n');
    }

    // Secondary byte cap
    if result.len() > max_bytes {
        let cut_point = result[..max_bytes].rfind('\n').unwrap_or(max_bytes);
        result.truncate(cut_point);
        result.push_str("\n[... truncated at byte limit ...]");
    }

    TruncationResult {
        output: result,
        truncated: true,
        original_lines,
        original_bytes,
    }
}

// =============================================================================
// Exit code registry
// =============================================================================

fn exit_code_meaning(code: i32) -> &'static str {
    match code {
        0 => "success",
        1 => "general error",
        2 => "misuse of shell command",
        126 => "command not executable",
        127 => "command not found",
        130 => "interrupted (SIGINT)",
        137 => "killed (SIGKILL/OOM)",
        139 => "segfault (SIGSEGV)",
        143 => "terminated (SIGTERM)",
        c if c > 128 => "signal",
        _ => "error",
    }
}

fn format_bytes(bytes: usize) -> String {
    if bytes < 1024 {
        format!("{}B", bytes)
    } else if bytes < 1024 * 1024 {
        format!("{:.1}KB", bytes as f64 / 1024.0)
    } else {
        format!("{:.1}MB", bytes as f64 / (1024.0 * 1024.0))
    }
}

// =============================================================================
// Output formatting
// =============================================================================

/// Format a CommandResult into a ToolOutput.
fn format_output(result: sandbox::CommandResult) -> ToolOutput {
    let is_error = result.exit_code != 0 || result.timed_out || result.sandbox_error.is_some();
    let stdout = result.stdout.trim();
    let stderr = result.stderr.trim();
    let duration_secs = result.duration.as_secs_f64();

    // Binary detection
    if is_binary_output(stdout) {
        let footer = format!(
            "---\nExit code: {} ({}) | Duration: {:.2}s | Binary output: {}",
            result.exit_code,
            exit_code_meaning(result.exit_code),
            duration_secs,
            format_bytes(result.stdout.len()),
        );
        let msg = format!(
            "[Binary output detected: {}, not displayed]\n\n{}",
            format_bytes(result.stdout.len()),
            footer,
        );
        return if is_error { ToolOutput::error(msg) } else { ToolOutput::success(msg) };
    }

    let mut output = String::new();

    // Prefix for timeout/sandbox errors
    if result.timed_out {
        output.push_str("[Command timed out]\n\n");
    } else if let Some(ref err) = result.sandbox_error {
        output.push_str(&format!("[Sandbox error: {}]\n\n", err));
    }

    // Truncate stdout
    if !stdout.is_empty() {
        let tr = truncate_output(stdout, MAX_OUTPUT_LINES, MAX_OUTPUT_BYTES);
        output.push_str(&tr.output);

        // Stderr
        if !stderr.is_empty() {
            output.push_str("\n\n");
            if is_error {
                output.push_str("STDERR:\n");
            } else {
                output.push_str("STDERR (warnings):\n");
            }
            for line in stderr.lines() {
                output.push_str("  ");
                output.push_str(line);
                output.push('\n');
            }
        }

        // Metadata footer
        let lines_display = if tr.truncated {
            format!("{} ({} shown)", tr.original_lines, MAX_OUTPUT_LINES)
        } else {
            format!("{}", tr.original_lines)
        };
        output.push_str(&format!(
            "\n---\nExit code: {} ({}) | Duration: {:.2}s | Lines: {} | Size: {}",
            result.exit_code,
            exit_code_meaning(result.exit_code),
            duration_secs,
            lines_display,
            format_bytes(tr.original_bytes),
        ));
    } else if !stderr.is_empty() {
        // No stdout, but has stderr
        if is_error {
            output.push_str("STDERR:\n");
        } else {
            output.push_str("STDERR (warnings):\n");
        }
        for line in stderr.lines() {
            output.push_str("  ");
            output.push_str(line);
            output.push('\n');
        }
        output.push_str(&format!(
            "\n---\nExit code: {} ({}) | Duration: {:.2}s",
            result.exit_code,
            exit_code_meaning(result.exit_code),
            duration_secs,
        ));
    } else {
        // No output at all
        output.push_str(&format!(
            "(no output)\n\n---\nExit code: {} ({}) | Duration: {:.2}s",
            result.exit_code,
            exit_code_meaning(result.exit_code),
            duration_secs,
        ));
    }

    if is_error {
        ToolOutput::error(output)
    } else {
        ToolOutput::success(output)
    }
}

/// Check whether a command name resolves to a Snap-managed binary.
fn is_snap_binary(cmd_name: &str) -> bool {
    if cmd_name.is_empty() || cmd_name.contains('/') {
        return false;
    }

    let output = match std::process::Command::new("which").arg(cmd_name).output() {
        Ok(o) => o,
        Err(_) => return false,
    };

    if !output.status.success() {
        return false;
    }

    let path = String::from_utf8_lossy(&output.stdout);
    let path = path.trim();

    if path.starts_with("/snap/") {
        return true;
    }

    if let Ok(target) = std::fs::read_link(path) {
        if target.to_string_lossy().contains("/snap") {
            return true;
        }
    }

    false
}

/// Build the dynamic tool description based on current mounts and sandbox mode.
fn build_tool_description(mounts: &SandboxMounts, executor: &SandboxExecutor, has_network: bool) -> String {
    let mode = executor.mode_name();
    let supports_shell = executor.supports_shell();
    let root = mounts.project_root().display();

    let mut desc = format!(
        "Execute shell commands in a sandboxed environment. This is your primary tool for \
         ALL file operations, code searching, building, and testing.\n\n\
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
            - Subshells: echo $(git rev-parse HEAD)\n\
            - Stdin: pass data via the 'stdin' parameter to pipe into the command\n");
    } else {
        desc.push_str("\nLimitations (app-level sandbox):\n\
            - No pipes, redirects, or shell operators\n\
            - Only read-only commands available\n\
            - Simple commands only (e.g., 'ls -la', 'grep pattern file')\n");
    }

    let network_text = if has_network {
        "Network access is available — commands like curl, wget, git clone, npm install, etc. can use the network freely."
    } else {
        "Network access is blocked by default — use request_network_access tool to request permission. \
If you have trouble connecting, check that you have requested network access first."
    };

    desc.push_str(&format!("\nFile operations:\n\
        - Read: cat file.txt, head -n 50 file.txt, tail -n 20 file.txt\n\
        - Write: cat > file.txt << 'EOF'\\n...\\nEOF, or tee file.txt\n\
        - Edit: sed -i 's/old/new/g' file.txt\n\
        - Search: grep -rn 'pattern' src/, find . -name '*.rs'\n\
        - File ops: cp, mv, mkdir -p, rm\n\n\
        Scratch space (/tmp):\n\
        Use /tmp for scripts, intermediate results, and working notes — it persists across commands.\n\
        Prefer writing scripts to /tmp over inlining them: echo 'script' > /tmp/check.sh && sh /tmp/check.sh\n\n\
        Permission tiers:\n\
        - Session (run immediately): ls, cat, grep, find, git log, git diff, cargo build, cargo test, npm test, etc.\n\
        - Per-call (requires user approval): cargo run, npm install, git commit, rm, mv, python, etc.\n\
        - Restricted (blocked by default): sudo, dd, kill, iptables, etc.\n\n\
        {}\n\
Commands execute with a 30-second default timeout.\n\n\
        Examples:\n\
        - List files: ls -la src/\n\
        - Read file: cat src/main.rs\n\
        - Search code: grep -rn 'TODO' src/ | sort\n\
        - Git history: git log --oneline -10\n\
        - Build project: cargo build\n\
        - Run tests: cargo test && cargo clippy\n\
        - Write file: cat > output.txt << 'EOF'\\ncontent here\\nEOF\n\
        - Edit file: sed -i 's/old_func/new_func/g' src/lib.rs\n\
        - Run binary: cargo run --bin myapp (requires approval)\n\
        - Install deps: npm install (requires approval)", network_text));

    desc
}

/// Create run tools for registration in a tool registry.
///
/// Returns `run`, `mount_external`, and `request_sensitive_access` as a bundle,
/// plus `request_network_access` when `ask_network` is true. The `path_policy`
/// is wrapped in an `Arc<RwLock<>>` shared between `RunTool` and
/// `RequestSensitiveAccessTool` so that approved sensitive-directory access
/// takes effect immediately.
pub fn create_run_tools(
    mounts: Arc<SandboxMounts>,
    permissions: Arc<PermissionStore>,
    approval: ApprovalChannel,
    path_policy: SandboxPathPolicy,
    ask_network: bool,
) -> Vec<Arc<dyn Tool>> {
    let path_policy = Arc::new(RwLock::new(path_policy));
    let run = Arc::new(RunTool::with_network_mode(
        Arc::clone(&mounts),
        Arc::clone(&permissions),
        approval.clone(),
        Arc::clone(&path_policy),
        !ask_network,
    ));
    let mount_ext = Arc::new(MountExternalTool::new(mounts, approval.clone()));
    let sensitive = Arc::new(RequestSensitiveAccessTool::new(path_policy, approval.clone()));
    let mut tools: Vec<Arc<dyn Tool>> = vec![run, mount_ext, sensitive];
    if ask_network {
        tools.push(Arc::new(RequestNetworkAccessTool::new(approval)));
    }
    tools
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn make_result(stdout: &str, stderr: &str, exit_code: i32) -> sandbox::CommandResult {
        sandbox::CommandResult {
            stdout: stdout.to_string(),
            stderr: stderr.to_string(),
            exit_code,
            timed_out: false,
            sandbox_error: None,
            duration: Duration::from_millis(123),
        }
    }

    #[test]
    fn test_format_output_success() {
        let result = make_result("hello world\n", "", 0);
        let output = format_output(result);
        assert!(!output.is_error);
        assert!(output.text_content().contains("hello world"));
        assert!(output.text_content().contains("Exit code: 0 (success)"));
        assert!(output.text_content().contains("Duration:"));
    }

    #[test]
    fn test_format_output_with_stderr() {
        let result = make_result("output\n", "warning: something\n", 0);
        let output = format_output(result);
        assert!(output.text_content().contains("output"));
        assert!(output.text_content().contains("STDERR (warnings):"));
        assert!(output.text_content().contains("  warning: something"));
    }

    #[test]
    fn test_format_output_error() {
        let result = make_result("", "error: not found\n", 1);
        let output = format_output(result);
        assert!(output.is_error);
        assert!(output.text_content().contains("Exit code: 1 (general error)"));
    }

    #[test]
    fn test_format_output_timeout() {
        let result = sandbox::CommandResult {
            stdout: "partial\n".to_string(),
            stderr: String::new(),
            exit_code: -1,
            timed_out: true,
            sandbox_error: None,
            duration: Duration::from_secs(30),
        };
        let output = format_output(result);
        assert!(output.is_error);
        assert!(output.text_content().contains("timed out"));
    }

    #[test]
    fn test_format_output_sandbox_error() {
        let result = sandbox::CommandResult {
            stdout: String::new(),
            stderr: String::new(),
            exit_code: 125,
            timed_out: false,
            sandbox_error: Some("mount: /etc: permission denied".to_string()),
            duration: Duration::from_millis(50),
        };
        let output = format_output(result);
        assert!(output.is_error);
        assert!(output.text_content().contains("Sandbox error"));
        assert!(output.text_content().contains("permission denied"));
    }

    #[test]
    fn test_format_output_empty() {
        let result = make_result("", "", 0);
        let output = format_output(result);
        assert!(output.text_content().contains("(no output)"));
        assert!(output.text_content().contains("Exit code: 0 (success)"));
    }

    #[test]
    fn test_binary_detection() {
        assert!(is_binary_output("\x7fELF\x00\x00"));
        // Test with null bytes embedded
        assert!(is_binary_output("abc\0def"));
        // Test with replacement chars (as if from_utf8_lossy produced them)
        assert!(is_binary_output("data\u{FFFD}\u{FFFD}more"));
        assert!(!is_binary_output("hello world\n"));
        assert!(!is_binary_output("line1\nline2\nline3\n"));
        assert!(!is_binary_output(""));
    }

    #[test]
    fn test_head_tail_truncation() {
        let lines: Vec<String> = (1..=500).map(|i| format!("line {}", i)).collect();
        let input = lines.join("\n");
        let tr = truncate_output(&input, 200, 64 * 1024);
        assert!(tr.truncated);
        assert_eq!(tr.original_lines, 500);
        assert!(tr.output.contains("line 1"));
        assert!(tr.output.contains("lines omitted"));
        assert!(tr.output.contains("line 500"));
    }

    #[test]
    fn test_no_truncation_needed() {
        let tr = truncate_output("short output", 200, 64 * 1024);
        assert!(!tr.truncated);
        assert_eq!(tr.output, "short output");
    }

    #[test]
    fn test_exit_code_meanings() {
        assert_eq!(exit_code_meaning(0), "success");
        assert_eq!(exit_code_meaning(1), "general error");
        assert_eq!(exit_code_meaning(127), "command not found");
        assert_eq!(exit_code_meaning(137), "killed (SIGKILL/OOM)");
        assert_eq!(exit_code_meaning(139), "segfault (SIGSEGV)");
        assert_eq!(exit_code_meaning(200), "signal"); // > 128
    }

    #[test]
    fn test_format_output_binary() {
        let result = sandbox::CommandResult {
            stdout: "\x7fELF\x00\x00binary content".to_string(),
            stderr: String::new(),
            exit_code: 0,
            timed_out: false,
            sandbox_error: None,
            duration: Duration::from_millis(10),
        };
        let output = format_output(result);
        assert!(output.text_content().contains("Binary output detected"));
        assert!(!output.is_error);
    }
}
