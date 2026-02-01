//! Summarizer agent for content summarization.

use super::InternalAgent;

const SYSTEM_PROMPT: &str = r#"You are a summarization expert. Your task is to create concise, accurate summaries that preserve the key information and insights from the original content.

Guidelines for effective summarization:
- Identify the main points and key takeaways
- Preserve important details, facts, and figures
- Maintain the original meaning and intent
- Use clear, concise language
- Organize information logically
- Highlight actionable insights when present
- Keep the summary proportional to the original length

Output format:
- For short content: A brief paragraph summary
- For longer content: Bullet points or sections
- Always capture the essential message

Do not:
- Add information not present in the original
- Include personal opinions or interpretations
- Lose critical nuances or context"#;

pub struct SummarizerAgent;

impl SummarizerAgent {
    pub fn new() -> Self {
        Self
    }
}

impl Default for SummarizerAgent {
    fn default() -> Self {
        Self::new()
    }
}

impl InternalAgent for SummarizerAgent {
    fn name(&self) -> &str {
        "summarizer"
    }

    fn description(&self) -> &str {
        "Summarize long content into concise summaries"
    }

    fn system_prompt(&self) -> &str {
        SYSTEM_PROMPT
    }

    fn tool_names(&self) -> &[&str] {
        // Summarizer is a pure LLM agent - no tools needed
        &[]
    }

    fn max_iterations(&self) -> usize {
        1 // Summarization typically completes in one turn
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_summarizer_agent() {
        let agent = SummarizerAgent::new();
        assert_eq!(agent.name(), "summarizer");
        assert!(!agent.description().is_empty());
        assert!(!agent.system_prompt().is_empty());
        assert!(agent.tool_names().is_empty()); // Pure LLM agent
    }
}
