//! Filesystem tools for reading, writing, and searching files.

use async_trait::async_trait;
use glob::glob;
use ignore::WalkBuilder;
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

    /// Resolve a path for directory operations (may not exist yet)
    fn resolve_path_for_walk(&self, path: Option<&str>) -> Result<PathBuf, Error> {
        match path {
            Some(p) => self.resolve_path(p),
            None => Ok(self.root.clone()),
        }
    }

    /// Validate and normalize a path for creation (doesn't require path to exist)
    fn normalize_path_for_creation(&self, path: &str) -> Result<PathBuf, Error> {
        let requested = Path::new(path);
        let joined = if requested.is_absolute() {
            requested.to_path_buf()
        } else {
            self.root.join(requested)
        };

        // Normalize without requiring existence (handle .. and .)
        let mut normalized = PathBuf::new();
        for component in joined.components() {
            match component {
                std::path::Component::ParentDir => {
                    normalized.pop();
                }
                std::path::Component::CurDir => {}
                c => normalized.push(c),
            }
        }

        // Security check
        let canonical_root = self.root.canonicalize().unwrap_or_else(|_| self.root.clone());
        if !normalized.starts_with(&canonical_root) {
            return Err(Error::tool(
                "filesystem",
                format!("Path '{}' is outside allowed root", path),
            ));
        }

        Ok(normalized)
    }
}

// =============================================================================
// Read File Tool (Enhanced)
// =============================================================================

pub struct ReadFileTool {
    config: FileSystemConfig,
}

impl ReadFileTool {
    pub fn new(config: FileSystemConfig) -> Self {
        Self { config }
    }
}

#[derive(Deserialize, Default)]
struct ReadFileArgs {
    path: String,
    /// DEPRECATED: Use start_line instead. Line number to start reading from (0-indexed)
    #[serde(default)]
    offset: Option<usize>,
    /// Starting line (1-indexed, inclusive)
    #[serde(default)]
    start_line: Option<usize>,
    /// Ending line (1-indexed, inclusive)
    #[serde(default)]
    end_line: Option<usize>,
    /// Maximum number of lines to read from start_line
    #[serde(default)]
    limit: Option<usize>,
    /// Read first N lines (shortcut)
    #[serde(default)]
    head: Option<usize>,
    /// Read last N lines (shortcut)
    #[serde(default)]
    tail: Option<usize>,
    /// Regex to filter lines
    #[serde(default)]
    grep: Option<String>,
    /// Context lines around grep matches
    #[serde(default)]
    context: Option<usize>,
    /// Include line numbers in output (default: true)
    #[serde(default = "default_true")]
    line_numbers: bool,
}

fn default_true() -> bool {
    true
}

#[async_trait]
impl Tool for ReadFileTool {
    fn name(&self) -> &str {
        "read_file"
    }

    fn description(&self) -> &str {
        "Read file contents with line ranges, grep, head/tail"
    }

    fn tool_description(&self) -> &str {
        "Read file contents with optional line ranges, grep filtering, and head/tail.\n\n\
         Usage guidance:\n\
         - The grep param accepts regex — use alternation to filter for multiple patterns at once: \
         grep=\"(TODO|FIXME|HACK)\" instead of calling read_file multiple times.\n\
         - For small files, just read the whole file instead of grepping repeatedly.\n\
         - When you know the target file, use read_file(grep=...) instead of search_files.\n\
         - Never re-read a file you already read in this session."
    }

