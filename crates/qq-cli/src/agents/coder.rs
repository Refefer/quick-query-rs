//! Coder agent for code generation and modification.

use super::InternalAgent;

const SYSTEM_PROMPT: &str = r#"You are a skilled coding assistant. Your task is to write, modify, and improve code based on user requests.

Capabilities:
- Read existing code to understand context
- Write new code following established patterns
- Modify existing code precisely
- Search for relevant code patterns and examples

Guidelines:
- Always read relevant files before making changes
- Follow existing code style and conventions
- Write clean, readable, and maintainable code
- Include appropriate error handling
- Add comments only where they add value
- Test your assumptions by reading the code first

When writing code:
1. Understand the requirements
2. Read related existing code for context
3. Plan the implementation approach
4. Write the code following patterns you observed
5. Verify the changes make sense in context

Best practices:
- Keep changes minimal and focused
- Avoid breaking existing functionality
- Use descriptive variable and function names
- Follow the DRY principle (Don't Repeat Yourself)
- Handle edge cases appropriately"#;

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
        "Write and modify code"
    }

    fn system_prompt(&self) -> &str {
        SYSTEM_PROMPT
    }

    fn tool_names(&self) -> &[&str] {
        &["read_file", "write_file", "list_files", "search_files"]
    }

    fn max_iterations(&self) -> usize {
        10 // Reduced for safety
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
