//! Sandbox execution backends: kernel (hakoniwa) and app-level fallback.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use super::mounts::SandboxMounts;
use super::parse;

/// Result of a command execution.
#[derive(Debug)]
pub struct CommandResult {
    pub stdout: String,
    pub stderr: String,
    pub exit_code: i32,
    pub timed_out: bool,
    /// Container-level error (e.g. namespace/mount setup failure).
    pub sandbox_error: Option<String>,
    /// Wall-clock duration of the command execution.
    pub duration: Duration,
}

/// Sandbox executor backend.
pub enum SandboxExecutor {
    /// Kernel-level isolation via hakoniwa (Linux with user namespaces).
    #[cfg(feature = "sandbox")]
    Kernel,
    /// Application-level sandboxing (restricted, no pipes).
    AppLevel,
}

/// Resolved PATH and mount configuration for the kernel sandbox.
///
/// All branching happens at construction time; `execute_kernel` iterates
/// the lists uniformly with no match arms.
#[derive(Clone, Debug)]
pub struct SandboxPathPolicy {
    /// PATH value to set in the sandbox.
    pub path_value: String,
    /// Extra directories to bind-mount read-only.
    /// User mode: includes $HOME + non-system PATH dirs (/opt/…, /snap/…).
    /// Agent mode: empty (only baseline system mounts).
    pub ro_mounts: Vec<PathBuf>,
    /// Paths to hide behind empty tmpfs (mounted after ro_mounts).
    /// User mode: sensitive dirs like $HOME/.ssh, $HOME/.aws.
    /// Agent mode: empty.
    pub tmpfs_mounts: Vec<PathBuf>,
    /// Env vars to set in the sandbox. Applied in order (last write wins).
    /// User mode: all host env vars + sandbox overrides (TMPDIR, TERM, etc.).
    /// Agent mode: minimal set (HOME=/tmp, TMPDIR=/tmp, TERM, LC_ALL, GIT_TERMINAL_PROMPT).
    pub env_vars: Vec<(String, String)>,
}

/// Prefixes already covered by the standard system bind mounts.
const SYSTEM_MOUNT_PREFIXES: &[&str] =
    &["/bin", "/usr", "/lib", "/lib64", "/lib32", "/etc", "/sbin"];

/// Directory names under $HOME to hide behind empty tmpfs mounts.
pub const SENSITIVE_DIR_NAMES: &[&str] = &[
    ".ssh",
    ".gnupg",
    ".gpg",
    ".aws",
    ".kube",
    ".docker",
    ".password-store",
    ".netrc",
];

impl SandboxPathPolicy {
    /// Agent-mode policy: empty mount lists, minimal env, HOME=/tmp.
    pub fn system_only() -> Self {
        Self {
            path_value: "/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin".into(),
            ro_mounts: Vec::new(),
            tmpfs_mounts: Vec::new(),
            env_vars: vec![
                ("HOME".into(), "/tmp".into()),
                ("TMPDIR".into(), "/tmp".into()),
                ("TERM".into(), "dumb".into()),
                ("GIT_TERMINAL_PROMPT".into(), "0".into()),
                ("LC_ALL".into(), "C.UTF-8".into()),
            ],
        }
    }

    /// Build from the host environment: mount $HOME RO, forward all env vars,
    /// and hide sensitive directories behind empty tmpfs mounts.
    ///
    /// `allowed_sensitive_dirs` is a list of directory names (e.g. `[".ssh", ".aws"]`)
    /// that should **not** be hidden — they are excluded from the tmpfs overlay so they
    /// remain accessible from the first bash command without any tool call or approval.
    /// Names that are not in `SENSITIVE_DIR_NAMES` are silently ignored (they were never
    /// going to be hidden anyway).
    ///
    /// Falls back to `system_only()` if `$HOME` or `$PATH` is unset/empty/invalid.
    pub fn from_host_env(allowed_sensitive_dirs: &[String]) -> Self {
        let home = match std::env::var("HOME") {
            Ok(h) if !h.is_empty() && Path::new(&h).is_dir() => PathBuf::from(h),
            _ => return Self::system_only(),
        };

        let path_value = match std::env::var("PATH") {
            Ok(v) if !v.is_empty() => v,
            _ => return Self::system_only(),
        };

        // ro_mounts: start with $HOME, then non-system PATH dirs not under $HOME
        let mut ro_mounts = vec![home.clone()];
        let mut seen = HashSet::new();
        seen.insert(home.clone());

        for entry in path_value.split(':') {
            if entry.is_empty() || is_under_system_prefix(entry) {
                continue;
            }

            let path = Path::new(entry);
            if !path.is_dir() {
                continue;
            }

            let canonical = match path.canonicalize() {
                Ok(c) => c,
                Err(_) => continue,
            };

            if let Some(s) = canonical.to_str() {
                if is_under_system_prefix(s) {
                    continue;
                }
            } else {
                continue;
            }

            // Skip if under $HOME (already covered by the $HOME mount)
            if canonical.starts_with(&home) {
                continue;
            }

            if seen.insert(canonical.clone()) {
                ro_mounts.push(canonical);
            }
        }

        // tmpfs_mounts: sensitive dirs that exist under $HOME, minus pre-approved ones
        let tmpfs_mounts: Vec<PathBuf> = SENSITIVE_DIR_NAMES
            .iter()
            .filter(|name| !allowed_sensitive_dirs.iter().any(|a| a == *name))
            .map(|name| home.join(name))
            .filter(|p| p.exists())
            .collect();

        // env_vars: all host env vars, then overrides (last write wins)
        let mut env_vars: Vec<(String, String)> = std::env::vars().collect();
        env_vars.extend([
            ("TMPDIR".into(), "/tmp".into()),
            ("GIT_TERMINAL_PROMPT".into(), "0".into()),
            ("TERM".into(), "dumb".into()),
            ("LC_ALL".into(), "C.UTF-8".into()),
        ]);

        Self {
            path_value,
            ro_mounts,
            tmpfs_mounts,
            env_vars,
        }
    }
}

