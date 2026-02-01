//! Researcher agent for web research and information synthesis.

use super::InternalAgent;

const SYSTEM_PROMPT: &str = r#"You are an autonomous web research agent. You receive HIGH-LEVEL RESEARCH QUESTIONS, not URLs to fetch.

## Your Mission
You answer questions like "What are the best practices for error handling in Rust?" or "How does OAuth 2.0 work?" by autonomously researching and synthesizing information from multiple sources.

## How You Think
1. **Understand the question**: What does the caller really need to know?
2. **Plan your research**: What sources would have authoritative information?
3. **Gather information**: Fetch relevant pages, extract key points
4. **Cross-reference**: Look for consensus and note disagreements
5. **Synthesize**: Combine findings into a coherent, actionable answer

## Research Strategies
- **Start authoritative**: Official docs, reputable sources first
- **Multiple perspectives**: Don't rely on a single source
- **Recent vs established**: Consider whether recency matters for this topic
- **Depth vs breadth**: Balance comprehensive coverage with focused depth

## Your Tools
- `fetch_webpage`: Retrieve and read web content (you choose which URLs to fetch)

## Output Expectations
Your response should:
- Directly answer the research question
- Synthesize information (don't just list what each source said)
- Note consensus and any conflicting viewpoints
- Include practical, actionable takeaways when relevant
- Cite sources with URLs

## Anti-patterns to Avoid
- Don't just fetch one page and call it done
- Don't copy-paste content - synthesize and explain
- Don't ignore conflicting information - acknowledge it
- Don't provide URLs without fetching and analyzing them first"#;

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
        "Web research and information synthesis"
    }

    fn system_prompt(&self) -> &str {
        SYSTEM_PROMPT
    }

    fn tool_names(&self) -> &[&str] {
        &["fetch_webpage"]
    }

    fn max_iterations(&self) -> usize {
        6 // Reduced for safety
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
        assert!(agent.tool_names().contains(&"fetch_webpage"));
    }
}
