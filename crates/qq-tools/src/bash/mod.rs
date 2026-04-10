//! Sandboxed run tool for executing shell commands.
//!
//! Provides kernel-level process isolation via hakoniwa (Linux) with
//! graceful fallback to app-level sandboxing on other platforms.

pub mod mounts;
pub mod network_access;
pub mod parse;
pub mod permissions;
pub mod sandbox;
pub mod sensitive_access;

use async_trait::async_trait;
use serde::Deserialize;
use std::sync::{Arc, RwLock};

use qq_core::{Error, PropertySchema, Tool, ToolDefinition, ToolOutput, ToolParameters};

pub use mounts::{MountExternalTool, MountPoint, SandboxMounts};
pub use network_access::RequestNetworkAccessTool;
pub use permissions::{
    create_approval_channel, ApprovalChannel, ApprovalRequest, ApprovalResponse, PermissionStore,
    Tier,
};
pub use sandbox::{SandboxExecutor, SandboxPathPolicy};
pub use sensitive_access::RequestSensitiveAccessTool;

/// Default command timeout in seconds.
const DEFAULT_TIMEOUT_SECS: u64 = 30;

/// Maximum output lines before head+tail truncation.
const MAX_OUTPUT_LINES: usize = 200;

/// Maximum output size in bytes before truncation.
const MAX_OUTPUT_BYTES: usize = 64 * 1024; // 64KB

/// Maximum spill file size. Caps disk usage if a command dumps a huge blob
/// (e.g. `dd if=/dev/urandom`, `find /`). Beyond this we head/tail-cap the
/// spill file itself, mirroring the inline head+tail strategy.
const MAX_SPILL_BYTES: usize = 16 * 1024 * 1024; // 16 MB