/// Check if a path string starts with any system mount prefix.
fn is_under_system_prefix(path: &str) -> bool {
    for prefix in SYSTEM_MOUNT_PREFIXES {
        if path == *prefix || path.starts_with(&format!("{}/", prefix)) {
            return true;
        }
    }
    false
}

impl SandboxExecutor {
    /// Detect the best available sandbox backend.
    pub fn detect() -> Self {
        #[cfg(feature = "sandbox")]
        {
            if probe_user_namespaces() {
                tracing::info!("Kernel sandbox available (user namespaces supported)");
                return SandboxExecutor::Kernel;
            }
            tracing::warn!("User namespaces not available, falling back to app-level sandbox");
        }

        #[cfg(not(feature = "sandbox"))]
        {
            tracing::info!("Sandbox feature disabled, using app-level sandbox");
        }

        SandboxExecutor::AppLevel
    }

    /// Human-readable name for this executor.
    pub fn mode_name(&self) -> &'static str {
        match self {
            #[cfg(feature = "sandbox")]
            SandboxExecutor::Kernel => "kernel",
            SandboxExecutor::AppLevel => "app-level",
        }
    }

    /// Whether this executor supports shell operators (pipes, redirects, etc.).
    pub fn supports_shell(&self) -> bool {
        match self {
            #[cfg(feature = "sandbox")]
            SandboxExecutor::Kernel => true,
            SandboxExecutor::AppLevel => false,
        }
    }

    /// Execute a command string in the sandbox.
    ///
    /// When `read_only` is true, the kernel sandbox mounts the project root
    /// read-only so that even shell redirects (`echo > file`) fail at the
    /// filesystem level.
    pub async fn execute(
        &self,
        command: &str,
        mounts: &Arc<SandboxMounts>,
        timeout_secs: u64,
        path_policy: &SandboxPathPolicy,
        stdin_data: Option<&str>,
        read_only: bool,
    ) -> Result<CommandResult, String> {
        match self {
            #[cfg(feature = "sandbox")]
            SandboxExecutor::Kernel => {
                let mounts = Arc::clone(mounts);
                let cmd = command.to_string();
                let policy = path_policy.clone();
                let stdin = stdin_data.map(|s| s.as_bytes().to_vec());
                tokio::task::spawn_blocking(move || {
                    execute_kernel(
                        &cmd,
                        &mounts,
                        timeout_secs,
                        &policy,
                        stdin.as_deref(),
                        read_only,
                    )
                })
                .await
                .map_err(|e| format!("Sandbox task failed: {}", e))?
            }
            SandboxExecutor::AppLevel => {
                execute_app_level(command, mounts, timeout_secs, stdin_data).await
            }
        }
    }
}

/// 0 = not probed, 1 = available, 2 = unavailable
#[cfg(feature = "sandbox")]
static USERNS_CACHE: AtomicU8 = AtomicU8::new(0);

/// Probe whether user namespaces are available (cached after first call).
#[cfg(feature = "sandbox")]
pub fn probe_user_namespaces() -> bool {
    match USERNS_CACHE.load(Ordering::Relaxed) {
        1 => return true,
        2 => return false,
        _ => {}
    }

    let result = probe_user_namespaces_uncached();
    USERNS_CACHE.store(if result { 1 } else { 2 }, Ordering::Relaxed);
    result
}

/// Actual probe — spins up a throwaway container to test namespace support.
#[cfg(feature = "sandbox")]
fn probe_user_namespaces_uncached() -> bool {
    use hakoniwa::Container;

    let mut container = Container::new();
    container.runctl(hakoniwa::Runctl::MountFallback);
    container
        .bindmount_ro("/bin", "/bin")
        .bindmount_ro("/usr", "/usr")
        .bindmount_ro("/lib", "/lib");

    if std::path::Path::new("/lib64").exists() {
        container.bindmount_ro("/lib64", "/lib64");
    }

    let output = container.command("/bin/true").wait_timeout(5).output();

    match output {
        Ok(o) => o.status.success(),
        Err(_) => false,
    }
}

