//! Shared preamble for all agent system prompts.
//!
//! Generates a common preamble explaining the quick-query agent framework,
//! conditionally including sections based on agent capabilities.

use chrono::Local;
use std::collections::HashMap;

/// Runtime context for agents - contains dynamic variables resolved at startup.
/// 
/// This struct provides extensible access to runtime values like current date/time,
/// working directory, and custom environment-specific variables.
#[derive(Debug, Clone)]
pub struct AgentContext {
    /// Current date in YYYY-MM-DD format
    pub current_date: String,
    /// Current day of week (e.g., "Monday")
    pub current_day: String,
    /// Present working directory (may be None if unavailable)
    pub pwd: Option<String>,
    /// Custom runtime variables for extensibility
    pub custom_vars: HashMap<String, String>,
}

impl AgentContext {
    /// Create a new AgentContext with populated runtime values.
    /// 
    /// This should be called at agent startup to resolve dynamic variables.
    pub fn new() -> Self {
        let now = Local::now();
        
        Self {
            current_date: now.format("%Y-%m-%d").to_string(),
            current_day: now.format("%A").to_string(),
            pwd: std::env::current_dir()
                .ok()
                .and_then(|p| p.to_str().map(|s| s.to_string())),
            custom_vars: HashMap::new(),
        }
    }

    /// Create a new AgentContext with the ability to set custom variables.
    pub fn with_custom_var(mut self, key: &str, value: &str) -> Self {
        self.custom_vars.insert(key.to_string(), value.to_string());
        self
    }

    /// Get all custom variables as a reference.
    pub fn get_custom_vars(&self) -> &HashMap<String, String> {
        &self.custom_vars
    }

    /// Get a specific custom variable, returning None if not found.
    pub fn get_custom_var(&self, key: &str) -> Option<&String> {
        self.custom_vars.get(key)
    }
}

impl Default for AgentContext {
    fn default() -> Self {
        Self::new()
    }
}

/// Context for generate_preamble - describes agent capabilities.
pub struct PreambleContext {
    /// Whether this agent has tools available.
    pub has_tools: bool,
    /// Whether this agent can delegate to sub-agents.
    pub has_sub_agents: bool,
    /// Whether this agent has the inform_user tool.
    pub has_inform_user: bool,
    /// Whether this agent has task tracking capabilities (update_my_task).
    pub has_task_tracking: bool,
    /// Whether this agent has preference tools (read_preference, update_preference).
    pub has_preferences: bool,
    /// Whether this agent has bash access.
    pub has_bash: bool,
    /// Whether this agent is read-only (must not modify files).
    pub is_read_only: bool,
}

