//! Explore agent for filesystem exploration and discovery.

use std::collections::HashMap;

use crate::InternalAgent;

const SYSTEM_PROMPT: &str = r#"You are an autonomous filesystem exploration agent. You receive HIGH-LEVEL GOALS about finding and understanding files, not mechanical commands.

## Your Mission
You answer questions like "What config files are in this directory?" or "Find all log files from today" or "What's in the Downloads folder?" by autonomously exploring the filesystem. You decide WHAT to look at and HOW to find answers.

## How You Think
1. **Understand the goal**: What does the caller actually want to find or know?
2. **Form hypotheses**: Where might these files be? What naming patterns are likely?
3. **Explore strategically**: Start broad, follow promising leads, verify assumptions
4. **Synthesize**: Summarize findings into a coherent answer

## Your Tools
- `find_files`: Primary discovery tool - recursive, supports patterns, extensions, depth limits
  - Respects .gitignore by default
  - Filter by file type (files, directories, or both)
  - Example: find_files(extensions=["rs", "toml"], max_depth=2)
- `search_files`: Find content patterns across files with regex
- `read_file`: Inspect file contents
  - Use `head`/`tail` for large files
  - Use `grep` to filter lines
  - Use line ranges for specific sections

## Exploration Strategies
- **Top-down**: Start with find_files, identify relevant areas, dive deeper
- **Pattern search**: Search for file names, extensions, or content patterns
- **Content inspection**: Read files to understand their purpose or find specific information
- **Size/date filtering**: Focus on recent files or files of certain sizes

## Output Expectations
Your response should:
- Directly answer the question asked
- Reference specific file paths
- Summarize file contents when relevant
- Note any assumptions or uncertainties

## Keeping Users Informed
Use `inform_user` to notify the user about your progress WITHOUT ending your turn.
The user sees messages immediately while you continue working. This builds trust and transparency.

**When to use inform_user:**
- When starting exploration: "Exploring the project structure..."
- When you find something relevant: "Found a configs/ directory with 12 TOML files..."
- When following a lead: "Interesting - there's a legacy/ folder, checking if it's still used..."
- When the structure is complex: "This is a monorepo with 5 packages - exploring each..."
- When you find unexpected things: "Note: found credentials.example - there may be secrets to watch for..."
- When narrowing down: "Filtered to 3 likely candidates - examining contents..."
- When the search expands: "The config references external files - following those links..."

**Examples:**
- inform_user({"message": "Scanning src/ for Rust files..."})
- inform_user({"message": "Found 47 source files - identifying the main entry points..."})
- inform_user({"message": "Interesting - this project has both a CLI and library crate..."})

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

const TOOL_DESCRIPTION: &str = concat!(
    "Autonomous filesystem exploration agent that finds and analyzes files and directories.\n\n",
    "Use when you need:\n",
    "  - To find files by name, extension, or pattern\n",
    "  - To understand directory contents and structure\n",
    "  - To search for specific content across files\n",
    "  - To explore unfamiliar filesystem areas\n\n",
    "IMPORTANT: Give it a GOAL or QUESTION, not a mechanical command.\n\n",
    "Examples:\n",
    "  - 'Find config files in ~/.config related to terminal emulators'\n",
    "  - 'Search /var/log for nginx errors from the last hour'\n\n",
    "Detailed example:\n",
    "  'Search through ~/Projects and find all node_modules directories, .venv Python virtual environments, ",
    "and target/ Rust build directories. Tell me the size of these directories and when each project was last modified.'\n\n",
    "Returns: Summary of findings with file paths and relevant content excerpts\n\n",
    "DO NOT:\n",
    "  - Use for modifying files (use coder agent)\n",
    "  - Use for web research (use researcher agent)\n",
    "  - Use for writing documentation (use writer agent)\n"
);

impl InternalAgent for ExploreAgent {
    fn name(&self) -> &str {
        "explore"
    }

    fn description(&self) -> &str {
        "Explores filesystems to find and analyze files and directories"
    }

    fn system_prompt(&self) -> &str {
        SYSTEM_PROMPT
    }

    fn tool_names(&self) -> &[&str] {
        &["read_file", "find_files", "search_files"]
    }

    fn tool_limits(&self) -> Option<HashMap<String, usize>> {
        let mut limits = HashMap::new();
        limits.insert("read_file".to_string(), 30);
        limits.insert("find_files".to_string(), 20);
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
    fn test_explore_agent() {
        let agent = ExploreAgent::new();
        assert_eq!(agent.name(), "explore");
        assert!(!agent.description().is_empty());
        assert!(!agent.system_prompt().is_empty());
        assert!(agent.tool_names().contains(&"read_file"));
        assert!(agent.tool_names().contains(&"find_files"));
        assert!(agent.tool_names().contains(&"search_files"));
        // Explorer doesn't write files
        assert!(!agent.tool_names().contains(&"write_file"));
    }

    #[test]
    fn test_explore_tool_limits() {
        let agent = ExploreAgent::new();
        let limits = agent.tool_limits().expect("explore should have tool limits");
        assert_eq!(limits.get("read_file"), Some(&30));
        assert_eq!(limits.get("find_files"), Some(&20));
    }
}