/// Execute a command in a hakoniwa kernel sandbox.
///
/// When `read_only` is true, the project root is mounted read-only so that
/// write operations (including shell redirects) fail at the filesystem level.
/// `/tmp` remains writable for scratch work.
#[cfg(feature = "sandbox")]
pub fn execute_kernel(
    command: &str,
    mounts: &SandboxMounts,
    timeout_secs: u64,
    policy: &SandboxPathPolicy,
    stdin_data: Option<&[u8]>,
    read_only: bool,
) -> Result<CommandResult, String> {
    use hakoniwa::Container;

    let root = mounts.project_root();
    let root_str = root.to_str().ok_or("Project root is not valid UTF-8")?;

    let mut container = Container::new();

    // Allow remount fallback: on newer kernels (6.17+), remounting bind mounts
    // requires preserving CL_UNPRIVILEGED locked flags (nodev, nosuid, etc.).
    // Without this, remount_rdonly fails with EPERM inside user namespaces.
    container.runctl(hakoniwa::Runctl::MountFallback);

    // Mount system directories read-only
    container
        .bindmount_ro("/bin", "/bin")
        .bindmount_ro("/usr", "/usr")
        .bindmount_ro("/lib", "/lib")
        .bindmount_ro("/etc", "/etc")
        .bindmount_ro("/sbin", "/sbin");

    // Mount lib64 if it exists (common on 64-bit Linux)
    if std::path::Path::new("/lib64").exists() {
        container.bindmount_ro("/lib64", "/lib64");
    }
    // Mount lib32 if it exists
    if std::path::Path::new("/lib32").exists() {
        container.bindmount_ro("/lib32", "/lib32");
    }

    // DNS resolution: /etc/resolv.conf is often a symlink (e.g. to
    // /run/systemd/resolve/stub-resolv.conf). Resolve the real path and
    // bind-mount its parent directory so DNS works inside the sandbox.
    if let Ok(real) = std::fs::canonicalize("/etc/resolv.conf") {
        if let Some(parent) = real.parent() {
            if !is_under_system_prefix(&parent.to_string_lossy()) {
                if let Some(parent_str) = parent.to_str() {
                    container.bindmount_ro(parent_str, parent_str);
                }
            }
        }
    }

    // Virtual filesystems
    container.procfsmount("/proc").devfsmount("/dev");

    // Bind-mount the per-instance /tmp directory (persists across commands)
    let tmp_path = mounts.tmp_dir();
    let tmp_str = tmp_path
        .to_str()
        .ok_or("Instance /tmp path is not valid UTF-8")?;
    container.bindmount_rw(tmp_str, "/tmp");

    // Project root: read-only for read-only agents, read-write otherwise
    if read_only {
        container.bindmount_ro(root_str, root_str);
    } else {
        container.bindmount_rw(root_str, root_str);
    }

    // Extra user mounts: read-only
    let extra_mounts = mounts.list_extra();
    let extra_mount_set: HashSet<&Path> =
        extra_mounts.iter().map(|m| m.host_path.as_path()).collect();
    for mount in &extra_mounts {
        if let Some(path_str) = mount.host_path.to_str() {
            container.bindmount_ro(path_str, path_str);
        }
    }

    // Policy ro_mounts: $HOME + non-system PATH dirs (user mode) or empty (agent mode).
    // Skip dirs already covered by project root or extra mounts.
    for dir in &policy.ro_mounts {
        if dir.as_path() == root || extra_mount_set.contains(dir.as_path()) {
            continue;
        }
        if let Some(dir_str) = dir.to_str() {
            container.bindmount_ro(dir_str, dir_str);
        }
    }

    // Policy tmpfs_mounts: hide sensitive dirs behind empty tmpfs.
    // hakoniwa sorts mounts alphabetically, so parent RO mounts apply first.
    for dir in &policy.tmpfs_mounts {
        if let Some(dir_str) = dir.to_str() {
            container.tmpfsmount(dir_str);
        }
    }

    // Collect env vars: policy vars first, then PATH override (last write wins)
    let mut env_vars: Vec<(&str, &str)> = policy
        .env_vars
        .iter()
        .map(|(k, v)| (k.as_str(), v.as_str()))
        .collect();
    env_vars.push(("PATH", &policy.path_value));

    let mut cmd = container.command("/bin/sh");
    let cmd = cmd.arg("-c").arg(command).current_dir(root_str);
    for (key, value) in &env_vars {
        cmd.env(key, value);
    }

    // Note: hakoniwa does not support piped stdin; if stdin_data is provided,
    // we write it to a temp file and redirect from there.
    let _stdin_tmpfile;
    let effective_command;
    if let Some(data) = stdin_data {
        let tmp = mounts.tmp_dir().join(".stdin_pipe");
        std::fs::write(&tmp, data).map_err(|e| format!("Failed to write stdin data: {}", e))?;
        effective_command = format!(
            "cat {} | /bin/sh -c {}",
            tmp.display(),
            shell_escape(command)
        );
        _stdin_tmpfile = Some(tmp);

        // Re-build the command with the piped version
        let mut cmd2 = container.command("/bin/sh");
        let cmd2 = cmd2.arg("-c").arg(&effective_command).current_dir(root_str);
        for (key, value) in &env_vars {
            cmd2.env(key, value);
        }

        let start = Instant::now();
        let output = cmd2
            .wait_timeout(timeout_secs)
            .output()
            .map_err(|e| format!("Sandbox execution failed: {}", e))?;
        let duration = start.elapsed();

        let (timed_out, sandbox_error) = classify_hakoniwa_status(&output);

        return Ok(CommandResult {
            stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
            exit_code: output.status.exit_code.unwrap_or(output.status.code),
            timed_out,
            sandbox_error,
            duration,
        });
    }

    let start = Instant::now();
    let output = cmd
        .wait_timeout(timeout_secs)
        .output()
        .map_err(|e| format!("Sandbox execution failed: {}", e))?;
    let duration = start.elapsed();

    let (timed_out, sandbox_error) = classify_hakoniwa_status(&output);

    Ok(CommandResult {
        stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        exit_code: output.status.exit_code.unwrap_or(output.status.code),
        timed_out,
        sandbox_error,
        duration,
    })
}

