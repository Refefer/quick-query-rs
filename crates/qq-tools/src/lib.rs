//! qq-tools: Built-in tools for quick-query
//!
//! This crate provides the default tools available to LLM agents:
//! - Filesystem: read, write, list, and search files
//! - Web: fetch and parse webpages
//! - Memory: persistent key-value storage
//! - Process data: chunk and summarize large content

pub mod filesystem;
pub mod memory;
pub mod process_data;
pub mod web;

pub use filesystem::{create_filesystem_tools, create_filesystem_tools_arc, FileSystemConfig};
pub use memory::{create_memory_tools, create_memory_tools_arc, MemoryStore};
pub use process_data::{create_process_data_tool, create_process_data_tool_arc, ProcessLargeDataTool};
pub use web::{create_web_tools, create_web_tools_arc, create_web_tools_with_search, WebSearchConfig};

use qq_core::{Tool, ToolRegistry};
use std::path::PathBuf;
use std::sync::Arc;

/// Configuration for the default tool set
#[derive(Clone)]
pub struct ToolsConfig {
    /// Root directory for filesystem operations
    pub root: PathBuf,
    /// Whether to allow write operations
    pub allow_write: bool,
    /// Path to memory database (None for in-memory)
    pub memory_db: Option<PathBuf>,
    /// Enable web tools
    pub enable_web: bool,
    /// Web search configuration (Perplexica)
    pub web_search: Option<WebSearchConfig>,
}

impl Default for ToolsConfig {
    fn default() -> Self {
        Self {
            root: std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
            allow_write: false,
            memory_db: None,
            enable_web: true,
            web_search: None,
        }
    }
}

impl ToolsConfig {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_root(mut self, root: impl Into<PathBuf>) -> Self {
        self.root = root.into();
        self
    }

    pub fn with_write(mut self, allow: bool) -> Self {
        self.allow_write = allow;
        self
    }

    pub fn with_memory_db(mut self, path: impl Into<PathBuf>) -> Self {
        self.memory_db = Some(path.into());
        self
    }

    pub fn with_web(mut self, enable: bool) -> Self {
        self.enable_web = enable;
        self
    }

    pub fn with_web_search(mut self, config: WebSearchConfig) -> Self {
        self.web_search = Some(config);
        self
    }
}

/// Create a registry with all default tools
pub fn create_default_registry(config: ToolsConfig) -> Result<ToolRegistry, qq_core::Error> {
    let mut registry = ToolRegistry::new();

    // Filesystem tools
    let fs_config = FileSystemConfig::new(&config.root).with_write(config.allow_write);
    for tool in create_filesystem_tools_arc(fs_config) {
        registry.register(tool);
    }

    // Memory tools
    let store = if let Some(db_path) = &config.memory_db {
        Arc::new(MemoryStore::new(db_path)?)
    } else {
        Arc::new(MemoryStore::in_memory()?)
    };
    for tool in create_memory_tools_arc(store) {
        registry.register(tool);
    }

    // Web tools
    if config.enable_web {
        for tool in create_web_tools_with_search(config.web_search.clone()) {
            registry.register(tool);
        }
    }

    Ok(registry)
}

/// Create individual tools for custom registries (boxed version for backward compatibility)
pub fn create_all_tools(config: ToolsConfig) -> Result<Vec<Box<dyn Tool>>, qq_core::Error> {
    let mut tools = Vec::new();

    // Filesystem tools
    let fs_config = FileSystemConfig::new(&config.root).with_write(config.allow_write);
    tools.extend(create_filesystem_tools(fs_config));

    // Memory tools
    let store = if let Some(db_path) = &config.memory_db {
        Arc::new(MemoryStore::new(db_path)?)
    } else {
        Arc::new(MemoryStore::in_memory()?)
    };
    tools.extend(create_memory_tools(store));

    // Web tools (boxed version doesn't support web_search for simplicity)
    if config.enable_web {
        tools.extend(create_web_tools());
    }

    Ok(tools)
}

/// Create individual tools for custom registries (Arc version)
pub fn create_all_tools_arc(config: ToolsConfig) -> Result<Vec<Arc<dyn Tool>>, qq_core::Error> {
    let mut tools = Vec::new();

    // Filesystem tools
    let fs_config = FileSystemConfig::new(&config.root).with_write(config.allow_write);
    tools.extend(create_filesystem_tools_arc(fs_config));

    // Memory tools
    let store = if let Some(db_path) = &config.memory_db {
        Arc::new(MemoryStore::new(db_path)?)
    } else {
        Arc::new(MemoryStore::in_memory()?)
    };
    tools.extend(create_memory_tools_arc(store));

    // Web tools
    if config.enable_web {
        tools.extend(create_web_tools_with_search(config.web_search.clone()));
    }

    Ok(tools)
}
