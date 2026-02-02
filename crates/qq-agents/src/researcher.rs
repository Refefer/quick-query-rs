//! Researcher agent for web research and information synthesis.

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
            "IMPORTANT: Always provide full context in your prompt so the agent understands the task.\n\n",
            "Examples with context:\n",
            "  - 'Best practices for Rust error handling - I'm building a CLI tool and want to decide between anyhow and thiserror'\n",
            "  - 'What's the current status of the log4j vulnerability? We're running Java 11 with log4j 2.14'\n\n",
            "Detailed example:\n",
            "  'In-depth research: We are building a real-time collaborative document editor like Google Docs. Our stack is React ",
            "frontend with a Rust backend. We need to choose between CRDTs (like Yjs or Automerge) and Operational Transformation. ",
            "Our requirements: support 50+ concurrent editors, offline editing with sync, and we need to store revision history. ",
            "The team has no experience with either approach. Research the tradeoffs, implementation complexity, library maturity, ",
            "and any production war stories. We prefer Rust-native solutions but will consider WASM if necessary.'\n\n",
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
