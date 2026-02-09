//! Three-tier permission model for bash command execution.
//!
//! Commands are classified into three tiers:
//! - **Session**: Pre-approved read-only commands that run immediately
//! - **PerCall**: Write operations requiring user approval each time
//! - **Restricted**: Always blocked, cannot be approved

use std::collections::{HashMap, HashSet};
use std::sync::RwLock;
use tokio::sync::{mpsc, oneshot};

/// Permission tier for a command.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Tier {
    /// Runs immediately, no approval needed.
    Session = 0,
    /// Requires user approval per invocation.
    PerCall = 1,
    /// Always blocked.
    Restricted = 2,
}

/// Result of checking a full pipeline's permissions.
#[derive(Debug)]
pub enum PipelinePermission {
    /// All commands are session-tier.
    Allowed,
    /// Some commands require per-call approval (lists which ones).
    NeedsApproval(Vec<String>),
    /// Some commands are restricted (lists which ones).
    Restricted(Vec<String>),
}

/// Tracks permission state for the session.
pub struct PermissionStore {
    /// Commands promoted to session-tier during this session.
    session_promoted: RwLock<HashSet<String>>,
    /// Config-level overrides.
    config_overrides: HashMap<String, Tier>,
}

impl PermissionStore {
    pub fn new(overrides: HashMap<String, Tier>) -> Self {
        Self {
            session_promoted: RwLock::new(HashSet::new()),
            config_overrides: overrides,
        }
    }

    /// Check the tier of a single command.
    pub fn check_tier(&self, command: &str) -> Tier {
        // 1. Config overrides take precedence
        if let Some(&tier) = self.config_overrides.get(command) {
            return tier;
        }

        // 2. Session promotions
        if let Ok(promoted) = self.session_promoted.read() {
            if promoted.contains(command) {
                return Tier::Session;
            }
        }

        // 3. Default classification
        default_tier(command)
    }

    /// Check a pipeline (list of extracted command names) and return the required permission level.
    pub fn check_pipeline(&self, commands: &[String]) -> PipelinePermission {
        let mut restricted = Vec::new();
        let mut needs_approval = Vec::new();

        for cmd in commands {
            match self.check_tier(cmd) {
                Tier::Restricted => {
                    restricted.push(cmd.clone());
                }
                Tier::PerCall => {
                    needs_approval.push(cmd.clone());
                }
                Tier::Session => {}
            }
        }

        if !restricted.is_empty() {
            PipelinePermission::Restricted(restricted)
        } else if !needs_approval.is_empty() {
            PipelinePermission::NeedsApproval(needs_approval)
        } else {
            PipelinePermission::Allowed
        }
    }

    /// Promote a command to session tier for the rest of this session.
    pub fn promote_to_session(&self, command: &str) {
        if let Ok(mut promoted) = self.session_promoted.write() {
            promoted.insert(command.to_string());
        }
    }
}

/// Default tier classification for a command.
fn default_tier(command: &str) -> Tier {
    // Check restricted first
    if RESTRICTED_COMMANDS.contains(&command) {
        return Tier::Restricted;
    }

    // Check session (read-only) commands
    if SESSION_COMMANDS.contains(&command) {
        return Tier::Session;
    }

    // Check per-call (write) commands
    if PER_CALL_COMMANDS.contains(&command) {
        return Tier::PerCall;
    }

    // Unknown commands default to per-call (safe default)
    Tier::PerCall
}

// --- Default command classifications ---

const SESSION_COMMANDS: &[&str] = &[
    // File viewing / text processing
    "ls", "cat", "head", "tail", "wc", "sort", "uniq", "diff", "comm", "join",
    "paste", "cut", "tr", "fold", "nl", "od", "xxd", "strings", "tac", "rev",
    "column", "seq", "yes", "tee", "less", "more",
    // File finding / searching
    "find", "grep", "egrep", "fgrep", "rg", "ag",
    // File info
    "file", "stat", "du", "df", "tree", "basename", "dirname", "realpath",
    "readlink",
    // System info
    "pwd", "uname", "whoami", "which", "env", "printenv", "echo", "printf",
    "date", "true", "false", "test",
    // Checksums
    "sha256sum", "sha1sum", "md5sum", "b2sum",
    // Git read-only operations
    "git-log", "git-diff", "git-show", "git-status", "git-blame", "git-branch",
    "git-tag", "git-rev-parse", "git-describe", "git-shortlog", "git-ls-files",
    "git-ls-tree", "git-cat-file", "git-rev-list", "git-name-rev",
    "git-merge-base", "git-remote", "git-stash-list",
    // Misc
    "xargs", "id",
];

const PER_CALL_COMMANDS: &[&str] = &[
    // Git write operations
    "git-commit", "git-add", "git-checkout", "git-switch", "git-merge",
    "git-rebase", "git-stash", "git-stash-push", "git-stash-pop",
    "git-stash-apply", "git-stash-drop", "git-push", "git-pull", "git-fetch",
    "git-reset", "git-clean", "git-restore", "git-cherry-pick", "git-revert",
    "git-init", "git-clone",
    // Build tools
    "cargo", "npm", "npx", "yarn", "pnpm", "pip", "pip3", "poetry",
    "make", "cmake", "ninja", "meson",
    // Interpreters
    "python", "python3", "node", "ruby", "perl",
    // File modification
    "mv", "cp", "rm", "mkdir", "rmdir", "touch", "chmod", "ln",
    // Text modification (in-place)
    "sed", "awk", "patch",
    // Shells (sub-shells)
    "sh", "bash", "zsh",
    // Generic git (if subcommand not recognized)
    "git",
];

