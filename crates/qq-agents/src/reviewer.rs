//! Reviewer agent for code review.

use std::collections::HashMap;

use crate::InternalAgent;

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
- `find_files`: Discover module structure (recursive, gitignore-aware)
- `search_files`: Find how the code is used, related patterns

## Output Expectations
Your response should:
- Start with a 1-2 sentence overall assessment
- List findings grouped by severity
- For each issue: location, problem, WHY it matters, suggested fix
- Note any positive patterns worth preserving
- Be specific (file:line when possible)

## Keeping Users Informed
Use `inform_user` to notify the user about your progress WITHOUT ending your turn.
The user sees messages immediately while you continue working. This builds trust and transparency.

**When to use inform_user:**
- When starting review: "Beginning security review of auth.rs..."
- When you spot issues early: "Found a potential SQL injection risk - investigating..."
- When the code looks good: "Authentication flow looks solid so far..."
- When checking dependencies: "Analyzing how this module interacts with user input..."
- When you find patterns: "This codebase uses prepared statements consistently - good practice..."
- When severity is clear: "Critical issue found - stopping to document before continuing..."
- When review scope expands: "This touches 3 other modules - expanding review scope..."

**Examples:**
- inform_user({"message": "Reviewing input validation in the API handlers..."})
- inform_user({"message": "Spotted potential issue with session handling - documenting..."})
- inform_user({"message": "Good news - no obvious security issues in the auth flow..."})

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

const TOOL_DESCRIPTION: &str = concat!(
    "Autonomous code review agent that analyzes code for bugs, security issues, and quality concerns.\n\n",
    "Use when you need:\n",
    "  - Code reviewed before merging\n",
    "  - Security audit performed\n",
    "  - Bug hunting in specific code\n",
    "  - Performance or quality assessment\n\n",
    "IMPORTANT: Give it CODE or a FILE PATH and ask for specific feedback.\n\n",
    "Examples:\n",
    "  - 'Review src/auth.rs for security issues - this handles JWT validation'\n",
    "  - 'Check parse_config in src/config.rs - users report crashes with malformed TOML'\n\n",
    "Detailed example:\n",
    "  'Security review of src/api/upload.rs before production. This handles user file uploads. ",
    "Check for: path traversal, filename sanitization, content-type validation, file size limits.'\n\n",
    "Returns: Structured feedback grouped by severity with file:line references and suggested fixes\n\n",
    "DO NOT:\n",
    "  - Use for implementing fixes (use coder agent after review)\n",
    "  - Use for filesystem exploration (use explore agent)\n",
    "  - Use for documentation (use writer agent)\n"
);

impl InternalAgent for ReviewerAgent {
    fn name(&self) -> &str {
        "reviewer"
    }

    fn description(&self) -> &str {
        "Reviews code for bugs, security issues, and quality concerns"
    }

    fn system_prompt(&self) -> &str {
        SYSTEM_PROMPT
    }

    fn tool_names(&self) -> &[&str] {
        &["read_file", "find_files", "search_files"]
    }

    fn tool_limits(&self) -> Option<HashMap<String, usize>> {
        let mut limits = HashMap::new();
        limits.insert("read_file".to_string(), 50);
        limits.insert("find_files".to_string(), 15);
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
    fn test_reviewer_agent() {
        let agent = ReviewerAgent::new();
        assert_eq!(agent.name(), "reviewer");
        assert!(!agent.description().is_empty());
        assert!(!agent.system_prompt().is_empty());
        assert!(agent.tool_names().contains(&"read_file"));
        assert!(agent.tool_names().contains(&"find_files"));
        assert!(agent.tool_names().contains(&"search_files"));
        // Reviewer doesn't write files
        assert!(!agent.tool_names().contains(&"write_file"));
    }

    #[test]
    fn test_reviewer_tool_limits() {
        let agent = ReviewerAgent::new();
        let limits = agent.tool_limits().expect("reviewer should have tool limits");
        assert_eq!(limits.get("read_file"), Some(&50));
        assert_eq!(limits.get("find_files"), Some(&15));
    }
}
