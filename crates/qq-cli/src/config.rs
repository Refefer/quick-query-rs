use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    /// Default profile to use (required - bundles provider, prompt, model, parameters)
    pub default_profile: String,

    #[serde(default)]
    pub providers: HashMap<String, ProviderConfigEntry>,

    #[serde(default)]
    pub prompts: HashMap<String, PromptEntry>,

    #[serde(default)]
    pub profiles: HashMap<String, ProfileEntry>,

    #[serde(default)]
    pub tools: ToolsConfigEntry,
}


/// A named system prompt
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct PromptEntry {
    /// The system prompt text
    pub prompt: String,
}

/// A profile bundles provider, prompt, model, and parameters together
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ProfileEntry {
    /// Provider name to use (references [providers.X])
    #[serde(default)]
    pub provider: Option<String>,

    /// Prompt name to use (references [prompts.X]) or inline system prompt
    #[serde(default)]
    pub prompt: Option<String>,

    /// Model override
    #[serde(default)]
    pub model: Option<String>,

    /// Extra parameters to pass to the API
    #[serde(default)]
    pub parameters: HashMap<String, serde_json::Value>,

    /// Agents enabled for this profile.
    /// - None: all agents enabled (default)
    /// - Some([]): no agents enabled
    /// - Some([names]): only listed agents enabled
    #[serde(default)]
    pub agents: Option<Vec<String>>,

    /// Primary agent to use for interactive sessions.
    /// Defaults to "pm" if not specified.
    /// Can be any internal or external agent name (e.g., "pm", "explore", "researcher").
    #[serde(default)]
    pub agent: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ProviderConfigEntry {
    /// Provider type: "openai", "anthropic", or "gemini".
    /// If omitted, inferred from base_url (defaults to openai) or provider name.
    #[serde(rename = "type", default)]
    pub provider_type: Option<String>,

    #[serde(default)]
    pub api_key: Option<String>,

    #[serde(default)]
    pub base_url: Option<String>,

    #[serde(default)]
    pub default_model: Option<String>,

    /// Extra parameters to pass to the API (e.g., reasoning_effort, chat_template_kwargs)
    #[serde(default)]
    pub parameters: std::collections::HashMap<String, serde_json::Value>,
}

/// Tools configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolsConfigEntry {
    /// Root directory for filesystem operations (supports $PWD, $HOME, ~)
    #[serde(default)]
    pub root: Option<String>,

    /// Path to memory database (supports $HOME, ~)
    #[serde(default)]
    pub memory_db: Option<String>,

    /// Allow write operations for filesystem tools
    #[serde(default = "default_true")]
    pub allow_write: bool,

    /// Enable web tools
    #[serde(default = "default_true")]
    pub enable_web: bool,

    /// Enable filesystem tools
    #[serde(default = "default_true")]
    pub enable_filesystem: bool,

    /// Enable memory tools
    #[serde(default = "default_true")]
    pub enable_memory: bool,

    /// Chunker configuration for large tool outputs
    #[serde(default)]
    pub chunker: ChunkerConfigEntry,

    /// Web search configuration (Perplexica)
    #[serde(default)]
    pub web_search: Option<WebSearchConfigEntry>,
}

/// Web search (Perplexica) configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WebSearchConfigEntry {
    /// Host URL (e.g., "http://localhost:3000")
    pub host: String,
    /// Chat model name (e.g., "gpt-4o-mini")
    pub chat_model: String,
    /// Embedding model name (e.g., "text-embedding-3-large")
    pub embed_model: String,
}

/// Chunker configuration for processing large tool outputs
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChunkerConfigEntry {
    /// Enable automatic chunking of large tool outputs
    #[serde(default = "default_true")]
    pub enabled: bool,

    /// Size threshold (in bytes) to trigger chunking (default: 50KB)
    #[serde(default = "default_threshold_bytes")]
    pub threshold_bytes: usize,

    /// Target size for each chunk in bytes (default: 10KB)
    #[serde(default = "default_chunk_size_bytes")]
    pub chunk_size_bytes: usize,

    /// Maximum number of chunks to process (default: 20)
    #[serde(default = "default_max_chunks")]
    pub max_chunks: usize,

    /// Process chunks in parallel (default: true)
    #[serde(default = "default_true")]
    pub parallel: bool,
}