const RESTRICTED_COMMANDS: &[&str] = &[
    // Privilege escalation
    "sudo", "su", "doas", "pkexec",
    // Disk operations
    "dd", "mkfs", "fdisk", "parted", "mount", "umount",
    // System control
    "shutdown", "reboot", "halt", "poweroff", "init", "systemctl",
    // Process control (could affect host)
    "kill", "killall", "pkill",
    // Network
    "iptables", "ip", "ifconfig", "route", "tc",
    // Network transfer
    "curl", "wget", "nc", "ncat", "socat", "ssh", "scp", "rsync", "ftp",
    // User management
    "chown", "chgrp", "useradd", "userdel", "passwd", "usermod", "groupadd",
];

/// Request sent to the UI for user approval.
pub struct ApprovalRequest {
    /// The full command string the agent wants to run.
    pub full_command: String,
    /// Which specific commands triggered the per-call requirement.
    pub trigger_commands: Vec<String>,
    /// Channel to send the user's response back.
    pub response_tx: oneshot::Sender<ApprovalResponse>,
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

/// Sender side of the approval channel, held by BashTool.
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
    ) -> Result<ApprovalResponse, String> {
        let (response_tx, response_rx) = oneshot::channel();

        self.request_tx
            .send(ApprovalRequest {
                full_command,
                trigger_commands,
                response_tx,
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
/// Returns the sender (for BashTool) and receiver (for TUI/CLI).
pub fn create_approval_channel() -> (ApprovalChannel, mpsc::Receiver<ApprovalRequest>) {
    let (tx, rx) = mpsc::channel(8);
    (ApprovalChannel { request_tx: tx }, rx)
}

/// Parse config-level permission overrides into a tier map.
pub fn parse_config_overrides(
    session: &[String],
    per_call: &[String],
    restricted: &[String],
) -> HashMap<String, Tier> {
    let mut map = HashMap::new();
    for cmd in session {
        map.insert(cmd.clone(), Tier::Session);
    }
    for cmd in per_call {
        map.insert(cmd.clone(), Tier::PerCall);
    }
    for cmd in restricted {
        map.insert(cmd.clone(), Tier::Restricted);
    }
    map
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store() -> PermissionStore {
        PermissionStore::new(HashMap::new())
    }

    #[test]
    fn test_session_commands() {
        let s = store();
        assert_eq!(s.check_tier("ls"), Tier::Session);
        assert_eq!(s.check_tier("grep"), Tier::Session);
        assert_eq!(s.check_tier("wc"), Tier::Session);
        assert_eq!(s.check_tier("git-log"), Tier::Session);
        assert_eq!(s.check_tier("git-diff"), Tier::Session);
        assert_eq!(s.check_tier("git-status"), Tier::Session);
    }

    #[test]
    fn test_per_call_commands() {
        let s = store();
        assert_eq!(s.check_tier("cargo"), Tier::PerCall);
        assert_eq!(s.check_tier("rm"), Tier::PerCall);
        assert_eq!(s.check_tier("git-commit"), Tier::PerCall);
        assert_eq!(s.check_tier("python"), Tier::PerCall);
    }

    #[test]
    fn test_restricted_commands() {
        let s = store();
        assert_eq!(s.check_tier("sudo"), Tier::Restricted);
        assert_eq!(s.check_tier("curl"), Tier::Restricted);
        assert_eq!(s.check_tier("ssh"), Tier::Restricted);
        assert_eq!(s.check_tier("dd"), Tier::Restricted);
    }

    #[test]
    fn test_unknown_defaults_to_per_call() {
        let s = store();
        assert_eq!(s.check_tier("some_unknown_command"), Tier::PerCall);
    }

    #[test]
    fn test_pipeline_all_session() {
        let s = store();
        let cmds = vec!["grep".to_string(), "wc".to_string()];
        assert!(matches!(s.check_pipeline(&cmds), PipelinePermission::Allowed));
    }

    #[test]
    fn test_pipeline_needs_approval() {
        let s = store();
        let cmds = vec!["grep".to_string(), "cargo".to_string()];
        match s.check_pipeline(&cmds) {
            PipelinePermission::NeedsApproval(trigger) => {
                assert_eq!(trigger, vec!["cargo"]);
            }
            other => panic!("Expected NeedsApproval, got {:?}", std::mem::discriminant(&other)),
        }
    }

    #[test]
    fn test_pipeline_restricted() {
        let s = store();
        let cmds = vec!["ls".to_string(), "sudo".to_string()];
        match s.check_pipeline(&cmds) {
            PipelinePermission::Restricted(blocked) => {
                assert_eq!(blocked, vec!["sudo"]);
            }
            other => panic!("Expected Restricted, got {:?}", std::mem::discriminant(&other)),
        }
    }

    #[test]
    fn test_promote_to_session() {
        let s = store();
        assert_eq!(s.check_tier("cargo"), Tier::PerCall);
        s.promote_to_session("cargo");
        assert_eq!(s.check_tier("cargo"), Tier::Session);
    }

    #[test]
    fn test_config_overrides() {
        let overrides = parse_config_overrides(
            &["cargo".to_string()],
            &["tee".to_string()],
            &["python".to_string()],
        );
        let s = PermissionStore::new(overrides);
        assert_eq!(s.check_tier("cargo"), Tier::Session);
        assert_eq!(s.check_tier("tee"), Tier::PerCall);
        assert_eq!(s.check_tier("python"), Tier::Restricted);
    }

    #[test]
    fn test_config_override_beats_session_promotion() {
        let overrides = parse_config_overrides(
            &[],
            &[],
            &["cargo".to_string()],
        );
        let s = PermissionStore::new(overrides);
        s.promote_to_session("cargo");
        // Config override wins
        assert_eq!(s.check_tier("cargo"), Tier::Restricted);
    }
}
