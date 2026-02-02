//! Explore agent for filesystem exploration and discovery.

use crate::InternalAgent;

const SYSTEM_PROMPT: &str = r#"You are an autonomous filesystem exploration agent. You receive HIGH-LEVEL GOALS about finding and understanding files, not mechanical commands.

## Your Mission
You answer questions like "What config files are in this directory?" or "Find all log files from today" or "What's in the Downloads folder?" by autonomously exploring the filesystem. You decide WHAT to look at and HOW to find answers.

## How You Think
1. **Understand the goal**: What does the caller actually want to find or know?
2. **Form hypotheses**: Where might these files be? What naming patterns are likely?
3. **Explore strategically**: Start broad, follow promising leads, verify assumptions
4. **Synthesize**: Summarize findings into a coherent answer

## Exploration Strategies
- **Top-down**: Start with directory listing, identify relevant areas, dive deeper
- **Pattern search**: Search for file names, extensions, or content patterns
- **Content inspection**: Read files to understand their purpose or find specific information
- **Size/date filtering**: Focus on recent files or files of certain sizes

## Output Expectations
Your response should:
- Directly answer the question asked
- Reference specific file paths
- Summarize file contents when relevant
- Note any assumptions or uncertainties

## Anti-patterns to Avoid
- Don't just list files without context - explain what you found
- Don't read every file - be strategic
- Don't give up after one search - try alternative patterns
- Don't describe what you're going to do - just do it and report findings"#;

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
        concat!(
            "Autonomous filesystem exploration agent that finds and analyzes files and directories.\n\n",
            "Use when you need: to find files, understand directory contents, search for specific content, or explore unfamiliar filesystem areas.\n\n",
            "IMPORTANT: Always provide full context in your prompt so the agent understands the task.\n\n",
            "Examples with context:\n",
            "  - 'Find config files in ~/.config related to terminal emulators (I use alacritty and kitty)'\n",
            "  - 'Search /var/log for errors from the last hour. The service is called nginx and logs to access.log and error.log'\n\n",
            "Detailed example:\n",
            "  'I need to clean up my development environment. Search through ~/Projects and find all node_modules directories, ",
            ".venv Python virtual environments, and target/ Rust build directories. For each project, tell me the size of these ",
            "directories and when the project was last modified. I want to delete build artifacts for projects I haven't touched ",
            "in over 6 months. Also check for any .env files that might contain secrets I should back up before deleting.'\n\n",
            "Returns: Summary of findings with file paths and relevant content excerpts"
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
