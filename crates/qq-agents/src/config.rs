//! Configuration types for agents.

use std::collections::HashMap;
use std::path::PathBuf;

use anyhow::Result;
use serde::{Deserialize, Serialize};

fn default_max_turns() -> usize {
    20
}

fn default_compaction_strategy() -> AgentMemoryStrategy {
    AgentMemoryStrategy::Compaction
}

/// Strategy for managing agent memory across execution.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum AgentMemoryStrategy {
    /// Post-execution LLM summarization with continuation support.
    Compaction,
    /// In-loop observational memory (messages -> observations -> reflections).
    ObsMemory,
}

impl Default for AgentMemoryStrategy {
    fn default() -> Self {
        Self::ObsMemory
    }
}

/// Configuration overrides for built-in agents.
///
/// Allows customizing tool limits and other settings for internal agents
/// without modifying the source code.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct BuiltinAgentOverride {
    /// Maximum agentic loop iterations (number of turns)
    #[serde(default)]
    pub max_turns: Option<usize>,

    /// Per-tool call limits (tool_name -> max_calls)
    #[serde(default)]
    pub tool_limits: HashMap<String, usize>,

    /// Optional compaction prompt override for agent memory summarization.
    /// When set, overrides the agent's built-in compact_prompt.
    /// Only used when memory_strategy is "compaction".
    #[serde(default)]
    pub compact_prompt: Option<String>,

    /// Memory strategy override: "compaction" or "obs-memory".
    #[serde(default)]
    pub memory_strategy: Option<AgentMemoryStrategy>,

    /// Maximum observations before requesting wrap-up (obs-memory only).
    #[serde(default)]
    pub max_observations: Option<u32>,

    /// Disable bash tool for this agent (default: false = bash enabled).
    #[serde(default)]
    pub no_bash: Option<bool>,
}

/// External agent definition from agents.toml.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentDefinition {
    /// Short description for display (/agents, /tools)
    pub description: String,

    /// Rich description sent to LLMs when this agent is exposed as a tool.
    /// Falls back to `description` if not set.
    #[serde(default)]
    pub tool_description: Option<String>,

    /// System prompt for the agent
    pub system_prompt: String,

    /// Optional provider override (uses profile default if not set)
    #[serde(default)]
    pub provider: Option<String>,

    /// Optional model override (uses profile default if not set)
    #[serde(default)]
    pub model: Option<String>,

    /// Tool names this agent can use (from the tool registry)
    #[serde(default)]
    pub tools: Vec<String>,

    /// Maximum agentic loop iterations
    #[serde(default = "default_max_turns")]
    pub max_turns: usize,

    /// Per-tool call limits (tool_name -> max_calls)
    /// When a tool reaches its limit, the agent receives an error message instead
    #[serde(default)]
    pub tool_limits: HashMap<String, usize>,

    /// Optional compaction prompt for agent memory summarization.
    /// Falls back to DEFAULT_COMPACT_PROMPT if omitted.
    /// Only used when memory_strategy is "compaction".
    #[serde(default)]
    pub compact_prompt: Option<String>,

    /// Memory strategy: "compaction" or "obs-memory".
    /// Defaults to "compaction" for external agents (backward compat).
    #[serde(default = "default_compaction_strategy")]
    pub memory_strategy: AgentMemoryStrategy,

    /// Maximum observations before requesting wrap-up (obs-memory only).
    #[serde(default)]
    pub max_observations: Option<u32>,

    /// Disable bash tool for this agent (default: false = bash is auto-injected).
    #[serde(default)]
    pub no_bash: bool,

    /// Whether this agent is read-only (default: false).
    #[serde(default)]
    pub read_only: bool,
}

/// Agents configuration file (agents.toml).
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct AgentsConfig {
    /// External agent definitions
    #[serde(default)]
    pub agents: HashMap<String, AgentDefinition>,

    /// Overrides for built-in agents (e.g., tool_limits)
    #[serde(default)]
    pub builtin: HashMap<String, BuiltinAgentOverride>,
}

impl AgentsConfig {
    /// Load agents configuration from ~/.config/qq/agents.toml.
    pub fn load() -> Result<Self> {
        let config_path = Self::config_path()?;

        if config_path.exists() {
            let content = std::fs::read_to_string(&config_path)?;
            let config: AgentsConfig = toml::from_str(&content)?;
            Ok(config)
        } else {
            // Return empty config if file doesn't exist
            Ok(Self::default())
        }
    }

    /// Get the path to the agents config file.
    pub fn config_path() -> Result<PathBuf> {
        let config_dir = dirs::config_dir()
            .ok_or_else(|| anyhow::anyhow!("Could not determine config directory"))?;
        Ok(config_dir.join("qq").join("agents.toml"))
    }

    /// Get an agent definition by name.
    pub fn get(&self, name: &str) -> Option<&AgentDefinition> {
        self.agents.get(name)
    }

    /// Check if an agent is defined.
    pub fn contains(&self, name: &str) -> bool {
        self.agents.contains_key(name)
    }

