//! Shared preamble for all agent system prompts.
//!
//! Generates a common preamble explaining the quick-query agent framework,
//! conditionally including sections based on agent capabilities.

/// Context for generating the shared agent preamble.
pub struct PreambleContext {
    /// Whether this agent has tools available.
    pub has_tools: bool,
    /// Whether this agent can delegate to sub-agents.
    pub has_sub_agents: bool,
    /// Whether this agent has the inform_user tool.
    pub has_inform_user: bool,
}

/// Generate the shared preamble that gets prepended to all agent system prompts.
///
/// Sections are conditionally included based on the agent's capabilities:
/// - Core sections (execution model, persistent memory) are always included
/// - Sub-agent delegation section only if `has_sub_agents` is true
/// - Inform user section only if `has_inform_user` is true
/// - Tool efficiency section only if `has_tools` is true
pub fn generate_preamble(ctx: &PreambleContext) -> String {
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
         ### Persistent Memory\n\
         You may be called multiple times within the same session. If your conversation includes\n\
         messages from a previous invocation, build on that context — do not repeat work already\n\
         done. Focus on the new task while leveraging prior discoveries and results."
            .to_string(),
    );

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
             context would be misleading for a completely unrelated task."
                .to_string(),
        );
    }

    // Inform user (conditional)
    if ctx.has_inform_user {
        sections.push(
            "### Keeping the User Informed\n\
             Use `inform_user` to send status messages to the user WITHOUT ending your turn.\n\
             The user sees these immediately while you continue working.\n\
             \n\
             When to use it:\n\
             - Before starting significant work: what you are about to do\n\
             - When you discover something notable: key findings or unexpected issues\n\
             - When completing phases of a multi-step task: progress updates\n\
             - When plans change: why you are adjusting your approach\n\
             \n\
             This is fire-and-forget: calling inform_user does not pause execution or wait\n\
             for a response. Use it freely for transparency.\n\
             \n\
             When executing multi-step plans, use inform_user to report completion of each step,\n\
             then keep going. Do NOT stop between steps to wait for confirmation."
                .to_string(),
        );
    }

    // Tool efficiency (conditional)
    if ctx.has_tools {
        sections.push(
            "### Tool Usage Efficiency\n\
             NEVER call the same tool multiple times when a single call would suffice. Before\n\
             making a tool call, check if you already have the information from a previous call.\n\
             \n\
             - Consolidate searches: use regex alternation (e.g., `\"(struct|impl) Config\"`)\n\
               instead of separate searches for each pattern.\n\
             - Consolidate file discovery: use arrays (e.g., `extensions=[\"rs\", \"toml\"]`)\n\
               instead of one call per file type.\n\
             - Consolidate edits: batch multiple edit operations into one edit_file call.\n\
             - When you know the target file, use read_file(grep=...) instead of search_files.\n\
             - For small files, just read the whole file instead of grepping repeatedly.\n\
             - Never re-read a file you already read in this session.\n\
             - One broad search is better than many narrow ones."
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
    fn test_minimal_preamble() {
        let preamble = generate_preamble(&PreambleContext {
            has_tools: false,
            has_sub_agents: false,
            has_inform_user: false,
        });

        // Core sections always present
        assert!(preamble.contains("Quick-Query Agent Framework"));
        assert!(preamble.contains("Execution Model"));
        assert!(preamble.contains("Persistent Memory"));

        // Conditional sections absent
        assert!(!preamble.contains("Delegating to Sub-Agents"));
        assert!(!preamble.contains("Keeping the User Informed"));
        assert!(!preamble.contains("Tool Usage Efficiency"));
        assert!(!preamble.contains("Resourcefulness"));
    }

    #[test]
    fn test_preamble_with_tools() {
        let preamble = generate_preamble(&PreambleContext {
            has_tools: true,
            has_sub_agents: false,
            has_inform_user: false,
        });

        assert!(preamble.contains("Tool Usage Efficiency"));
        assert!(preamble.contains("Resourcefulness"));
        assert!(!preamble.contains("Delegating to Sub-Agents"));
        assert!(!preamble.contains("Keeping the User Informed"));
    }

    #[test]
    fn test_preamble_with_sub_agents() {
        let preamble = generate_preamble(&PreambleContext {
            has_tools: false,
            has_sub_agents: true,
            has_inform_user: false,
        });

        assert!(preamble.contains("Delegating to Sub-Agents"));
        assert!(preamble.contains("new_instance"));
        assert!(preamble.contains("Resourcefulness"));
        assert!(!preamble.contains("Tool Usage Efficiency"));
        assert!(!preamble.contains("Keeping the User Informed"));
    }

    #[test]
    fn test_preamble_with_inform_user() {
        let preamble = generate_preamble(&PreambleContext {
            has_tools: false,
            has_sub_agents: false,
            has_inform_user: true,
        });

        assert!(preamble.contains("Keeping the User Informed"));
        assert!(preamble.contains("inform_user"));
        assert!(!preamble.contains("Tool Usage Efficiency"));
        assert!(!preamble.contains("Delegating to Sub-Agents"));
    }

    #[test]
    fn test_full_preamble() {
        let preamble = generate_preamble(&PreambleContext {
            has_tools: true,
            has_sub_agents: true,
            has_inform_user: true,
        });

        // All sections present
        assert!(preamble.contains("Quick-Query Agent Framework"));
        assert!(preamble.contains("Execution Model"));
        assert!(preamble.contains("Persistent Memory"));
        assert!(preamble.contains("Delegating to Sub-Agents"));
        assert!(preamble.contains("Keeping the User Informed"));
        assert!(preamble.contains("Tool Usage Efficiency"));
        assert!(preamble.contains("Resourcefulness"));
    }
}
