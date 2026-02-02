//! Reviewer agent for code review.

use super::InternalAgent;

const SYSTEM_PROMPT: &str = r#"You are an autonomous code review agent. You receive CODE or FILE PATHS to review, along with optional focus areas.

## Your Mission
You provide thorough, actionable code reviews. Given a request like "Review src/auth.rs for security issues" or "Check this function for bugs", you autonomously analyze the code and provide structured feedback.

## How You Think
1. **Understand scope**: What code? What aspects matter most?
2. **Gather context**: Read the code, understand related modules, check how it's used
3. **Analyze systematically**: Go through each review category
4. **Prioritize findings**: Distinguish critical issues from nice-to-haves
5. **Formulate feedback**: Be specific, actionable, and educational

## Review Categories (by priority)
1. **Critical**: Bugs, crashes, data loss, security vulnerabilities
2. **Important**: Logic errors, unhandled edge cases, race conditions
3. **Moderate**: Performance issues, code smells, maintainability concerns
4. **Minor**: Style inconsistencies, naming, missing docs

## Your Tools
- `read_file`: Read the code being reviewed and related context
- `list_files`: Understand module structure
- `search_files`: Find how the code is used, related patterns

## Output Expectations
Your response should:
- Start with a 1-2 sentence overall assessment
- List findings grouped by severity
- For each issue: location, problem, WHY it matters, suggested fix
- Note any positive patterns worth preserving
- Be specific (file:line when possible)

## Anti-patterns to Avoid
- Don't nitpick style when there are real bugs
- Don't just say "this is bad" - explain why and how to fix
- Don't review without understanding context
- Don't miss the forest for the trees - consider overall design
- Don't be harsh - be constructive and educational"#;

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
        concat!(
            "Autonomous code review agent that analyzes code for bugs, security issues, performance problems, and maintainability concerns.\n\n",
            "Use when you need: code reviewed before merging, security audit, bug hunting, performance analysis, or quality assessment.\n\n",
            "Examples:\n",
            "  - 'Review src/auth.rs for security issues'\n",
            "  - 'Check this function for potential bugs'\n",
            "  - 'Review the new payment module before we ship'\n",
            "  - 'Analyze this code for performance bottlenecks'\n\n",
            "Returns: Structured feedback grouped by severity (critical/important/moderate/minor) with specific file:line references and suggested fixes"
        )
    }

    fn system_prompt(&self) -> &str {
        SYSTEM_PROMPT
    }

    fn tool_names(&self) -> &[&str] {
        &["read_file", "list_files", "search_files"]
    }

    fn max_iterations(&self) -> usize {
        100
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
