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

## Your Tools
- `find_files`: Discover project structure (recursive, gitignore-aware, extension filtering)
- `search_files`: Find patterns, usages, similar implementations across codebase
- `read_file`: Understand existing code deeply before modifying (supports line ranges, grep)
- `edit_file`: Make precise modifications - PREFERRED for changes
  - `replace`: Search/replace text (literal or regex)
  - `insert`: Add lines at specific positions
  - `delete`: Remove line ranges
  - `replace_lines`: Replace entire line ranges
- `write_file`: Create new files (use only when creating, not modifying)
- `move_file`: Move or rename files and directories (useful for refactoring)
- `create_directory`: Create new directories (with recursive parent creation)
- `rm_file`: Remove a file
- `rm_directory`: Remove a directory (supports recursive deletion)

## Tool Usage Guidelines
- ALWAYS use `edit_file` for modifying existing files (more precise, shows diff)
- Use `write_file` only for creating new files
- Use `find_files` instead of guessing file locations
- Search first, read second, modify third

## Output Expectations
Your response should:
- Confirm what you implemented
- Note any design decisions you made
- List files created or modified
- Highlight anything the caller should verify or test

## Keeping Users Informed
Use `inform_user` to notify the user about your progress WITHOUT ending your turn.
The user sees messages immediately while you continue working. This builds trust and transparency.

**When to use inform_user:**
- When gathering context: "Reading existing auth module to understand patterns..."
- When you discover relevant code: "Found existing validation helpers we can reuse..."
- When you find issues: "Note: the current implementation has a potential race condition..."
- Before major changes: "Implementing the new validation logic in auth.rs..."
- For multi-file updates: "This change affects 3 files - updating them consistently..."
- When making design decisions: "Using the builder pattern to match existing code style..."
- When something unexpected happens: "The function signature differs from expected - adapting approach..."
- **When completing a phase or task in a plan**: "Phase 1 complete: auth module scaffolding done. Moving to Phase 2..."

**Executing plans:** When you are given a multi-step plan, use `inform_user` to report completion of each phase or task, then **keep going** with the next step. Do NOT stop and wait for confirmation between steps — execute the full plan continuously, using `inform_user` to keep the user updated on progress.

**Examples:**
- inform_user({"message": "Reading src/auth.rs to understand the current structure..."})
- inform_user({"message": "Good news - found existing error types we can extend..."})
- inform_user({"message": "Updating auth.rs, then propagating changes to 2 dependent files..."})
- inform_user({"message": "Step 3 complete: validation logic added. Proceeding to step 4..."})

## Avoiding Redundant Tool Calls
NEVER call the same tool multiple times when a single call would suffice. Before making a tool call, check if you already have the information from a previous call.

**find_files consolidation:**
- Use `extensions` array instead of multiple calls:
  BAD:  find_files(extensions=["rs"]) + find_files(extensions=["toml"])
  GOOD: find_files(extensions=["rs", "toml"])
- Search broadly then filter mentally, rather than making many narrow searches

**search_files consolidation:**
- Use regex alternation instead of multiple searches:
  BAD:  search_files(pattern="struct Config") + search_files(pattern="impl Config")
  GOOD: search_files(pattern="(struct|impl) Config")

**read_file efficiency:**
- Never re-read a file you already read in this session
- Use `grep` to extract specific information: read_file(path="lib.rs", grep="pub fn")

## Anti-patterns to Avoid
- NEVER write code without first reading related existing code
- Don't invent new patterns when the codebase has established ones
- Don't over-engineer - implement what was asked
- Don't leave placeholder code or TODOs
- Don't make unrelated "improvements" while you're there
- Don't call the same tool with the same arguments twice - use results you already have"#;

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
        &["read_file", "edit_file", "write_file", "move_file", "create_directory", "rm_file", "rm_directory", "find_files", "search_files"]
    }

    fn tool_limits(&self) -> Option<HashMap<String, usize>> {
        let mut limits = HashMap::new();
        limits.insert("write_file".to_string(), 20);
        limits.insert("edit_file".to_string(), 50);
        limits.insert("move_file".to_string(), 20);
        limits.insert("create_directory".to_string(), 10);
        limits.insert("rm_file".to_string(), 20);
        limits.insert("rm_directory".to_string(), 10);
        limits.insert("find_files".to_string(), 10);
        Some(limits)
    }

    fn max_turns(&self) -> usize {
        100
    }

    fn tool_description(&self) -> &str {
        TOOL_DESCRIPTION
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
        assert!(agent.tool_names().contains(&"create_directory"));
        assert!(agent.tool_names().contains(&"rm_file"));
        assert!(agent.tool_names().contains(&"rm_directory"));
        assert!(agent.tool_names().contains(&"find_files"));
        assert!(agent.tool_names().contains(&"search_files"));
    }

    #[test]
    fn test_coder_tool_limits() {
        let agent = CoderAgent::new();
        let limits = agent.tool_limits().expect("coder should have tool limits");
        assert_eq!(limits.get("write_file"), Some(&20));
        assert_eq!(limits.get("edit_file"), Some(&50));
        assert_eq!(limits.get("move_file"), Some(&20));
        assert_eq!(limits.get("create_directory"), Some(&10));
        assert_eq!(limits.get("rm_file"), Some(&20));
        assert_eq!(limits.get("rm_directory"), Some(&10));
        assert_eq!(limits.get("find_files"), Some(&10));
    }
}
