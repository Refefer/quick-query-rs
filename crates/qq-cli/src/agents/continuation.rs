//! Agent continuation support for handling max_turns exceeded scenarios.
//!
//! When an agent hits its max_turns limit, this module provides functionality
//! to generate a summary of progress and continue execution with that context.

use std::sync::Arc;

use qq_core::{Agent, AgentConfig, AgentProgressHandler, CompletionRequest, Message, Provider, ToolRegistry};

use crate::event_bus::{AgentEvent, AgentEventBus};

/// Configuration for agent continuation behavior.
#[derive(Debug, Clone)]
pub struct ContinuationConfig {
    /// Maximum number of continuation attempts (default: 3)
    pub max_continuations: u32,
    /// Whether continuation is enabled (default: true)
    pub enabled: bool,
}

impl Default for ContinuationConfig {
    fn default() -> Self {
        Self {
            max_continuations: 3,
            enabled: true,
        }
    }
}

/// Summary of agent execution state for continuation.
#[derive(Debug, Clone)]
pub struct ExecutionSummary {
    pub steps_taken: String,
    pub discoveries: String,
    pub accomplishments: String,
    pub remaining_work: String,
    pub important_context: String,
}

/// Prompt for generating execution summary.
const SUMMARY_PROMPT: &str = r#"You have run out of turns while working on a task. Generate a detailed summary of your progress so you can continue effectively.

Provide your summary in the following format:

<steps_taken>
List each action you took, in order. Include tool calls and their results.
</steps_taken>

<discoveries>
What did you learn? What information did you find that's relevant to the task?
</discoveries>

<accomplishments>
What parts of the task have you completed? Be specific.
</accomplishments>

<remaining_work>
What still needs to be done to complete the task?
</remaining_work>

<important_context>
Any critical context, state, or information needed to continue (file paths, error messages, partial results, etc.)
</important_context>

Be thorough - this summary will be used to continue your work."#;

/// Generate a summary of the agent's current execution state.
///
/// This is called when an agent exceeds max_turns and needs to continue.
pub async fn generate_summary(
    provider: Arc<dyn Provider>,
    messages: &[Message],
) -> Result<ExecutionSummary, qq_core::Error> {
    // Build summary request: conversation history + summary request
    let mut summary_messages = messages.to_vec();
    summary_messages.push(Message::user(SUMMARY_PROMPT));

    let request = CompletionRequest::new(summary_messages);
    let response = provider.complete(request).await?;

    // Parse XML-tagged sections from response
    let content = response.message.content.to_string_lossy();

    Ok(ExecutionSummary {
        steps_taken: extract_tag(&content, "steps_taken"),
        discoveries: extract_tag(&content, "discoveries"),
        accomplishments: extract_tag(&content, "accomplishments"),
        remaining_work: extract_tag(&content, "remaining_work"),
        important_context: extract_tag(&content, "important_context"),
    })
}

/// Extract content between XML tags.
fn extract_tag(content: &str, tag: &str) -> String {
    let start_tag = format!("<{}>", tag);
    let end_tag = format!("</{}>", tag);

    if let Some(start) = content.find(&start_tag) {
        if let Some(end) = content.find(&end_tag) {
            let start_idx = start + start_tag.len();
            if start_idx < end {
                return content[start_idx..end].trim().to_string();
            }
        }
    }
    String::new()
}

/// Format a summary for inclusion in agent context.
pub fn format_summary_context(summary: &ExecutionSummary, original_task: &str) -> String {
    format!(
        r#"## Continuation Context

You are continuing a task that was interrupted. Here is your previous progress:

### Original Task
{original_task}

### Steps Already Taken
{steps_taken}

### Discoveries Made
{discoveries}

### Accomplishments So Far
{accomplishments}

### Remaining Work
{remaining_work}

### Important Context
{important_context}

---

Continue from where you left off. Do NOT repeat work already done. Focus on completing the remaining tasks."#,
        original_task = original_task,
        steps_taken = summary.steps_taken,
        discoveries = summary.discoveries,
        accomplishments = summary.accomplishments,
        remaining_work = summary.remaining_work,
        important_context = summary.important_context,
    )
}

/// Result of an agent execution with potential continuation.
pub enum AgentExecutionResult {
    /// Task completed successfully
    Success(String),
    /// Max continuations reached, partial result
    MaxContinuationsReached {
        partial_result: String,
        continuations: u32,
    },
    /// Error during execution
    Error(qq_core::Error),
}

