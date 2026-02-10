//! Writer agent for content creation.

use std::collections::HashMap;

use crate::InternalAgent;

const SYSTEM_PROMPT: &str = r#"You are an autonomous writing agent. You receive HIGH-LEVEL GOALS about content to create, not step-by-step instructions.

## Your Mission
You create written content like "Write a README for this project" or "Create user documentation for the API" by understanding context, planning structure, and producing polished prose.

## Output Destination (CRITICAL)
The caller's prompt MUST specify where to put the output. Look for:
- Explicit file path: "Write to README.md", "Save to docs/guide.md"
- File type direction: "Write to an appropriate file"
- Return directive: "Return as response", "Return as summary"

Your behavior:
1. If destination is specified → follow it exactly
2. If destination is unclear or missing → STOP and respond asking where the output should go

NEVER assume a destination. If the prompt says "Write a README" without specifying where, ask:
"Where should I write the README? Please specify a file path (e.g., README.md) or tell me to return it as a response."

## How You Think
1. **Understand the audience**: Who will read this? What do they need to know?
2. **Gather context**: Read existing code/docs to understand what you're writing about
3. **Plan the structure**: Outline before writing - what sections, what flow?
4. **Write with purpose**: Every paragraph should serve the reader's needs
5. **Review and refine**: Re-read to ensure clarity, accuracy, and completeness

## Writing Strategies
- **Documentation**: Technical accuracy, clear examples, progressive disclosure
- **README files**: Quick start first, details later, make it scannable
- **Articles/Guides**: Hook the reader, build understanding step by step
- **API docs**: Consistent format, show don't tell, cover edge cases
- **Changelog/Release notes**: What changed, why it matters, migration steps

## Output Expectations
Your response should:
- Confirm the output destination was followed (or note that clarification was requested)
- Confirm what you created
- Note the target audience and scope decisions
- List files created or modified
- Suggest what to review or verify

## Quality Principles
- **Accuracy**: Never document features that don't exist
- **Clarity**: Simple words, short sentences, clear structure
- **Completeness**: Cover what readers need, skip what they don't
- **Consistency**: Match existing docs style when extending

## Anti-patterns to Avoid
- NEVER write docs without reading the code/content you're documenting
- Don't use jargon without explaining it (unless audience is experts)
- Don't bury important information - lead with what matters
- Don't write walls of text - use headings, lists, code blocks
- Don't be verbose when concise will do
- Don't add placeholder content or TODOs"#;

pub struct WriterAgent;

impl WriterAgent {
    pub fn new() -> Self {
        Self
    }
}

impl Default for WriterAgent {
    fn default() -> Self {
        Self::new()
    }
}

const COMPACT_PROMPT: &str = r#"Summarize this writing session so it can continue effectively with reduced context. Preserve:
1. Documents created or edited (with full file paths)
2. Content structure decisions (outline, sections, organization)
3. Audience and tone choices made
4. Key content already written (section summaries, not full text)
5. Source material referenced and key facts incorporated
6. Remaining sections or content still to be written

Focus on file paths, structural decisions, and what content has been produced. Omit raw source material - keep only how it informed the writing."#;

const TOOL_DESCRIPTION: &str = concat!(
    "Autonomous agent for creating written content: documentation, README files, guides, and articles.\n\n",
    "Use when you need:\n",
    "  - README files created or updated\n",
    "  - Documentation written\n",
    "  - Tutorials or guides created\n",
    "  - Changelog entries generated\n\n",
    "IMPORTANT: Give it a GOAL describing what to write and for whom, not literal text to output.\n\n",
    "OUTPUT DESTINATION (REQUIRED):\n",
    "  - 'Write to <path>' - creates a file at the specified location\n",
    "  - 'Return as response' - returns content directly without writing to disk\n\n",
    "Examples:\n",
    "  - 'Write a README for this project. Save to README.md'\n",
    "  - 'Create API docs for src/api/users.rs. Return as response for review.'\n\n",
    "Detailed example:\n",
    "  'Write a getting started guide for our CLI tool. The audience is developers who have never used it before. ",
    "Include installation, basic usage, and configuration. Save to docs/getting-started.md'\n\n",
    "Returns: Confirmation of content created with file locations and any scope decisions made\n\n",
    "DO NOT:\n",
    "  - Use for code changes (use coder agent)\n",
    "  - Use for code review (use reviewer agent)\n",
    "  - Use for web research (use researcher agent)\n"
);

impl InternalAgent for WriterAgent {
    fn name(&self) -> &str {
        "writer"
    }

    fn description(&self) -> &str {
        "Creates documentation, READMEs, guides, and other written content"
    }

    fn system_prompt(&self) -> &str {
        SYSTEM_PROMPT
    }

    fn tool_names(&self) -> &[&str] {
        &["read_file", "write_file", "edit_file", "copy_file", "create_directory", "find_files", "search_files", "bash", "mount_external", "update_my_task"]
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
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_writer_agent() {
        let agent = WriterAgent::new();
        assert_eq!(agent.name(), "writer");
        assert!(!agent.description().is_empty());
        assert!(!agent.system_prompt().is_empty());
        assert!(agent.tool_names().contains(&"read_file"));
        assert!(agent.tool_names().contains(&"write_file"));
        assert!(agent.tool_names().contains(&"edit_file"));
        assert!(agent.tool_names().contains(&"copy_file"));
        assert!(agent.tool_names().contains(&"create_directory"));
        assert!(agent.tool_names().contains(&"find_files"));
        assert!(agent.tool_names().contains(&"search_files"));
        assert!(agent.tool_names().contains(&"bash"));
        assert!(agent.tool_names().contains(&"mount_external"));
        assert!(agent.tool_names().contains(&"update_my_task"));
    }

    #[test]
    fn test_writer_tool_limits() {
        let agent = WriterAgent::new();
        assert!(agent.tool_limits().is_none());
    }
}
