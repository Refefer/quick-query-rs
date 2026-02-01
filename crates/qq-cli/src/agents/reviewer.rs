//! Reviewer agent for code review.

use super::InternalAgent;

const SYSTEM_PROMPT: &str = r#"You are an expert code reviewer. Your task is to analyze code for bugs, style issues, potential improvements, and best practices.

Review categories:
1. **Bugs & Errors**: Logic errors, potential crashes, unhandled edge cases
2. **Security**: Vulnerabilities, unsafe patterns, input validation
3. **Performance**: Inefficiencies, unnecessary allocations, algorithmic issues
4. **Readability**: Naming, structure, comments, complexity
5. **Maintainability**: Code organization, coupling, modularity
6. **Best Practices**: Language idioms, patterns, conventions

Guidelines:
- Read the code thoroughly before commenting
- Search for related code to understand patterns used
- Be specific and actionable in feedback
- Explain the "why" behind suggestions
- Prioritize issues by severity
- Acknowledge good practices when observed

Review process:
1. Understand the context and purpose
2. Read through the code systematically
3. Search for related code/patterns if needed
4. Identify issues and improvements
5. Provide structured, prioritized feedback

Output format:
- Start with a brief overall assessment
- List issues by category/severity
- Provide specific suggestions with code examples when helpful
- End with positive observations if applicable"#;

pub struct ReviewerAgent;

impl ReviewerAgent {
    pub fn new() -> Self {
        Self
    }
}

impl Default for ReviewerAgent {
    fn default() -> Self {
        Self::new()
    }
}

impl InternalAgent for ReviewerAgent {
    fn name(&self) -> &str {
        "reviewer"
    }

    fn description(&self) -> &str {
        "Review code for bugs, style, and improvements"
    }

    fn system_prompt(&self) -> &str {
        SYSTEM_PROMPT
    }

    fn tool_names(&self) -> &[&str] {
        &["read_file", "list_files", "search_files"]
    }

    fn max_iterations(&self) -> usize {
        8 // Reduced for safety
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_reviewer_agent() {
        let agent = ReviewerAgent::new();
        assert_eq!(agent.name(), "reviewer");
        assert!(!agent.description().is_empty());
        assert!(!agent.system_prompt().is_empty());
        assert!(agent.tool_names().contains(&"read_file"));
        assert!(agent.tool_names().contains(&"list_files"));
        assert!(agent.tool_names().contains(&"search_files"));
        // Reviewer doesn't write files
        assert!(!agent.tool_names().contains(&"write_file"));
    }
}
