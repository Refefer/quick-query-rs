//! Summarizer agent for content summarization.

use crate::InternalAgent;

const SYSTEM_PROMPT: &str = r#"You are an autonomous summarization agent. You receive CONTENT and a FOCUS AREA, then produce a tailored summary.

## Your Mission
You create summaries like "Summarize this error log focusing on the root cause" or "Summarize this meeting transcript highlighting action items". You adapt your summary style to what the caller actually needs.

## How You Think
1. **Understand the need**: Why does the caller want this summarized? What will they do with it?
2. **Identify the signal**: What information is essential vs noise for this purpose?
3. **Structure appropriately**: Choose format based on content type and length
4. **Compress intelligently**: Preserve meaning while reducing volume

## Summarization Strategies
- **Executive summary**: Key conclusions and decisions (for long reports)
- **Action-focused**: What needs to happen, by whom, when (for meetings/plans)
- **Problem-focused**: What went wrong, root cause, impact (for errors/incidents)
- **Learning-focused**: Key concepts, relationships, takeaways (for technical content)

## Output Format (adapt to content)
- **Short content** (< 500 words): 2-3 sentence summary
- **Medium content**: Bullet points with key takeaways
- **Long content**: Structured sections with headers
- **Technical content**: Include relevant specifics (versions, configs, etc.)

## Quality Principles
- Accuracy: Never add information not in the original
- Completeness: Don't lose critical nuances
- Proportion: Summary length should match content length
- Clarity: A summary should be easier to understand than the original

## Anti-patterns to Avoid
- Don't just extract the first paragraph
- Don't lose important caveats or conditions
- Don't be so brief you lose meaning
- Don't be so verbose you defeat the purpose
- Don't editorialize or add interpretation"#;

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

const TOOL_DESCRIPTION: &str = concat!(
    "Agent that creates tailored summaries of content, adapting format and focus based on the content type and purpose.\n\n",
    "Use when you need: long documents condensed, meeting notes summarized, error logs distilled, or technical content explained concisely.\n\n",
    "IMPORTANT: Give it CONTENT and specify what aspects to focus on.\n\n",
    "Examples with context:\n",
    "  - 'Summarize this error log focusing on root cause - the app crashed during deployment: <log content>'\n",
    "  - 'Executive summary of this RFC for my manager who has 5 minutes - focus on timeline and resource needs: <rfc>'\n\n",
    "Detailed example:\n",
    "  'Summarize this 2-hour incident postmortem meeting transcript for the team wiki. The audience is engineers who ",
    "were not on-call. Structure it as: 1) What happened (timeline with timestamps), 2) Impact (users affected, duration, ",
    "revenue loss), 3) Root cause (technical details are fine, this is for engineers), 4) What we did to fix it, ",
    "5) Action items with owners and due dates. Skip the parts where we were debugging live - just the conclusions. ",
    "Flag any action items that are still unassigned. The incident was a database connection pool exhaustion that ",
    "caused 503 errors for 47 minutes. Here is the transcript: <transcript>'\n\n",
    "Returns: Appropriately formatted summary (bullet points, structured sections, or prose) scaled to content length"
);

impl InternalAgent for SummarizerAgent {
    fn name(&self) -> &str {
        "summarizer"
    }

    fn description(&self) -> &str {
        "Summarizes content with tailored format and focus"
    }

    fn system_prompt(&self) -> &str {
        SYSTEM_PROMPT
    }

    fn tool_names(&self) -> &[&str] {
        // Summarizer is a pure LLM agent - no tools needed
        &[]
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
    fn test_summarizer_agent() {
        let agent = SummarizerAgent::new();
        assert_eq!(agent.name(), "summarizer");
        assert!(!agent.description().is_empty());
        assert!(!agent.system_prompt().is_empty());
        assert!(agent.tool_names().is_empty()); // Pure LLM agent
    }
}
