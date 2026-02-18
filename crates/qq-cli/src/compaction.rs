//! LLM-powered context compaction using the Observational Memory pattern.
//!
//! This module implements the model-specific Observer and Reflector agents
//! that convert raw conversation messages into structured observation logs.

use std::sync::Arc;

use async_trait::async_trait;

use qq_core::{CompletionRequest, ContextCompactor, Error, Message, Provider};

/// Observer system prompt template. `{current_date}` is replaced at call time.
const OBSERVER_PROMPT: &str = r#"You are an Observer agent. Your job is to convert raw conversation messages into a structured observation log.

## Output Format

Produce a dated, prioritized observation log using this exact format:

### [{current_date}] Observations

- **[High priority observation title]**
  - Observation date: {current_date}
  - Referenced date: YYYY-MM-DD (if any date was mentioned in content)
  - Relative date: "N days from today" (computed offset, or "today" / "N days ago")
  - [Supporting detail]
  - [Supporting detail]

- **[Medium priority observation title]**
  - Observation date: {current_date}
  - Referenced date: N/A
  - Relative date: today
  - [Supporting detail]

- **[Low priority observation title]**
  - Observation date: {current_date}
  - Referenced date: N/A
  - Relative date: today
  - [Supporting detail]

## Priority Guidelines

- High: Key decisions, architectural choices, user requirements, errors/bugs found, critical file paths, configuration changes, breaking changes
- Medium: Implementation details, tool results, intermediate findings, code patterns identified, dependencies noted
- Low: Exploratory actions, routine file reads, minor clarifications, navigation steps, standard boilerplate

## Rules

1. Each observation captures ONE specific event, decision, or finding
2. Be concise but preserve specifics: file paths, function names, error messages, exact values
3. Use the three-date model on EVERY observation
4. Today's date is {current_date}
5. Do NOT write narrative summaries — write discrete, dated event records
6. Do NOT include greetings, pleasantries, or meta-commentary
7. Preserve all tool call results that contain meaningful data
8. When a decision supersedes a prior one, note what changed"#;

/// Reflector system prompt template. `{current_date}` is replaced at call time.
const REFLECTOR_PROMPT: &str = r#"You are a Reflector agent. Your job is to restructure and compress an observation log while preserving the same format.

## Input
You will receive an observation log consisting of dated, prioritized observations.

## Task
1. **Merge** related observations (same topic, same date range, same entity)
2. **Prune** low-priority entries that have been superseded by later events
3. **Preserve** the three-date model (observation date, referenced date, relative date) on ALL surviving entries
4. **Update** relative dates based on today's date: {current_date}
5. **Rewrite** the log in the SAME structured observation format

## Rules
- Do NOT produce narrative summaries
- Do NOT discard information arbitrarily — only superseded or clearly redundant entries
- Do NOT change the format — output must be dated, prioritized, bulleted observations
- Preserve all high-priority observations unless explicitly superseded
- When merging, keep the most specific details (file paths, error messages, exact values)
- Output should be meaningfully smaller than input (target: 40-60% of original size)"#;

/// LLM-powered context compactor implementing the Observer/Reflector pattern.
pub struct LlmCompactor {
    provider: Arc<dyn Provider>,
    model: Option<String>,
}

impl LlmCompactor {
    pub fn new(provider: Arc<dyn Provider>, model: Option<String>) -> Self {
        Self { provider, model }
    }

    fn current_date(&self) -> String {
        chrono::Local::now().format("%Y-%m-%d").to_string()
    }

    fn build_request(&self, system: &str, user_content: &str) -> CompletionRequest {
        let messages = vec![Message::user(user_content)];
        let mut request = CompletionRequest::new(messages)
            .with_system(system)
            .with_stream(false);

        if let Some(ref model) = self.model {
            request = request.with_model(model);
        }

        request
    }
}

#[async_trait]
impl ContextCompactor for LlmCompactor {
    async fn observe(&self, messages: &[Message]) -> Result<String, Error> {
        let date = self.current_date();
        let system = OBSERVER_PROMPT.replace("{current_date}", &date);

        // Format messages into a readable representation for the LLM
        let mut formatted = String::new();
        for msg in messages {
            let role = msg.role.to_string();
            let content = msg.content.to_string_lossy();
            formatted.push_str(&format!("[{}]: {}\n", role, content));

            for tc in &msg.tool_calls {
                formatted.push_str(&format!(
                    "  -> tool_call: {}({})\n",
                    tc.name, tc.arguments
                ));
            }

            if let Some(ref tc_id) = msg.tool_call_id {
                formatted.push_str(&format!("  (tool_call_id: {})\n", tc_id));
            }
        }

        let request = self.build_request(&system, &formatted);
        let response = self.provider.complete(request).await?;
        let result = response.message.content.to_string_lossy();

        if result.is_empty() {
            return Err(Error::Unknown("Observer returned empty response".to_string()));
        }

        Ok(result)
    }

