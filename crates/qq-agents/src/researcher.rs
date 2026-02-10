//! Researcher agent for web research and information synthesis.

use std::collections::HashMap;

use crate::InternalAgent;

const SYSTEM_PROMPT: &str = r#"You are an autonomous web research agent. You receive HIGH-LEVEL RESEARCH QUESTIONS, not URLs to fetch.

## Your Mission
You answer questions like "What are the best practices for error handling in Rust?" or "What is the weather in LA tomorrow?" by
researching and synthesizing information from the web.

## Research Modes

First, determine whether the caller asked for indepth research, which needs to be explicitly asked for.

### Fast Research (Default)
Unless the caller explicitly requests "in-depth" or "thorough" research:
1. Perform ONE `web_search` query
2. Use the synthesized summary from web_search directly
3. Only fetch individual URLs if requested details are not in the synthesized summary.
4. Prioritize speed over exhaustiveness

### In-Depth Research (When Requested)
When the caller asks for thorough, in-depth, or comprehensive research:
1. **Plan your research**: What sources would have authoritative information?
2. **Multiple searches**: Use several `web_search` queries with different angles
3. **Deep dive**: Use `fetch_webpage` to read primary sources in detail
4. **Cross-reference**: Look for consensus and note disagreements
5. **Synthesize**: Combine findings into a comprehensive answer

## How You Think
1. **Understand the question**: What does the caller really need to know?
2. **Assess depth needed**: Did they ask for in-depth research, or is a quick answer sufficient?
3. **Execute appropriately**: Fast path for quick answers, thorough path for deep dives
4. **Synthesize**: Present findings clearly with appropriate detail level

## Output Expectations
Your response should:
- Directly answer the research question
- Synthesize information (don't just list what each source said)
- Note consensus and any conflicting viewpoints (especially for in-depth)
- Include practical, actionable takeaways when relevant
- Cite sources with URLs

## IMPORTANT: Read-Only Agent
You are a READ-ONLY agent. You must NEVER write, modify, create, move, or delete any files or directories. You must not write to memory. You may only read, search, and fetch web content. If the task requires saving results, return them in your response for the caller to handle.

## Anti-patterns to Avoid
- Don't over-research simple questions - one good search is often enough
- Don't copy-paste content - synthesize and explain
- Don't ignore conflicting information - acknowledge it
- Don't provide URLs you haven't verified contain relevant information"#;

pub struct ResearcherAgent;

impl ResearcherAgent {
    pub fn new() -> Self {
        Self
    }
}

impl Default for ResearcherAgent {
    fn default() -> Self {
        Self::new()
    }
}

const COMPACT_PROMPT: &str = r#"Summarize this research session so it can continue effectively with reduced context. Preserve:
1. Research questions asked and answers found (with source URLs)
2. Key facts and data points discovered
3. Conflicting information and how it was resolved
4. The original research goal and what aspects have been covered
5. Areas needing more research or follow-up queries
6. Authoritative sources identified for each topic

Focus on facts with citations. Omit raw webpage content - keep only synthesized findings."#;

const TOOL_DESCRIPTION: &str = concat!(
    "Autonomous web research agent that answers questions by searching the internet and synthesizing information.\n\n",
    "Use when you need:\n",
    "  - Current information from the web\n",
    "  - External knowledge not in the codebase\n",
    "  - Best practices research\n",
    "  - Technology comparisons\n\n",
    "IMPORTANT: Give it a RESEARCH QUESTION, not a URL to fetch.\n\n",
    "Modes:\n",
    "  - Default: Fast search with synthesized summary\n",
    "  - In-depth: Request 'thorough' research for comprehensive analysis\n\n",
    "Examples:\n",
    "  - 'Best practices for Rust error handling - anyhow vs thiserror'\n",
    "  - 'Current status of log4j vulnerability for Java 11'\n\n",
    "Detailed example:\n",
    "  'In-depth research: Compare CRDTs vs Operational Transformation for real-time collaboration. ",
    "Requirements: 50+ concurrent editors, offline editing, revision history. Prefer Rust-native solutions.'\n\n",
    "Returns: Synthesized answer with citations and source URLs\n\n",
    "DO NOT:\n",
    "  - Use for filesystem exploration (use explore agent)\n",
    "  - Use for code changes (use coder agent)\n",
    "  - Use for code review (use reviewer agent)\n"
);

impl InternalAgent for ResearcherAgent {
    fn name(&self) -> &str {
        "researcher"
    }

    fn description(&self) -> &str {
        "Researches topics on the web and synthesizes findings"
    }

    fn system_prompt(&self) -> &str {
        SYSTEM_PROMPT
    }

    fn tool_names(&self) -> &[&str] {
        &["web_search", "fetch_webpage", "read_memory", "bash", "mount_external", "update_my_task"]
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

    fn tool_limits(&self) -> Option<HashMap<String, usize>> {
        let mut limits = HashMap::new();
        limits.insert("web_search".to_string(), 5);
        limits.insert("fetch_webpage".to_string(), 10);
        Some(limits)
    }

    fn is_read_only(&self) -> bool {
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_researcher_agent() {
        let agent = ResearcherAgent::new();
        assert_eq!(agent.name(), "researcher");
        assert!(!agent.description().is_empty());
        assert!(!agent.system_prompt().is_empty());
        assert!(agent.tool_names().contains(&"web_search"));
        assert!(agent.tool_names().contains(&"fetch_webpage"));
        assert!(agent.tool_names().contains(&"read_memory"));
        assert!(agent.tool_names().contains(&"update_my_task"));
        assert!(agent.tool_names().contains(&"bash"));
        assert!(agent.tool_names().contains(&"mount_external"));
        // Researcher is read-only - no write tools
        assert!(!agent.tool_names().contains(&"add_memory"));
        assert!(!agent.tool_names().contains(&"write_file"));
    }

    #[test]
    fn test_researcher_tool_limits() {
        let agent = ResearcherAgent::new();
        let limits = agent.tool_limits().expect("researcher should have tool limits");
        assert_eq!(limits.get("web_search"), Some(&5));
        assert_eq!(limits.get("fetch_webpage"), Some(&10));
    }
}
