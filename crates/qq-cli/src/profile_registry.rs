//! Session-scoped registry of provider/profile bindings.
//!
//! Holds one pre-instantiated [`qq_core::Provider`] per configured profile,
//! plus the metadata (model, parameters, system prompt, ...) needed to build
//! requests against it. Resolves which profile drives the main chat versus
//! each individual agent. Mutated at runtime by the TUI `/profiles` command.
//!
//! # Resolution rules
//!
//! - The main chat (default target) uses `default_profile`.
//! - An agent uses `agent_overrides[name]` if set, otherwise `default_profile`.
//! - All profile names referenced must exist in the registry — startup errors
//!   out if an agent's configured `profile` field names an unknown profile.
//!
//! # Concurrency
//!
//! [`SharedProfileRegistry`] is `Arc<RwLock<ProfileRegistry>>`. Reads (one per
//! agent execution / chat turn) hold the read lock briefly to clone an `Arc`;
//! writes (only on `/profiles`) hold the write lock. Eager pre-instantiation
//! means the picker never has to do async work behind the lock.

use std::collections::HashMap;
use std::sync::Arc;

use anyhow::{anyhow, Context, Result};
use tokio::sync::RwLock;

use qq_core::Provider;

use crate::config::Config;

/// Runtime data for a single profile: the instantiated provider plus the
/// fields used at request-build time and (for the default-chat target) at
/// session setup time.
///
/// Some fields (`system_prompt`, `include_tool_reasoning`, `agent`, ...) are
/// only consumed when this runtime represents the *default chat* — agents
/// never read them. They are populated for completeness so consumers that
/// need them later don't have to re-resolve from config.
#[allow(dead_code)]
pub struct ResolvedProfileRuntime {
    pub profile_name: String,
    pub provider: Arc<dyn Provider>,
    /// Model name to pass on each `CompletionRequest::with_model`. May be
    /// `None`, in which case the provider's `default_model` is used.
    pub model: Option<String>,
    /// Per-profile extra parameters (e.g., `reasoning_effort`). Merged into
    /// each `CompletionRequest` via `with_extra`. May be empty.
    pub parameters: HashMap<String, serde_json::Value>,
    /// Profile-level system prompt. Used only by the default-chat target —
    /// agents keep their own `system_prompt`.
    pub system_prompt: Option<String>,
    pub include_tool_reasoning: bool,
    pub context_window: Option<u32>,
    pub supported_content_types: Option<Vec<String>>,
    /// Primary agent suggested by the profile (only relevant for the default
    /// target; ignored when this profile is assigned to a specific agent).
    pub agent: String,
    /// Enabled-agent filter from the profile (only relevant for the default
    /// target).
    pub agents: Option<Vec<String>>,
}

pub type SharedProfileRegistry = Arc<RwLock<ProfileRegistry>>;

/// Session-scoped registry of profiles.
pub struct ProfileRegistry {
    /// All known profiles, pre-instantiated. Keyed by profile name.
    profiles: HashMap<String, Arc<ResolvedProfileRuntime>>,
    /// Profile used by the main chat (the "default chat" target in /profiles).
    default_profile: String,
    /// Per-agent overrides. agent_name -> profile_name.
    agent_overrides: HashMap<String, String>,
}

impl ProfileRegistry {
    /// Construct a registry from a fully built map of pre-instantiated
    /// runtimes plus the chosen default and agent overrides.
    ///
    /// Validates that `default_profile` and every value in `agent_overrides`
    /// names a profile present in `profiles`.
    pub fn new(
        profiles: HashMap<String, Arc<ResolvedProfileRuntime>>,
        default_profile: String,
        agent_overrides: HashMap<String, String>,
    ) -> Result<Self> {
        if !profiles.contains_key(&default_profile) {
            return Err(anyhow!(
                "Default profile '{}' is not present in the registry",
                default_profile
            ));
        }
        for (agent, profile) in &agent_overrides {
            if !profiles.contains_key(profile) {
                return Err(anyhow!(
                    "Agent '{}' references unknown profile '{}'",
                    agent,
                    profile
                ));
            }
        }
        Ok(Self {
            profiles,
            default_profile,
            agent_overrides,
        })
    }

    /// Wrap this registry in a `SharedProfileRegistry` for plumbing.
    pub fn into_shared(self) -> SharedProfileRegistry {
        Arc::new(RwLock::new(self))
    }

    /// Name of the registry-wide fallback profile. Agents without their own
    /// override resolve to this.
    pub fn default_profile(&self) -> &str {
        &self.default_profile
    }