    /// Get all agent names.
    pub fn names(&self) -> Vec<&str> {
        self.agents.keys().map(|s| s.as_str()).collect()
    }

    /// Get override configuration for a built-in agent.
    pub fn get_builtin_override(&self, name: &str) -> Option<&BuiltinAgentOverride> {
        self.builtin.get(name)
    }

    /// Get tool limits for a built-in agent from config overrides.
    ///
    /// Returns None if no override is configured.
    pub fn get_builtin_tool_limits(&self, name: &str) -> Option<&HashMap<String, usize>> {
        self.builtin
            .get(name)
            .map(|o| &o.tool_limits)
            .filter(|limits| !limits.is_empty())
    }

    /// Get max_turns for a built-in agent from config overrides.
    ///
    /// Returns None if no override is configured.
    pub fn get_builtin_max_turns(&self, name: &str) -> Option<usize> {
        self.builtin.get(name).and_then(|o| o.max_turns)
    }

    /// Get compact_prompt for a built-in agent from config overrides.
    ///
    /// Returns None if no override is configured.
    pub fn get_builtin_compact_prompt(&self, name: &str) -> Option<&str> {
        self.builtin
            .get(name)
            .and_then(|o| o.compact_prompt.as_deref())
    }

    /// Check if bash is disabled for a built-in agent via config overrides.
    pub fn get_builtin_no_bash(&self, name: &str) -> bool {
        self.builtin
            .get(name)
            .and_then(|o| o.no_bash)
            .unwrap_or(false)
    }

    /// Get memory strategy override for a built-in agent.
    pub fn get_builtin_memory_strategy(&self, name: &str) -> Option<&AgentMemoryStrategy> {
        self.builtin
            .get(name)
            .and_then(|o| o.memory_strategy.as_ref())
    }

