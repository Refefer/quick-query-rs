use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    #[serde(default = "default_provider")]
    pub default_provider: String,

    #[serde(default)]
    pub default_profile: Option<String>,

    #[serde(default)]
    pub default_model: Option<String>,

    #[serde(default)]
    pub temperature: Option<f32>,

    #[serde(default)]
    pub max_tokens: Option<u32>,

    #[serde(default)]
    pub providers: HashMap<String, ProviderConfigEntry>,

    #[serde(default)]
    pub prompts: HashMap<String, PromptEntry>,

    #[serde(default)]
    pub profiles: HashMap<String, ProfileEntry>,
}

fn default_provider() -> String {
    "openai".to_string()
}

impl Default for Config {
    fn default() -> Self {
        Self {
            default_provider: default_provider(),
            default_profile: None,
            default_model: None,
            temperature: None,
            max_tokens: None,
            providers: HashMap::new(),
            prompts: HashMap::new(),
            profiles: HashMap::new(),
        }
    }
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
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ProviderConfigEntry {
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

impl Config {
    pub fn load() -> Result<Self> {
        let config_path = Self::config_path()?;

        if config_path.exists() {
            let content = std::fs::read_to_string(&config_path)?;
            let config: Config = toml::from_str(&content)?;
            Ok(config)
        } else {
            Ok(Config::default())
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

        // Get the provider name (from profile or default)
        let provider_name = profile
            .provider
            .clone()
            .unwrap_or_else(|| self.default_provider.clone());

        // Get the provider config
        let provider_config = self.providers.get(&provider_name);

        // Resolve the system prompt
        let system_prompt = profile.prompt.as_ref().and_then(|p| {
            // First check if it's a named prompt
            if let Some(prompt_entry) = self.prompts.get(p) {
                Some(prompt_entry.prompt.clone())
            } else {
                // Otherwise treat it as an inline prompt
                Some(p.clone())
            }
        });

        // Merge parameters: provider params + profile params (profile wins)
        let mut parameters = provider_config
            .map(|p| p.parameters.clone())
            .unwrap_or_default();
        parameters.extend(profile.parameters.clone());

        Some(ResolvedProfile {
            provider_name,
            provider_config: provider_config.cloned(),
            system_prompt,
            model: profile.model.clone(),
            parameters,
        })
    }
}

/// Resolved profile with all settings expanded
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct ResolvedProfile {
    pub provider_name: String,
    pub provider_config: Option<ProviderConfigEntry>,
    pub system_prompt: Option<String>,
    pub model: Option<String>,
    pub parameters: HashMap<String, serde_json::Value>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = Config::default();
        assert_eq!(config.default_provider, "openai");
    }

    #[test]
    fn test_parse_config() {
        let toml = r#"
            default_provider = "anthropic"
            default_model = "claude-3-opus"

            [providers.openai]
            api_key = "sk-test"
            default_model = "gpt-4"

            [providers.anthropic]
            api_key = "sk-ant-test"
        "#;

        let config: Config = toml::from_str(toml).unwrap();
        assert_eq!(config.default_provider, "anthropic");
        assert_eq!(config.default_model, Some("claude-3-opus".to_string()));
        assert!(config.providers.contains_key("openai"));
        assert!(config.providers.contains_key("anthropic"));
    }
}