    fn definition(&self) -> ToolDefinition {
        ToolDefinition::new(self.name(), self.tool_description()).with_parameters(
            ToolParameters::new()
                .add_property("path", PropertySchema::string("Path to the file to read"), true)
                .add_property(
                    "start_line",
                    PropertySchema::integer("Starting line (1-indexed, inclusive)"),
                    false,
                )
                .add_property(
                    "end_line",
                    PropertySchema::integer("Ending line (1-indexed, inclusive)"),
                    false,
                )
                .add_property(
                    "limit",
                    PropertySchema::integer("Maximum number of lines to read from start_line"),
                    false,
                )
                .add_property(
                    "head",
                    PropertySchema::integer("Read first N lines (shortcut, takes precedence)"),
                    false,
                )
                .add_property(
                    "tail",
                    PropertySchema::integer("Read last N lines (shortcut)"),
                    false,
                )
                .add_property(
                    "grep",
                    PropertySchema::string("Regex pattern to filter lines"),
                    false,
                )
                .add_property(
                    "context",
                    PropertySchema::integer("Number of context lines around grep matches"),
                    false,
                )
                .add_property(
                    "line_numbers",
                    PropertySchema::boolean("Include line numbers in output (default: true)"),
                    false,
                )
                .add_property(
                    "offset",
                    PropertySchema::integer("DEPRECATED: Use start_line instead. Line offset (0-indexed)"),
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

        let lines: Vec<&str> = content.lines().collect();
        let total_lines = lines.len();

        // Apply grep filter first if specified
        let (filtered_lines, line_indices): (Vec<&str>, Vec<usize>) = if let Some(grep_pattern) = &args.grep {
            let regex = Regex::new(grep_pattern)
                .map_err(|e| Error::tool("read_file", format!("Invalid grep pattern: {}", e)))?;

            let context = args.context.unwrap_or(0);
            let mut included = vec![false; total_lines];

            // Mark matching lines and their context
            for (i, line) in lines.iter().enumerate() {
                if regex.is_match(line) {
                    let start = i.saturating_sub(context);
                    let end = (i + context + 1).min(total_lines);
                    for item in included.iter_mut().take(end).skip(start) {
                        *item = true;
                    }
                }
            }

            lines
                .iter()
                .enumerate()
                .filter(|(i, _)| included[*i])
                .map(|(i, line)| (*line, i))
                .unzip()
        } else {
            lines.iter().enumerate().map(|(i, line)| (*line, i)).unzip()
        };

        // Determine line range based on parameter precedence: head > tail > start_line/end_line/limit > entire file
        let (start_idx, end_idx) = if let Some(head_n) = args.head {
            (0, head_n.min(filtered_lines.len()))
        } else if let Some(tail_n) = args.tail {
            let len = filtered_lines.len();
            (len.saturating_sub(tail_n), len)
        } else {
            // Handle start_line/end_line/limit with backward compatibility for offset
            let start = if let Some(sl) = args.start_line {
                sl.saturating_sub(1) // Convert 1-indexed to 0-indexed
            } else {
                args.offset.unwrap_or_default()
            };

            let end = if let Some(el) = args.end_line {
                el.min(filtered_lines.len()) // end_line is inclusive, so use directly
            } else if let Some(lim) = args.limit {
                (start + lim).min(filtered_lines.len())
            } else {
                filtered_lines.len()
            };

            (start.min(filtered_lines.len()), end)
        };

        // Build output
        let result_lines: Vec<String> = filtered_lines[start_idx..end_idx]
            .iter()
            .zip(&line_indices[start_idx..end_idx])
            .map(|(line, &orig_idx)| {
                if args.line_numbers {
                    format!("{:>6}│ {}", orig_idx + 1, line)
                } else {
                    line.to_string()
                }
            })
            .collect();

        let output = if result_lines.is_empty() {
            if let Some(ref grep) = args.grep {
                format!("No lines matching '{}' in {}", grep, args.path)
            } else {
                format!("File {} is empty or line range is out of bounds", args.path)
            }
        } else {
            result_lines.join("\n")
        };

        Ok(ToolOutput::success(output))
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
}

#[async_trait]
impl Tool for ListFilesTool {
    fn name(&self) -> &str {
        "list_files"
    }

    fn description(&self) -> &str {
        "List files in a directory (non-recursive)"
    }

    fn tool_description(&self) -> &str {
        "List files in a directory (non-recursive). Supports glob filtering. For recursive search, use find_files."
    }

    fn is_blocking(&self) -> bool {
        true
    }

    fn definition(&self) -> ToolDefinition {
        ToolDefinition::new(self.name(), self.tool_description()).with_parameters(
            ToolParameters::new()
                .add_property(
                    "path",
                    PropertySchema::string("Directory path to list (default: current directory)"),
                    false,
                )
                .add_property(
                    "pattern",
                    PropertySchema::string("Glob pattern to filter files (e.g., '*.rs')"),
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
        let glob_pattern = format!("{}/{}", base_path.display(), pattern);

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
// Find Files Tool (NEW)
// =============================================================================

pub struct FindFilesTool {
    config: FileSystemConfig,
}

impl FindFilesTool {
    pub fn new(config: FileSystemConfig) -> Self {
        Self { config }
    }
}

#[derive(Deserialize, Debug, Clone, Copy, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
enum FileType {
    #[default]
    File,
    Directory,
    Both,
}

#[derive(Deserialize, Default)]
struct FindFilesArgs {
    /// Starting directory
    #[serde(default)]
    path: Option<String>,
    /// Glob pattern (e.g., "**/*.rs")
    #[serde(default)]
    pattern: Option<String>,
    /// Filter by extensions (e.g., ["rs", "toml"])
    #[serde(default)]
    extensions: Option<Vec<String>>,
    /// Directory depth (1 = non-recursive)
    #[serde(default)]
    max_depth: Option<usize>,
    /// "file", "directory", or "both"
    #[serde(default)]
    file_type: FileType,
    /// Honor .gitignore files (default: true)
    #[serde(default = "default_true")]
    respect_gitignore: bool,
    /// Additional ignore patterns
    #[serde(default)]
    ignore_patterns: Option<Vec<String>>,
    /// Max results (default: 500)
    #[serde(default = "default_limit")]
    limit: usize,
    /// Include dotfiles (default: false)
    #[serde(default)]
    include_hidden: bool,
}

fn default_limit() -> usize {
    500
}

#[async_trait]
impl Tool for FindFilesTool {
    fn name(&self) -> &str {
        "find_files"
    }

    fn description(&self) -> &str {
        "Find files recursively with pattern/extension filtering"
    }

    fn tool_description(&self) -> &str {
        "Recursive file discovery with gitignore support. Returns matching paths.\n\n\
         Usage guidance:\n\
         - Use extensions array for multiple types: extensions=[\"rs\",\"toml\"] instead of separate calls.\n\
         - Combine with pattern glob for further filtering.\n\
         - Respects .gitignore by default."
    }

    fn is_blocking(&self) -> bool {
        true
    }

    fn definition(&self) -> ToolDefinition {
        ToolDefinition::new(self.name(), self.tool_description()).with_parameters(
            ToolParameters::new()
                .add_property(
                    "path",
                    PropertySchema::string("Starting directory (default: root)"),
                    false,
                )
                .add_property(
                    "pattern",
                    PropertySchema::string("Glob pattern (e.g., '**/*.rs', '*.toml')"),
                    false,
                )
                .add_property(
                    "extensions",
                    PropertySchema::array(
                        "Filter by extensions (e.g., ['rs', 'toml'])",
                        PropertySchema::string("File extension"),
                    ),
                    false,
                )
                .add_property(
                    "max_depth",
                    PropertySchema::integer("Directory depth limit (1 = non-recursive)"),
                    false,
                )
                .add_property(
                    "file_type",
                    PropertySchema::string("Type filter: 'file', 'directory', or 'both' (default: 'file')"),
                    false,
                )
                .add_property(
                    "respect_gitignore",
                    PropertySchema::boolean("Honor .gitignore files (default: true)"),
                    false,
                )
                .add_property(
                    "ignore_patterns",
                    PropertySchema::array(
                        "Additional ignore patterns (gitignore syntax)",
                        PropertySchema::string("Ignore pattern"),
                    ),
                    false,
                )
                .add_property(
                    "limit",
                    PropertySchema::integer("Maximum results (default: 500)"),
                    false,
                )
                .add_property(
                    "include_hidden",
                    PropertySchema::boolean("Include dotfiles/hidden files (default: false)"),
                    false,
                ),
        )
    }

    async fn execute(&self, arguments: serde_json::Value) -> Result<ToolOutput, Error> {
        let args: FindFilesArgs = serde_json::from_value(arguments)
            .map_err(|e| Error::tool("find_files", format!("Invalid arguments: {}", e)))?;

        let base_path = self.config.resolve_path_for_walk(args.path.as_deref())?;
        let root = self.config.root.clone();
        let limit = args.limit;

        // Build glob pattern matcher if specified
        let glob_pattern = args.pattern.clone();
        let extensions = args.extensions.clone();
        let file_type = args.file_type;
        let ignore_patterns = args.ignore_patterns.clone();
        let max_depth = args.max_depth;
        let respect_gitignore = args.respect_gitignore;
        let include_hidden = args.include_hidden;

        // Run file walking in blocking thread (ignore crate is synchronous)
        let results = qq_core::run_blocking(move || {
            find_files_sync(
                &base_path,
                &root,
                glob_pattern.as_deref(),
                extensions.as_deref(),
                file_type,
                respect_gitignore,
                ignore_patterns.as_deref(),
                max_depth,
                include_hidden,
                limit,
            )
        })
        .await?;

        if results.is_empty() {
            Ok(ToolOutput::success("No files found matching criteria".to_string()))
        } else {
            let truncated = results.len() >= limit;
            let output = if truncated {
                format!(
                    "{}\n\n(Results truncated at {} files)",
                    results.join("\n"),
                    limit
                )
            } else {
                results.join("\n")
            };
            Ok(ToolOutput::success(output))
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn find_files_sync(
    base_path: &Path,
    root: &Path,
    glob_pattern: Option<&str>,
    extensions: Option<&[String]>,
    file_type: FileType,
    respect_gitignore: bool,
    ignore_patterns: Option<&[String]>,
    max_depth: Option<usize>,
    include_hidden: bool,
    limit: usize,
) -> Vec<String> {
    let mut builder = WalkBuilder::new(base_path);

    // Configure walker
    builder
        .git_ignore(respect_gitignore)
        .git_global(respect_gitignore)
        .git_exclude(respect_gitignore)
        .hidden(!include_hidden);

    if let Some(depth) = max_depth {
        builder.max_depth(Some(depth));
    }

    // Add custom ignore patterns
    if let Some(patterns) = ignore_patterns {
        for pattern in patterns {
            // Use overrides for ignore patterns
            let mut overrides = ignore::overrides::OverrideBuilder::new(base_path);
            if overrides.add(&format!("!{}", pattern)).is_ok() {
                if let Ok(built) = overrides.build() {
                    builder.overrides(built);
                }
            }
        }
    }

    // Compile glob pattern if specified
    let glob_matcher = glob_pattern.and_then(|p| glob::Pattern::new(p).ok());

    let mut results = Vec::new();
    for entry in builder.build().flatten() {
        if results.len() >= limit {
            break;
        }

        let path = entry.path();

        // Filter by file type
        let is_dir = path.is_dir();
        let type_matches = match file_type {
            FileType::File => !is_dir,
            FileType::Directory => is_dir,
            FileType::Both => true,
        };
        if !type_matches {
            continue;
        }

        // Filter by extension
        if let Some(exts) = extensions {
            if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
                if !exts.iter().any(|e| e.eq_ignore_ascii_case(ext)) {
                    continue;
                }
            } else if !is_dir {
                continue; // Skip files without extensions when filtering by extension
            }
        }

        // Filter by glob pattern
        if let Some(ref matcher) = glob_matcher {
            let rel_path = path.strip_prefix(base_path).unwrap_or(path);
            if !matcher.matches_path(rel_path) && !matcher.matches_path(path) {
                continue;
            }
        }

        // Get relative path from root
        let display_path = path.strip_prefix(root).unwrap_or(path).display().to_string();
        if !display_path.is_empty() {
            results.push(display_path);
        }
    }

    results.sort();
    results
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
        "Search for regex patterns across files"
    }

    fn tool_description(&self) -> &str {
        "Search for a regex pattern across files. Returns matching lines with paths and line numbers.\n\n\
         Usage guidance:\n\
         - Use alternation to search for multiple terms in one call: pattern=\"(foo|bar|baz)\" \
         instead of separate calls per term.\n\
         - For a single known file, prefer read_file with grep instead.\n\
         - One broad search is better than many narrow ones."
    }

    fn is_blocking(&self) -> bool {
        true
    }

    fn definition(&self) -> ToolDefinition {
        ToolDefinition::new(self.name(), self.tool_description()).with_parameters(
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

        // Collect file paths (fast glob enumeration)
        let file_paths: Vec<PathBuf> = glob(&glob_pattern)
            .map_err(|e| Error::tool("search_files", e.to_string()))?
            .filter_map(|entry| entry.ok())
            .filter(|path| path.is_file())
            .collect();

        // Read file contents asynchronously
        let mut file_contents: Vec<(PathBuf, String)> = Vec::new();
        for path in &file_paths {
            if let Ok(content) = fs::read_to_string(&path).await {
                file_contents.push((path.clone(), content));
            }
        }

        // CPU-intensive regex matching in blocking threadpool
        let root = self.config.root.clone();
        let (results, files_searched) = qq_core::run_blocking(move || {
            search_content_with_regex(&file_contents, &regex, &root)
        })
        .await?;

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

/// CPU-intensive regex matching over file contents (runs in spawn_blocking).
fn search_content_with_regex(
    file_contents: &[(PathBuf, String)],
    regex: &Regex,
    root: &Path,
) -> (Vec<String>, usize) {
    let mut results = Vec::new();
    let files_searched = file_contents.len();

    for (path, content) in file_contents {
        for (line_num, line) in content.lines().enumerate() {
            if regex.is_match(line) {
                let rel_path = path.strip_prefix(root).unwrap_or(path).display();
                results.push(format!("{}:{}: {}", rel_path, line_num + 1, line.trim()));
            }
        }
    }

    (results, files_searched)
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
        "Create a file or overwrite existing content"
    }

    fn tool_description(&self) -> &str {
        "Write full content to a file (creates or overwrites).\n\n\
         Usage guidance:\n\
         - Use ONLY for creating NEW files.\n\
         - For modifying existing files, ALWAYS use edit_file instead (more precise, shows diff).\n\
         - Never overwrite a file you haven't read first."
    }

    fn definition(&self) -> ToolDefinition {
        ToolDefinition::new(self.name(), self.tool_description()).with_parameters(
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
// Edit File Tool (NEW)
// =============================================================================

pub struct EditFileTool {
    config: FileSystemConfig,
}

impl EditFileTool {
    pub fn new(config: FileSystemConfig) -> Self {
        Self { config }
    }
}

#[derive(Deserialize, Debug, Clone)]
#[serde(tag = "type", rename_all = "snake_case")]
enum EditOperation {
    /// Replace text (literal or regex)
    Replace {
        search: String,
        replace: String,
        #[serde(default)]
        regex: bool,
        #[serde(default)]
        all: bool,
        #[serde(default = "default_true")]
        must_match: bool,
    },
    /// Insert at line (1-indexed, 0 or omit to append)
    Insert {
        #[serde(default)]
        line: usize,
        content: String,
    },
    /// Delete lines (1-indexed)
    Delete {
        start_line: usize,
        #[serde(default)]
        end_line: Option<usize>,
    },
    /// Replace line range (1-indexed)
    ReplaceLines {
        start_line: usize,
        end_line: usize,
        content: String,
    },
}

#[derive(Deserialize)]
struct EditFileArgs {
    path: String,
    edits: Vec<EditOperation>,
    #[serde(default)]
    create_if_missing: bool,
    #[serde(default)]
    show_diff: bool,
    #[serde(default)]
    dry_run: bool,
}

#[async_trait]
impl Tool for EditFileTool {
    fn name(&self) -> &str {
        "edit_file"
    }

    fn description(&self) -> &str {
        "Edit files with search/replace, insert, delete operations"
    }

    fn tool_description(&self) -> &str {
        "Precision file editing: search/replace, insert, delete, and replace_lines operations.\n\n\
         Usage guidance:\n\
         - PREFERRED tool for modifying existing files (more precise than write_file, shows diff).\n\
         - The edits array accepts multiple operations in sequence — batch all edits to a file \
         into one call instead of calling edit_file repeatedly for each change.\n\
         - Operation types: replace (literal or regex), insert (at line), delete (line range), \
         replace_lines (replace entire line range).\n\
         - Pattern: search first, read second, modify third."
    }

    fn is_blocking(&self) -> bool {
        true
    }

    fn definition(&self) -> ToolDefinition {
        ToolDefinition::new(self.name(), self.tool_description()).with_parameters(
            ToolParameters::new()
                .add_property("path", PropertySchema::string("Path to the file to edit"), true)
                .add_property(
                    "edits",
                    PropertySchema::array(
                        "List of edit operations to apply in order",
                        PropertySchema::string("Edit operation object (replace, insert, delete, replace_lines)"),
                    ),
                    true,
                )
                .add_property(
                    "create_if_missing",
                    PropertySchema::boolean("Create file if it doesn't exist (default: false)"),
                    false,
                )
                .add_property(
                    "show_diff",
                    PropertySchema::boolean("Return unified diff (default: false)"),
                    false,
                )
                .add_property(
                    "dry_run",
                    PropertySchema::boolean("Validate without applying changes (default: false)"),
                    false,
                ),
        )
    }

    async fn execute(&self, arguments: serde_json::Value) -> Result<ToolOutput, Error> {
        if !self.config.allow_write {
            return Err(Error::tool("edit_file", "Write operations are disabled"));
        }

        let args: EditFileArgs = serde_json::from_value(arguments)
            .map_err(|e| Error::tool("edit_file", format!("Invalid arguments: {}", e)))?;

        if args.edits.is_empty() {
            return Err(Error::tool("edit_file", "No edits specified"));
        }

        // Resolve path
        let path = if args.create_if_missing {
            // Allow non-existent files if create_if_missing is true
            let requested = Path::new(&args.path);
            let resolved = if requested.is_absolute() {
                requested.to_path_buf()
            } else {
                self.config.root.join(requested)
            };

            // Validate parent exists and is within root
            if let Some(parent) = resolved.parent() {
                let canonical_parent = parent.canonicalize().map_err(|e| {
                    Error::tool("edit_file", format!("Parent directory doesn't exist: {}", e))
                })?;
                let canonical_root = self.config.root.canonicalize().unwrap_or_else(|_| self.config.root.clone());
                if !canonical_parent.starts_with(&canonical_root) {
                    return Err(Error::tool(
                        "edit_file",
                        format!("Path '{}' is outside allowed root", args.path),
                    ));
                }
            }
            resolved
        } else {
            self.config.resolve_path(&args.path)?
        };

        // Read existing content or start empty
        let original_content = if path.exists() {
            fs::read_to_string(&path)
                .await
                .map_err(|e| Error::tool("edit_file", format!("Failed to read '{}': {}", args.path, e)))?
        } else if args.create_if_missing {
            String::new()
        } else {
            return Err(Error::tool(
                "edit_file",
                format!("File '{}' does not exist (use create_if_missing: true)", args.path),
            ));
        };

        // Apply edits
        let edits = args.edits.clone();
        let path_display = args.path.clone();
        let original_for_diff = if args.show_diff {
            Some(original_content.clone())
        } else {
            None
        };
        let (new_content, applied_count) =
            qq_core::run_blocking(move || apply_edits(&original_content, &edits, &path_display)).await??;

        // Generate diff if requested
        let diff = if let Some(orig) = original_for_diff {
            Some(generate_diff(&orig, &new_content, &args.path))
        } else {
            None
        };

        // Write if not dry run
        if !args.dry_run {
            // Create parent directories if needed
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent)
                    .await
                    .map_err(|e| Error::tool("edit_file", format!("Failed to create directories: {}", e)))?;
            }

            // Write atomically via temp file
            let temp_path = path.with_extension("tmp");
            fs::write(&temp_path, &new_content)
                .await
                .map_err(|e| Error::tool("edit_file", format!("Failed to write temp file: {}", e)))?;
            fs::rename(&temp_path, &path)
                .await
                .map_err(|e| Error::tool("edit_file", format!("Failed to rename temp file: {}", e)))?;
        }

        let mut result = format!(
            "{} edit(s) applied to {}{}",
            applied_count,
            args.path,
            if args.dry_run { " (dry run)" } else { "" }
        );

        if let Some(diff_str) = diff {
            result.push_str("\n\n");
            result.push_str(&diff_str);
        }

        Ok(ToolOutput::success(result))
    }
}

fn apply_edits(content: &str, edits: &[EditOperation], path: &str) -> Result<(String, usize), Error> {
    let mut lines: Vec<String> = content.lines().map(|s| s.to_string()).collect();

    // Track if content ends with newline
    let ends_with_newline = content.ends_with('\n');

    let mut applied = 0;

    for (edit_idx, edit) in edits.iter().enumerate() {
        match edit {
            EditOperation::Replace {
                search,
                replace,
                regex,
                all,
                must_match,
            } => {
                let full_content = lines.join("\n");
                let (new_content, count) = if *regex {
                    let re = Regex::new(search).map_err(|e| {
                        Error::tool("edit_file", format!("Invalid regex in edit {}: {}", edit_idx + 1, e))
                    })?;
                    if *all {
                        let new = re.replace_all(&full_content, replace.as_str()).to_string();
                        let count = re.find_iter(&full_content).count();
                        (new, count)
                    } else {
                        let count = if re.is_match(&full_content) { 1 } else { 0 };
                        let new = re.replace(&full_content, replace.as_str()).to_string();
                        (new, count)
                    }
                } else if *all {
                    let count = full_content.matches(search).count();
                    (full_content.replace(search, replace), count)
                } else {
                    let count = if full_content.contains(search) { 1 } else { 0 };
                    (full_content.replacen(search, replace, 1), count)
                };

                if count == 0 && *must_match {
                    return Err(Error::tool(
                        "edit_file",
                        format!(
                            "Edit {} failed: search string '{}' not found in {}",
                            edit_idx + 1,
                            search,
                            path
                        ),
                    ));
                }

                lines = new_content.lines().map(|s| s.to_string()).collect();
                if count > 0 {
                    applied += 1;
                }
            }

            EditOperation::Insert { line, content: text } => {
                let insert_lines: Vec<String> = text.lines().map(|s| s.to_string()).collect();
                if *line == 0 || *line > lines.len() {
                    // Append at end
                    lines.extend(insert_lines);
                } else {
                    // Insert before line (1-indexed)
                    let idx = line - 1;
                    for (i, l) in insert_lines.into_iter().enumerate() {
                        lines.insert(idx + i, l);
                    }
                }
                applied += 1;
            }

            EditOperation::Delete { start_line, end_line } => {
                let start_idx = start_line.saturating_sub(1);
                let end_idx = end_line.unwrap_or(*start_line).saturating_sub(1);

                if start_idx >= lines.len() {
                    return Err(Error::tool(
                        "edit_file",
                        format!(
                            "Edit {} failed: start_line {} is beyond file length {}",
                            edit_idx + 1,
                            start_line,
                            lines.len()
                        ),
                    ));
                }

                let actual_end = end_idx.min(lines.len().saturating_sub(1));
                if start_idx <= actual_end {
                    lines.drain(start_idx..=actual_end);
                    applied += 1;
                }
            }

            EditOperation::ReplaceLines {
                start_line,
                end_line,
                content: text,
            } => {
                let start_idx = start_line.saturating_sub(1);
                let end_idx = end_line.saturating_sub(1);

                if start_idx >= lines.len() {
                    return Err(Error::tool(
                        "edit_file",
                        format!(
                            "Edit {} failed: start_line {} is beyond file length {}",
                            edit_idx + 1,
                            start_line,
                            lines.len()
                        ),
                    ));
                }

                let actual_end = end_idx.min(lines.len().saturating_sub(1));
                let replacement: Vec<String> = text.lines().map(|s| s.to_string()).collect();

                // Remove old lines and insert new
                if start_idx <= actual_end {
                    lines.drain(start_idx..=actual_end);
                }
                for (i, l) in replacement.into_iter().enumerate() {
                    lines.insert(start_idx + i, l);
                }
                applied += 1;
            }
        }
    }

    // Reconstruct content preserving trailing newline behavior
    let mut result = lines.join("\n");
    if ends_with_newline
        || (!content.is_empty() && result.is_empty())
        || (!content.is_empty() && !result.is_empty() && content.ends_with('\n'))
    {
        result.push('\n');
    }

    // Handle empty content case
    if content.is_empty() && !result.is_empty() && !result.ends_with('\n') {
        result.push('\n');
    }

    Ok((result, applied))
}

fn generate_diff(old: &str, new: &str, path: &str) -> String {
    let old_lines: Vec<&str> = old.lines().collect();
    let new_lines: Vec<&str> = new.lines().collect();

    let mut diff = format!("--- a/{}\n+++ b/{}\n", path, path);

    // Simple unified diff generation
    let mut old_idx = 0;
    let mut new_idx = 0;

    while old_idx < old_lines.len() || new_idx < new_lines.len() {
        // Find next difference
        let context_start_old = old_idx;
        let context_start_new = new_idx;

        // Skip matching lines
        while old_idx < old_lines.len()
            && new_idx < new_lines.len()
            && old_lines[old_idx] == new_lines[new_idx]
        {
            old_idx += 1;
            new_idx += 1;
        }

        if old_idx >= old_lines.len() && new_idx >= new_lines.len() {
            break;
        }

        // Found a difference, show context
        let context_lines = 3;
        let hunk_start_old = context_start_old.saturating_sub(context_lines);
        let hunk_start_new = context_start_new.saturating_sub(context_lines);

        // Find end of this hunk
        let mut hunk_end_old = old_idx;
        let mut hunk_end_new = new_idx;

        // Skip differing lines
        while (hunk_end_old < old_lines.len() || hunk_end_new < new_lines.len())
            && (hunk_end_old >= old_lines.len()
                || hunk_end_new >= new_lines.len()
                || old_lines.get(hunk_end_old) != new_lines.get(hunk_end_new))
        {
            if hunk_end_old < old_lines.len() {
                hunk_end_old += 1;
            }
            if hunk_end_new < new_lines.len() {
                hunk_end_new += 1;
            }
        }

        let hunk_end_old = (hunk_end_old + context_lines).min(old_lines.len());
        let hunk_end_new = (hunk_end_new + context_lines).min(new_lines.len());

        // Write hunk header
        diff.push_str(&format!(
            "@@ -{},{} +{},{} @@\n",
            hunk_start_old + 1,
            hunk_end_old - hunk_start_old,
            hunk_start_new + 1,
            hunk_end_new - hunk_start_new
        ));

        // Write context and changes
        let mut i = hunk_start_old;
        let mut j = hunk_start_new;

        while i < hunk_end_old || j < hunk_end_new {
            if i < old_lines.len() && j < new_lines.len() && old_lines[i] == new_lines[j] {
                diff.push_str(&format!(" {}\n", old_lines[i]));
                i += 1;
                j += 1;
            } else {
                // Output removed lines
                while i < hunk_end_old
                    && (j >= new_lines.len() || old_lines.get(i) != new_lines.get(j))
                {
                    if i < old_lines.len() {
                        diff.push_str(&format!("-{}\n", old_lines[i]));
                    }
                    i += 1;
                }
                // Output added lines
                while j < hunk_end_new
                    && (i >= old_lines.len() || old_lines.get(i) != new_lines.get(j))
                {
                    if j < new_lines.len() {
                        diff.push_str(&format!("+{}\n", new_lines[j]));
                    }
                    j += 1;
                }
            }
        }

        old_idx = hunk_end_old;
        new_idx = hunk_end_new;
    }

    diff
}

// =============================================================================
// Move File Tool
// =============================================================================

pub struct MoveFileTool {
    config: FileSystemConfig,
}

impl MoveFileTool {
    pub fn new(config: FileSystemConfig) -> Self {
        Self { config }
    }
}

#[derive(Deserialize)]
struct MoveFileArgs {
    source: String,
    destination: String,
}

#[async_trait]
impl Tool for MoveFileTool {
    fn name(&self) -> &str {
        "move_file"
    }

    fn description(&self) -> &str {
        "Move or rename a file or directory"
    }

    fn definition(&self) -> ToolDefinition {
        ToolDefinition::new(self.name(), self.description()).with_parameters(
            ToolParameters::new()
                .add_property(
                    "source",
                    PropertySchema::string("Path to the file or directory to move"),
                    true,
                )
                .add_property(
                    "destination",
                    PropertySchema::string("Target path for the file or directory"),
                    true,
                ),
        )
    }

    async fn execute(&self, arguments: serde_json::Value) -> Result<ToolOutput, Error> {
        if !self.config.allow_write {
            return Err(Error::tool("move_file", "Write operations are disabled"));
        }

        let args: MoveFileArgs = serde_json::from_value(arguments)
            .map_err(|e| Error::tool("move_file", format!("Invalid arguments: {}", e)))?;

        // Resolve source path (must exist)
        let source_path = self.config.resolve_path(&args.source)?;

        // Check source exists
        if !source_path.exists() {
            return Err(Error::tool(
                "move_file",
                format!("Source '{}' does not exist", args.source),
            ));
        }

        // Resolve destination path (may not exist yet)
        let dest_path = self.config.normalize_path_for_creation(&args.destination)?;

        // Check destination doesn't already exist
        if dest_path.exists() {
            return Err(Error::tool(
                "move_file",
                format!("Destination '{}' already exists", args.destination),
            ));
        }

        // Check destination parent exists
        if let Some(parent) = dest_path.parent() {
            if !parent.exists() {
                return Err(Error::tool(
                    "move_file",
                    format!(
                        "Destination parent directory '{}' does not exist",
                        parent.display()
                    ),
                ));
            }
        }

        // Perform the move
        fs::rename(&source_path, &dest_path)
            .await
            .map_err(|e| Error::tool("move_file", format!("Failed to move: {}", e)))?;

        Ok(ToolOutput::success(format!(
            "Successfully moved '{}' to '{}'",
            args.source, args.destination
        )))
    }
}

// =============================================================================
// Copy File Tool
// =============================================================================

pub struct CopyFileTool {
    config: FileSystemConfig,
}

impl CopyFileTool {
    pub fn new(config: FileSystemConfig) -> Self {
        Self { config }
    }
}

#[derive(Deserialize)]
struct CopyFileArgs {
    source: String,
    destination: String,
}

#[async_trait]
impl Tool for CopyFileTool {
    fn name(&self) -> &str {
        "copy_file"
    }

    fn description(&self) -> &str {
        "Copy a file to a new location"
    }

    fn definition(&self) -> ToolDefinition {
        ToolDefinition::new(self.name(), self.description()).with_parameters(
            ToolParameters::new()
                .add_property(
                    "source",
                    PropertySchema::string("Path to the file to copy"),
                    true,
                )
                .add_property(
                    "destination",
                    PropertySchema::string("Target path for the copy"),
                    true,
                ),
        )
    }

    async fn execute(&self, arguments: serde_json::Value) -> Result<ToolOutput, Error> {
        if !self.config.allow_write {
            return Err(Error::tool("copy_file", "Write operations are disabled"));
        }

        let args: CopyFileArgs = serde_json::from_value(arguments)
            .map_err(|e| Error::tool("copy_file", format!("Invalid arguments: {}", e)))?;

        // Resolve source path (must exist and be within root)
        let source_path = self.config.resolve_path(&args.source)?;

        // Verify source is a file (tokio::fs::copy only works on files)
        if source_path.is_dir() {
            return Err(Error::tool(
                "copy_file",
                "Source is a directory, not a file",
            ));
        }

        // Resolve destination path (may not exist yet)
        let dest_path = self.config.normalize_path_for_creation(&args.destination)?;

        // Check destination doesn't already exist
        if dest_path.exists() {
            return Err(Error::tool(
                "copy_file",
                format!("Destination '{}' already exists", args.destination),
            ));
        }

        // Check destination parent exists
        if let Some(parent) = dest_path.parent() {
            if !parent.exists() {
                return Err(Error::tool(
                    "copy_file",
                    format!(
                        "Destination parent directory '{}' does not exist",
                        parent.display()
                    ),
                ));
            }
        }

        // Perform the copy
        let bytes_copied = fs::copy(&source_path, &dest_path)
            .await
            .map_err(|e| Error::tool("copy_file", format!("Failed to copy: {}", e)))?;

        Ok(ToolOutput::success(format!(
            "Successfully copied '{}' to '{}' ({} bytes)",
            args.source, args.destination, bytes_copied
        )))
    }
}

// =============================================================================
// Create Directory Tool
// =============================================================================

pub struct CreateDirectoryTool {
    config: FileSystemConfig,
}

impl CreateDirectoryTool {
    pub fn new(config: FileSystemConfig) -> Self {
        Self { config }
    }
}

#[derive(Deserialize)]
struct CreateDirectoryArgs {
    path: String,
    #[serde(default = "default_true")]
    recursive: bool,
}

#[async_trait]
impl Tool for CreateDirectoryTool {
    fn name(&self) -> &str {
        "create_directory"
    }

    fn description(&self) -> &str {
        "Create a new directory"
    }

    fn definition(&self) -> ToolDefinition {
        ToolDefinition::new(self.name(), self.description()).with_parameters(
            ToolParameters::new()
                .add_property(
                    "path",
                    PropertySchema::string("Path for the new directory"),
                    true,
                )
                .add_property(
                    "recursive",
                    PropertySchema::boolean("Create parent directories if needed (default: true)"),
                    false,
                ),
        )
    }

    async fn execute(&self, arguments: serde_json::Value) -> Result<ToolOutput, Error> {
        if !self.config.allow_write {
            return Err(Error::tool(
                "create_directory",
                "Write operations are disabled",
            ));
        }

        let args: CreateDirectoryArgs = serde_json::from_value(arguments)
            .map_err(|e| Error::tool("create_directory", format!("Invalid arguments: {}", e)))?;

        // Normalize and validate the path
        let dir_path = self.config.normalize_path_for_creation(&args.path)?;

        // Check if path already exists
        if dir_path.exists() {
            if dir_path.is_dir() {
                // Idempotent: directory already exists
                return Ok(ToolOutput::success(format!(
                    "Directory '{}' already exists",
                    args.path
                )));
            } else {
                // Path exists but is a file
                return Err(Error::tool(
                    "create_directory",
                    format!("Path '{}' exists but is not a directory", args.path),
                ));
            }
        }

        // Create the directory
        if args.recursive {
            fs::create_dir_all(&dir_path)
                .await
                .map_err(|e| Error::tool("create_directory", format!("Failed to create directory: {}", e)))?;
        } else {
            fs::create_dir(&dir_path)
                .await
                .map_err(|e| Error::tool("create_directory", format!("Failed to create directory: {}", e)))?;
        }

        Ok(ToolOutput::success(format!(
            "Successfully created directory '{}'",
            args.path
        )))
    }
}

// =============================================================================
// Remove File Tool
// =============================================================================

pub struct RemoveFileTool {
    config: FileSystemConfig,
}

impl RemoveFileTool {
    pub fn new(config: FileSystemConfig) -> Self {
        Self { config }
    }
}

#[derive(Deserialize)]
struct RemoveFileArgs {
    path: String,
}

#[async_trait]
impl Tool for RemoveFileTool {
    fn name(&self) -> &str {
        "rm_file"
    }

    fn description(&self) -> &str {
        "Remove a file"
    }

    fn definition(&self) -> ToolDefinition {
        ToolDefinition::new(self.name(), self.description()).with_parameters(
            ToolParameters::new().add_property(
                "path",
                PropertySchema::string("Path to the file to remove"),
                true,
            ),
        )
    }

    async fn execute(&self, arguments: serde_json::Value) -> Result<ToolOutput, Error> {
        if !self.config.allow_write {
            return Err(Error::tool("rm_file", "Write operations are disabled"));
        }

        let args: RemoveFileArgs = serde_json::from_value(arguments)
            .map_err(|e| Error::tool("rm_file", format!("Invalid arguments: {}", e)))?;

        let path = self.config.resolve_path(&args.path)?;

        if !path.exists() {
            return Err(Error::tool(
                "rm_file",
                format!("Path '{}' does not exist", args.path),
            ));
        }

        if !path.is_file() {
            return Err(Error::tool(
                "rm_file",
                format!(
                    "Path '{}' is not a file (use rm_directory for directories)",
                    args.path
                ),
            ));
        }

        fs::remove_file(&path)
            .await
            .map_err(|e| Error::tool("rm_file", format!("Failed to remove file: {}", e)))?;

        Ok(ToolOutput::success(format!(
            "Successfully removed file '{}'",
            args.path
        )))
    }
}

// =============================================================================
// Remove Directory Tool
// =============================================================================

pub struct RemoveDirectoryTool {
    config: FileSystemConfig,
}

impl RemoveDirectoryTool {
    pub fn new(config: FileSystemConfig) -> Self {
        Self { config }
    }
}

#[derive(Deserialize)]
struct RemoveDirectoryArgs {
    path: String,
    #[serde(default)]
    recursive: bool,
}

#[async_trait]
impl Tool for RemoveDirectoryTool {
    fn name(&self) -> &str {
        "rm_directory"
    }

    fn description(&self) -> &str {
        "Remove a directory"
    }

    fn definition(&self) -> ToolDefinition {
        ToolDefinition::new(self.name(), self.description()).with_parameters(
            ToolParameters::new()
                .add_property(
                    "path",
                    PropertySchema::string("Path to the directory to remove"),
                    true,
                )
                .add_property(
                    "recursive",
                    PropertySchema::boolean(
                        "Remove directory contents recursively (default: false)",
                    ),
                    false,
                ),
        )
    }

    async fn execute(&self, arguments: serde_json::Value) -> Result<ToolOutput, Error> {
        if !self.config.allow_write {
            return Err(Error::tool(
                "rm_directory",
                "Write operations are disabled",
            ));
        }

        let args: RemoveDirectoryArgs = serde_json::from_value(arguments)
            .map_err(|e| Error::tool("rm_directory", format!("Invalid arguments: {}", e)))?;

        let path = self.config.resolve_path(&args.path)?;

        if !path.exists() {
            return Err(Error::tool(
                "rm_directory",
                format!("Path '{}' does not exist", args.path),
            ));
        }

        if !path.is_dir() {
            return Err(Error::tool(
                "rm_directory",
                format!(
                    "Path '{}' is not a directory (use rm_file for files)",
                    args.path
                ),
            ));
        }

        if args.recursive {
            fs::remove_dir_all(&path).await.map_err(|e| {
                Error::tool(
                    "rm_directory",
                    format!("Failed to remove directory: {}", e),
                )
            })?;
        } else {
            fs::remove_dir(&path).await.map_err(|e| {
                Error::tool(
                    "rm_directory",
                    format!("Failed to remove directory: {}", e),
                )
            })?;
        }

        Ok(ToolOutput::success(format!(
            "Successfully removed directory '{}'",
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
        Box::new(FindFilesTool::new(config.clone())),
        Box::new(SearchFilesTool::new(config.clone())),
    ];

    if config.allow_write {
        tools.push(Box::new(WriteFileTool::new(config.clone())));
        tools.push(Box::new(EditFileTool::new(config.clone())));
        tools.push(Box::new(MoveFileTool::new(config.clone())));
        tools.push(Box::new(CreateDirectoryTool::new(config.clone())));
        tools.push(Box::new(RemoveFileTool::new(config.clone())));
        tools.push(Box::new(RemoveDirectoryTool::new(config)));
    }

    tools
}

/// Create all filesystem tools with the given configuration (Arc version)
pub fn create_filesystem_tools_arc(config: FileSystemConfig) -> Vec<Arc<dyn Tool>> {
    let mut tools: Vec<Arc<dyn Tool>> = vec![
        Arc::new(ReadFileTool::new(config.clone())),
        Arc::new(ListFilesTool::new(config.clone())),
        Arc::new(FindFilesTool::new(config.clone())),
        Arc::new(SearchFilesTool::new(config.clone())),
    ];

    if config.allow_write {
        tools.push(Arc::new(WriteFileTool::new(config.clone())));
        tools.push(Arc::new(EditFileTool::new(config.clone())));
        tools.push(Arc::new(MoveFileTool::new(config.clone())));
        tools.push(Arc::new(CopyFileTool::new(config.clone())));
        tools.push(Arc::new(CreateDirectoryTool::new(config.clone())));
        tools.push(Arc::new(RemoveFileTool::new(config.clone())));
        tools.push(Arc::new(RemoveDirectoryTool::new(config)));
    }

    tools
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    // =========================================================================
    // Read File Tests
    // =========================================================================

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
    async fn test_read_file_with_line_numbers() {
        let dir = TempDir::new().unwrap();
        let file_path = dir.path().join("test.txt");
        std::fs::write(&file_path, "Line 1\nLine 2\nLine 3").unwrap();

        let config = FileSystemConfig::new(dir.path());
        let tool = ReadFileTool::new(config);

        let result = tool
            .execute(serde_json::json!({"path": "test.txt", "line_numbers": true}))
            .await
            .unwrap();

        assert!(!result.is_error);
        assert!(result.content.contains("1│"));
        assert!(result.content.contains("2│"));
        assert!(result.content.contains("3│"));
    }

    #[tokio::test]
    async fn test_read_file_head() {
        let dir = TempDir::new().unwrap();
        let file_path = dir.path().join("test.txt");
        std::fs::write(&file_path, "Line 1\nLine 2\nLine 3\nLine 4\nLine 5").unwrap();

        let config = FileSystemConfig::new(dir.path());
        let tool = ReadFileTool::new(config);

        let result = tool
            .execute(serde_json::json!({"path": "test.txt", "head": 2, "line_numbers": false}))
            .await
            .unwrap();

        assert!(!result.is_error);
        assert_eq!(result.content, "Line 1\nLine 2");
    }

    #[tokio::test]
    async fn test_read_file_tail() {
        let dir = TempDir::new().unwrap();
        let file_path = dir.path().join("test.txt");
        std::fs::write(&file_path, "Line 1\nLine 2\nLine 3\nLine 4\nLine 5").unwrap();

        let config = FileSystemConfig::new(dir.path());
        let tool = ReadFileTool::new(config);

        let result = tool
            .execute(serde_json::json!({"path": "test.txt", "tail": 2, "line_numbers": false}))
            .await
            .unwrap();

        assert!(!result.is_error);
        assert_eq!(result.content, "Line 4\nLine 5");
    }

    #[tokio::test]
    async fn test_read_file_line_range() {
        let dir = TempDir::new().unwrap();
        let file_path = dir.path().join("test.txt");
        std::fs::write(&file_path, "Line 1\nLine 2\nLine 3\nLine 4\nLine 5").unwrap();

        let config = FileSystemConfig::new(dir.path());
        let tool = ReadFileTool::new(config);

        let result = tool
            .execute(serde_json::json!({
                "path": "test.txt",
                "start_line": 2,
                "end_line": 4,
                "line_numbers": false
            }))
            .await
            .unwrap();

        assert!(!result.is_error);
        assert_eq!(result.content, "Line 2\nLine 3\nLine 4");
    }

    #[tokio::test]
    async fn test_read_file_grep() {
        let dir = TempDir::new().unwrap();
        let file_path = dir.path().join("test.txt");
        std::fs::write(
            &file_path,
            "apple\nbanana\napricot\ncherry\navocado\nblueberry",
        )
        .unwrap();

        let config = FileSystemConfig::new(dir.path());
        let tool = ReadFileTool::new(config);

        let result = tool
            .execute(serde_json::json!({
                "path": "test.txt",
                "grep": "^a",
                "line_numbers": false
            }))
            .await
            .unwrap();

        assert!(!result.is_error);
        assert_eq!(result.content, "apple\napricot\navocado");
    }

    #[tokio::test]
    async fn test_read_file_grep_with_context() {
        let dir = TempDir::new().unwrap();
        let file_path = dir.path().join("test.txt");
        std::fs::write(&file_path, "1\n2\n3\nMATCH\n5\n6\n7").unwrap();

        let config = FileSystemConfig::new(dir.path());
        let tool = ReadFileTool::new(config);

        let result = tool
            .execute(serde_json::json!({
                "path": "test.txt",
                "grep": "MATCH",
                "context": 1,
                "line_numbers": false
            }))
            .await
            .unwrap();

        assert!(!result.is_error);
        assert_eq!(result.content, "3\nMATCH\n5");
    }

    #[tokio::test]
    async fn test_read_file_backward_compat_offset() {
        let dir = TempDir::new().unwrap();
        let file_path = dir.path().join("test.txt");
        std::fs::write(&file_path, "Line 1\nLine 2\nLine 3\nLine 4\nLine 5").unwrap();

        let config = FileSystemConfig::new(dir.path());
        let tool = ReadFileTool::new(config);

        // Old offset parameter (0-indexed) should still work
        let result = tool
            .execute(serde_json::json!({
                "path": "test.txt",
                "offset": 2,
                "limit": 2,
                "line_numbers": false
            }))
            .await
            .unwrap();

        assert!(!result.is_error);
        assert_eq!(result.content, "Line 3\nLine 4");
    }

    // =========================================================================
    // List Files Tests
    // =========================================================================

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

    // =========================================================================
    // Find Files Tests
    // =========================================================================

    #[tokio::test]
    async fn test_find_files_basic() {
        let dir = TempDir::new().unwrap();
        std::fs::write(dir.path().join("file1.rs"), "").unwrap();
        std::fs::write(dir.path().join("file2.rs"), "").unwrap();
        std::fs::write(dir.path().join("file3.txt"), "").unwrap();

        let config = FileSystemConfig::new(dir.path());
        let tool = FindFilesTool::new(config);

        let result = tool
            .execute(serde_json::json!({"extensions": ["rs"]}))
            .await
            .unwrap();

        assert!(!result.is_error);
        assert!(result.content.contains("file1.rs"));
        assert!(result.content.contains("file2.rs"));
        assert!(!result.content.contains("file3.txt"));
    }

    #[tokio::test]
    async fn test_find_files_recursive() {
        let dir = TempDir::new().unwrap();
        std::fs::create_dir_all(dir.path().join("subdir")).unwrap();
        std::fs::write(dir.path().join("root.rs"), "").unwrap();
        std::fs::write(dir.path().join("subdir/nested.rs"), "").unwrap();

        let config = FileSystemConfig::new(dir.path());
        let tool = FindFilesTool::new(config);

        let result = tool
            .execute(serde_json::json!({"extensions": ["rs"]}))
            .await
            .unwrap();

        assert!(!result.is_error);
        assert!(result.content.contains("root.rs"));
        assert!(result.content.contains("nested.rs"));
    }

    #[tokio::test]
    async fn test_find_files_max_depth() {
        let dir = TempDir::new().unwrap();
        std::fs::create_dir_all(dir.path().join("subdir")).unwrap();
        std::fs::write(dir.path().join("root.rs"), "").unwrap();
        std::fs::write(dir.path().join("subdir/nested.rs"), "").unwrap();

        let config = FileSystemConfig::new(dir.path());
        let tool = FindFilesTool::new(config);

        let result = tool
            .execute(serde_json::json!({"extensions": ["rs"], "max_depth": 1}))
            .await
            .unwrap();

        assert!(!result.is_error);
        assert!(result.content.contains("root.rs"));
        assert!(!result.content.contains("nested.rs"));
    }

    #[tokio::test]
    async fn test_find_files_hidden() {
        let dir = TempDir::new().unwrap();
        std::fs::write(dir.path().join("visible.txt"), "").unwrap();
        std::fs::write(dir.path().join(".hidden.txt"), "").unwrap();

        let config = FileSystemConfig::new(dir.path());
        let tool = FindFilesTool::new(config);

        // Without hidden
        let result = tool
            .execute(serde_json::json!({"include_hidden": false}))
            .await
            .unwrap();
        assert!(result.content.contains("visible.txt"));
        assert!(!result.content.contains(".hidden.txt"));

        // With hidden
        let result = tool
            .execute(serde_json::json!({"include_hidden": true}))
            .await
            .unwrap();
        assert!(result.content.contains("visible.txt"));
        assert!(result.content.contains(".hidden.txt"));
    }

    #[tokio::test]
    async fn test_find_files_limit() {
        let dir = TempDir::new().unwrap();
        for i in 0..10 {
            std::fs::write(dir.path().join(format!("file{}.txt", i)), "").unwrap();
        }

        let config = FileSystemConfig::new(dir.path());
        let tool = FindFilesTool::new(config);

        let result = tool
            .execute(serde_json::json!({"limit": 3}))
            .await
            .unwrap();

        assert!(!result.is_error);
        assert!(result.content.contains("truncated"));
    }

    #[tokio::test]
    async fn test_find_files_directories() {
        let dir = TempDir::new().unwrap();
        std::fs::create_dir_all(dir.path().join("subdir1")).unwrap();
        std::fs::create_dir_all(dir.path().join("subdir2")).unwrap();
        std::fs::write(dir.path().join("file.txt"), "").unwrap();

        let config = FileSystemConfig::new(dir.path());
        let tool = FindFilesTool::new(config);

        let result = tool
            .execute(serde_json::json!({"file_type": "directory"}))
            .await
            .unwrap();

        assert!(!result.is_error);
        assert!(result.content.contains("subdir1"));
        assert!(result.content.contains("subdir2"));
        assert!(!result.content.contains("file.txt"));
    }

    // =========================================================================
    // Write File Tests
    // =========================================================================

    #[tokio::test]
    async fn test_write_file() {
        let dir = TempDir::new().unwrap();

        let config = FileSystemConfig::new(dir.path()).with_write(true);
        let tool = WriteFileTool::new(config);

        let result = tool
            .execute(serde_json::json!({
                "path": "output.txt",
                "content": "Hello from write_file test!"
            }))
            .await
            .unwrap();

        assert!(!result.is_error);
        assert!(result.content.contains("Successfully wrote"));

        let written_content = std::fs::read_to_string(dir.path().join("output.txt")).unwrap();
        assert_eq!(written_content, "Hello from write_file test!");
    }

    #[tokio::test]
    async fn test_write_file_disabled() {
        let dir = TempDir::new().unwrap();

        let config = FileSystemConfig::new(dir.path());
        let tool = WriteFileTool::new(config);

        let result = tool
            .execute(serde_json::json!({
                "path": "output.txt",
                "content": "Should not be written"
            }))
            .await;

        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.to_string().contains("Write operations are disabled"));
    }

    #[tokio::test]
    async fn test_write_file_in_existing_subdir() {
        let dir = TempDir::new().unwrap();
        std::fs::create_dir_all(dir.path().join("subdir")).unwrap();

        let config = FileSystemConfig::new(dir.path()).with_write(true);
        let tool = WriteFileTool::new(config);

        let result = tool
            .execute(serde_json::json!({
                "path": "subdir/file.txt",
                "content": "Subdir content"
            }))
            .await
            .unwrap();

        assert!(!result.is_error);

        let written_content = std::fs::read_to_string(dir.path().join("subdir/file.txt")).unwrap();
        assert_eq!(written_content, "Subdir content");
    }

    #[tokio::test]
    async fn test_write_file_overwrite() {
        let dir = TempDir::new().unwrap();
        let file_path = dir.path().join("existing.txt");
        std::fs::write(&file_path, "Original content").unwrap();

        let config = FileSystemConfig::new(dir.path()).with_write(true);
        let tool = WriteFileTool::new(config);

        let result = tool
            .execute(serde_json::json!({
                "path": "existing.txt",
                "content": "New content"
            }))
            .await
            .unwrap();

        assert!(!result.is_error);

        let written_content = std::fs::read_to_string(&file_path).unwrap();
        assert_eq!(written_content, "New content");
    }

    // =========================================================================
    // Edit File Tests
    // =========================================================================

    #[tokio::test]
    async fn test_edit_file_replace() {
        let dir = TempDir::new().unwrap();
        let file_path = dir.path().join("test.txt");
        std::fs::write(&file_path, "Hello World\nGoodbye World\n").unwrap();

        let config = FileSystemConfig::new(dir.path()).with_write(true);
        let tool = EditFileTool::new(config);

        let result = tool
            .execute(serde_json::json!({
                "path": "test.txt",
                "edits": [
                    {"type": "replace", "search": "World", "replace": "Universe"}
                ]
            }))
            .await
            .unwrap();

        assert!(!result.is_error);

        let content = std::fs::read_to_string(&file_path).unwrap();
        assert_eq!(content, "Hello Universe\nGoodbye World\n");
    }

    #[tokio::test]
    async fn test_edit_file_replace_all() {
        let dir = TempDir::new().unwrap();
        let file_path = dir.path().join("test.txt");
        std::fs::write(&file_path, "Hello World\nGoodbye World\n").unwrap();

        let config = FileSystemConfig::new(dir.path()).with_write(true);
        let tool = EditFileTool::new(config);

        let result = tool
            .execute(serde_json::json!({
                "path": "test.txt",
                "edits": [
                    {"type": "replace", "search": "World", "replace": "Universe", "all": true}
                ]
            }))
            .await
            .unwrap();

        assert!(!result.is_error);

        let content = std::fs::read_to_string(&file_path).unwrap();
        assert_eq!(content, "Hello Universe\nGoodbye Universe\n");
    }

    #[tokio::test]
    async fn test_edit_file_replace_regex() {
        let dir = TempDir::new().unwrap();
        let file_path = dir.path().join("test.txt");
        std::fs::write(&file_path, "foo123bar\nfoo456bar\n").unwrap();

        let config = FileSystemConfig::new(dir.path()).with_write(true);
        let tool = EditFileTool::new(config);

        let result = tool
            .execute(serde_json::json!({
                "path": "test.txt",
                "edits": [
                    {"type": "replace", "search": "\\d+", "replace": "XXX", "regex": true, "all": true}
                ]
            }))
            .await
            .unwrap();

        assert!(!result.is_error);

        let content = std::fs::read_to_string(&file_path).unwrap();
        assert_eq!(content, "fooXXXbar\nfooXXXbar\n");
    }

    #[tokio::test]
    async fn test_edit_file_insert() {
        let dir = TempDir::new().unwrap();
        let file_path = dir.path().join("test.txt");
        std::fs::write(&file_path, "Line 1\nLine 3\n").unwrap();

        let config = FileSystemConfig::new(dir.path()).with_write(true);
        let tool = EditFileTool::new(config);

        let result = tool
            .execute(serde_json::json!({
                "path": "test.txt",
                "edits": [
                    {"type": "insert", "line": 2, "content": "Line 2"}
                ]
            }))
            .await
            .unwrap();

        assert!(!result.is_error);

        let content = std::fs::read_to_string(&file_path).unwrap();
        assert_eq!(content, "Line 1\nLine 2\nLine 3\n");
    }

    #[tokio::test]
    async fn test_edit_file_insert_append() {
        let dir = TempDir::new().unwrap();
        let file_path = dir.path().join("test.txt");
        std::fs::write(&file_path, "Line 1\nLine 2\n").unwrap();

        let config = FileSystemConfig::new(dir.path()).with_write(true);
        let tool = EditFileTool::new(config);

        let result = tool
            .execute(serde_json::json!({
                "path": "test.txt",
                "edits": [
                    {"type": "insert", "line": 0, "content": "Line 3"}
                ]
            }))
            .await
            .unwrap();

        assert!(!result.is_error);

        let content = std::fs::read_to_string(&file_path).unwrap();
        assert_eq!(content, "Line 1\nLine 2\nLine 3\n");
    }

    #[tokio::test]
    async fn test_edit_file_delete() {
        let dir = TempDir::new().unwrap();
        let file_path = dir.path().join("test.txt");
        std::fs::write(&file_path, "Line 1\nLine 2\nLine 3\n").unwrap();

        let config = FileSystemConfig::new(dir.path()).with_write(true);
        let tool = EditFileTool::new(config);

        let result = tool
            .execute(serde_json::json!({
                "path": "test.txt",
                "edits": [
                    {"type": "delete", "start_line": 2}
                ]
            }))
            .await
            .unwrap();

        assert!(!result.is_error);

        let content = std::fs::read_to_string(&file_path).unwrap();
        assert_eq!(content, "Line 1\nLine 3\n");
    }

    #[tokio::test]
    async fn test_edit_file_delete_range() {
        let dir = TempDir::new().unwrap();
        let file_path = dir.path().join("test.txt");
        std::fs::write(&file_path, "Line 1\nLine 2\nLine 3\nLine 4\n").unwrap();

        let config = FileSystemConfig::new(dir.path()).with_write(true);
        let tool = EditFileTool::new(config);

        let result = tool
            .execute(serde_json::json!({
                "path": "test.txt",
                "edits": [
                    {"type": "delete", "start_line": 2, "end_line": 3}
                ]
            }))
            .await
            .unwrap();

        assert!(!result.is_error);

        let content = std::fs::read_to_string(&file_path).unwrap();
        assert_eq!(content, "Line 1\nLine 4\n");
    }

    #[tokio::test]
    async fn test_edit_file_replace_lines() {
        let dir = TempDir::new().unwrap();
        let file_path = dir.path().join("test.txt");
        std::fs::write(&file_path, "Line 1\nOld 2\nOld 3\nLine 4\n").unwrap();

        let config = FileSystemConfig::new(dir.path()).with_write(true);
        let tool = EditFileTool::new(config);

        let result = tool
            .execute(serde_json::json!({
                "path": "test.txt",
                "edits": [
                    {"type": "replace_lines", "start_line": 2, "end_line": 3, "content": "New 2\nNew 3"}
                ]
            }))
            .await
            .unwrap();

        assert!(!result.is_error);

        let content = std::fs::read_to_string(&file_path).unwrap();
        assert_eq!(content, "Line 1\nNew 2\nNew 3\nLine 4\n");
    }

    #[tokio::test]
    async fn test_edit_file_dry_run() {
        let dir = TempDir::new().unwrap();
        let file_path = dir.path().join("test.txt");
        std::fs::write(&file_path, "Original content\n").unwrap();

        let config = FileSystemConfig::new(dir.path()).with_write(true);
        let tool = EditFileTool::new(config);

        let result = tool
            .execute(serde_json::json!({
                "path": "test.txt",
                "edits": [
                    {"type": "replace", "search": "Original", "replace": "Modified"}
                ],
                "dry_run": true
            }))
            .await
            .unwrap();

        assert!(!result.is_error);
        assert!(result.content.contains("dry run"));

        // File should be unchanged
        let content = std::fs::read_to_string(&file_path).unwrap();
        assert_eq!(content, "Original content\n");
    }

    #[tokio::test]
    async fn test_edit_file_show_diff() {
        let dir = TempDir::new().unwrap();
        let file_path = dir.path().join("test.txt");
        std::fs::write(&file_path, "Hello World\n").unwrap();

        let config = FileSystemConfig::new(dir.path()).with_write(true);
        let tool = EditFileTool::new(config);

        let result = tool
            .execute(serde_json::json!({
                "path": "test.txt",
                "edits": [
                    {"type": "replace", "search": "World", "replace": "Universe"}
                ],
                "show_diff": true
            }))
            .await
            .unwrap();

        assert!(!result.is_error);
        assert!(result.content.contains("---"));
        assert!(result.content.contains("+++"));
        assert!(result.content.contains("-Hello World"));
        assert!(result.content.contains("+Hello Universe"));
    }

    #[tokio::test]
    async fn test_edit_file_create_if_missing() {
        let dir = TempDir::new().unwrap();

        let config = FileSystemConfig::new(dir.path()).with_write(true);
        let tool = EditFileTool::new(config);

        let result = tool
            .execute(serde_json::json!({
                "path": "new_file.txt",
                "edits": [
                    {"type": "insert", "line": 0, "content": "New content"}
                ],
                "create_if_missing": true
            }))
            .await
            .unwrap();

        assert!(!result.is_error);

        let content = std::fs::read_to_string(dir.path().join("new_file.txt")).unwrap();
        assert_eq!(content, "New content\n");
    }

    #[tokio::test]
    async fn test_edit_file_missing_without_create() {
        let dir = TempDir::new().unwrap();

        let config = FileSystemConfig::new(dir.path()).with_write(true);
        let tool = EditFileTool::new(config);

        let result = tool
            .execute(serde_json::json!({
                "path": "nonexistent.txt",
                "edits": [
                    {"type": "replace", "search": "foo", "replace": "bar"}
                ]
            }))
            .await;

        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.to_string().contains("does not exist"));
    }

    #[tokio::test]
    async fn test_edit_file_must_match_failure() {
        let dir = TempDir::new().unwrap();
        let file_path = dir.path().join("test.txt");
        std::fs::write(&file_path, "Hello World\n").unwrap();

        let config = FileSystemConfig::new(dir.path()).with_write(true);
        let tool = EditFileTool::new(config);

        let result = tool
            .execute(serde_json::json!({
                "path": "test.txt",
                "edits": [
                    {"type": "replace", "search": "NotFound", "replace": "Something", "must_match": true}
                ]
            }))
            .await;

        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.to_string().contains("not found"));
    }

    #[tokio::test]
    async fn test_edit_file_must_match_false() {
        let dir = TempDir::new().unwrap();
        let file_path = dir.path().join("test.txt");
        std::fs::write(&file_path, "Hello World\n").unwrap();

        let config = FileSystemConfig::new(dir.path()).with_write(true);
        let tool = EditFileTool::new(config);

        let result = tool
            .execute(serde_json::json!({
                "path": "test.txt",
                "edits": [
                    {"type": "replace", "search": "NotFound", "replace": "Something", "must_match": false}
                ]
            }))
            .await
            .unwrap();

        // Should succeed but report 0 edits applied
        assert!(!result.is_error);
        // File unchanged
        let content = std::fs::read_to_string(&file_path).unwrap();
        assert_eq!(content, "Hello World\n");
    }

    #[tokio::test]
    async fn test_edit_file_multiple_edits() {
        let dir = TempDir::new().unwrap();
        let file_path = dir.path().join("test.txt");
        std::fs::write(&file_path, "Line 1\nLine 2\nLine 3\n").unwrap();

        let config = FileSystemConfig::new(dir.path()).with_write(true);
        let tool = EditFileTool::new(config);

        let result = tool
            .execute(serde_json::json!({
                "path": "test.txt",
                "edits": [
                    {"type": "replace", "search": "Line 2", "replace": "Modified 2"},
                    {"type": "insert", "line": 0, "content": "Line 4"}
                ]
            }))
            .await
            .unwrap();

        assert!(!result.is_error);

        let content = std::fs::read_to_string(&file_path).unwrap();
        assert_eq!(content, "Line 1\nModified 2\nLine 3\nLine 4\n");
    }

    #[tokio::test]
    async fn test_edit_file_disabled() {
        let dir = TempDir::new().unwrap();
        let file_path = dir.path().join("test.txt");
        std::fs::write(&file_path, "Hello World\n").unwrap();

        let config = FileSystemConfig::new(dir.path()); // allow_write = false
        let tool = EditFileTool::new(config);

        let result = tool
            .execute(serde_json::json!({
                "path": "test.txt",
                "edits": [
                    {"type": "replace", "search": "Hello", "replace": "Hi"}
                ]
            }))
            .await;

        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.to_string().contains("Write operations are disabled"));
    }

    // =========================================================================
    // Move File Tests
    // =========================================================================

    #[tokio::test]
    async fn test_move_file_basic() {
        let dir = TempDir::new().unwrap();
        let source_path = dir.path().join("source.txt");
        std::fs::write(&source_path, "Hello World").unwrap();

        let config = FileSystemConfig::new(dir.path()).with_write(true);
        let tool = MoveFileTool::new(config);

        let result = tool
            .execute(serde_json::json!({
                "source": "source.txt",
                "destination": "dest.txt"
            }))
            .await
            .unwrap();

        assert!(!result.is_error);
        assert!(result.content.contains("Successfully moved"));

        // Source should no longer exist
        assert!(!source_path.exists());
        // Destination should exist with content
        let dest_content = std::fs::read_to_string(dir.path().join("dest.txt")).unwrap();
        assert_eq!(dest_content, "Hello World");
    }

    #[tokio::test]
    async fn test_move_file_rename() {
        let dir = TempDir::new().unwrap();
        let source_path = dir.path().join("old_name.txt");
        std::fs::write(&source_path, "Content").unwrap();

        let config = FileSystemConfig::new(dir.path()).with_write(true);
        let tool = MoveFileTool::new(config);

        let result = tool
            .execute(serde_json::json!({
                "source": "old_name.txt",
                "destination": "new_name.txt"
            }))
            .await
            .unwrap();

        assert!(!result.is_error);
        assert!(!source_path.exists());
        assert!(dir.path().join("new_name.txt").exists());
    }

    #[tokio::test]
    async fn test_move_directory() {
        let dir = TempDir::new().unwrap();
        std::fs::create_dir_all(dir.path().join("source_dir")).unwrap();
        std::fs::write(dir.path().join("source_dir/file.txt"), "Nested file").unwrap();

        let config = FileSystemConfig::new(dir.path()).with_write(true);
        let tool = MoveFileTool::new(config);

        let result = tool
            .execute(serde_json::json!({
                "source": "source_dir",
                "destination": "dest_dir"
            }))
            .await
            .unwrap();

        assert!(!result.is_error);
        assert!(!dir.path().join("source_dir").exists());
        assert!(dir.path().join("dest_dir").exists());
        assert!(dir.path().join("dest_dir/file.txt").exists());

        let content = std::fs::read_to_string(dir.path().join("dest_dir/file.txt")).unwrap();
        assert_eq!(content, "Nested file");
    }

    #[tokio::test]
    async fn test_move_file_source_not_found() {
        let dir = TempDir::new().unwrap();

        let config = FileSystemConfig::new(dir.path()).with_write(true);
        let tool = MoveFileTool::new(config);

        let result = tool
            .execute(serde_json::json!({
                "source": "nonexistent.txt",
                "destination": "dest.txt"
            }))
            .await;

        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.to_string().contains("does not exist") || err.to_string().contains("Invalid path"));
    }

    #[tokio::test]
    async fn test_move_file_destination_exists() {
        let dir = TempDir::new().unwrap();
        std::fs::write(dir.path().join("source.txt"), "Source").unwrap();
        std::fs::write(dir.path().join("dest.txt"), "Existing").unwrap();

        let config = FileSystemConfig::new(dir.path()).with_write(true);
        let tool = MoveFileTool::new(config);

        let result = tool
            .execute(serde_json::json!({
                "source": "source.txt",
                "destination": "dest.txt"
            }))
            .await;

        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.to_string().contains("already exists"));

        // Both files should remain unchanged
        assert!(dir.path().join("source.txt").exists());
        assert!(dir.path().join("dest.txt").exists());
    }

    #[tokio::test]
    async fn test_move_file_write_disabled() {
        let dir = TempDir::new().unwrap();
        std::fs::write(dir.path().join("source.txt"), "Content").unwrap();

        let config = FileSystemConfig::new(dir.path()); // allow_write = false
        let tool = MoveFileTool::new(config);

        let result = tool
            .execute(serde_json::json!({
                "source": "source.txt",
                "destination": "dest.txt"
            }))
            .await;

        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.to_string().contains("Write operations are disabled"));
    }

    // =========================================================================
    // Create Directory Tests
    // =========================================================================

    #[tokio::test]
    async fn test_create_directory_basic() {
        let dir = TempDir::new().unwrap();

        let config = FileSystemConfig::new(dir.path()).with_write(true);
        let tool = CreateDirectoryTool::new(config);

        let result = tool
            .execute(serde_json::json!({
                "path": "new_dir"
            }))
            .await
            .unwrap();

        assert!(!result.is_error);
        assert!(result.content.contains("Successfully created"));
        assert!(dir.path().join("new_dir").is_dir());
    }

    #[tokio::test]
    async fn test_create_directory_recursive() {
        let dir = TempDir::new().unwrap();

        let config = FileSystemConfig::new(dir.path()).with_write(true);
        let tool = CreateDirectoryTool::new(config);

        let result = tool
            .execute(serde_json::json!({
                "path": "parent/child/grandchild",
                "recursive": true
            }))
            .await
            .unwrap();

        assert!(!result.is_error);
        assert!(dir.path().join("parent/child/grandchild").is_dir());
    }

    #[tokio::test]
    async fn test_create_directory_already_exists() {
        let dir = TempDir::new().unwrap();
        std::fs::create_dir(dir.path().join("existing_dir")).unwrap();

        let config = FileSystemConfig::new(dir.path()).with_write(true);
        let tool = CreateDirectoryTool::new(config);

        let result = tool
            .execute(serde_json::json!({
                "path": "existing_dir"
            }))
            .await
            .unwrap();

        // Should succeed (idempotent)
        assert!(!result.is_error);
        assert!(result.content.contains("already exists"));
    }

    #[tokio::test]
    async fn test_create_directory_non_recursive_missing_parent() {
        let dir = TempDir::new().unwrap();

        let config = FileSystemConfig::new(dir.path()).with_write(true);
        let tool = CreateDirectoryTool::new(config);

        let result = tool
            .execute(serde_json::json!({
                "path": "parent/child",
                "recursive": false
            }))
            .await;

        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_create_directory_path_is_file() {
        let dir = TempDir::new().unwrap();
        std::fs::write(dir.path().join("existing_file"), "content").unwrap();

        let config = FileSystemConfig::new(dir.path()).with_write(true);
        let tool = CreateDirectoryTool::new(config);

        let result = tool
            .execute(serde_json::json!({
                "path": "existing_file"
            }))
            .await;

        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.to_string().contains("not a directory"));
    }

    #[tokio::test]
    async fn test_create_directory_write_disabled() {
        let dir = TempDir::new().unwrap();

        let config = FileSystemConfig::new(dir.path()); // allow_write = false
        let tool = CreateDirectoryTool::new(config);

        let result = tool
            .execute(serde_json::json!({
                "path": "new_dir"
            }))
            .await;

        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.to_string().contains("Write operations are disabled"));
    }

    #[tokio::test]
    async fn test_create_directory_path_traversal() {
        let dir = TempDir::new().unwrap();

        let config = FileSystemConfig::new(dir.path()).with_write(true);
        let tool = CreateDirectoryTool::new(config);

        let result = tool
            .execute(serde_json::json!({
                "path": "../outside_root"
            }))
            .await;

        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.to_string().contains("outside allowed root"));
    }

    // =========================================================================
    // Remove File Tests
    // =========================================================================

    #[tokio::test]
    async fn test_rm_file_basic() {
        let dir = TempDir::new().unwrap();
        let file_path = dir.path().join("to_remove.txt");
        std::fs::write(&file_path, "delete me").unwrap();

        let config = FileSystemConfig::new(dir.path()).with_write(true);
        let tool = RemoveFileTool::new(config);

        let result = tool
            .execute(serde_json::json!({"path": "to_remove.txt"}))
            .await
            .unwrap();

        assert!(!result.is_error);
        assert!(result.content.contains("Successfully removed file"));
        assert!(!file_path.exists());
    }

    #[tokio::test]
    async fn test_rm_file_not_found() {
        let dir = TempDir::new().unwrap();

        let config = FileSystemConfig::new(dir.path()).with_write(true);
        let tool = RemoveFileTool::new(config);

        let result = tool
            .execute(serde_json::json!({"path": "nonexistent.txt"}))
            .await;

        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(
            err.to_string().contains("does not exist")
                || err.to_string().contains("Invalid path")
        );
    }

    #[tokio::test]
    async fn test_rm_file_is_directory() {
        let dir = TempDir::new().unwrap();
        std::fs::create_dir(dir.path().join("a_dir")).unwrap();

        let config = FileSystemConfig::new(dir.path()).with_write(true);
        let tool = RemoveFileTool::new(config);

        let result = tool
            .execute(serde_json::json!({"path": "a_dir"}))
            .await;

        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.to_string().contains("is not a file"));
    }

    #[tokio::test]
    async fn test_rm_file_write_disabled() {
        let dir = TempDir::new().unwrap();
        std::fs::write(dir.path().join("file.txt"), "content").unwrap();

        let config = FileSystemConfig::new(dir.path());
        let tool = RemoveFileTool::new(config);

        let result = tool
            .execute(serde_json::json!({"path": "file.txt"}))
            .await;

        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.to_string().contains("Write operations are disabled"));
    }

    #[tokio::test]
    async fn test_rm_file_path_traversal() {
        let dir = TempDir::new().unwrap();

        let config = FileSystemConfig::new(dir.path()).with_write(true);
        let tool = RemoveFileTool::new(config);

        let result = tool
            .execute(serde_json::json!({"path": "../outside_root"}))
            .await;

        assert!(result.is_err());
    }

    // =========================================================================
    // Remove Directory Tests
    // =========================================================================

    #[tokio::test]
    async fn test_rm_directory_basic() {
        let dir = TempDir::new().unwrap();
        std::fs::create_dir(dir.path().join("empty_dir")).unwrap();

        let config = FileSystemConfig::new(dir.path()).with_write(true);
        let tool = RemoveDirectoryTool::new(config);

        let result = tool
            .execute(serde_json::json!({"path": "empty_dir"}))
            .await
            .unwrap();

        assert!(!result.is_error);
        assert!(result.content.contains("Successfully removed directory"));
        assert!(!dir.path().join("empty_dir").exists());
    }

    #[tokio::test]
    async fn test_rm_directory_recursive() {
        let dir = TempDir::new().unwrap();
        std::fs::create_dir_all(dir.path().join("parent/child")).unwrap();
        std::fs::write(dir.path().join("parent/child/file.txt"), "nested").unwrap();
        std::fs::write(dir.path().join("parent/top.txt"), "top level").unwrap();

        let config = FileSystemConfig::new(dir.path()).with_write(true);
        let tool = RemoveDirectoryTool::new(config);

        let result = tool
            .execute(serde_json::json!({"path": "parent", "recursive": true}))
            .await
            .unwrap();

        assert!(!result.is_error);
        assert!(!dir.path().join("parent").exists());
    }

    #[tokio::test]
    async fn test_rm_directory_non_empty_not_recursive() {
        let dir = TempDir::new().unwrap();
        std::fs::create_dir(dir.path().join("non_empty")).unwrap();
        std::fs::write(dir.path().join("non_empty/file.txt"), "content").unwrap();

        let config = FileSystemConfig::new(dir.path()).with_write(true);
        let tool = RemoveDirectoryTool::new(config);

        let result = tool
            .execute(serde_json::json!({"path": "non_empty"}))
            .await;

        assert!(result.is_err());
        // Directory should still exist
        assert!(dir.path().join("non_empty").exists());
    }

    #[tokio::test]
    async fn test_rm_directory_not_found() {
        let dir = TempDir::new().unwrap();

        let config = FileSystemConfig::new(dir.path()).with_write(true);
        let tool = RemoveDirectoryTool::new(config);

        let result = tool
            .execute(serde_json::json!({"path": "nonexistent_dir"}))
            .await;

        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(
            err.to_string().contains("does not exist")
                || err.to_string().contains("Invalid path")
        );
    }

    #[tokio::test]
    async fn test_rm_directory_is_file() {
        let dir = TempDir::new().unwrap();
        std::fs::write(dir.path().join("a_file.txt"), "content").unwrap();

        let config = FileSystemConfig::new(dir.path()).with_write(true);
        let tool = RemoveDirectoryTool::new(config);

        let result = tool
            .execute(serde_json::json!({"path": "a_file.txt"}))
            .await;

        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.to_string().contains("is not a directory"));
    }

    #[tokio::test]
    async fn test_rm_directory_write_disabled() {
        let dir = TempDir::new().unwrap();
        std::fs::create_dir(dir.path().join("some_dir")).unwrap();

        let config = FileSystemConfig::new(dir.path());
        let tool = RemoveDirectoryTool::new(config);

        let result = tool
            .execute(serde_json::json!({"path": "some_dir"}))
            .await;

        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.to_string().contains("Write operations are disabled"));
    }

    #[tokio::test]
    async fn test_rm_directory_path_traversal() {
        let dir = TempDir::new().unwrap();

        let config = FileSystemConfig::new(dir.path()).with_write(true);
        let tool = RemoveDirectoryTool::new(config);

        let result = tool
            .execute(serde_json::json!({"path": "../outside_root"}))
            .await;

        assert!(result.is_err());
    }
}
