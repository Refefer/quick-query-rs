//! Filesystem tools for reading, writing, and searching files.

use async_trait::async_trait;
use glob::glob;
use regex::Regex;
use serde::Deserialize;
use std::path::{Path, PathBuf};
use tokio::fs;

use qq_core::{Error, PropertySchema, Tool, ToolDefinition, ToolOutput, ToolParameters};

/// Base path for filesystem operations (security boundary)
#[derive(Clone)]
pub struct FileSystemConfig {
    pub root: PathBuf,
    pub allow_write: bool,
}

impl Default for FileSystemConfig {
    fn default() -> Self {
        Self {
            root: std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
            allow_write: false,
        }
    }
}

impl FileSystemConfig {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self {
            root: root.into(),
            allow_write: false,
        }
    }

    pub fn with_write(mut self, allow: bool) -> Self {
        self.allow_write = allow;
        self
    }

    /// Resolve and validate a path is within the root
    fn resolve_path(&self, path: &str) -> Result<PathBuf, Error> {
        let requested = Path::new(path);
        let resolved = if requested.is_absolute() {
            requested.to_path_buf()
        } else {
            self.root.join(requested)
        };

        // Canonicalize to resolve .. and symlinks
        let canonical = resolved
            .canonicalize()
            .or_else(|_| {
                // If file doesn't exist yet, check parent
                if let Some(parent) = resolved.parent() {
                    parent.canonicalize().map(|p| p.join(resolved.file_name().unwrap_or_default()))
                } else {
                    Err(std::io::Error::new(std::io::ErrorKind::NotFound, "Invalid path"))
                }
            })
            .map_err(|e| Error::tool("filesystem", format!("Invalid path '{}': {}", path, e)))?;

        // Security check: ensure path is within root
        let canonical_root = self.root.canonicalize().unwrap_or_else(|_| self.root.clone());
        if !canonical.starts_with(&canonical_root) {
            return Err(Error::tool(
                "filesystem",
                format!("Path '{}' is outside allowed root", path),
            ));
        }

        Ok(canonical)
    }
}

// =============================================================================
// Read File Tool
// =============================================================================

pub struct ReadFileTool {
    config: FileSystemConfig,
}

impl ReadFileTool {
    pub fn new(config: FileSystemConfig) -> Self {
        Self { config }
    }
}

#[derive(Deserialize)]
struct ReadFileArgs {
    path: String,
    #[serde(default)]
    offset: Option<usize>,
    #[serde(default)]
    limit: Option<usize>,
}

#[async_trait]
impl Tool for ReadFileTool {
    fn name(&self) -> &str {
        "read_file"
    }

    fn description(&self) -> &str {
        "Read the contents of a file. Returns the file content as text."
    }

    fn definition(&self) -> ToolDefinition {
        ToolDefinition::new(self.name(), self.description()).with_parameters(
            ToolParameters::new()
                .add_property("path", PropertySchema::string("Path to the file to read"), true)
                .add_property(
                    "offset",
                    PropertySchema::integer("Line number to start reading from (0-indexed)"),
                    false,
                )
                .add_property(
                    "limit",
                    PropertySchema::integer("Maximum number of lines to read"),
                    false,
                ),
        )
    }

    async fn execute(&self, arguments: serde_json::Value) -> Result<ToolOutput, Error> {
        let args: ReadFileArgs = serde_json::from_value(arguments)
            .map_err(|e| Error::tool("read_file", format!("Invalid arguments: {}", e)))?;

        let path = self.config.resolve_path(&args.path)?;
        let content = fs::read_to_string(&path)
            .await
            .map_err(|e| Error::tool("read_file", format!("Failed to read '{}': {}", args.path, e)))?;

        let result = if args.offset.is_some() || args.limit.is_some() {
            let lines: Vec<&str> = content.lines().collect();
            let offset = args.offset.unwrap_or(0);
            let limit = args.limit.unwrap_or(lines.len());
            lines
                .into_iter()
                .skip(offset)
                .take(limit)
                .collect::<Vec<_>>()
                .join("\n")
        } else {
            content
        };

        Ok(ToolOutput::success(result))
    }
}

// =============================================================================
// List Files Tool
// =============================================================================

pub struct ListFilesTool {
    config: FileSystemConfig,
}

impl ListFilesTool {
    pub fn new(config: FileSystemConfig) -> Self {
        Self { config }
    }
}

#[derive(Deserialize)]
struct ListFilesArgs {
    #[serde(default)]
    path: Option<String>,
    #[serde(default)]
    pattern: Option<String>,
    #[serde(default)]
    recursive: Option<bool>,
}