/// Generate the shared preamble that gets prepended to all agent system prompts.
///
/// Sections are conditionally included based on the agent's capabilities:
/// - Core sections (execution model, conversation continuity) are always included
/// - Runtime context section with dynamic variables (current date, day, pwd)
/// - Sub-agent delegation section only if `has_sub_agents` is true
/// - Inform user section only if `has_inform_user` is true
/// - Tool efficiency section only if `has_tools` is true
pub fn generate_preamble(ctx: &PreambleContext, agent_ctx: &AgentContext) -> String {
    let mut sections = Vec::new();

    // Core sections always included
    sections.push(
        "## Quick-Query Agent Framework\n\
         \n\
         You are an agent in the quick-query multi-agent system. You operate autonomously\n\
         in a loop: you receive a task, use your tools to accomplish it, and return a final\n\
         text response when done. You do NOT interact with the user directly — your caller\n\
         receives your final response.\n\
         \n\
         ### Execution Model\n\
         - You run in an agentic loop: each iteration, you may call tools or return a final response.\n\
         - When you return text without any tool calls, your execution ends and that text becomes your result.\n\
         - You have a limited number of turns. If you exhaust them, your progress is automatically \
         summarized and you may be continued with that summary as context. Work efficiently to avoid \
         hitting the limit.\n\
         - Do NOT stop and ask for confirmation mid-task. Execute your full task autonomously, then return results.\n\
         \n\
         ### Conversation Continuity\n\
         You may be called multiple times within the same session. If your conversation includes\n\
         messages from a previous invocation, build on that context — do not repeat work already\n\
         done. Focus on the new task while leveraging prior discoveries and results."
            .to_string(),
    );

    // Runtime context section - always included with dynamic variables
    let mut runtime_context = format!(
        "### Runtime Context\n\
         You have access to runtime context that was resolved at your startup:\n\
         - **Current Date**: {}\n\
         - **Current Day**: {}",
        agent_ctx.current_date, agent_ctx.current_day
    );

    if let Some(ref pwd) = agent_ctx.pwd {
        runtime_context.push_str(&format!("\n- **Working Directory**: {}", pwd));
    }

    if !agent_ctx.custom_vars.is_empty() {
        runtime_context.push_str("\n\n**Custom Variables**:\n");
        for (key, value) in &agent_ctx.custom_vars {
            runtime_context.push_str(&format!("- **{}**: {}\n", key, value));
        }
    }

    sections.push(runtime_context);

    // Sub-agent delegation (conditional)
    if ctx.has_sub_agents {
        sections.push(
            "### Delegating to Sub-Agents\n\
             You have access to other agents as tools (e.g., Agent[explore], Agent[coder]).\n\
             These agents also persist their conversation history across your calls to them.\n\
             If you called an agent earlier, calling it again lets it build on what it already\n\
             discovered — you don't need to re-explain context.\n\
             \n\
             The `new_instance` parameter (default: false) controls agent memory:\n\
             - `false`: The agent continues with full context from prior calls.\n\
             - `true`: Clears the agent's memory for a fresh start. Use only when prior\n\
             context would be misleading for a completely unrelated task.\n\
             \n\
             The `instance_id` parameter isolates agent memory per task. Pass `instance_id`\n\
             using the format \"{agent}-agent:{task_id}\" (e.g. \"coder-agent:3\") when dispatching\n\
             agents for tracked tasks. Agents with different instance_ids maintain separate memory,\n\
             enabling safe parallel dispatch of the same agent type."
                .to_string(),
        );
    }

    // Inform user (conditional)
    if ctx.has_inform_user {
        sections.push(
            "### Keeping the User Informed\n\
             You have the `inform_user` tool for sending status messages to the user without ending your turn.\n\
             Use it before starting significant work, when discovering something notable, and when completing\n\
             phases of multi-step tasks. See the tool's description for full guidance."
                .to_string(),
        );
    }

    // Tool efficiency (conditional)
    if ctx.has_tools {
        sections.push(
            "### Tool Usage Efficiency\n\
             Before making a tool call, check if you already have the information from a previous call.\n\
             Read each tool's description carefully — they contain batching and consolidation guidance.\n\
             Never re-read a file you already read in this session."
                .to_string(),
        );
    }

    // Task tracking (conditional)
    if ctx.has_task_tracking {
        sections.push(
            "### Task Tracking\n\
             Your task may include a **Current Task Board** section showing the PM's tracked tasks.\n\
             This gives you visibility into the overall plan and where your work fits.\n\
             \n\
             You have the `update_my_task` tool to report progress:\n\
             - **Mark done**: `{\"id\": \"3\", \"status\": \"done\"}` when your task is complete.\n\
             - **Add notes**: `{\"id\": \"3\", \"add_note\": \"Found 3 files to modify\"}` to log findings, progress, or blockers.\n\
             - **Flag blockers**: `{\"id\": \"3\", \"status\": \"blocked\", \"add_note\": \"Waiting on auth module refactor\"}` if you're stuck.\n\
             \n\
             This helps the PM track progress across all agents. Update your task before returning your final result."
                .to_string(),
        );
    }

    // User Preferences (conditional)
    if ctx.has_preferences {
        sections.push(
            "### User Preferences\n\
             You have access to persistent preference tools (`read_preference`, `update_preference`, etc.)\n\
             for storing **user preferences only** — long-lived facts about the user that persist across sessions.\n\
             \n\
             **Good uses** (preferences that persist):\n\
             - User's name, role, or team\n\
             - Coding style: indent style, naming conventions, preferred patterns\n\
             - Preferred tools, frameworks, or libraries\n\
             - Communication preferences (verbosity, format)\n\
             \n\
             **Bad uses** (task-specific data — use /tmp files instead):\n\
             - Lists of files to modify for the current task\n\
             - Intermediate analysis results or gathered data\n\
             - Working notes, TODOs, or task progress\n\
             - Code snippets or diffs being worked on\n\
             \n\
             For task-specific working data, write to /tmp files and pass file paths in context or\n\
             return them to your caller."
                .to_string(),
        );
    }

    // Shell access (conditional)
    if ctx.has_bash {
        sections.push(
            "### Shell Access\n\
             You have sandboxed shell access via the `run` tool. Read-only commands (grep, find, git log, git diff, wc, tree, etc.)\n\
             run without approval. Write commands (cargo build, git commit, npm install, rm, etc.) require user approval.\n\
             The `run` tool is your primary tool for ALL file operations — reading (cat, head), writing (cat >, tee),\n\
             editing (sed -i), searching (grep -rn, find), and file management (cp, mv, mkdir -p, rm).\n\
             Network access is blocked by default — call the `request_network_access` tool before running\n\
             any command that needs the internet (curl, wget, git clone, npm install from remote, etc.).\n\
             Sensitive home directories (.ssh, .aws, .kube, .docker, etc.) are hidden by default — call\n\
             `request_sensitive_access` before running tools that need stored credentials (gh, kubectl, docker, aws, etc.).\n\
             \n\
             ### /tmp Scratch Space\n\
             /tmp is a writable scratch space shared across all your tools in this session. **Use it liberally\n\
             for complex tasks** — your context window is finite and can lose details over long sessions,\n\
             but files in /tmp persist reliably for the entire session.\n\
             \n\
             Recommended uses:\n\
             - **Intermediate results**: `find . -name '*.rs' > /tmp/files.txt` then process the list\n\
             - **Scripts**: Write multi-step logic to /tmp/script.sh and run it — avoids inline escaping\n\
               issues and keeps complex operations reproducible\n\
             - **Working notes**: Save command output, analysis results, or gathered data to /tmp files\n\
               rather than trying to hold it all in context\n\
             - **Staged changes**: Draft file contents in /tmp before writing to the project\n\
             - **Diff/comparison**: Save snapshots to /tmp for before/after comparison\n\
             - **Cross-agent data**: Write data to /tmp and pass the file path when delegating to\n\
               other agents or returning results to your caller\n\
             \n\
             Rule of thumb: if a task involves more than 2-3 intermediate steps, use /tmp files to track\n\
             state between steps rather than relying on context alone."
                .to_string(),
        );
    }

    // Read-only reinforcement (conditional)
    if ctx.is_read_only {
        sections.push(
            "### CRITICAL: Read-Only Agent\n\
             You are a READ-ONLY agent. You must NEVER modify project files or directories.\n\
             - No writing, creating, moving, or deleting project files\n\
             - No write commands that affect the project (no cargo build, git commit, npm install, rm, mv, etc.)\n\
             \n\
             You may ONLY: read files, search content, and run read-only commands (grep, find, cat, \
             git log, git diff, git blame, wc, tree, head, tail, ls, etc.).\n\
             \n\
             **Exception: /tmp is allowed.** You CAN write to /tmp for scratch work — saving intermediate\n\
             results, command output, or working notes. This does not modify the project.\n\
             \n\
             If your task requires project modifications, report your findings and recommend the appropriate agent."
                .to_string(),
        );
    }

    // Resourcefulness (conditional: has tools or sub-agents)
    if ctx.has_tools || ctx.has_sub_agents {
        sections.push(
            "### Resourcefulness\n\
             When your task references files, data, or information without giving exact paths or details,\n\
             use your tools and sub-agents to discover what you need. Explore the filesystem, search for\n\
             patterns, research topics — exhaust your available resources before concluding that you need\n\
             to ask for clarification. Only ask when discovery genuinely fails or yields ambiguous results\n\
             that require human judgment to resolve."
                .to_string(),
        );
    }

    sections.join("\n\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_agent_context_new() {
        let ctx = AgentContext::new();
        
        // Should have valid date format (YYYY-MM-DD)
        assert!(ctx.current_date.chars().filter(|&c| c == '-').count() == 2);
        
        // Day should not be empty
        assert!(!ctx.current_day.is_empty());
        
        // Custom vars should be empty initially
        assert!(ctx.custom_vars.is_empty());
    }

    #[test]
    fn test_agent_context_with_custom_var() {
        let ctx = AgentContext::new()
            .with_custom_var("ENV", "test")
            .with_custom_var("PROJECT", "quick-query");
        
        assert_eq!(ctx.get_custom_var("ENV"), Some(&"test".to_string()));
        assert_eq!(ctx.get_custom_var("PROJECT"), Some(&"quick-query".to_string()));
        assert_eq!(ctx.get_custom_var("NONEXISTENT"), None);
    }

    #[test]
    fn test_minimal_preamble() {
        let agent_ctx = AgentContext::new();
        let preamble = generate_preamble(&PreambleContext {
            has_tools: false,
            has_sub_agents: false,
            has_inform_user: false,
            has_task_tracking: false,
            has_preferences: false,
            has_bash: false,
            is_read_only: false,
        }, &agent_ctx);

        // Core sections always present
        assert!(preamble.contains("Quick-Query Agent Framework"));
        assert!(preamble.contains("Execution Model"));
        assert!(preamble.contains("Conversation Continuity"));
        
        // Runtime context should be present
        assert!(preamble.contains("Runtime Context"));
        assert!(preamble.contains("Current Date"));
        assert!(preamble.contains("Current Day"));

        // Conditional sections absent
        assert!(!preamble.contains("Delegating to Sub-Agents"));
        assert!(!preamble.contains("Keeping the User Informed"));
        assert!(!preamble.contains("Tool Usage Efficiency"));
        assert!(!preamble.contains("Resourcefulness"));
        assert!(!preamble.contains("Task Tracking"));
        assert!(!preamble.contains("User Preferences"));
        assert!(!preamble.contains("Shell Access"));
        assert!(!preamble.contains("CRITICAL: Read-Only Agent"));
    }

    #[test]
    fn test_preamble_with_tools() {
        let agent_ctx = AgentContext::new();
        let preamble = generate_preamble(&PreambleContext {
            has_tools: true,
            has_sub_agents: false,
            has_inform_user: false,
            has_task_tracking: false,
            has_preferences: false,
            has_bash: false,
            is_read_only: false,
        }, &agent_ctx);

        assert!(preamble.contains("Tool Usage Efficiency"));
        assert!(preamble.contains("Resourcefulness"));
        assert!(!preamble.contains("Delegating to Sub-Agents"));
        assert!(!preamble.contains("Keeping the User Informed"));
    }

    #[test]
    fn test_preamble_with_sub_agents() {
        let agent_ctx = AgentContext::new();
        let preamble = generate_preamble(&PreambleContext {
            has_tools: false,
            has_sub_agents: true,
            has_inform_user: false,
            has_task_tracking: false,
            has_preferences: false,
            has_bash: false,
            is_read_only: false,
        }, &agent_ctx);

        assert!(preamble.contains("Delegating to Sub-Agents"));
        assert!(preamble.contains("new_instance"));
        assert!(preamble.contains("Resourcefulness"));
        assert!(!preamble.contains("Tool Usage Efficiency"));
        assert!(!preamble.contains("Keeping the User Informed"));
    }

    #[test]
    fn test_preamble_with_inform_user() {
        let agent_ctx = AgentContext::new();
        let preamble = generate_preamble(&PreambleContext {
            has_tools: false,
            has_sub_agents: false,
            has_inform_user: true,
            has_task_tracking: false,
            has_preferences: false,
            has_bash: false,
            is_read_only: false,
        }, &agent_ctx);

        assert!(preamble.contains("Keeping the User Informed"));
        assert!(preamble.contains("inform_user"));
        assert!(!preamble.contains("Tool Usage Efficiency"));
        assert!(!preamble.contains("Delegating to Sub-Agents"));
    }

    #[test]
    fn test_preamble_with_task_tracking() {
        let agent_ctx = AgentContext::new();
        let preamble = generate_preamble(&PreambleContext {
            has_tools: false,
            has_sub_agents: false,
            has_inform_user: false,
            has_task_tracking: true,
            has_preferences: false,
            has_bash: false,
            is_read_only: false,
        }, &agent_ctx);

        assert!(preamble.contains("Task Tracking"));
        assert!(preamble.contains("update_my_task"));
        assert!(preamble.contains("Current Task Board"));
    }

    #[test]
    fn test_preamble_with_preferences() {
        let agent_ctx = AgentContext::new();
        let preamble = generate_preamble(&PreambleContext {
            has_tools: false,
            has_sub_agents: false,
            has_inform_user: false,
            has_task_tracking: false,
            has_preferences: true,
            has_bash: false,
            is_read_only: false,
        }, &agent_ctx);

        assert!(preamble.contains("User Preferences"));
        assert!(preamble.contains("read_preference"));
        assert!(preamble.contains("update_preference"));
        assert!(preamble.contains("Good uses"));
        assert!(preamble.contains("Bad uses"));
        assert!(preamble.contains("/tmp files"));
    }

    #[test]
    fn test_preamble_with_bash() {
        let agent_ctx = AgentContext::new();
        let preamble = generate_preamble(&PreambleContext {
            has_tools: true,
            has_sub_agents: false,
            has_inform_user: false,
            has_task_tracking: false,
            has_preferences: false,
            has_bash: true,
            is_read_only: false,
        }, &agent_ctx);

        assert!(preamble.contains("Shell Access"));
        assert!(preamble.contains("sandboxed shell access"));
        assert!(preamble.contains("/tmp"));
        assert!(preamble.contains("Cross-agent data"));
        assert!(!preamble.contains("CRITICAL: Read-Only Agent"));
    }

    #[test]
    fn test_preamble_with_read_only() {
        let agent_ctx = AgentContext::new();
        let preamble = generate_preamble(&PreambleContext {
            has_tools: true,
            has_sub_agents: false,
            has_inform_user: false,
            has_task_tracking: false,
            has_preferences: false,
            has_bash: false,
            is_read_only: true,
        }, &agent_ctx);

        assert!(preamble.contains("CRITICAL: Read-Only Agent"));
        assert!(preamble.contains("READ-ONLY agent"));
        assert!(preamble.contains("NEVER"));
        assert!(!preamble.contains("Shell Access"));
    }

    #[test]
    fn test_preamble_read_only_with_bash() {
        let agent_ctx = AgentContext::new();
        let preamble = generate_preamble(&PreambleContext {
            has_tools: true,
            has_sub_agents: false,
            has_inform_user: false,
            has_task_tracking: false,
            has_preferences: false,
            has_bash: true,
            is_read_only: true,
        }, &agent_ctx);

        // Both sections should appear
        assert!(preamble.contains("Shell Access"));
        assert!(preamble.contains("CRITICAL: Read-Only Agent"));
    }

    #[test]
    fn test_full_preamble() {
        let agent_ctx = AgentContext::new();
        let preamble = generate_preamble(&PreambleContext {
            has_tools: true,
            has_sub_agents: true,
            has_inform_user: true,
            has_task_tracking: true,
            has_preferences: true,
            has_bash: true,
            is_read_only: false,
        }, &agent_ctx);

        // All sections present
        assert!(preamble.contains("Quick-Query Agent Framework"));
        assert!(preamble.contains("Execution Model"));
        assert!(preamble.contains("Conversation Continuity"));
        assert!(preamble.contains("Delegating to Sub-Agents"));
        assert!(preamble.contains("Keeping the User Informed"));
        assert!(preamble.contains("Tool Usage Efficiency"));
        assert!(preamble.contains("Resourcefulness"));
        assert!(preamble.contains("Task Tracking"));
        assert!(preamble.contains("User Preferences"));
        assert!(preamble.contains("Shell Access"));
    }

    #[test]
    fn test_preamble_with_custom_vars() {
        let agent_ctx = AgentContext::new()
            .with_custom_var("ENV", "development")
            .with_custom_var("TEAM", "platform");
        
        let preamble = generate_preamble(&PreambleContext {
            has_tools: false,
            has_sub_agents: false,
            has_inform_user: false,
            has_task_tracking: false,
            has_preferences: false,
            has_bash: false,
            is_read_only: false,
        }, &agent_ctx);

        assert!(preamble.contains("Custom Variables"));
        assert!(preamble.contains("ENV"));
        assert!(preamble.contains("development"));
        assert!(preamble.contains("TEAM"));
        assert!(preamble.contains("platform"));
    }
}
