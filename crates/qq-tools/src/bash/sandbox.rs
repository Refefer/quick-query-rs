//! Sandbox execution backends: kernel (hakoniwa) and app-level fallback.

use std::path::PathBuf;
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::Arc;

use super::mounts::SandboxMounts;
use super::parse;

/// Result of a command execution.
#[derive(Debug)]
pub struct CommandResult {
    pub stdout: String,
    pub stderr: String,
    pub exit_code: i32,
    pub timed_out: bool,
}

/// Sandbox executor backend.
pub enum SandboxExecutor {
    /// Kernel-level isolation via hakoniwa (Linux with user namespaces).
    #[cfg(feature = "sandbox")]
    Kernel,
    /// Application-level sandboxing (restricted, no pipes).
    AppLevel,
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
    pub async fn execute(
        &self,
        command: &str,
        mounts: &Arc<SandboxMounts>,
        timeout_secs: u64,
    ) -> Result<CommandResult, String> {
        match self {
            #[cfg(feature = "sandbox")]
            SandboxExecutor::Kernel => {
                let mounts = Arc::clone(mounts);
                let cmd = command.to_string();
                tokio::task::spawn_blocking(move || {
                    execute_kernel(&cmd, &mounts, timeout_secs)
                })
                .await
                .map_err(|e| format!("Sandbox task failed: {}", e))?
            }
            SandboxExecutor::AppLevel => {
                execute_app_level(command, mounts, timeout_secs).await
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
    container
        .bindmount_ro("/bin", "/bin")
        .bindmount_ro("/usr", "/usr")
        .bindmount_ro("/lib", "/lib");

    if std::path::Path::new("/lib64").exists() {
        container.bindmount_ro("/lib64", "/lib64");
    }

    let output = container
        .command("/bin/true")
        .wait_timeout(5)
        .output();

    match output {
        Ok(o) => o.status.success(),
        Err(_) => false,
    }
}

/// Execute a command in a hakoniwa kernel sandbox.
#[cfg(feature = "sandbox")]
pub fn execute_kernel(
    command: &str,
    mounts: &SandboxMounts,
    timeout_secs: u64,
) -> Result<CommandResult, String> {
    use hakoniwa::Container;

    let root = mounts.project_root();
    let root_str = root.to_str().ok_or("Project root is not valid UTF-8")?;

    let mut container = Container::new();

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

    // Virtual filesystems
    container
        .procfsmount("/proc")
        .devfsmount("/dev");

    // Bind-mount the per-instance /tmp directory (persists across commands)
    let tmp_path = mounts.tmp_dir();
    let tmp_str = tmp_path.to_str().ok_or("Instance /tmp path is not valid UTF-8")?;
    container.bindmount_rw(tmp_str, "/tmp");

    // Project root: read-write
    container.bindmount_rw(root_str, root_str);

    // Extra mounts: read-only
    for mount in mounts.list_extra() {
        if let Some(path_str) = mount.host_path.to_str() {
            container.bindmount_ro(path_str, path_str);
        }
    }

    let output = container
        .command("/bin/sh")
        .arg("-c")
        .arg(command)
        .current_dir(root_str)
        .env("HOME", "/tmp")
        .env("TMPDIR", "/tmp")
        .env("TERM", "dumb")
        .env("PATH", "/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin")
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("LC_ALL", "C.UTF-8")
        .wait_timeout(timeout_secs)
        .output()
        .map_err(|e| format!("Sandbox execution failed: {}", e))?;

    let timed_out = !output.status.success() && output.status.exit_code.is_none();

    Ok(CommandResult {
        stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        exit_code: output.status.exit_code.unwrap_or(output.status.code),
        timed_out,
    })
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
    let tokens = parse::tokenize(command)
        .map_err(|e| format!("Failed to parse command: {}", e))?;

    if tokens.is_empty() {
        return Err("Empty command".to_string());
    }

    let program = &tokens[0];
    let args = &tokens[1..];

    // Resolve the program path
    let program_path = resolve_program(program)?;

    let mut cmd = tokio::process::Command::new(&program_path);
    cmd.args(args);
    cmd.current_dir(mounts.project_root());
    let tmp_str = mounts.tmp_dir().to_str().unwrap_or("/tmp");
    cmd.env("HOME", tmp_str);
    cmd.env("TMPDIR", tmp_str);
    cmd.env("TERM", "dumb");
    cmd.env("GIT_TERMINAL_PROMPT", "0");
    cmd.env("LC_ALL", "C.UTF-8");

    // Capture output
    cmd.stdout(std::process::Stdio::piped());
    cmd.stderr(std::process::Stdio::piped());

    let result = tokio::time::timeout(
        std::time::Duration::from_secs(timeout_secs),
        cmd.output(),
    )
    .await;

    match result {
        Ok(Ok(output)) => Ok(CommandResult {
            stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
            exit_code: output.status.code().unwrap_or(-1),
            timed_out: false,
        }),
        Ok(Err(e)) => Err(format!("Failed to execute command: {}", e)),
        Err(_) => Ok(CommandResult {
            stdout: String::new(),
            stderr: format!("Command timed out after {} seconds", timeout_secs),
            exit_code: -1,
            timed_out: true,
        }),
    }
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
        let mounts = Arc::new(SandboxMounts::new(
            std::env::current_dir().unwrap(),
        ).unwrap());
        let result = execute_app_level("echo hello", &mounts, 10).await.unwrap();
        assert_eq!(result.stdout.trim(), "hello");
        assert_eq!(result.exit_code, 0);
    }

    #[tokio::test]
    async fn test_app_level_rejects_pipes() {
        let mounts = Arc::new(SandboxMounts::new(
            std::env::current_dir().unwrap(),
        ).unwrap());
        let result = execute_app_level("echo hello | cat", &mounts, 10).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("not supported"));
    }

    #[cfg(all(feature = "sandbox", target_os = "linux"))]
    mod kernel_tests {
        use super::*;

        #[test]
        fn test_kernel_echo() {
            if !probe_user_namespaces() {
                eprintln!("Skipping: user namespaces not available");
                return;
            }
            let mounts = SandboxMounts::new(std::env::current_dir().unwrap()).unwrap();
            let result = execute_kernel("echo hello", &mounts, 10).unwrap();
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
            let result = execute_kernel("echo hello | cat", &mounts, 10).unwrap();
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
            // Write in first command
            let r1 = execute_kernel("echo persist > /tmp/persist.txt", &mounts, 10).unwrap();
            assert_eq!(r1.exit_code, 0);
            // Read in second, separate command
            let r2 = execute_kernel("cat /tmp/persist.txt", &mounts, 10).unwrap();
            assert_eq!(r2.stdout.trim(), "persist");
        }

        #[test]
        fn test_kernel_glob() {
            if !probe_user_namespaces() {
                eprintln!("Skipping: user namespaces not available");
                return;
            }
            let mounts = SandboxMounts::new(std::env::current_dir().unwrap()).unwrap();
            let result = execute_kernel("ls *.toml 2>/dev/null || echo no-match", &mounts, 10).unwrap();
            // Should either list toml files or say no-match
            assert!(!result.stdout.is_empty());
        }
    }
}