fn default_threshold_bytes() -> usize {
    50_000 // 50KB
}

fn default_chunk_size_bytes() -> usize {
    10_000 // 10KB
}

fn default_max_chunks() -> usize {
    20
}

impl Default for ChunkerConfigEntry {
    fn default() -> Self {
        Self {
            enabled: true,
            threshold_bytes: default_threshold_bytes(),
            chunk_size_bytes: default_chunk_size_bytes(),
            max_chunks: default_max_chunks(),
            parallel: true,
        }
    }
}

impl ChunkerConfigEntry {
    /// Convert to qq_core::ChunkerConfig
    pub fn to_chunker_config(&self) -> qq_core::ChunkerConfig {
        qq_core::ChunkerConfig {
            enabled: self.enabled,
            threshold_bytes: self.threshold_bytes,
            chunk_size_bytes: self.chunk_size_bytes,
            max_chunks: self.max_chunks,
            parallel: self.parallel,
        }
    }
}

fn default_true() -> bool {
    true
}

impl Default for ToolsConfigEntry {
    fn default() -> Self {
        Self {
            root: None, // Will default to $PWD at runtime
            memory_db: None, // Will default to ~/.config/qq/memory.db
            allow_write: true,
            enable_web: true,
            enable_filesystem: true,
            enable_memory: true,
            chunker: ChunkerConfigEntry::default(),
            web_search: None,
        }
    }
}

/// Expand environment variables in a path string
/// Supports: $VAR, ${VAR}, ~
pub fn expand_path(path: &str) -> PathBuf {
    let mut result = path.to_string();

    // Expand ~ at the start
    if result.starts_with("~/") {
        if let Some(home) = dirs::home_dir() {
            result = format!("{}{}", home.display(), &result[1..]);
        }
    } else if result == "~" {
        if let Some(home) = dirs::home_dir() {
            return home;
        }
    }

    // Expand $VAR and ${VAR}
    let re = regex::Regex::new(r"\$\{?([A-Za-z_][A-Za-z0-9_]*)\}?").unwrap();
    let expanded = re.replace_all(&result, |caps: &regex::Captures| {
        let var_name = &caps[1];
        std::env::var(var_name).unwrap_or_else(|_| caps[0].to_string())
    });

    PathBuf::from(expanded.to_string())
}

impl Config {
    pub fn load() -> Result<Self> {
        let config_path = Self::config_path()?;

        if config_path.exists() {
            let content = std::fs::read_to_string(&config_path)?;
            let config: Config = toml::from_str(&content)?;
            Ok(config)
        } else {
            anyhow::bail!(
                "No configuration found. Create ~/.config/qq/config.toml with at least:\n\n\
                 default_profile = \"default\"\n\n\
                 [providers.openai]\n\
                 api_key = \"sk-...\"\n\n\
                 [profiles.default]\n\
                 provider = \"openai\"\n\n\
                 Supported providers: openai, anthropic, gemini (set type in provider config)\n"
            )
        }
    }

    pub fn config_path() -> Result<PathBuf> {
        let config_dir = dirs::config_dir()
            .ok_or_else(|| anyhow::anyhow!("Could not determine config directory"))?;
        Ok(config_dir.join("qq").join("config.toml"))
    }

    #[allow(dead_code)]
    pub fn config_dir() -> Result<PathBuf> {
        let config_dir = dirs::config_dir()
            .ok_or_else(|| anyhow::anyhow!("Could not determine config directory"))?;
        Ok(config_dir.join("qq"))
    }