/// Sandboxed run tool for executing shell commands.
pub struct RunTool {
    mounts: Arc<SandboxMounts>,
    permissions: Arc<PermissionStore>,
    approval: ApprovalChannel,
    executor: SandboxExecutor,
    path_policy: Arc<RwLock<SandboxPathPolicy>>,
    tool_desc: String,
    timeout_secs: u64,
    read_only: bool,
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
        let tool_desc = build_tool_description(&mounts, &executor, has_network, false);
        Self {
            mounts,
            permissions,
            approval,
            executor,
            path_policy,
            tool_desc,
            timeout_secs: DEFAULT_TIMEOUT_SECS,
            read_only: false,
        }
    }

    pub fn with_timeout(mut self, timeout_secs: u64) -> Self {
        self.timeout_secs = timeout_secs;
        self
    }

    pub fn with_read_only(mut self, read_only: bool) -> Self {
        self.read_only = read_only;
        if read_only {
            self.tool_desc = build_tool_description(
                &self.mounts,
                &self.executor,
                false, // network irrelevant for read-only
                true,
            );
        }
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
                // Read-only agents: block all per-call (write) commands
                if self.read_only {
                    return Ok(ToolOutput::error(format!(
                        "Read-only agent: write commands blocked: {}. \
                         Only read-only commands (grep, find, cat, git log, cargo test, etc.) are allowed.",
                        trigger_cmds.join(", ")
                    )));
                }

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
                    Ok(permissions::ApprovalResponse::Deny(reason)) => {
                        let msg = match reason {
                            Some(r) => format!("Command denied by user: {r}"),
                            None => "Command denied by user.".to_string(),
                        };
                        return Ok(ToolOutput::error(msg));
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
        let result = match self
            .executor
            .execute(
                command,
                &self.mounts,
                timeout,
                &path_policy,
                args.stdin.as_deref(),
                self.read_only,
            )
            .await
        {
            Ok(result) => result,
            Err(e) => return Ok(ToolOutput::error(format!("Execution failed: {}", e))),
        };

        // 5. Format output
        Ok(format_output(result, &self.mounts))
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
        if bytes.starts_with(b"\x7fELF") {
            return true;
        } // ELF
        if bytes.starts_with(b"\x89PNG") {
            return true;
        } // PNG
        if bytes.starts_with(b"%PDF") {
            return true;
        } // PDF
        if bytes.starts_with(b"PK\x03\x04") {
            return true;
        } // ZIP
    }
    if bytes.len() >= 3 && bytes[0] == 0xFF && bytes[1] == 0xD8 && bytes[2] == 0xFF {
        return true;
    } // JPEG
    if bytes.len() >= 2 && bytes[0] == 0x1F && bytes[1] == 0x8B {
        return true;
    } // gzip

    // Check control character ratio
    let control_count = sample
        .iter()
        .filter(|&&b| b < 0x20 && b != b'\n' && b != b'\r' && b != b'\t')
        .count();
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
// Spill files
// =============================================================================

/// Metadata about a spill file written to the sandbox's /tmp directory.
///
/// A spill file preserves the full untruncated stdout of a command whose
/// inline output was head/tail-truncated. The LLM can inspect the omitted
/// middle with `sed -n 'X,Yp'`, `grep`, or `head`/`tail` in a follow-up
/// `run` call, rather than re-running the original command or escalating
/// to sub-agents.
struct SpillInfo {
    /// Path as the sandbox sees it, e.g. `/tmp/qq-spill-3.txt`.
    sandbox_path: String,
    /// True if the spill file itself was head/tail-capped at `MAX_SPILL_BYTES`.
    hard_capped: bool,
}

/// Write the full untruncated stdout to a session-scoped spill file.
///
/// Returns `None` if the write fails (disk full, permission, EIO). A spill
/// failure is deliberately non-fatal: the caller still returns the inline
/// truncated output, just without a spill footer.
fn write_spill_file(mounts: &SandboxMounts, stdout: &str) -> Option<SpillInfo> {
    let host_path = mounts.next_spill_path();
    let file_name = host_path.file_name()?.to_string_lossy().into_owned();

    let write_result = if stdout.len() <= MAX_SPILL_BYTES {
        std::fs::write(&host_path, stdout.as_bytes())
    } else {
        // Hard-cap: head 8 MB + marker + tail 8 MB, on char boundaries.
        let half = MAX_SPILL_BYTES / 2;
        let head_end = floor_char_boundary(stdout, half);
        let tail_start = ceil_char_boundary(stdout, stdout.len() - half);
        let mut capped = String::with_capacity(MAX_SPILL_BYTES + 128);
        capped.push_str(&stdout[..head_end]);
        capped.push_str("\n\n[... spill file hard-capped, middle omitted ...]\n\n");
        capped.push_str(&stdout[tail_start..]);
        std::fs::write(&host_path, capped.as_bytes())
    };

    match write_result {
        Ok(()) => Some(SpillInfo {
            sandbox_path: format!("/tmp/{}", file_name),
            hard_capped: stdout.len() > MAX_SPILL_BYTES,
        }),
        Err(e) => {
            tracing::warn!(
                host_path = %host_path.display(),
                error = %e,
                "Failed to write spill file",
            );
            None
        }
    }
}

/// Largest valid char-boundary index `<= index`.
///
/// `str::floor_char_boundary` is nightly-only, so we hand-roll it to keep the
/// workspace on stable Rust.
fn floor_char_boundary(s: &str, index: usize) -> usize {
    let mut i = index.min(s.len());
    while i > 0 && !s.is_char_boundary(i) {
        i -= 1;
    }
    i
}

/// Smallest valid char-boundary index `>= index`.
fn ceil_char_boundary(s: &str, index: usize) -> usize {
    let mut i = index.min(s.len());
    while i < s.len() && !s.is_char_boundary(i) {
        i += 1;
    }
    i
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
pub(crate) fn format_output(
    result: sandbox::CommandResult,
    mounts: &SandboxMounts,
) -> ToolOutput {
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
        return if is_error {
            ToolOutput::error(msg)
        } else {
            ToolOutput::success(msg)
        };
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

        // Spill the full untruncated stdout to /tmp so the LLM can grep/sed
        // the omitted middle in a follow-up run call instead of re-running
        // the original command or escalating to sub-agents.
        let spill_info = if tr.truncated {
            write_spill_file(mounts, &result.stdout)
        } else {
            None
        };

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

        if let Some(info) = spill_info {
            let mid = tr.original_lines / 2;
            let sample_start = mid.saturating_sub(50).max(1);
            let sample_end = sample_start + 100;
            output.push_str(&format!(
                "\nFull output saved to {p} — inspect with:\n  \
                 sed -n '{s},{e}p' {p}\n  \
                 grep -n PATTERN {p}\n  \
                 wc -l {p}",
                p = info.sandbox_path,
                s = sample_start,
                e = sample_end,
            ));
            if info.hard_capped {
                output.push_str(&format!(
                    "\n  (note: spill file itself was hard-capped at {})",
                    format_bytes(MAX_SPILL_BYTES),
                ));
            }
        }
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
fn build_tool_description(
    mounts: &SandboxMounts,
    executor: &SandboxExecutor,
    has_network: bool,
    read_only: bool,
) -> String {
    let mode = executor.mode_name();
    let supports_shell = executor.supports_shell();
    let root = mounts.project_root().display();

    if read_only {
        let mut desc = format!(
            "Execute read-only shell commands in a sandboxed environment. \
             This tool is restricted to read-only operations — all write commands are blocked.\n\n\
             Sandbox mode: {mode}\n\
             Project root: {root} (read-only)\n\
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
            desc.push_str(
                "\nCapabilities:\n\
                - Pipes: grep pattern file | sort | uniq -c\n\
                - Globs: ls *.rs\n\
                - Environment variables: echo $PWD\n\
                - Subshells: echo $(git rev-parse HEAD)\n\
                - Stdin: pass data via the 'stdin' parameter to pipe into the command\n",
            );
        }

        desc.push_str(
            "\nAllowed operations (read-only):\n\
            - Read: cat file.txt, head -n 50 file.txt, tail -n 20 file.txt\n\
            - Search: grep -rn 'pattern' src/, find . -name '*.rs'\n\
            - Build/test: cargo build, cargo test, cargo clippy, npm test\n\
            - Git (read): git log, git diff, git status, git show\n\n\
            Blocked operations:\n\
            - All writes: cat > file, tee file, sed -i, echo > file\n\
            - File mutations: rm, mv, cp, mkdir\n\
            - Executables: cargo run, python, node, sh scripts\n\
            - Git mutations: git commit, git push, git checkout\n\n\
            Scratch space (/tmp):\n\
            Use /tmp for intermediate results — it is the only writable location.\n\n\
            Spill files:\n\
            When a command produces more than 200 lines or 64 KB of stdout, the full \
            untruncated output is automatically saved to /tmp/qq-spill-<N>.txt. The \
            inline response shows head+tail plus the spill file path. Inspect the \
            omitted middle with `sed -n 'X,Yp'`, `grep`, or `head`/`tail` on the spill \
            file — do NOT re-run the original command, and do NOT delegate to a \
            sub-agent to work around the truncation.\n\n\
            Commands execute with a 30-second default timeout.\n\n\
            Examples:\n\
            - List files: ls -la src/\n\
            - Read file: cat src/main.rs\n\
            - Search code: grep -rn 'TODO' src/ | sort\n\
            - Git history: git log --oneline -10\n\
            - Build project: cargo build\n\
            - Run tests: cargo test && cargo clippy",
        );

        return desc;
    }

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
        desc.push_str(
            "\nCapabilities:\n\
            - Pipes: grep pattern file | sort | uniq -c\n\
            - Redirects: cargo build 2>&1 | head -50\n\
            - Globs: ls *.rs\n\
            - Environment variables: echo $PWD\n\
            - Subshells: echo $(git rev-parse HEAD)\n\
            - Stdin: pass data via the 'stdin' parameter to pipe into the command\n",
        );
    } else {
        desc.push_str(
            "\nLimitations (app-level sandbox):\n\
            - No pipes, redirects, or shell operators\n\
            - Only read-only commands available\n\
            - Simple commands only (e.g., 'ls -la', 'grep pattern file')\n",
        );
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
        Spill files:\n\
        When a command produces more than 200 lines or 64 KB of stdout, the full \
        untruncated output is automatically saved to /tmp/qq-spill-<N>.txt. The inline \
        response shows head+tail plus the spill file path. Inspect the omitted middle \
        with `sed -n 'X,Yp'`, `grep`, or `head`/`tail` on the spill file — do NOT \
        re-run the original command, and do NOT delegate to a sub-agent to work around \
        the truncation.\n\n\
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
/// Returns `(tools, read_only_run)`:
/// - `tools`: `run`, `mount_external`, and `request_sensitive_access` as a bundle,
///   plus `request_network_access` when `ask_network` is true.
/// - `read_only_run`: a read-only variant of the `run` tool that blocks write commands
///   and mounts the project root read-only in kernel sandbox mode.
///
/// The `path_policy` is wrapped in an `Arc<RwLock<>>` shared between `RunTool` and
/// `RequestSensitiveAccessTool` so that approved sensitive-directory access
/// takes effect immediately.
pub fn create_run_tools(
    mounts: Arc<SandboxMounts>,
    permissions: Arc<PermissionStore>,
    approval: ApprovalChannel,
    path_policy: SandboxPathPolicy,
    ask_network: bool,
) -> (Vec<Arc<dyn Tool>>, Arc<dyn Tool>) {
    let path_policy = Arc::new(RwLock::new(path_policy));
    let run = Arc::new(RunTool::with_network_mode(
        Arc::clone(&mounts),
        Arc::clone(&permissions),
        approval.clone(),
        Arc::clone(&path_policy),
        !ask_network,
    ));
    let read_only_run: Arc<dyn Tool> = Arc::new(
        RunTool::with_network_mode(
            Arc::clone(&mounts),
            Arc::clone(&permissions),
            approval.clone(),
            Arc::clone(&path_policy),
            !ask_network,
        )
        .with_read_only(true),
    );
    let mount_ext = Arc::new(MountExternalTool::new(mounts, approval.clone()));
    let sensitive = Arc::new(RequestSensitiveAccessTool::new(
        path_policy,
        approval.clone(),
    ));
    let mut tools: Vec<Arc<dyn Tool>> = vec![run, mount_ext, sensitive];
    if ask_network {
        tools.push(Arc::new(RequestNetworkAccessTool::new(approval)));
    }
    (tools, read_only_run)
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

    /// Fresh `SandboxMounts` rooted at a per-test `TempDir` so spill files
    /// from one test never leak into another and tests don't race on the
    /// shared `spill_counter`.
    fn test_mounts() -> (SandboxMounts, tempfile::TempDir) {
        let root = tempfile::TempDir::new().unwrap();
        let mounts = SandboxMounts::new(root.path().to_path_buf()).unwrap();
        (mounts, root)
    }

    #[test]
    fn test_format_output_success() {
        let result = make_result("hello world\n", "", 0);
        let (mounts, _root) = test_mounts();
        let output = format_output(result, &mounts);
        assert!(!output.is_error);
        assert!(output.text_content().contains("hello world"));
        assert!(output.text_content().contains("Exit code: 0 (success)"));
        assert!(output.text_content().contains("Duration:"));
    }

    #[test]
    fn test_format_output_with_stderr() {
        let result = make_result("output\n", "warning: something\n", 0);
        let (mounts, _root) = test_mounts();
        let output = format_output(result, &mounts);
        assert!(output.text_content().contains("output"));
        assert!(output.text_content().contains("STDERR (warnings):"));
        assert!(output.text_content().contains("  warning: something"));
    }

    #[test]
    fn test_format_output_error() {
        let result = make_result("", "error: not found\n", 1);
        let (mounts, _root) = test_mounts();
        let output = format_output(result, &mounts);
        assert!(output.is_error);
        assert!(output
            .text_content()
            .contains("Exit code: 1 (general error)"));
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
        let (mounts, _root) = test_mounts();
        let output = format_output(result, &mounts);
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
        let (mounts, _root) = test_mounts();
        let output = format_output(result, &mounts);
        assert!(output.is_error);
        assert!(output.text_content().contains("Sandbox error"));
        assert!(output.text_content().contains("permission denied"));
    }

    #[test]
    fn test_format_output_empty() {
        let result = make_result("", "", 0);
        let (mounts, _root) = test_mounts();
        let output = format_output(result, &mounts);
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
        let (mounts, _root) = test_mounts();
        let output = format_output(result, &mounts);
        assert!(output.text_content().contains("Binary output detected"));
        assert!(!output.is_error);
    }

    // =========================================================================
    // Spill file tests
    // =========================================================================

    fn big_stdout(lines: usize) -> String {
        (1..=lines)
            .map(|i| format!("line {}", i))
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn test_spill_not_created_when_not_truncated() {
        let result = make_result("short output\n", "", 0);
        let (mounts, _root) = test_mounts();
        let output = format_output(result, &mounts);
        assert!(!output.text_content().contains("qq-spill"));
        assert!(!output.text_content().contains("Full output saved"));

        // No spill file should exist in the tmp dir.
        let entries: Vec<_> = std::fs::read_dir(mounts.tmp_dir())
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_name().to_string_lossy().starts_with("qq-spill-"))
            .collect();
        assert!(
            entries.is_empty(),
            "expected no spill files, found {}",
            entries.len()
        );
    }

    #[test]
    fn test_spill_created_on_truncation() {
        let input = big_stdout(500);
        let result = make_result(&input, "", 0);
        let (mounts, _root) = test_mounts();
        let output = format_output(result, &mounts);
        let text = output.text_content();

        assert!(
            text.contains("/tmp/qq-spill-1.txt"),
            "footer missing spill path: {}",
            text
        );
        assert!(
            text.contains("Full output saved"),
            "footer missing spill header"
        );
        assert!(text.contains("sed -n"), "footer missing sed sample");
        assert!(
            text.contains("grep -n PATTERN"),
            "footer missing grep sample"
        );
        assert!(text.contains("wc -l"), "footer missing wc sample");
        assert!(
            !text.contains("hard-capped"),
            "unexpected hard-cap note on small spill"
        );

        // The spill file on disk must contain the FULL untruncated stdout.
        let host_path = mounts.tmp_dir().join("qq-spill-1.txt");
        let contents = std::fs::read_to_string(&host_path).unwrap();
        assert_eq!(
            contents, input,
            "spill file contents must match original stdout"
        );
        assert!(contents.contains("line 1\n"));
        assert!(contents.contains("line 250\n"));
        assert!(contents.ends_with("line 500"));
    }

    #[test]
    fn test_spill_counter_increments_across_calls() {
        let input = big_stdout(500);
        let (mounts, _root) = test_mounts();

        let out1 = format_output(make_result(&input, "", 0), &mounts);
        let out2 = format_output(make_result(&input, "", 0), &mounts);

        assert!(out1.text_content().contains("/tmp/qq-spill-1.txt"));
        assert!(out2.text_content().contains("/tmp/qq-spill-2.txt"));
        assert!(mounts.tmp_dir().join("qq-spill-1.txt").exists());
        assert!(mounts.tmp_dir().join("qq-spill-2.txt").exists());
    }

    #[test]
    fn test_spill_hard_cap_branch() {
        // A ~20 MB stdout forces the hard-cap path. Keep it ASCII so the
        // char-boundary slice is trivial.
        let blob = "a".repeat(20 * 1024 * 1024);
        let result = make_result(&blob, "", 0);
        let (mounts, _root) = test_mounts();
        let output = format_output(result, &mounts);
        let text = output.text_content();

        assert!(text.contains("/tmp/qq-spill-1.txt"));
        assert!(
            text.contains("hard-capped"),
            "footer must mention hard-cap: {}",
            text
        );

        let spill_bytes = std::fs::read(mounts.tmp_dir().join("qq-spill-1.txt")).unwrap();
        // Head 8 MB + marker (< 128 B) + tail 8 MB < 20 MB.
        assert!(
            spill_bytes.len() < 20 * 1024 * 1024,
            "spill file should be smaller than original"
        );
        assert!(
            spill_bytes.len() <= MAX_SPILL_BYTES + 128,
            "spill file should fit in MAX_SPILL_BYTES + small marker"
        );
        let spill_text = String::from_utf8(spill_bytes).unwrap();
        assert!(spill_text.contains("spill file hard-capped"));
    }

    #[test]
    fn test_spill_not_created_for_binary_output() {
        let result = sandbox::CommandResult {
            // A binary blob large enough that, if it were text, it would
            // trigger truncation. But binary detection must short-circuit
            // before any spill consideration.
            stdout: {
                let mut v = vec![0u8; 80 * 1024];
                v[..4].copy_from_slice(&[0x7f, b'E', b'L', b'F']);
                String::from_utf8_lossy(&v).into_owned()
            },
            stderr: String::new(),
            exit_code: 0,
            timed_out: false,
            sandbox_error: None,
            duration: Duration::from_millis(10),
        };
        let (mounts, _root) = test_mounts();
        let output = format_output(result, &mounts);
        let text = output.text_content();

        assert!(text.contains("Binary output detected"));
        assert!(
            !text.contains("qq-spill"),
            "binary output must not spill: {}",
            text
        );

        let entries: Vec<_> = std::fs::read_dir(mounts.tmp_dir())
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_name().to_string_lossy().starts_with("qq-spill-"))
            .collect();
        assert!(entries.is_empty());
    }

    #[test]
    fn test_spill_footer_line_number_sample_reflects_original_size() {
        // 2000-line input → midpoint ≈ 1000 → sample window 950,1050.
        let input = big_stdout(2000);
        let result = make_result(&input, "", 0);
        let (mounts, _root) = test_mounts();
        let output = format_output(result, &mounts);
        let text = output.text_content();

        assert!(
            text.contains("'950,1050p'"),
            "sample window should be near midpoint: {}",
            text
        );
    }

    #[test]
    fn test_char_boundary_helpers_on_multibyte() {
        // 4-byte char at the boundary.
        let s = "aaa🦀bbb";
        let crab_start = 3; // byte index of 🦀
        let crab_end = 7;

        // floor_char_boundary rounds down into the middle of the 4-byte seq.
        assert_eq!(floor_char_boundary(s, crab_start + 2), crab_start);
        assert_eq!(floor_char_boundary(s, crab_end), crab_end);
        // ceil_char_boundary rounds up.
        assert_eq!(ceil_char_boundary(s, crab_start + 2), crab_end);
        assert_eq!(ceil_char_boundary(s, crab_start), crab_start);
    }

    // =========================================================================
    // Read-only RunTool tests
    // =========================================================================

    fn make_read_only_run_tool() -> RunTool {
        let mounts = Arc::new(SandboxMounts::new(std::env::current_dir().unwrap()).unwrap());
        let permissions = Arc::new(PermissionStore::new(std::collections::HashMap::new()));
        let (approval, _rx) = create_approval_channel();
        let path_policy = Arc::new(RwLock::new(SandboxPathPolicy::system_only()));
        RunTool::new(mounts, permissions, approval, path_policy).with_read_only(true)
    }

    #[tokio::test]
    async fn test_read_only_run_tool_blocks_write() {
        let tool = make_read_only_run_tool();

        // sed -i is a per-call command → should be blocked by read-only gate
        let result = tool
            .execute(serde_json::json!({
                "command": "sed -i 's/a/b/' somefile.txt"
            }))
            .await
            .unwrap();
        assert!(result.is_error);
        assert!(result.text_content().contains("Read-only agent"));

        // rm is also per-call → blocked
        let result = tool
            .execute(serde_json::json!({
                "command": "rm somefile.txt"
            }))
            .await
            .unwrap();
        assert!(result.is_error);
        assert!(result.text_content().contains("Read-only agent"));

        // python is per-call → blocked
        let result = tool
            .execute(serde_json::json!({
                "command": "python -c 'print(1)'"
            }))
            .await
            .unwrap();
        assert!(result.is_error);
        assert!(result.text_content().contains("Read-only agent"));
    }

    #[tokio::test]
    async fn test_read_only_run_tool_allows_reads() {
        let tool = make_read_only_run_tool();

        // grep is a session command → should be allowed
        let result = tool
            .execute(serde_json::json!({
                "command": "grep -rn 'fn main' Cargo.toml"
            }))
            .await
            .unwrap();
        // May or may not find matches, but should not be blocked
        assert!(!result.text_content().contains("Read-only agent"));

        // cat is a session command → should be allowed
        let result = tool
            .execute(serde_json::json!({
                "command": "cat Cargo.toml"
            }))
            .await
            .unwrap();
        assert!(!result.text_content().contains("Read-only agent"));
        assert!(!result.is_error);

        // find is a session command → should be allowed
        let result = tool
            .execute(serde_json::json!({
                "command": "find . -name 'Cargo.toml' -maxdepth 1"
            }))
            .await
            .unwrap();
        assert!(!result.text_content().contains("Read-only agent"));
    }

    #[test]
    fn test_read_only_tool_description() {
        let tool = make_read_only_run_tool();
        let desc = tool.tool_description();
        assert!(
            desc.contains("read-only"),
            "Description should mention read-only"
        );
        assert!(
            desc.contains("Blocked operations"),
            "Description should list blocked operations"
        );
        assert!(
            !desc.contains("Permission tiers"),
            "Description should not show permission tiers"
        );
    }
}
