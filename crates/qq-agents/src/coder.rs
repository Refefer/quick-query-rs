//! Coder agent for code generation and modification.

use std::collections::HashMap;

use crate::InternalAgent;

const SYSTEM_PROMPT: &str = r#"You are an autonomous coding agent. You receive HIGH-LEVEL GOALS about code to write or modify, not step-by-step instructions.

## Your Mission
You implement features like "Add input validation to the login form" or "Refactor the config module to support multiple profiles" by autonomously understanding context, planning, and writing code.

## How You Think
1. **Understand the goal**: What functionality is being requested?
2. **Gather context**: Read existing code to understand patterns, conventions, dependencies
3. **Plan the approach**: What files need to change? What's the cleanest design?
4. **Implement**: Write code that fits naturally into the existing codebase
5. **Verify**: Re-read to ensure changes are correct and complete

## Implementation Strategy
- **Context first**: ALWAYS read related code before writing anything
- **Follow patterns**: Match existing style, naming, error handling approaches
- **Minimal changes**: Do exactly what's needed, no more
- **Incremental**: For complex tasks, build up in logical steps

## Output Expectations
Your response should:
- Confirm what you implemented
- Note any design decisions you made
- List files created or modified
- Highlight anything the caller should verify or test

## Anti-patterns to Avoid
- NEVER write code without first reading related existing code
- Don't invent new patterns when the codebase has established ones
- Don't over-engineer - implement what was asked
- Don't leave placeholder code or TODOs
- Don't make unrelated "improvements" while you're there"#;

pub struct CoderAgent;

impl CoderAgent {
    pub fn new() -> Self {
        Self
    }
}

impl Default for CoderAgent {
    fn default() -> Self {
        Self::new()
    }
}

const COMPACT_PROMPT: &str = r#"Summarize this coding session so it can continue effectively with reduced context. Preserve:
1. Files modified or created (with full paths) and what changes were made to each
2. Code patterns and conventions discovered in the existing codebase
3. The original coding goal and any sub-tasks identified
4. Design decisions made and their rationale
5. Errors encountered during implementation and how they were resolved
6. Any remaining work or files still needing modification

Focus on file paths, concrete changes, and architectural decisions. Include key code snippets only if they represent patterns to follow."#;

const TOOL_DESCRIPTION: &str = concat!(
    "Autonomous coding agent that implements features, fixes bugs, and modifies code by understanding context and following existing patterns.\n\n",
    "Use when you need:\n",
    "  - New features implemented\n",
    "  - Bugs fixed\n",
    "  - Code refactored\n",
    "  - Files created, modified, or deleted\n\n",
    "IMPORTANT: Give it a GOAL describing what you want built or changed, not step-by-step instructions.\n\n",
    "Examples:\n",
    "  - 'Add input validation to src/components/LoginForm.tsx - email must be valid format, password min 8 chars'\n",
    "  - 'Implement retry with exponential backoff in src/api/client.rs - max 3 retries, start at 100ms'\n\n",
    "Detailed example:\n",
    "  'Implement a caching layer for our API client in src/api/. We make repeated calls to /users/:id and /products/:id ",
    "that rarely change. Add an in-memory LRU cache with configurable max size (default 1000 entries) and TTL (default 5 ",
    "minutes). Cache keys should be the full URL including query params. Respect Cache-Control headers from responses.'\n\n",
    "Returns: Confirmation of changes with list of modified files and any design decisions made\n\n",
    "DO NOT:\n",
    "  - Use for read-only exploration (use explore agent)\n",
    "  - Use for documentation writing (use writer agent)\n",
    "  - Use for code review without changes (use reviewer agent)\n"
);

impl InternalAgent for CoderAgent {
    fn name(&self) -> &str {
        "coder"
    }

    fn description(&self) -> &str {
        "Writes and modifies code following existing patterns"
    }

    fn system_prompt(&self) -> &str {
        SYSTEM_PROMPT
    }

    fn tool_names(&self) -> &[&str] {
        &["read_file", "edit_file", "write_file", "move_file", "copy_file", "create_directory", "rm_file", "rm_directory", "find_files", "search_files", "bash", "mount_external", "update_my_task"]
    }

    fn tool_limits(&self) -> Option<HashMap<String, usize>> {
        None
    }

    fn max_turns(&self) -> usize {
        100
    }

    fn tool_description(&self) -> &str {
        TOOL_DESCRIPTION
    }

    fn compact_prompt(&self) -> &str {
        COMPACT_PROMPT
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_coder_agent() {
        let agent = CoderAgent::new();
        assert_eq!(agent.name(), "coder");
        assert!(!agent.description().is_empty());
        assert!(!agent.system_prompt().is_empty());
        assert!(agent.tool_names().contains(&"read_file"));
        assert!(agent.tool_names().contains(&"edit_file"));
        assert!(agent.tool_names().contains(&"write_file"));
        assert!(agent.tool_names().contains(&"move_file"));
        assert!(agent.tool_names().contains(&"copy_file"));
        assert!(agent.tool_names().contains(&"create_directory"));
        assert!(agent.tool_names().contains(&"rm_file"));
        assert!(agent.tool_names().contains(&"rm_directory"));
        assert!(agent.tool_names().contains(&"find_files"));
        assert!(agent.tool_names().contains(&"search_files"));
        assert!(agent.tool_names().contains(&"update_my_task"));
        assert!(agent.tool_names().contains(&"bash"));
        assert!(agent.tool_names().contains(&"mount_external"));
    }

    #[test]
    fn test_coder_tool_limits() {
        let agent = CoderAgent::new();
        assert!(agent.tool_limits().is_none());
    }
}
