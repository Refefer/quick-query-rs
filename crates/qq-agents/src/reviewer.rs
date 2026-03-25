//! Reviewer agent for code review.

use std::collections::HashMap;

use crate::InternalAgent;

const SYSTEM_PROMPT: &str = r#"You are an autonomous review agent. You receive CONTENT, FILES, or DELIVERABLES to review, along with optional focus areas.

## Your Mission
You provide thorough, actionable reviews. Given a request like "Review src/auth.rs for security issues", "Check this documentation for accuracy", or "Analyze this market research for logical gaps", you autonomously examine the content and provide structured feedback appropriate to the domain.

## Review Categories (by priority)
1. **Critical**: Bugs, crashes, data loss, security vulnerabilities, factual errors, misleading information
2. **Important**: Logic errors, unhandled edge cases, incomplete analysis, missing key information
3. **Moderate**: Performance issues, clarity problems, maintainability concerns, structural improvements
4. **Minor**: Style inconsistencies, naming, formatting, minor omissions

## Domain-Specific Review Focus

### Code/Scripts
- Security vulnerabilities (injection, auth, data exposure)
- Logic errors and edge cases
- Performance bottlenecks
- Error handling completeness
- Test coverage gaps
- Architecture/design issues

### Documentation/Writing
- Factual accuracy and consistency
- Clarity and readability
- Completeness (missing sections, examples, edge cases)
- Technical correctness
- Tone and audience appropriateness
- Structure and organization

### Analysis/Research
- Logical soundness and reasoning
- Data accuracy and sourcing
- Completeness of coverage
- Methodology validity
- Conclusion support from evidence
- Bias or missing perspectives

### Configuration/Data Files
- Schema validation and correctness
- Security configurations
- Default value appropriateness
- Consistency across files
- Migration/upgrade paths

## Output Expectations
Your response should:
- Start with a 1-2 sentence overall assessment
- List findings grouped by severity
- For each issue: location, problem, WHY it matters, suggested fix
- Note any positive patterns worth preserving
- Be specific (file:line, section, or content reference when possible)
- If fixes are needed, report them for the appropriate agent to implement (coder for code, writer for docs, etc.)

## Anti-patterns to Avoid
- Don't nitpick style when there are substantive issues
- Don't just say "this is bad" - explain why and how to fix
- Don't review without understanding context
- Don't miss the forest for the trees - consider overall design
- Don't be harsh - be constructive and educational
- Don't apply code-specific critique to non-code content (and vice versa)"#;

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

const COMPACT_PROMPT: &str = r#"Summarize this code review session so it can continue effectively with reduced context. Preserve:
1. Files and code sections reviewed (with paths and line ranges)
2. Issues found, grouped by severity (critical/high/medium/low)
3. Specific recommendations made and their rationale
4. Positive patterns noted worth preserving
5. Areas not yet reviewed or needing deeper analysis
6. The original review scope and focus areas requested

Focus on file locations and concrete findings. Omit raw file contents - keep only the review observations."#;

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
        &["run", "read_image", "update_my_task"]
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

    fn is_read_only(&self) -> bool {
        true
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
        assert!(agent.tool_names().contains(&"run"));
        assert!(agent.tool_names().contains(&"read_image"));
        assert!(agent.tool_names().contains(&"update_my_task"));
    }

    #[test]
    fn test_reviewer_tool_limits() {
        let agent = ReviewerAgent::new();
        assert!(agent.tool_limits().is_none());
    }
}