    /// Get max_observations override for a built-in agent.
    pub fn get_builtin_max_observations(&self, name: &str) -> Option<u32> {
        self.builtin
            .get(name)
            .and_then(|o| o.max_observations)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_builtin_overrides() {
        let toml_content = r#"
[builtin.researcher]
max_turns = 15
tool_limits = { web_search = 2, fetch_webpage = 5 }

[builtin.coder]
max_turns = 50
tool_limits = { write_file = 10 }
"#;
        let config: AgentsConfig = toml::from_str(toml_content).unwrap();

        // Check researcher
        assert_eq!(config.get_builtin_max_turns("researcher"), Some(15));
        let researcher_limits = config.get_builtin_tool_limits("researcher").unwrap();
        assert_eq!(researcher_limits.get("web_search"), Some(&2));
        assert_eq!(researcher_limits.get("fetch_webpage"), Some(&5));

        // Check coder
        assert_eq!(config.get_builtin_max_turns("coder"), Some(50));
        let coder_limits = config.get_builtin_tool_limits("coder").unwrap();
        assert_eq!(coder_limits.get("write_file"), Some(&10));

        // Check non-existent agent returns None
        assert!(config.get_builtin_tool_limits("nonexistent").is_none());
        assert!(config.get_builtin_max_turns("nonexistent").is_none());
    }

    #[test]
    fn test_parse_external_agent_with_tool_limits() {
        let toml_content = r#"
[agents.my-agent]
description = "Test agent"
system_prompt = "You are a test agent"
tools = ["read_file", "write_file"]
tool_limits = { read_file = 10, write_file = 5 }
"#;
        let config: AgentsConfig = toml::from_str(toml_content).unwrap();

        let agent = config.get("my-agent").unwrap();
        assert_eq!(agent.tool_limits.get("read_file"), Some(&10));
        assert_eq!(agent.tool_limits.get("write_file"), Some(&5));
    }

    #[test]
    fn test_parse_mixed_config() {
        let toml_content = r#"
[builtin.researcher]
tool_limits = { web_search = 3 }

[agents.custom-agent]
description = "Custom agent"
system_prompt = "You are custom"
tools = ["fetch_webpage"]
tool_limits = { fetch_webpage = 2 }
"#;
        let config: AgentsConfig = toml::from_str(toml_content).unwrap();

        // Builtin override
        let researcher_limits = config.get_builtin_tool_limits("researcher").unwrap();
        assert_eq!(researcher_limits.get("web_search"), Some(&3));

        // External agent
        let custom = config.get("custom-agent").unwrap();
        assert_eq!(custom.tool_limits.get("fetch_webpage"), Some(&2));
    }

    #[test]
    fn test_empty_tool_limits_returns_none() {
        let toml_content = r#"
[builtin.researcher]
# Empty tool_limits
"#;
        let config: AgentsConfig = toml::from_str(toml_content).unwrap();

        // Empty limits should return None
        assert!(config.get_builtin_tool_limits("researcher").is_none());
    }

    #[test]
    fn test_max_turns_only() {
        let toml_content = r#"
[builtin.explore]
max_turns = 100
"#;
        let config: AgentsConfig = toml::from_str(toml_content).unwrap();

        // Should have max_turns but no tool_limits
        assert_eq!(config.get_builtin_max_turns("explore"), Some(100));
        assert!(config.get_builtin_tool_limits("explore").is_none());
    }

    #[test]
    fn test_tool_limits_only() {
        let toml_content = r#"
[builtin.reviewer]
tool_limits = { read_file = 25 }
"#;
        let config: AgentsConfig = toml::from_str(toml_content).unwrap();

        // Should have tool_limits but no max_turns
        assert!(config.get_builtin_max_turns("reviewer").is_none());
        let limits = config.get_builtin_tool_limits("reviewer").unwrap();
        assert_eq!(limits.get("read_file"), Some(&25));
    }

    #[test]
    fn test_builtin_compact_prompt_override() {
        let toml_content = r#"
[builtin.coder]
compact_prompt = "Custom coder compaction prompt"
"#;
        let config: AgentsConfig = toml::from_str(toml_content).unwrap();

        assert_eq!(
            config.get_builtin_compact_prompt("coder"),
            Some("Custom coder compaction prompt")
        );
        // Non-configured agent returns None
        assert!(config.get_builtin_compact_prompt("explore").is_none());
    }

    #[test]
    fn test_external_agent_compact_prompt() {
        let toml_content = r#"
[agents.my-agent]
description = "Test agent"
system_prompt = "You are a test agent"
tools = ["read_file"]
compact_prompt = "Preserve test-specific context"
"#;
        let config: AgentsConfig = toml::from_str(toml_content).unwrap();

        let agent = config.get("my-agent").unwrap();
        assert_eq!(
            agent.compact_prompt.as_deref(),
            Some("Preserve test-specific context")
        );
    }

    #[test]
    fn test_builtin_no_bash_override() {
        let toml_content = r#"
[builtin.researcher]
no_bash = true

[builtin.coder]
# no_bash not set, defaults to false
"#;
        let config: AgentsConfig = toml::from_str(toml_content).unwrap();

        assert!(config.get_builtin_no_bash("researcher"));
        assert!(!config.get_builtin_no_bash("coder"));
        assert!(!config.get_builtin_no_bash("nonexistent"));
    }

    #[test]
    fn test_external_agent_no_bash_and_read_only() {
        let toml_content = r#"
[agents.my-agent]
description = "Test agent"
system_prompt = "You are a test agent"
tools = ["read_file"]
no_bash = true
read_only = true
"#;
        let config: AgentsConfig = toml::from_str(toml_content).unwrap();

        let agent = config.get("my-agent").unwrap();
        assert!(agent.no_bash);
        assert!(agent.read_only);
    }

    #[test]
    fn test_external_agent_no_bash_default() {
        let toml_content = r#"
[agents.my-agent]
description = "Test agent"
system_prompt = "You are a test agent"
"#;
        let config: AgentsConfig = toml::from_str(toml_content).unwrap();

        let agent = config.get("my-agent").unwrap();
        assert!(!agent.no_bash);
        assert!(!agent.read_only);
    }

    #[test]
    fn test_compact_prompt_absent_is_none() {
        let toml_content = r#"
[agents.simple-agent]
description = "Simple agent"
system_prompt = "You are simple"
"#;
        let config: AgentsConfig = toml::from_str(toml_content).unwrap();

        let agent = config.get("simple-agent").unwrap();
        assert!(agent.compact_prompt.is_none());
    }

    #[test]
    fn test_builtin_memory_strategy_override() {
        let toml_content = r#"
[builtin.coder]
memory_strategy = "compaction"

[builtin.explore]
memory_strategy = "obs-memory"
max_observations = 15
"#;
        let config: AgentsConfig = toml::from_str(toml_content).unwrap();

        assert_eq!(
            config.get_builtin_memory_strategy("coder"),
            Some(&AgentMemoryStrategy::Compaction)
        );
        assert_eq!(
            config.get_builtin_memory_strategy("explore"),
            Some(&AgentMemoryStrategy::ObsMemory)
        );
        assert_eq!(config.get_builtin_max_observations("explore"), Some(15));
        assert!(config.get_builtin_memory_strategy("nonexistent").is_none());
        assert!(config.get_builtin_max_observations("coder").is_none());
    }

    #[test]
    fn test_external_agent_memory_strategy() {
        let toml_content = r#"
[agents.obs-agent]
description = "Obs agent"
system_prompt = "You are an obs agent"
memory_strategy = "obs-memory"
max_observations = 20
"#;
        let config: AgentsConfig = toml::from_str(toml_content).unwrap();

        let agent = config.get("obs-agent").unwrap();
        assert_eq!(agent.memory_strategy, AgentMemoryStrategy::ObsMemory);
        assert_eq!(agent.max_observations, Some(20));
    }

    #[test]
    fn test_external_agent_memory_strategy_default() {
        let toml_content = r#"
[agents.default-agent]
description = "Default agent"
system_prompt = "You are default"
"#;
        let config: AgentsConfig = toml::from_str(toml_content).unwrap();

        let agent = config.get("default-agent").unwrap();
        // External agents default to compaction for backward compat
        assert_eq!(agent.memory_strategy, AgentMemoryStrategy::Compaction);
        assert!(agent.max_observations.is_none());
    }
}
