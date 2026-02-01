//! Researcher agent for web research and information synthesis.

use super::InternalAgent;

const SYSTEM_PROMPT: &str = r#"You are a research assistant specialized in finding and synthesizing information from the web.

Your capabilities:
- Fetch and analyze web pages using the fetch_webpage tool
- Extract key information from sources
- Synthesize findings into clear, comprehensive answers
- Cite sources when appropriate

Guidelines:
- Search for authoritative and recent sources
- Cross-reference information when possible
- Present findings in a clear, organized manner
- Acknowledge when information is uncertain or conflicting
- Focus on answering the user's specific question

When researching:
1. Identify the key aspects of the question
2. Fetch relevant web pages
3. Extract and analyze the content
4. Synthesize a comprehensive answer
5. Cite your sources"#;

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