#[async_trait]
impl Tool for ListFilesTool {
    fn name(&self) -> &str {
        "list_files"
    }

    fn description(&self) -> &str {
        "List files in a directory. Can filter by glob pattern and search recursively."
    }

    fn definition(&self) -> ToolDefinition {
        ToolDefinition::new(self.name(), self.description()).with_parameters(
            ToolParameters::new()
                .add_property(
                    "path",
                    PropertySchema::string("Directory path to list (default: current directory)"),
                    false,
                )
                .add_property(
                    "pattern",
                    PropertySchema::string("Glob pattern to filter files (e.g., '*.rs', '**/*.toml')"),
                    false,
                )
                .add_property(
                    "recursive",
                    PropertySchema::boolean("Whether to search recursively"),
                    false,
                ),
        )
    }

    async fn execute(&self, arguments: serde_json::Value) -> Result<ToolOutput, Error> {
        let args: ListFilesArgs = serde_json::from_value(arguments)
            .map_err(|e| Error::tool("list_files", format!("Invalid arguments: {}", e)))?;

        let base_path = if let Some(p) = &args.path {
            self.config.resolve_path(p)?
        } else {
            self.config.root.clone()
        };

        let pattern = args.pattern.as_deref().unwrap_or("*");
        let recursive = args.recursive.unwrap_or(false);

        let glob_pattern = if recursive && !pattern.starts_with("**/") {
            format!("{}/**/{}", base_path.display(), pattern)
        } else {
            format!("{}/{}", base_path.display(), pattern)
        };

        let mut files = Vec::new();
        for entry in glob(&glob_pattern).map_err(|e| Error::tool("list_files", e.to_string()))? {
            match entry {
                Ok(path) => {
                    if let Ok(rel) = path.strip_prefix(&self.config.root) {
                        files.push(rel.display().to_string());
                    } else {
                        files.push(path.display().to_string());
                    }
                }
                Err(e) => {
                    tracing::warn!("Glob error: {}", e);
                }
            }
        }

        files.sort();
        Ok(ToolOutput::success(files.join("\n")))
    }
}

// =============================================================================
// Search Files Tool
// =============================================================================

pub struct SearchFilesTool {
    config: FileSystemConfig,
}

impl SearchFilesTool {
    pub fn new(config: FileSystemConfig) -> Self {
        Self { config }
    }
}

#[derive(Deserialize)]
struct SearchFilesArgs {
    pattern: String,
    #[serde(default)]
    path: Option<String>,
    #[serde(default)]
    file_pattern: Option<String>,
}

#[async_trait]
impl Tool for SearchFilesTool {
    fn name(&self) -> &str {
        "search_files"
    }

    fn description(&self) -> &str {
        "Search for a regex pattern in files. Returns matching lines with file paths and line numbers."
    }

    fn definition(&self) -> ToolDefinition {
        ToolDefinition::new(self.name(), self.description()).with_parameters(
            ToolParameters::new()
                .add_property(
                    "pattern",
                    PropertySchema::string("Regex pattern to search for"),
                    true,
                )
                .add_property(
                    "path",
                    PropertySchema::string("Directory to search in (default: current directory)"),
                    false,
                )
                .add_property(
                    "file_pattern",
                    PropertySchema::string("Glob pattern for files to search (e.g., '*.rs')"),
                    false,
                ),
        )
    }

    async fn execute(&self, arguments: serde_json::Value) -> Result<ToolOutput, Error> {
        let args: SearchFilesArgs = serde_json::from_value(arguments)
            .map_err(|e| Error::tool("search_files", format!("Invalid arguments: {}", e)))?;

        let regex = Regex::new(&args.pattern)
            .map_err(|e| Error::tool("search_files", format!("Invalid regex: {}", e)))?;

        let base_path = if let Some(p) = &args.path {
            self.config.resolve_path(p)?
        } else {
            self.config.root.clone()
        };

        let file_pattern = args.file_pattern.as_deref().unwrap_or("**/*");
        let glob_pattern = format!("{}/{}", base_path.display(), file_pattern);

        let mut results = Vec::new();
        let mut files_searched = 0;

        for entry in glob(&glob_pattern).map_err(|e| Error::tool("search_files", e.to_string()))? {
            if let Ok(path) = entry {
                if path.is_file() {
                    files_searched += 1;
                    if let Ok(content) = fs::read_to_string(&path).await {
                        for (line_num, line) in content.lines().enumerate() {
                            if regex.is_match(line) {
                                let rel_path = path
                                    .strip_prefix(&self.config.root)
                                    .unwrap_or(&path)
                                    .display();
                                results.push(format!("{}:{}: {}", rel_path, line_num + 1, line.trim()));
                            }
                        }
                    }
                }
            }
        }

        if results.is_empty() {
            Ok(ToolOutput::success(format!(
                "No matches found (searched {} files)",
                files_searched
            )))
        } else {
            Ok(ToolOutput::success(format!(
                "{} matches in {} files:\n{}",
                results.len(),
                files_searched,
                results.join("\n")
            )))
        }
    }
}

