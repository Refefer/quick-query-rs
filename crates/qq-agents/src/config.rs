//! Configuration types for external agents.

use std::collections::HashMap;
use std::path::PathBuf;

use anyhow::Result;
use serde::{Deserialize, Serialize};

fn default_max_iterations() -> usize {
    20
}

/// External agent definition from agents.toml.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentDefinition {
    /// Description of what the agent does
    pub description: String,

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
    #[serde(default = "default_max_iterations")]
    pub max_iterations: usize,
}

/// Agents configuration file (agents.toml).
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct AgentsConfig {
    /// External agent definitions
    #[serde(default)]
    pub agents: HashMap<String, AgentDefinition>,
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
}
