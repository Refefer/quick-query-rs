//! Researcher agent for web research and information synthesis.

use super::InternalAgent;

const SYSTEM_PROMPT: &str = r#"You are an autonomous web research agent. You receive HIGH-LEVEL RESEARCH QUESTIONS, not URLs to fetch.

## Your Mission
You answer questions like "What are the best practices for error handling in Rust?" or "How does OAuth 2.0 work?" by researching and synthesizing information from the web.

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

impl InternalAgent for ResearcherAgent {
    fn name(&self) -> &str {
        "researcher"
    }

    fn description(&self) -> &str {
        concat!(
            "Autonomous web research agent that answers questions by searching the internet and synthesizing information.\n\n",
            "Use when you need: current information, external knowledge, best practices research, comparisons, or answers to questions not in the codebase.\n\n",
            "Modes:\n",
            "  - Default: Fast search with synthesized summary (one query, quick answer)\n",
            "  - In-depth: Request 'thorough' or 'in-depth' research for comprehensive multi-source analysis\n\n",
            "Examples:\n",
            "  - 'What are best practices for Rust error handling?'\n",
            "  - 'In-depth: Compare React vs Vue for single-page applications'\n",
            "  - 'Thorough research on OAuth 2.0 security considerations'\n\n",
            "Returns: Synthesized answer with citations and source URLs"
        )
    }

    fn system_prompt(&self) -> &str {
        SYSTEM_PROMPT
    }

    fn tool_names(&self) -> &[&str] {
        &["web_search", "fetch_webpage"]
    }

    fn max_iterations(&self) -> usize {
        100
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
    }
}