    /// Resolve a profile name to its effective settings
    pub fn resolve_profile(&self, profile_name: &str) -> Option<ResolvedProfile> {
        let profile = self.profiles.get(profile_name)?;

        // Provider is required in profile
        let provider_name = profile.provider.clone()?;

        // Get the provider config
        let provider_config = self.providers.get(&provider_name);

        // Resolve the system prompt
        let system_prompt = profile.prompt.as_ref().map(|p| {
            // First check if it's a named prompt
            if let Some(prompt_entry) = self.prompts.get(p) {
                prompt_entry.prompt.clone()
            } else {
                // Otherwise treat it as an inline prompt
                p.clone()
            }
        });

        // Merge parameters: provider params + profile params (profile wins)
        let mut parameters = provider_config
            .map(|p| p.parameters.clone())
            .unwrap_or_default();
        parameters.extend(profile.parameters.clone());

        let provider_type = provider_config.and_then(|p| p.provider_type.clone());

        Some(ResolvedProfile {
            provider_name,
            provider_type,
            provider_config: provider_config.cloned(),
            system_prompt,
            model: profile.model.clone(),
            parameters,
            agents: profile.agents.clone(),
            agent: profile.agent.clone().unwrap_or_else(|| "pm".to_string()),
        })
    }
}

/// Resolved profile with all settings expanded
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct ResolvedProfile {
    pub provider_name: String,
    pub provider_type: Option<String>,
    pub provider_config: Option<ProviderConfigEntry>,
    pub system_prompt: Option<String>,
    pub model: Option<String>,
    pub parameters: HashMap<String, serde_json::Value>,
    pub agents: Option<Vec<String>>,
    /// Primary agent for interactive sessions (default: "pm")
    pub agent: String,
}

// Re-export agent config types from qq-agents
pub use qq_agents::AgentsConfig;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_config() {
        let toml = r#"
            default_profile = "default"

            [providers.openai]
            api_key = "sk-test"
            default_model = "gpt-4"

            [providers.anthropic]
            api_key = "sk-ant-test"

            [profiles.default]
            provider = "openai"
            model = "gpt-4o"
        "#;

        let config: Config = toml::from_str(toml).unwrap();
        assert_eq!(config.default_profile, "default");
        assert!(config.providers.contains_key("openai"));
        assert!(config.providers.contains_key("anthropic"));
        assert!(config.profiles.contains_key("default"));
    }

    #[test]
    fn test_parse_provider_with_type() {
        let toml = r#"
            default_profile = "default"

            [providers.claude]
            type = "anthropic"
            api_key = "sk-ant-test"
            default_model = "claude-sonnet-4-20250514"

            [providers.google]
            type = "gemini"
            api_key = "AIza-test"

            [providers.ollama]
            base_url = "http://localhost:11434/v1"

            [profiles.default]
            provider = "claude"
        "#;

        let config: Config = toml::from_str(toml).unwrap();

        let claude = config.providers.get("claude").unwrap();
        assert_eq!(claude.provider_type.as_deref(), Some("anthropic"));
        assert_eq!(claude.default_model.as_deref(), Some("claude-sonnet-4-20250514"));

        let google = config.providers.get("google").unwrap();
        assert_eq!(google.provider_type.as_deref(), Some("gemini"));

        let ollama = config.providers.get("ollama").unwrap();
        assert_eq!(ollama.provider_type, None);
    }

    #[test]
    fn test_resolve_profile() {
        let toml = r#"
            default_profile = "coding"

            [providers.openai]
            api_key = "sk-test"
            default_model = "gpt-4"

            [prompts.coder]
            prompt = "You are a coding assistant."

            [profiles.coding]
            provider = "openai"
            prompt = "coder"
            model = "gpt-4o"
        "#;

        let config: Config = toml::from_str(toml).unwrap();
        let resolved = config.resolve_profile("coding").unwrap();
        assert_eq!(resolved.provider_name, "openai");
        assert_eq!(resolved.system_prompt, Some("You are a coding assistant.".to_string()));
        assert_eq!(resolved.model, Some("gpt-4o".to_string()));
    }
}
