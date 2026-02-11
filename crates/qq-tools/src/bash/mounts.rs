//! Sandbox mount management for bash tool execution.

use async_trait::async_trait;
use serde::Deserialize;
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};
use tempfile::TempDir;

use qq_core::{Error, PropertySchema, Tool, ToolDefinition, ToolOutput, ToolParameters};

use super::permissions::ApprovalChannel;

/// A mount point for the sandbox.
#[derive(Debug, Clone)]
pub struct MountPoint {
    /// Path on the host filesystem.
    pub host_path: PathBuf,
    /// Optional label for display.
    pub label: Option<String>,
}

/// Manages mount points for the sandbox.
pub struct SandboxMounts {
    project_root: PathBuf,
    extra: RwLock<Vec<MountPoint>>,
    tmp_dir: TempDir,
}

impl SandboxMounts {
    pub fn new(project_root: PathBuf) -> Result<Self, std::io::Error> {
        let tmp_dir = TempDir::with_prefix("qq-")?;
        Ok(Self {
            project_root,
            extra: RwLock::new(Vec::new()),
            tmp_dir,
        })
    }

    pub fn project_root(&self) -> &PathBuf {
        &self.project_root
    }

    /// Per-instance scratch directory that persists across bash commands.
    pub fn tmp_dir(&self) -> &Path {
        self.tmp_dir.path()
    }

    pub fn add_mount(&self, mount: MountPoint) {
        if let Ok(mut extra) = self.extra.write() {
            // Don't add duplicates
            if !extra.iter().any(|m| m.host_path == mount.host_path) {
                extra.push(mount);
            }
        }
    }

    pub fn remove_mount(&self, path: &PathBuf) {
        if let Ok(mut extra) = self.extra.write() {
            extra.retain(|m| &m.host_path != path);
        }
    }

    pub fn list_extra(&self) -> Vec<MountPoint> {
        self.extra
            .read()
            .map(|e| e.clone())
            .unwrap_or_default()
    }

    /// Format mounts for display.
    pub fn format_mounts(&self) -> String {
        let mut lines = Vec::new();
        lines.push(format!(
            "  {} (read-write, project root)",
            self.project_root.display()
        ));
        lines.push(format!(
            "  /tmp -> {} (read-write, per-session scratch)",
            self.tmp_dir.path().display()
        ));
        if let Ok(extra) = self.extra.read() {
            for mount in extra.iter() {
                let label = mount
                    .label
                    .as_deref()
                    .map(|l| format!(" ({})", l))
                    .unwrap_or_default();
                lines.push(format!(
                    "  {} (read-only{})",
                    mount.host_path.display(),
                    label,
                ));
            }
        }
        lines.join("\n")
    }
}

/// LLM-callable tool for requesting additional mounts.
pub struct MountExternalTool {
    mounts: Arc<SandboxMounts>,
    approval: ApprovalChannel,
}

#[derive(Deserialize)]
struct MountExternalArgs {
    path: String,
    reason: String,
}

impl MountExternalTool {
    pub fn new(mounts: Arc<SandboxMounts>, approval: ApprovalChannel) -> Self {
        Self { mounts, approval }
    }
}

const MOUNT_TOOL_DESC: &str = "\
Request read-only access to an additional directory outside the project root.

Use this when you need to read files from a path that isn't within the project directory. \
The user will be prompted to approve the mount. If approved, the directory becomes \
accessible (read-only) in subsequent bash commands.

Parameters:
  - path: Absolute path to the directory to mount
  - reason: Brief explanation of why access is needed

The mount persists for the remainder of the session.";

#[async_trait]
impl Tool for MountExternalTool {
    fn name(&self) -> &str {
        "mount_external"
    }

    fn description(&self) -> &str {
        "Request read-only access to a directory outside the project root"
    }

    fn tool_description(&self) -> &str {
        MOUNT_TOOL_DESC
    }

    fn definition(&self) -> ToolDefinition {
        ToolDefinition::new(self.name(), self.tool_description()).with_parameters(
            ToolParameters::new()
                .add_property(
                    "path",
                    PropertySchema::string("Absolute path to the directory to mount"),
                    true,
                )
                .add_property(
                    "reason",
                    PropertySchema::string("Brief explanation of why this access is needed"),
                    true,
                ),
        )
    }

    async fn execute(&self, arguments: serde_json::Value) -> Result<ToolOutput, Error> {
        let args: MountExternalArgs = serde_json::from_value(arguments)
            .map_err(|e| Error::tool("mount_external", format!("Invalid arguments: {}", e)))?;

        let path = PathBuf::from(&args.path);

        // Validate the path
        if !path.is_absolute() {
            return Ok(ToolOutput::error(
                "Path must be absolute (e.g., /data/datasets)",
            ));
        }

        if !path.exists() {
            return Ok(ToolOutput::error(format!(
                "Path does not exist: {}",
                path.display()
            )));
        }

        if !path.is_dir() {
            return Ok(ToolOutput::error(format!(
                "Path is not a directory: {}",
                path.display()
            )));
        }

        // Canonicalize to resolve symlinks
        let canonical = path.canonicalize().map_err(|e| {
            Error::tool(
                "mount_external",
                format!("Failed to resolve path: {}", e),
            )
        })?;

        // Check if already mounted
        let already_mounted = self.mounts.list_extra().iter().any(|m| m.host_path == canonical)
            || self.mounts.project_root() == &canonical;
        if already_mounted {
            return Ok(ToolOutput::success(format!(
                "Directory already accessible: {}",
                canonical.display()
            )));
        }

        // Request approval from user
        let approval_msg = format!("mount_external: {}", canonical.display());
        match self
            .approval
            .request_approval(
                format!("Mount {} (reason: {})", canonical.display(), args.reason),
                vec![approval_msg],
            )
            .await
        {
            Ok(super::permissions::ApprovalResponse::Allow)
            | Ok(super::permissions::ApprovalResponse::AllowForSession) => {
                self.mounts.add_mount(MountPoint {
                    host_path: canonical.clone(),
                    label: Some(args.reason),
                });
                Ok(ToolOutput::success(format!(
                    "Mount approved. {} is now accessible (read-only) in bash commands.",
                    canonical.display()
                )))
            }
            Ok(super::permissions::ApprovalResponse::Deny) => {
                Ok(ToolOutput::error("Mount request denied by user."))
            }
            Err(e) => Ok(ToolOutput::error(format!("Mount approval failed: {}", e))),
        }
    }
}
