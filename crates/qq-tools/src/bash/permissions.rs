//! Three-tier permission model for bash command execution.
//!
//! Commands are classified into three tiers:
//! - **Session**: Pre-approved read-only commands that run immediately
//! - **PerCall**: Write operations requiring user approval each time
//! - **Restricted**: Always blocked, cannot be approved

use std::collections::{HashMap, HashSet};
use std::sync::RwLock;

pub use crate::approval::{create_approval_channel, ApprovalChannel, ApprovalRequest, ApprovalResponse};

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
    // Cargo read-only operations
    "cargo-build", "cargo-test", "cargo-check", "cargo-clippy", "cargo-bench",
    "cargo-doc", "cargo-tree", "cargo-metadata",
    // npm read-only operations
    "npm-test", "npm-ls", "npm-list", "npm-outdated", "npm-view", "npm-audit",
    // yarn read-only operations
    "yarn-list", "yarn-outdated", "yarn-info", "yarn-audit",
    // pnpm read-only operations
    "pnpm-list", "pnpm-outdated", "pnpm-audit",
    // pip/pip3 read-only operations
    "pip-list", "pip-freeze", "pip-show", "pip-check",
    "pip3-list", "pip3-freeze", "pip3-show", "pip3-check",
    // poetry read-only operations
    "poetry-show", "poetry-check", "poetry-version",
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
    // Cargo write/mutate operations
    "cargo-fmt", "cargo-fix", "cargo-add", "cargo-remove", "cargo-install",
    "cargo-uninstall", "cargo-publish", "cargo-init", "cargo-new", "cargo-run",
    "cargo-clean", "cargo-update",
    // npm write/mutate operations
    "npm-install", "npm-uninstall", "npm-update", "npm-publish", "npm-init",
    "npm-link", "npm-run", "npm-start", "npm-build", "npm-exec",
    // yarn write/mutate operations
    "yarn-add", "yarn-remove", "yarn-install", "yarn-upgrade", "yarn-publish",
    "yarn-run", "yarn-start", "yarn-build", "yarn-exec",
    // pnpm write/mutate operations
    "pnpm-add", "pnpm-remove", "pnpm-install", "pnpm-update", "pnpm-publish",
    "pnpm-run", "pnpm-start", "pnpm-build", "pnpm-exec",
    // pip/pip3 write/mutate operations
    "pip-install", "pip-uninstall",
    "pip3-install", "pip3-uninstall",
    // poetry write/mutate operations
    "poetry-add", "poetry-remove", "poetry-install", "poetry-update",
    "poetry-build", "poetry-publish", "poetry-run", "poetry-init", "poetry-new",
    // npx (subcommands are arbitrary executables)
    "npx",
    // Build tools (no subcommand extraction)
    "make", "cmake", "ninja", "meson",
    // Interpreters
    "python", "python3", "node", "ruby", "perl",
    // File modification
    "mv", "cp", "rm", "mkdir", "rmdir", "touch", "chmod", "ln",
    // Text modification (in-place)
    "sed", "awk", "patch",
    // Shells (sub-shells)
    "sh", "bash", "zsh",
    // Generic fallbacks (unrecognized subcommands default here)
    "git", "cargo", "npm", "yarn", "pnpm", "pip", "pip3", "poetry",
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
        assert_eq!(s.check_tier("cargo-run"), Tier::PerCall);
        assert_eq!(s.check_tier("rm"), Tier::PerCall);
        assert_eq!(s.check_tier("git-commit"), Tier::PerCall);
        assert_eq!(s.check_tier("python"), Tier::PerCall);
        assert_eq!(s.check_tier("npm-install"), Tier::PerCall);
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

    #[test]
    fn test_cargo_subcommand_tiers() {
        let s = store();
        // Session-tier (read-only build/test)
        assert_eq!(s.check_tier("cargo-build"), Tier::Session);
        assert_eq!(s.check_tier("cargo-test"), Tier::Session);
        assert_eq!(s.check_tier("cargo-check"), Tier::Session);
        assert_eq!(s.check_tier("cargo-clippy"), Tier::Session);
        assert_eq!(s.check_tier("cargo-bench"), Tier::Session);
        assert_eq!(s.check_tier("cargo-doc"), Tier::Session);
        assert_eq!(s.check_tier("cargo-tree"), Tier::Session);
        assert_eq!(s.check_tier("cargo-metadata"), Tier::Session);
        // Per-call (mutating)
        assert_eq!(s.check_tier("cargo-run"), Tier::PerCall);
        assert_eq!(s.check_tier("cargo-install"), Tier::PerCall);
        assert_eq!(s.check_tier("cargo-publish"), Tier::PerCall);
        assert_eq!(s.check_tier("cargo-fmt"), Tier::PerCall);
        assert_eq!(s.check_tier("cargo-clean"), Tier::PerCall);
        // Fallback for unrecognized subcommands
        assert_eq!(s.check_tier("cargo"), Tier::PerCall);
    }

    #[test]
    fn test_npm_subcommand_tiers() {
        let s = store();
        assert_eq!(s.check_tier("npm-test"), Tier::Session);
        assert_eq!(s.check_tier("npm-ls"), Tier::Session);
        assert_eq!(s.check_tier("npm-audit"), Tier::Session);
        assert_eq!(s.check_tier("npm-install"), Tier::PerCall);
        assert_eq!(s.check_tier("npm-publish"), Tier::PerCall);
        assert_eq!(s.check_tier("npm-run"), Tier::PerCall);
        assert_eq!(s.check_tier("npm"), Tier::PerCall);
    }

    #[test]
    fn test_pip_subcommand_tiers() {
        let s = store();
        assert_eq!(s.check_tier("pip-list"), Tier::Session);
        assert_eq!(s.check_tier("pip-freeze"), Tier::Session);
        assert_eq!(s.check_tier("pip-show"), Tier::Session);
        assert_eq!(s.check_tier("pip-install"), Tier::PerCall);
        assert_eq!(s.check_tier("pip3-list"), Tier::Session);
        assert_eq!(s.check_tier("pip3-install"), Tier::PerCall);
        assert_eq!(s.check_tier("pip"), Tier::PerCall);
        assert_eq!(s.check_tier("pip3"), Tier::PerCall);
    }

    #[test]
    fn test_yarn_pnpm_poetry_subcommand_tiers() {
        let s = store();
        // yarn
        assert_eq!(s.check_tier("yarn-list"), Tier::Session);
        assert_eq!(s.check_tier("yarn-audit"), Tier::Session);
        assert_eq!(s.check_tier("yarn-add"), Tier::PerCall);
        assert_eq!(s.check_tier("yarn"), Tier::PerCall);
        // pnpm
        assert_eq!(s.check_tier("pnpm-list"), Tier::Session);
        assert_eq!(s.check_tier("pnpm-install"), Tier::PerCall);
        assert_eq!(s.check_tier("pnpm"), Tier::PerCall);
        // poetry
        assert_eq!(s.check_tier("poetry-show"), Tier::Session);
        assert_eq!(s.check_tier("poetry-check"), Tier::Session);
        assert_eq!(s.check_tier("poetry-install"), Tier::PerCall);
        assert_eq!(s.check_tier("poetry"), Tier::PerCall);
    }

    #[test]
    fn test_subcommand_session_promotion() {
        let s = store();
        // cargo-run starts as per-call
        assert_eq!(s.check_tier("cargo-run"), Tier::PerCall);
        // Promote it
        s.promote_to_session("cargo-run");
        assert_eq!(s.check_tier("cargo-run"), Tier::Session);
        // Other cargo subcommands are unaffected
        assert_eq!(s.check_tier("cargo-publish"), Tier::PerCall);
    }

    #[test]
    fn test_config_override_at_subcommand_level() {
        let overrides = parse_config_overrides(
            &["cargo-run".to_string()],
            &[],
            &["npm-install".to_string()],
        );
        let s = PermissionStore::new(overrides);
        assert_eq!(s.check_tier("cargo-run"), Tier::Session);
        assert_eq!(s.check_tier("npm-install"), Tier::Restricted);
        // Other subcommands use defaults
        assert_eq!(s.check_tier("cargo-build"), Tier::Session);
        assert_eq!(s.check_tier("npm-test"), Tier::Session);
    }

    #[test]
    fn test_mixed_pipeline_with_subcommands() {
        let s = store();
        // All session: cargo-build + grep
        let cmds = vec!["cargo-build".to_string(), "grep".to_string()];
        assert!(matches!(s.check_pipeline(&cmds), PipelinePermission::Allowed));
        // Mixed: cargo-build (session) + cargo-run (per-call)
        let cmds = vec!["cargo-build".to_string(), "cargo-run".to_string()];
        match s.check_pipeline(&cmds) {
            PipelinePermission::NeedsApproval(trigger) => {
                assert_eq!(trigger, vec!["cargo-run"]);
            }
            other => panic!("Expected NeedsApproval, got {:?}", std::mem::discriminant(&other)),
        }
    }
}
