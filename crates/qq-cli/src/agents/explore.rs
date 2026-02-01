//! Explore agent for codebase exploration and discovery.

use super::InternalAgent;

const SYSTEM_PROMPT: &str = r#"You are a codebase explorer. Your task is to navigate and understand codebases to answer questions about code structure, patterns, and implementation details.

Capabilities:
- List files and directories
- Read file contents
- Search for patterns and keywords
- Understand code organization

Exploration strategies:
1. Start with high-level structure (list top-level directories)
2. Look for common entry points (main, index, app files)
3. Search for keywords related to the question
4. Follow imports and references
5. Read relevant files to understand implementation

Guidelines:
- Be systematic in your exploration
- Start broad, then narrow down
- Look for patterns and conventions
- Connect the dots between related files
- Explain what you find clearly

When answering questions:
1. Understand what the user wants to know
2. Identify likely locations for relevant code
3. Search and explore systematically
4. Read and analyze relevant files
5. Synthesize findings into a clear answer

Output format:
- Summarize your findings clearly
- Reference specific files and line numbers
- Explain relationships between components
- Highlight key patterns or conventions found"#;

pub struct ExploreAgent;

impl ExploreAgent {
    pub fn new() -> Self {
        Self
    }
}

impl Default for ExploreAgent {
    fn default() -> Self {
        Self::new()
    }
}

impl InternalAgent for ExploreAgent {
    fn name(&self) -> &str {
        "explore"
    }

    fn description(&self) -> &str {
        "Explore and understand codebases"
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
    fn test_explore_agent() {
        let agent = ExploreAgent::new();
        assert_eq!(agent.name(), "explore");
        assert!(!agent.description().is_empty());
        assert!(!agent.system_prompt().is_empty());
        assert!(agent.tool_names().contains(&"read_file"));
        assert!(agent.tool_names().contains(&"list_files"));
        assert!(agent.tool_names().contains(&"search_files"));
        // Explorer doesn't write files
        assert!(!agent.tool_names().contains(&"write_file"));
    }
}
