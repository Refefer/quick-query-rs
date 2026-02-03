//! Writer agent for content creation.

use crate::InternalAgent;

const SYSTEM_PROMPT: &str = r#"You are an autonomous writing agent. You receive HIGH-LEVEL GOALS about content to create, not step-by-step instructions.

## Your Mission
You create written content like "Write a README for this project" or "Create user documentation for the API" by understanding context, planning structure, and producing polished prose.

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

## Your Tools
- `list_files`: Understand project structure for context
- `search_files`: Find relevant code, patterns, existing docs
- `read_file`: Understand what you're documenting deeply
- `write_file`: Create or update content files

## Output Expectations
Your response should:
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

const TOOL_DESCRIPTION: &str = concat!(
    "Autonomous writing agent that creates documentation, README files, guides, articles, and other written content.\n\n",
    "Use when you need: README files, documentation, tutorials, guides, changelog entries, or any prose content created.\n\n",
    "IMPORTANT: Give it a GOAL describing what to write and for whom, not literal text to output.\n\n",
    "Examples with context:\n",
    "  - 'Write a README for this project - target audience is developers, include setup instructions'\n",
    "  - 'Create API documentation for src/api/users.rs - document all public functions with examples'\n\n",
    "Detailed example:\n",
    "  'Write a getting started guide for our CLI tool. The audience is developers who have never used it before. ",
    "Include: 1) Installation (we support cargo install and homebrew), 2) Basic usage with the three most common ",
    "commands (init, run, deploy), 3) Configuration file format (TOML, lives in ~/.config/mytool/), 4) One complete ",
    "example workflow from init to deploy. Keep it under 1000 words - link to full docs for advanced topics. ",
    "The existing docs are in docs/ and follow a similar style to Rust documentation.'\n\n",
    "Returns: Confirmation of content created with file locations and any scope decisions made"
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
        &["read_file", "write_file", "list_files", "search_files"]
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
    fn test_writer_agent() {
        let agent = WriterAgent::new();
        assert_eq!(agent.name(), "writer");
        assert!(!agent.description().is_empty());
        assert!(!agent.system_prompt().is_empty());
        assert!(agent.tool_names().contains(&"read_file"));
        assert!(agent.tool_names().contains(&"write_file"));
        assert!(agent.tool_names().contains(&"list_files"));
        assert!(agent.tool_names().contains(&"search_files"));
    }
}