/// Classify hakoniwa exit status into timed_out / sandbox_error.
#[cfg(feature = "sandbox")]
fn classify_hakoniwa_status(output: &hakoniwa::Output) -> (bool, Option<String>) {
    if !output.status.success() && output.status.exit_code.is_none() {
        if output.status.code == 128 + 9 {
            (true, None)
        } else {
            (false, Some(output.status.reason.clone()))
        }
    } else {
        (false, None)
    }
}

/// Shell-escape a string for use in a shell command.
#[cfg(feature = "sandbox")]
fn shell_escape(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

/// Execute a command using app-level sandboxing (no kernel isolation).
///
/// This is significantly more restricted:
/// - No pipes, redirects, or shell operators
/// - Direct exec only (no shell)
/// - Only session-tier commands allowed (enforced by caller)
async fn execute_app_level(
    command: &str,
    mounts: &Arc<SandboxMounts>,
    timeout_secs: u64,
    stdin_data: Option<&str>,
) -> Result<CommandResult, String> {
    // Reject shell operators — we can't safely sandbox them without namespaces
    if parse::has_shell_operators(command) {
        return Err(
            "Shell operators (pipes, redirects, etc.) are not supported in app-level sandbox mode. \
             Kernel sandbox (Linux with user namespaces) is required for pipeline commands."
                .to_string(),
        );
    }

    // Tokenize the command
    let tokens = parse::tokenize(command).map_err(|e| format!("Failed to parse command: {}", e))?;

    if tokens.is_empty() {
        return Err("Empty command".to_string());
    }

    // Rewrite /tmp references to the session temp dir so that app-level bash
    // commands see the same files as filesystem tools (which use remap_tmp).
    let tmp_str = mounts.tmp_dir().to_str().unwrap_or("/tmp");
    let tokens: Vec<String> = tokens
        .into_iter()
        .map(|t| remap_tmp_in_token(&t, tmp_str))
        .collect();

    let program = &tokens[0];
    let args = &tokens[1..];

    // Resolve the program path
    let program_path = resolve_program(program)?;

    let mut cmd = tokio::process::Command::new(&program_path);
    cmd.args(args);
    cmd.current_dir(mounts.project_root());
    cmd.env("HOME", tmp_str);
    cmd.env("TMPDIR", tmp_str);
    cmd.env("TERM", "dumb");
    cmd.env("GIT_TERMINAL_PROMPT", "0");
    cmd.env("LC_ALL", "C.UTF-8");

    // Capture output
    cmd.stdout(std::process::Stdio::piped());
    cmd.stderr(std::process::Stdio::piped());

    if stdin_data.is_some() {
        cmd.stdin(std::process::Stdio::piped());
    }

    let start = Instant::now();
    let result = if let Some(data) = stdin_data {
        // Spawn, write to stdin, then wait
        let child = cmd
            .spawn()
            .map_err(|e| format!("Failed to spawn command: {}", e))?;
        let mut child = child;
        if let Some(mut stdin) = child.stdin.take() {
            use tokio::io::AsyncWriteExt;
            let _ = stdin.write_all(data.as_bytes()).await;
            drop(stdin);
        }
        tokio::time::timeout(Duration::from_secs(timeout_secs), child.wait_with_output()).await
    } else {
        tokio::time::timeout(Duration::from_secs(timeout_secs), cmd.output()).await
    };
    let duration = start.elapsed();

    match result {
        Ok(Ok(output)) => Ok(CommandResult {
            stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
            exit_code: output.status.code().unwrap_or(-1),
            timed_out: false,
            sandbox_error: None,
            duration,
        }),
        Ok(Err(e)) => Err(format!("Failed to execute command: {}", e)),
        Err(_) => Ok(CommandResult {
            stdout: String::new(),
            stderr: format!("Command timed out after {} seconds", timeout_secs),
            exit_code: -1,
            timed_out: true,
            sandbox_error: None,
            duration: start.elapsed(),
        }),
    }
}

/// Rewrite `/tmp` references in a single command token to point at the session temp dir.
///
/// This mirrors the `remap_tmp()` behavior that filesystem tools use, so that bash commands
/// in app-level sandbox mode can access the same files as `write_file("/tmp/foo", ...)`.
///
/// Handles:
/// - Exact `/tmp` → session tmp path
/// - `/tmp/...` prefix → session tmp path + suffix
/// - Embedded `=/tmp` or `=/tmp/...` (e.g., `--output=/tmp/result.json`)
fn remap_tmp_in_token(token: &str, session_tmp: &str) -> String {
    // Exact match
    if token == "/tmp" {
        return session_tmp.to_string();
    }

    // Prefix match: /tmp/ followed by more path
    if let Some(rest) = token.strip_prefix("/tmp/") {
        return format!("{}/{}", session_tmp, rest);
    }

    // Embedded: something=/tmp or something=/tmp/...
    if let Some(eq_pos) = token.find('=') {
        let (prefix, value) = token.split_at(eq_pos + 1); // prefix includes '='
        if value == "/tmp" {
            return format!("{}{}", prefix, session_tmp);
        }
        if let Some(rest) = value.strip_prefix("/tmp/") {
            return format!("{}{}/{}", prefix, session_tmp, rest);
        }
    }

    token.to_string()
}

/// Resolve a program name to a path.
fn resolve_program(program: &str) -> Result<PathBuf, String> {
    // If it's already a path, validate it
    if program.contains('/') {
        let path = PathBuf::from(program);
        if path.exists() {
            return Ok(path);
        }
        return Err(format!("Program not found: {}", program));
    }

    // Search PATH
    if let Ok(path_env) = std::env::var("PATH") {
        for dir in path_env.split(':') {
            let candidate = PathBuf::from(dir).join(program);
            if candidate.exists() {
                return Ok(candidate);
            }
        }
    }

    Err(format!("Program not found in PATH: {}", program))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sandbox_detect() {
        let executor = SandboxExecutor::detect();
        // Should return some variant without panicking
        let _ = executor.mode_name();
    }

    #[test]
    fn test_resolve_program_ls() {
        let path = resolve_program("ls").unwrap();
        assert!(path.exists());
    }

    #[test]
    fn test_resolve_program_not_found() {
        assert!(resolve_program("definitely_not_a_program_xyz").is_err());
    }

    #[tokio::test]
    async fn test_app_level_simple() {
        let mounts = Arc::new(SandboxMounts::new(std::env::current_dir().unwrap()).unwrap());
        let result = execute_app_level("echo hello", &mounts, 10, None)
            .await
            .unwrap();
        assert_eq!(result.stdout.trim(), "hello");
        assert_eq!(result.exit_code, 0);
    }

    #[tokio::test]
    async fn test_app_level_rejects_pipes() {
        let mounts = Arc::new(SandboxMounts::new(std::env::current_dir().unwrap()).unwrap());
        let result = execute_app_level("echo hello | cat", &mounts, 10, None).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("not supported"));
    }

    #[test]
    fn test_remap_tmp_exact() {
        assert_eq!(remap_tmp_in_token("/tmp", "/sess/tmp"), "/sess/tmp");
    }

    #[test]
    fn test_remap_tmp_subpath() {
        assert_eq!(
            remap_tmp_in_token("/tmp/foo.rs", "/sess/tmp"),
            "/sess/tmp/foo.rs"
        );
    }

    #[test]
    fn test_remap_tmp_nested() {
        assert_eq!(
            remap_tmp_in_token("/tmp/a/b/c.txt", "/sess/tmp"),
            "/sess/tmp/a/b/c.txt"
        );
    }

    #[test]
    fn test_remap_tmp_trailing_slash() {
        assert_eq!(remap_tmp_in_token("/tmp/", "/sess/tmp"), "/sess/tmp/");
    }

    #[test]
    fn test_remap_tmp_no_match() {
        assert_eq!(remap_tmp_in_token("hello", "/sess/tmp"), "hello");
        assert_eq!(
            remap_tmp_in_token("/var/tmp/foo", "/sess/tmp"),
            "/var/tmp/foo"
        );
        assert_eq!(remap_tmp_in_token("/tmpfoo", "/sess/tmp"), "/tmpfoo");
        assert_eq!(remap_tmp_in_token("-la", "/sess/tmp"), "-la");
    }

    #[test]
    fn test_remap_tmp_embedded_eq() {
        assert_eq!(
            remap_tmp_in_token("--output=/tmp/result.json", "/sess/tmp"),
            "--output=/sess/tmp/result.json"
        );
        assert_eq!(
            remap_tmp_in_token("--dir=/tmp", "/sess/tmp"),
            "--dir=/sess/tmp"
        );
        // No /tmp in value — unchanged
        assert_eq!(
            remap_tmp_in_token("--output=/var/out.json", "/sess/tmp"),
            "--output=/var/out.json"
        );
    }

    #[tokio::test]
    async fn test_app_level_tmp_rewrite() {
        let mounts = Arc::new(SandboxMounts::new(std::env::current_dir().unwrap()).unwrap());

        // Write a file directly into the session tmp dir
        let tmp_file = mounts.tmp_dir().join("remap_test.txt");
        std::fs::write(&tmp_file, "remapped-content").unwrap();

        // Run cat /tmp/remap_test.txt — should be rewritten to the session dir
        let result = execute_app_level("cat /tmp/remap_test.txt", &mounts, 10, None)
            .await
            .unwrap();
        assert_eq!(result.stdout.trim(), "remapped-content");
        assert_eq!(result.exit_code, 0);
    }

    #[test]
    fn test_path_policy_filters_system_dirs() {
        // Test the filtering logic directly without mutating the process env
        assert!(is_under_system_prefix("/usr/bin"));
        assert!(is_under_system_prefix("/usr/local/bin"));
        assert!(is_under_system_prefix("/bin"));
        assert!(is_under_system_prefix("/lib/x86_64-linux-gnu"));
        assert!(is_under_system_prefix("/sbin"));
        assert!(is_under_system_prefix("/etc/alternatives"));
        assert!(!is_under_system_prefix("/home/user/.cargo/bin"));
        assert!(!is_under_system_prefix("/opt/local/bin"));
        assert!(!is_under_system_prefix("/snap/bin"));
    }

    #[test]
    fn test_path_policy_from_host_env() {
        // from_host_env reads $HOME and $PATH which are set in test processes;
        // just verify it returns a valid struct without panicking
        let policy = SandboxPathPolicy::from_host_env(&[]);
        assert!(!policy.path_value.is_empty());
        // All ro_mounts should be non-system
        for dir in &policy.ro_mounts {
            let s = dir.to_str().unwrap();
            assert!(!is_under_system_prefix(s), "system dir leaked: {}", s);
        }
        // If $HOME is set, it should be the first ro_mount
        if let Ok(home) = std::env::var("HOME") {
            if !home.is_empty() && Path::new(&home).is_dir() {
                assert_eq!(policy.ro_mounts[0], PathBuf::from(&home));
            }
        }
    }

    #[test]
    fn test_path_policy_system_only() {
        let policy = SandboxPathPolicy::system_only();
        assert!(policy.ro_mounts.is_empty());
        assert!(policy.tmpfs_mounts.is_empty());
        assert_eq!(
            policy.path_value,
            "/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin"
        );
        // Should have HOME=/tmp in env_vars
        assert!(policy
            .env_vars
            .iter()
            .any(|(k, v)| k == "HOME" && v == "/tmp"));
    }

    #[test]
    fn test_path_policy_deduplication() {
        // Test dedup logic by constructing the policy manually: parse the same
        // PATH-like string through the same algorithm without env mutation.
        let tmpdir = std::env::temp_dir();
        let tmpdir_str = tmpdir.to_str().unwrap();

        // Simulate from_host_env logic inline
        let fake_path = format!("{}:{}", tmpdir_str, tmpdir_str);
        let mut seen = std::collections::HashSet::new();
        let mut extra_dirs = Vec::new();
        for entry in fake_path.split(':') {
            if entry.is_empty() || is_under_system_prefix(entry) {
                continue;
            }
            let path = Path::new(entry);
            if !path.is_dir() {
                continue;
            }
            let canonical = match path.canonicalize() {
                Ok(c) => c,
                Err(_) => continue,
            };
            if let Some(s) = canonical.to_str() {
                if is_under_system_prefix(s) {
                    continue;
                }
            }
            if seen.insert(canonical.clone()) {
                extra_dirs.push(canonical);
            }
        }
        // After dedup, should have at most 1 entry for the same tmpdir
        assert!(
            extra_dirs.len() <= 1,
            "Expected deduplication, got {} entries",
            extra_dirs.len()
        );
    }

    #[cfg(all(feature = "sandbox", target_os = "linux"))]
    mod kernel_tests {
        use super::*;

        fn sys_policy() -> SandboxPathPolicy {
            SandboxPathPolicy::system_only()
        }

        #[test]
        fn test_kernel_echo() {
            if !probe_user_namespaces() {
                eprintln!("Skipping: user namespaces not available");
                return;
            }
            let mounts = SandboxMounts::new(std::env::current_dir().unwrap()).unwrap();
            let result =
                execute_kernel("echo hello", &mounts, 10, &sys_policy(), None, false).unwrap();
            assert_eq!(result.stdout.trim(), "hello");
            assert_eq!(result.exit_code, 0);
        }

        #[test]
        fn test_kernel_pipe() {
            if !probe_user_namespaces() {
                eprintln!("Skipping: user namespaces not available");
                return;
            }
            let mounts = SandboxMounts::new(std::env::current_dir().unwrap()).unwrap();
            let result =
                execute_kernel("echo hello | cat", &mounts, 10, &sys_policy(), None, false)
                    .unwrap();
            assert_eq!(result.stdout.trim(), "hello");
        }

        #[test]
        fn test_kernel_tmpfs_write() {
            if !probe_user_namespaces() {
                eprintln!("Skipping: user namespaces not available");
                return;
            }
            let mounts = SandboxMounts::new(std::env::current_dir().unwrap()).unwrap();
            let result = execute_kernel(
                "echo test > /tmp/test.txt && cat /tmp/test.txt",
                &mounts,
                10,
                &sys_policy(),
                None,
                false,
            )
            .unwrap();
            assert_eq!(result.stdout.trim(), "test");
        }

        #[test]
        fn test_kernel_tmp_persistence() {
            if !probe_user_namespaces() {
                eprintln!("Skipping: user namespaces not available");
                return;
            }
            let mounts = SandboxMounts::new(std::env::current_dir().unwrap()).unwrap();
            let policy = sys_policy();
            // Write in first command
            let r1 = execute_kernel(
                "echo persist > /tmp/persist.txt",
                &mounts,
                10,
                &policy,
                None,
                false,
            )
            .unwrap();
            assert_eq!(r1.exit_code, 0);
            // Read in second, separate command
            let r2 =
                execute_kernel("cat /tmp/persist.txt", &mounts, 10, &policy, None, false).unwrap();
            assert_eq!(r2.stdout.trim(), "persist");
        }

        /// End-to-end: a command that overflows truncation should produce a
        /// spill file the next `run` call can query from inside the sandbox.
        #[test]
        fn test_kernel_spill_file_cross_call() {
            if !probe_user_namespaces() {
                eprintln!("Skipping: user namespaces not available");
                return;
            }
            let mounts = SandboxMounts::new(std::env::current_dir().unwrap()).unwrap();
            let policy = sys_policy();

            // 1. Produce 500 lines of output — exceeds the 200-line truncation cap.
            let r1 = execute_kernel(
                "yes hello | head -n 500",
                &mounts,
                10,
                &policy,
                None,
                false,
            )
            .unwrap();
            assert_eq!(r1.exit_code, 0);
            assert_eq!(r1.stdout.lines().count(), 500);

            // 2. format_output writes the spill file as a side-effect.
            let out1 = crate::bash::format_output(r1, &mounts);
            let text = out1.text_content();
            assert!(
                text.contains("/tmp/qq-spill-1.txt"),
                "footer should reference spill path: {}",
                text
            );

            // 3. A second, separate kernel command must be able to read it.
            let r2 = execute_kernel(
                "wc -l /tmp/qq-spill-1.txt",
                &mounts,
                10,
                &policy,
                None,
                false,
            )
            .unwrap();
            assert_eq!(r2.exit_code, 0);
            assert!(
                r2.stdout.trim_start().starts_with("500 "),
                "wc -l should report 500 lines, got: {:?}",
                r2.stdout
            );

            // 4. Spot-check that sed into the middle works too.
            let r3 = execute_kernel(
                "sed -n '250p' /tmp/qq-spill-1.txt",
                &mounts,
                10,
                &policy,
                None,
                false,
            )
            .unwrap();
            assert_eq!(r3.exit_code, 0);
            assert_eq!(r3.stdout.trim(), "hello");
        }

        #[test]
        fn test_kernel_glob() {
            if !probe_user_namespaces() {
                eprintln!("Skipping: user namespaces not available");
                return;
            }
            let mounts = SandboxMounts::new(std::env::current_dir().unwrap()).unwrap();
            let result = execute_kernel(
                "ls *.toml 2>/dev/null || echo no-match",
                &mounts,
                10,
                &sys_policy(),
                None,
                false,
            )
            .unwrap();
            // Should either list toml files or say no-match
            assert!(!result.stdout.is_empty());
        }

        #[test]
        fn test_kernel_system_only_path() {
            if !probe_user_namespaces() {
                eprintln!("Skipping: user namespaces not available");
                return;
            }
            let mounts = SandboxMounts::new(std::env::current_dir().unwrap()).unwrap();
            let result =
                execute_kernel("echo $PATH", &mounts, 10, &sys_policy(), None, false).unwrap();
            assert_eq!(
                result.stdout.trim(),
                "/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin"
            );
        }

        #[test]
        fn test_kernel_host_path() {
            if !probe_user_namespaces() {
                eprintln!("Skipping: user namespaces not available");
                return;
            }
            let mounts = SandboxMounts::new(std::env::current_dir().unwrap()).unwrap();
            let custom_path = "/usr/local/bin:/usr/bin:/bin:/custom/path";
            let policy = SandboxPathPolicy {
                path_value: custom_path.into(),
                ..SandboxPathPolicy::system_only()
            };
            let result = execute_kernel("echo $PATH", &mounts, 10, &policy, None, false).unwrap();
            assert_eq!(result.stdout.trim(), custom_path);
        }

        #[test]
        fn test_kernel_home_dir_mounted() {
            if !probe_user_namespaces() {
                eprintln!("Skipping: user namespaces not available");
                return;
            }
            let home = match std::env::var("HOME") {
                Ok(h) if Path::new(&h).is_dir() => h,
                _ => {
                    eprintln!("Skipping: $HOME not set or not a directory");
                    return;
                }
            };
            let mounts = SandboxMounts::new(std::env::current_dir().unwrap()).unwrap();
            let policy = SandboxPathPolicy::from_host_env(&[]);
            // Should be able to list the home directory contents
            let cmd = format!("ls -d {}", home);
            let result = execute_kernel(&cmd, &mounts, 10, &policy, None, false).unwrap();
            assert_eq!(result.exit_code, 0, "stderr: {}", result.stderr);
            assert!(result.stdout.trim().contains(&home));
        }

        #[test]
        fn test_kernel_sensitive_dirs_hidden() {
            if !probe_user_namespaces() {
                eprintln!("Skipping: user namespaces not available");
                return;
            }
            let home = match std::env::var("HOME") {
                Ok(h) if Path::new(&h).is_dir() => h,
                _ => {
                    eprintln!("Skipping: $HOME not set or not a directory");
                    return;
                }
            };
            let ssh_dir = PathBuf::from(&home).join(".ssh");
            if !ssh_dir.exists() {
                eprintln!("Skipping: ~/.ssh does not exist");
                return;
            }
            let mounts = SandboxMounts::new(std::env::current_dir().unwrap()).unwrap();
            let policy = SandboxPathPolicy::from_host_env(&[]);
            // .ssh should be empty (hidden by tmpfs)
            let cmd = format!("ls -A {}/.ssh 2>&1", home);
            let result = execute_kernel(&cmd, &mounts, 10, &policy, None, false).unwrap();
            assert!(
                result.stdout.trim().is_empty(),
                "Expected ~/.ssh to be empty in sandbox, got: {}",
                result.stdout.trim()
            );
        }

        #[test]
        fn test_kernel_read_only_blocks_write() {
            if !probe_user_namespaces() {
                eprintln!("Skipping: user namespaces not available");
                return;
            }
            let mounts = SandboxMounts::new(std::env::current_dir().unwrap()).unwrap();
            // Attempt to write to the project root — should fail with read-only mount
            let _result = execute_kernel(
                "echo test > /tmp/ro_test_canary.txt 2>&1; echo test > Cargo.toml.bak 2>&1; echo $?",
                &mounts,
                10,
                &sys_policy(),
                None,
                true, // read_only
            )
            .unwrap();
            // The write to Cargo.toml.bak should fail (read-only filesystem)
            // while /tmp write should succeed
            assert!(
                !std::path::Path::new("Cargo.toml.bak").exists(),
                "Write to project root should have been blocked by read-only mount"
            );
        }

        #[test]
        fn test_kernel_read_only_allows_tmp_write() {
            if !probe_user_namespaces() {
                eprintln!("Skipping: user namespaces not available");
                return;
            }
            let mounts = SandboxMounts::new(std::env::current_dir().unwrap()).unwrap();
            // /tmp should still be writable even in read-only mode
            let result = execute_kernel(
                "echo scratch > /tmp/ro_scratch.txt && cat /tmp/ro_scratch.txt",
                &mounts,
                10,
                &sys_policy(),
                None,
                true, // read_only
            )
            .unwrap();
            assert_eq!(result.stdout.trim(), "scratch");
            assert_eq!(result.exit_code, 0);
        }

        #[test]
        fn test_kernel_read_only_allows_reads() {
            if !probe_user_namespaces() {
                eprintln!("Skipping: user namespaces not available");
                return;
            }
            let mounts = SandboxMounts::new(std::env::current_dir().unwrap()).unwrap();
            // Reading from the project root should work fine
            let result = execute_kernel(
                "cat Cargo.toml | head -1",
                &mounts,
                10,
                &sys_policy(),
                None,
                true, // read_only
            )
            .unwrap();
            assert_eq!(result.exit_code, 0);
            assert!(!result.stdout.trim().is_empty());
        }
    }
}
