//! Coder agent for code generation and modification.

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

## Your Tools
- `list_files`: Understand project structure
- `search_files`: Find relevant code, patterns, similar implementations
- `read_file`: Understand existing code deeply before modifying
- `write_file`: Create or update files (only after understanding context)

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

impl InternalAgent for CoderAgent {
    fn name(&self) -> &str {
        "coder"
    }

    fn description(&self) -> &str {
        concat!(
            "Autonomous coding agent that implements features, fixes bugs, and modifies code by understanding context and following existing patterns.\n\n",
            "Use when you need: new features implemented, bugs fixed, code refactored, files created, or existing code modified.\n\n",
            "IMPORTANT: Always provide full context in your prompt so the agent understands the task.\n\n",
            "Examples with context:\n",
            "  - 'Add input validation to src/components/LoginForm.tsx - email must be valid format, password min 8 chars'\n",
            "  - 'Implement retry with exponential backoff in src/api/client.rs - max 3 retries, start at 100ms'\n\n",
            "Detailed example:\n",
            "  'Implement a caching layer for our API client in src/api/. We make repeated calls to /users/:id and /products/:id ",
            "that rarely change. Add an in-memory LRU cache with configurable max size (default 1000 entries) and TTL (default 5 ",
            "minutes). Cache keys should be the full URL including query params. Respect Cache-Control headers from responses - ",
            "honor max-age and no-store directives. Add a way to manually invalidate specific keys or clear the whole cache. ",
            "The existing code uses reqwest and tokio. Make sure the cache is thread-safe. Add cache hit/miss metrics that ",
            "integrate with our existing metrics in src/telemetry.rs which uses the metrics crate.'\n\n",
            "Returns: Confirmation of changes with list of modified files and any design decisions made"
        )
    }

    fn system_prompt(&self) -> &str {
        SYSTEM_PROMPT
    }

    fn tool_names(&self) -> &[&str] {
        &["read_file", "write_file", "list_files", "search_files"]
    }

    fn max_iterations(&self) -> usize {
        100
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
        assert!(agent.tool_names().contains(&"write_file"));
        assert!(agent.tool_names().contains(&"list_files"));
        assert!(agent.tool_names().contains(&"search_files"));
    }
}