/// Execute an agent with continuation support.
///
/// This wraps `Agent::run_once_with_progress()` and handles max_turns exceeded
/// by generating a summary and re-executing with context.
pub async fn execute_with_continuation(
    provider: Arc<dyn Provider>,
    tools: Arc<ToolRegistry>,
    config: AgentConfig,
    original_task: String,
    progress: Option<Arc<dyn AgentProgressHandler>>,
    continuation_config: ContinuationConfig,
    event_bus: Option<&AgentEventBus>,
) -> AgentExecutionResult {
    if !continuation_config.enabled {
        // No continuation - just run once
        return match Agent::run_once_with_progress(
            provider,
            tools,
            config,
            vec![Message::user(original_task.as_str())],
            progress,
        )
        .await
        {
            Ok(result) => AgentExecutionResult::Success(result),
            Err(e) => AgentExecutionResult::Error(e),
        };
    }

    let mut continuation_count = 0u32;
    let mut current_context = vec![Message::user(original_task.as_str())];
    let mut last_partial_result = String::new();

    loop {
        let result = Agent::run_once_with_progress(
            Arc::clone(&provider),
            Arc::clone(&tools),
            config.clone(),
            current_context.clone(),
            progress.clone(),
        )
        .await;

        match result {
            Ok(final_result) => {
                return AgentExecutionResult::Success(final_result);
            }
            Err(e) => {
                // Check if this is a max_turns exceeded error
                let error_msg = e.to_string();
                if !error_msg.contains("exceeded max iterations") {
                    return AgentExecutionResult::Error(e);
                }

                // Max turns exceeded - check if we can continue
                continuation_count += 1;
                if continuation_count > continuation_config.max_continuations {
                    return AgentExecutionResult::MaxContinuationsReached {
                        partial_result: last_partial_result,
                        continuations: continuation_count - 1,
                    };
                }

                // Publish continuation event
                if let Some(bus) = event_bus {
                    bus.publish(AgentEvent::ContinuationStarted {
                        agent_name: config.id.0.clone(),
                        continuation_number: continuation_count,
                        max_continuations: continuation_config.max_continuations,
                    });
                }

                // Generate summary from current context
                // Note: The current context may be limited, but the LLM can recall
                // what it did from its conversation history in the context
                let summary = match generate_summary(Arc::clone(&provider), &current_context).await
                {
                    Ok(s) => s,
                    Err(e) => {
                        return AgentExecutionResult::Error(e);
                    }
                };

                // Store partial result from summary
                last_partial_result = format!(
                    "Completed: {}\n\nRemaining: {}",
                    summary.accomplishments, summary.remaining_work
                );

                // Build new context with summary
                let continuation_prompt = format_summary_context(&summary, &original_task);
                current_context = vec![Message::user(continuation_prompt.as_str())];
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_tag() {
        let content = r#"
<steps_taken>
1. Read the file
2. Analyzed content
</steps_taken>

<discoveries>
Found important data
</discoveries>
"#;

        assert_eq!(
            extract_tag(content, "steps_taken"),
            "1. Read the file\n2. Analyzed content"
        );
        assert_eq!(extract_tag(content, "discoveries"), "Found important data");
        assert_eq!(extract_tag(content, "nonexistent"), "");
    }

    #[test]
    fn test_extract_tag_empty() {
        let content = "<steps_taken></steps_taken>";
        assert_eq!(extract_tag(content, "steps_taken"), "");
    }

    #[test]
    fn test_extract_tag_nested_content() {
        let content = "<outer><inner>value</inner></outer>";
        assert_eq!(extract_tag(content, "inner"), "value");
    }

    #[test]
    fn test_format_summary_context() {
        let summary = ExecutionSummary {
            steps_taken: "Step 1, Step 2".to_string(),
            discoveries: "Found X".to_string(),
            accomplishments: "Did Y".to_string(),
            remaining_work: "Need to do Z".to_string(),
            important_context: "File path: /foo/bar".to_string(),
        };

        let context = format_summary_context(&summary, "Original task description");

        assert!(context.contains("Original task description"));
        assert!(context.contains("Step 1, Step 2"));
        assert!(context.contains("Found X"));
        assert!(context.contains("Did Y"));
        assert!(context.contains("Need to do Z"));
        assert!(context.contains("/foo/bar"));
        assert!(context.contains("Continue from where you left off"));
    }

    #[test]
    fn test_continuation_config_default() {
        let config = ContinuationConfig::default();
        assert_eq!(config.max_continuations, 3);
        assert!(config.enabled);
    }
}