    /// Switch the registry-wide fallback profile. Agents that have their own
    /// override are unaffected; only those that resolve through the fallback
    /// move to the new profile.
    pub fn set_default_profile(&mut self, name: &str) -> Result<()> {
        if !self.profiles.contains_key(name) {
            return Err(anyhow!("Unknown profile: {}", name));
        }
        self.default_profile = name.to_string();
        Ok(())
    }

    /// Profile used by `agent_name` (override if set, default otherwise).
    pub fn for_agent(&self, agent_name: &str) -> Arc<ResolvedProfileRuntime> {
        let profile = self
            .agent_overrides
            .get(agent_name)
            .map(String::as_str)
            .unwrap_or(self.default_profile.as_str());
        Arc::clone(
            self.profiles
                .get(profile)
                .expect("agent_overrides values are validated on each setter"),
        )
    }

    /// Sorted list of all configured profile names.
    pub fn list_profile_names(&self) -> Vec<String> {
        let mut names: Vec<String> = self.profiles.keys().cloned().collect();
        names.sort();
        names
    }

    /// Current agent overrides (agent_name -> profile_name), sorted by agent name.
    #[allow(dead_code)]
    pub fn list_agent_overrides(&self) -> Vec<(String, String)> {
        let mut entries: Vec<(String, String)> = self
            .agent_overrides
            .iter()
            .map(|(a, p)| (a.clone(), p.clone()))
            .collect();
        entries.sort_by(|a, b| a.0.cmp(&b.0));
        entries
    }

    /// Assign `profile` to `agent`. Errors out if no such profile.
    pub fn set_agent_profile(&mut self, agent: &str, profile: &str) -> Result<()> {
        if !self.profiles.contains_key(profile) {
            return Err(anyhow!("Unknown profile: {}", profile));
        }
        self.agent_overrides
            .insert(agent.to_string(), profile.to_string());
        Ok(())
    }

    /// Remove the override for `agent`, falling back to the default profile.
    #[allow(dead_code)]
    pub fn clear_agent_override(&mut self, agent: &str) {
        self.agent_overrides.remove(agent);
    }
}

/// Build a registry from `config` and the (already loaded) `agents_config`.
///
/// The runtime entry for `default_profile_name` is built from `default_runtime`
/// (which the caller has already resolved with CLI flag overrides applied).
/// All other profiles are instantiated from their `[profiles.X]` definition
/// without CLI overrides.
///
/// Agent overrides are seeded from `agents_config.get_agent_profile()` for
/// every known agent name. Unknown profile references return an error so the
/// user gets a clear startup-time message instead of a confusing fallback at
/// the first agent call.
pub fn build_registry(
    config: &Config,
    agents_config: &qq_agents::AgentsConfig,
    default_profile_name: String,
    default_runtime: ResolvedProfileRuntime,
    instantiate: impl Fn(&str, &Config) -> Result<ResolvedProfileRuntime>,
) -> Result<ProfileRegistry> {
    let mut profiles: HashMap<String, Arc<ResolvedProfileRuntime>> = HashMap::new();

    // Insert the default first (with CLI overrides baked in).
    profiles.insert(default_profile_name.clone(), Arc::new(default_runtime));

    // Build runtimes for every other profile defined in the config.
    for name in config.profiles.keys() {
        if name == &default_profile_name {
            continue;
        }
        let runtime = instantiate(name, config)
            .with_context(|| format!("Failed to instantiate profile '{}'", name))?;
        profiles.insert(name.clone(), Arc::new(runtime));
    }

    // Seed agent overrides from agents_config.
    let mut agent_overrides: HashMap<String, String> = HashMap::new();
    let mut all_agent_names: std::collections::HashSet<&str> = agents_config
        .builtin
        .keys()
        .map(String::as_str)
        .collect();
    all_agent_names.extend(agents_config.agents.keys().map(String::as_str));
    for name in all_agent_names {
        if let Some(profile) = agents_config.get_agent_profile(name) {
            if !profiles.contains_key(profile) {
                return Err(anyhow!(
                    "Agent '{}' references unknown profile '{}' \
                     (available profiles: {:?})",
                    name,
                    profile,
                    profiles.keys().collect::<Vec<_>>()
                ));
            }
            agent_overrides.insert(name.to_string(), profile.to_string());
        }
    }

    ProfileRegistry::new(profiles, default_profile_name, agent_overrides)
}

#[cfg(test)]
mod tests {
    use super::*;
    use qq_core::testing::MockProvider;