// =============================================================================
// Write File Tool
// =============================================================================

pub struct WriteFileTool {
    config: FileSystemConfig,
}

impl WriteFileTool {
    pub fn new(config: FileSystemConfig) -> Self {
        Self { config }
    }
}

#[derive(Deserialize)]
struct WriteFileArgs {
    path: String,
    content: String,
}

#[async_trait]
impl Tool for WriteFileTool {
    fn name(&self) -> &str {
        "write_file"
    }

    fn description(&self) -> &str {
        "Write content to a file. Creates the file if it doesn't exist, overwrites if it does."
    }

    fn definition(&self) -> ToolDefinition {
        ToolDefinition::new(self.name(), self.description()).with_parameters(
            ToolParameters::new()
                .add_property("path", PropertySchema::string("Path to the file to write"), true)
                .add_property("content", PropertySchema::string("Content to write to the file"), true),
        )
    }

    async fn execute(&self, arguments: serde_json::Value) -> Result<ToolOutput, Error> {
        if !self.config.allow_write {
            return Err(Error::tool("write_file", "Write operations are disabled"));
        }

        let args: WriteFileArgs = serde_json::from_value(arguments)
            .map_err(|e| Error::tool("write_file", format!("Invalid arguments: {}", e)))?;

        let path = self.config.resolve_path(&args.path)?;

        // Create parent directories if needed
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .await
                .map_err(|e| Error::tool("write_file", format!("Failed to create directories: {}", e)))?;
        }

        fs::write(&path, &args.content)
            .await
            .map_err(|e| Error::tool("write_file", format!("Failed to write '{}': {}", args.path, e)))?;

        Ok(ToolOutput::success(format!(
            "Successfully wrote {} bytes to {}",
            args.content.len(),
            args.path
        )))
    }
}

// =============================================================================
// Factory functions
// =============================================================================

use std::sync::Arc;

/// Create all filesystem tools with the given configuration (boxed version)
pub fn create_filesystem_tools(config: FileSystemConfig) -> Vec<Box<dyn Tool>> {
    let mut tools: Vec<Box<dyn Tool>> = vec![
        Box::new(ReadFileTool::new(config.clone())),
        Box::new(ListFilesTool::new(config.clone())),
        Box::new(SearchFilesTool::new(config.clone())),
    ];

    if config.allow_write {
        tools.push(Box::new(WriteFileTool::new(config)));
    }

    tools
}

/// Create all filesystem tools with the given configuration (Arc version)
pub fn create_filesystem_tools_arc(config: FileSystemConfig) -> Vec<Arc<dyn Tool>> {
    let mut tools: Vec<Arc<dyn Tool>> = vec![
        Arc::new(ReadFileTool::new(config.clone())),
        Arc::new(ListFilesTool::new(config.clone())),
        Arc::new(SearchFilesTool::new(config.clone())),
    ];

    if config.allow_write {
        tools.push(Arc::new(WriteFileTool::new(config)));
    }

    tools
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[tokio::test]
    async fn test_read_file() {
        let dir = TempDir::new().unwrap();
        let file_path = dir.path().join("test.txt");
        std::fs::write(&file_path, "Hello, world!\nLine 2\nLine 3").unwrap();

        let config = FileSystemConfig::new(dir.path());
        let tool = ReadFileTool::new(config);

        let result = tool
            .execute(serde_json::json!({"path": "test.txt"}))
            .await
            .unwrap();

        assert!(!result.is_error);
        assert!(result.content.contains("Hello, world!"));
    }

    #[tokio::test]
    async fn test_list_files() {
        let dir = TempDir::new().unwrap();
        std::fs::write(dir.path().join("file1.txt"), "").unwrap();
        std::fs::write(dir.path().join("file2.rs"), "").unwrap();

        let config = FileSystemConfig::new(dir.path());
        let tool = ListFilesTool::new(config);

        let result = tool
            .execute(serde_json::json!({"pattern": "*.txt"}))
            .await
            .unwrap();

        assert!(!result.is_error);
        assert!(result.content.contains("file1.txt"));
        assert!(!result.content.contains("file2.rs"));
    }
}