    async fn reflect(&self, observation_log: &str) -> Result<String, Error> {
        let date = self.current_date();
        let system = REFLECTOR_PROMPT.replace("{current_date}", &date);

        let request = self.build_request(&system, observation_log);
        let response = self.provider.complete(request).await?;
        let result = response.message.content.to_string_lossy();

        if result.is_empty() {
            return Err(Error::Unknown("Reflector returned empty response".to_string()));
        }

        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use qq_core::testing::MockProvider;

    #[tokio::test]
    async fn test_observe_sends_correct_prompt() {
        let provider = Arc::new(MockProvider::new());
        provider.queue_response("## Observations\n- Found something");

        let compactor = LlmCompactor::new(provider.clone(), None);
        let messages = vec![Message::user("hello"), Message::assistant("world")];

        let result = compactor.observe(&messages).await.unwrap();
        assert!(result.contains("Found something"));

        // Verify request was captured
        assert_eq!(provider.request_count(), 1);
        let req = provider.last_request().unwrap();

        // System prompt should contain observer instructions
        let system = req.system.unwrap();
        assert!(system.contains("Observer agent"));
        assert!(system.contains("observation log"));

        // Messages should be formatted in the user content
        let user_content = req.messages[0].content.to_string_lossy();
        assert!(user_content.contains("[user]: hello"));
        assert!(user_content.contains("[assistant]: world"));
    }

    #[tokio::test]
    async fn test_observe_returns_provider_response() {
        let provider = Arc::new(MockProvider::new());
        provider.queue_response("- Important observation");

        let compactor = LlmCompactor::new(provider, None);
        let result = compactor.observe(&[Message::user("test")]).await.unwrap();
        assert_eq!(result, "- Important observation");
    }

    #[tokio::test]
    async fn test_observe_propagates_provider_error() {
        let provider = Arc::new(MockProvider::new());
        // Don't queue any response — will error

        let compactor = LlmCompactor::new(provider, None);
        let result = compactor.observe(&[Message::user("test")]).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_reflect_sends_correct_prompt() {
        let provider = Arc::new(MockProvider::new());
        provider.queue_response("compressed observations");

        let compactor = LlmCompactor::new(provider.clone(), None);
        let log = "## Observations\n- Old entry 1\n- Old entry 2";

        let result = compactor.reflect(log).await.unwrap();
        assert_eq!(result, "compressed observations");

        let req = provider.last_request().unwrap();
        let system = req.system.unwrap();
        assert!(system.contains("Reflector agent"));

        let user_content = req.messages[0].content.to_string_lossy();
        assert!(user_content.contains("Old entry 1"));
    }

    #[tokio::test]
    async fn test_reflect_returns_provider_response() {
        let provider = Arc::new(MockProvider::new());
        provider.queue_response("merged observations");

        let compactor = LlmCompactor::new(provider, None);
        let result = compactor.reflect("some log").await.unwrap();
        assert_eq!(result, "merged observations");
    }

    #[tokio::test]
    async fn test_reflect_propagates_provider_error() {
        let provider = Arc::new(MockProvider::new());

        let compactor = LlmCompactor::new(provider, None);
        let result = compactor.reflect("some log").await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_model_override_applied() {
        let provider = Arc::new(MockProvider::new());
        provider.queue_response("response");

        let compactor = LlmCompactor::new(provider.clone(), Some("custom-model".to_string()));
        compactor.observe(&[Message::user("test")]).await.unwrap();

        let req = provider.last_request().unwrap();
        assert_eq!(req.model.as_deref(), Some("custom-model"));
    }

    #[tokio::test]
    async fn test_model_override_none() {
        let provider = Arc::new(MockProvider::new());
        provider.queue_response("response");

        let compactor = LlmCompactor::new(provider.clone(), None);
        compactor.observe(&[Message::user("test")]).await.unwrap();

        let req = provider.last_request().unwrap();
        assert!(req.model.is_none());
    }
}