    fn make_runtime(name: &str) -> ResolvedProfileRuntime {
        ResolvedProfileRuntime {
            profile_name: name.to_string(),
            provider: Arc::new(MockProvider::new()),
            model: None,
            parameters: HashMap::new(),
            system_prompt: None,
            include_tool_reasoning: false,
            context_window: None,
            supported_content_types: None,
            agent: "pm".to_string(),
            agents: None,
        }
    }

    fn registry_with(profiles: &[&str], default: &str, overrides: &[(&str, &str)]) -> ProfileRegistry {
        let map: HashMap<String, Arc<ResolvedProfileRuntime>> = profiles
            .iter()
            .map(|n| (n.to_string(), Arc::new(make_runtime(n))))
            .collect();
        let overrides: HashMap<String, String> = overrides
            .iter()
            .map(|(a, p)| (a.to_string(), p.to_string()))
            .collect();
        ProfileRegistry::new(map, default.to_string(), overrides).unwrap()
    }

    #[test]
    fn for_agent_returns_override_when_present() {
        let r = registry_with(&["fast", "default"], "default", &[("explore", "fast")]);
        assert_eq!(r.for_agent("explore").profile_name, "fast");
    }

    #[test]
    fn for_agent_falls_back_to_default() {
        let r = registry_with(&["fast", "default"], "default", &[("explore", "fast")]);
        assert_eq!(r.for_agent("planner").profile_name, "default");
    }

    #[test]
    fn set_agent_profile_assigns_override() {
        let mut r = registry_with(&["fast", "default"], "default", &[]);
        r.set_agent_profile("explore", "fast").unwrap();
        assert_eq!(r.for_agent("explore").profile_name, "fast");
    }

    #[test]
    fn set_agent_profile_rejects_unknown() {
        let mut r = registry_with(&["fast", "default"], "default", &[]);
        assert!(r.set_agent_profile("explore", "nope").is_err());
    }

    #[test]
    fn clear_agent_override_falls_back_to_default() {
        let mut r = registry_with(&["fast", "default"], "default", &[("explore", "fast")]);
        r.clear_agent_override("explore");
        assert_eq!(r.for_agent("explore").profile_name, "default");
    }

    #[test]
    fn list_profile_names_is_sorted() {
        let r = registry_with(&["zeta", "alpha", "mid"], "alpha", &[]);
        assert_eq!(r.list_profile_names(), vec!["alpha", "mid", "zeta"]);
    }

    #[test]
    fn new_rejects_default_not_in_map() {
        let map: HashMap<String, Arc<ResolvedProfileRuntime>> = HashMap::new();
        assert!(ProfileRegistry::new(map, "missing".to_string(), HashMap::new()).is_err());
    }

    /// Setting the primary agent's per-agent override moves only that agent.
    /// Other agents that resolve through the fallback are unaffected.
    #[test]
    fn setting_primary_agent_override_does_not_move_other_agents() {
        let mut r = registry_with(&["fast", "default"], "default", &[]);
        assert_eq!(r.for_agent("explore").profile_name, "default");

        r.set_agent_profile("pm", "fast").unwrap();

        assert_eq!(r.for_agent("pm").profile_name, "fast");
        assert_eq!(r.for_agent("explore").profile_name, "default");
    }

    /// Setting the registry-wide fallback moves every agent that resolves
    /// through it, but agents with their own override are pinned in place.
    #[test]
    fn setting_default_profile_moves_only_unoverridden_agents() {
        let mut r = registry_with(
            &["fast", "default"],
            "default",
            &[("pm", "fast")],
        );
        assert_eq!(r.for_agent("pm").profile_name, "fast");
        assert_eq!(r.for_agent("explore").profile_name, "default");

        r.set_default_profile("fast").unwrap();

        assert_eq!(r.for_agent("pm").profile_name, "fast");
        assert_eq!(r.for_agent("explore").profile_name, "fast");
    }

    #[test]
    fn set_default_profile_rejects_unknown() {
        let mut r = registry_with(&["fast", "default"], "default", &[]);
        assert!(r.set_default_profile("nope").is_err());
    }

    #[test]
    fn default_profile_returns_current_default() {
        let r = registry_with(&["fast", "default"], "fast", &[]);
        assert_eq!(r.default_profile(), "fast");
    }

    #[test]
    fn new_rejects_override_pointing_at_unknown_profile() {
        let map: HashMap<String, Arc<ResolvedProfileRuntime>> = [(
            "default".to_string(),
            Arc::new(make_runtime("default")),
        )]
        .into_iter()
        .collect();
        let mut overrides: HashMap<String, String> = HashMap::new();
        overrides.insert("explore".to_string(), "missing".to_string());
        assert!(ProfileRegistry::new(map, "default".to_string(), overrides).is_err());
    }
}
