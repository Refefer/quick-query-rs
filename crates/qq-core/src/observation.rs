//! Observational Memory framework for context compaction.
//!
//! Implements a three-tier architecture (messages -> observations -> reflections)
//! that maintains a structured, append-only observation log with dated, prioritized entries.

use async_trait::async_trait;

use crate::error::Error;
use crate::message::{Message, Role};

/// Async trait for LLM-powered observation and reflection.
/// Implemented in qq-cli with a concrete provider.
#[async_trait]
pub trait ContextCompactor: Send + Sync {
    /// Convert raw messages into formatted observation text.
    async fn observe(&self, messages: &[Message]) -> Result<String, Error>;
    /// Restructure/compress an observation log into a tighter version.
    async fn reflect(&self, observation_log: &str) -> Result<String, Error>;
}

/// Configuration for the OM thresholds.
#[derive(Debug, Clone)]
pub struct ObservationConfig {
    /// Byte threshold for unobserved messages before triggering Observer.
    pub message_threshold_bytes: usize,
    /// Byte threshold for observation log before triggering Reflector.
    pub observation_threshold_bytes: usize,
    /// Number of recent messages to always keep as raw messages.
    pub preserve_recent: usize,
    /// Hysteresis multiplier to prevent compaction thrashing.
    pub hysteresis: f64,
}

impl ObservationConfig {
    /// Agent-tuned defaults: smaller thresholds for agent contexts.
    pub fn for_agents() -> Self {
        Self {
            message_threshold_bytes: 30_000,
            observation_threshold_bytes: 100_000,
            preserve_recent: 6,
            hysteresis: 1.1,
        }
    }
}

impl Default for ObservationConfig {
    fn default() -> Self {
        Self {
            message_threshold_bytes: 50_000,
            observation_threshold_bytes: 200_000,
            preserve_recent: 10,
            hysteresis: 1.1,
        }
    }
}

/// The core Observational Memory state machine.
pub struct ObservationalMemory {
    /// The accumulated observation log (formatted text).
    observation_log: String,
    /// Index into session messages: messages before this have been observed.
    observed_up_to: usize,
    /// Number of observation passes completed.
    pub observation_count: u32,
    /// Number of reflection passes completed.
    pub reflection_count: u32,
    /// Configuration thresholds.
    config: ObservationConfig,
}

impl ObservationalMemory {
    pub fn new(config: ObservationConfig) -> Self {
        Self {
            observation_log: String::new(),
            observed_up_to: 0,
            observation_count: 0,
            reflection_count: 0,
            config,
        }
    }

    /// Create with a pre-existing observation log (for resuming stateful agents).
    /// All loaded messages are treated as unobserved (`observed_up_to: 0`).
    pub fn with_observation_log(config: ObservationConfig, log: String) -> Self {
        Self {
            observation_log: log,
            observed_up_to: 0,
            observation_count: 0,
            reflection_count: 0,
            config,
        }
    }

    /// Decompose into parts for storage: (observation_log, observation_count, reflection_count).
    pub fn into_parts(self) -> (String, u32, u32) {
        (self.observation_log, self.observation_count, self.reflection_count)
    }

    /// Get the number of observation passes completed.
    pub fn observation_count(&self) -> u32 {
        self.observation_count
    }

    /// Check if unobserved messages exceed the message threshold.
    pub fn needs_observation(&self, messages: &[Message]) -> bool {
        let preserve = self.config.preserve_recent.min(messages.len());
        let unobserved_end = messages.len().saturating_sub(preserve);

        if unobserved_end <= self.observed_up_to {
            return false;
        }

        let unobserved_bytes: usize = messages[self.observed_up_to..unobserved_end]
            .iter()
            .map(|m| m.byte_count())
            .sum();

        let threshold =
            (self.config.message_threshold_bytes as f64 * self.config.hysteresis) as usize;

        let triggered = unobserved_bytes > threshold;

        tracing::debug!(
            total_messages = messages.len(),
            preserve_recent = preserve,
            unobserved_range = %(format!("{}..{}", self.observed_up_to, unobserved_end)),
            unobserved_bytes = unobserved_bytes,
            threshold = threshold,
            triggered = triggered,
            "Observation check"
        );

        triggered
    }

    /// Check if the observation log exceeds the observation threshold.
    pub fn needs_reflection(&self) -> bool {
        let threshold =
            (self.config.observation_threshold_bytes as f64 * self.config.hysteresis) as usize;
        let triggered = self.observation_log.len() > threshold;

        tracing::debug!(
            log_bytes = self.observation_log.len(),
            threshold = threshold,
            triggered = triggered,
            "Reflection check"
        );

        triggered
    }

    /// Run the full compaction pipeline: observe then reflect if needed.
    /// This is the main entry point called from ChatSession.
    pub async fn compact(
        &mut self,
        messages: &mut Vec<Message>,
        compactor: &dyn ContextCompactor,
    ) -> Result<(), Error> {
        // 1. Check if observation is needed
        let preserve = self.config.preserve_recent.min(messages.len());
        let unobserved_end = messages.len().saturating_sub(preserve);

        if unobserved_end > self.observed_up_to {
            let unobserved_bytes: usize = messages[self.observed_up_to..unobserved_end]
                .iter()
                .map(|m| m.byte_count())
                .sum();

            let msg_threshold =
                (self.config.message_threshold_bytes as f64 * self.config.hysteresis) as usize;

            if unobserved_bytes > msg_threshold {
                // Find safe split point (don't break tool call sequences)
                let safe_end = find_safe_split_point(messages, unobserved_end);
                if safe_end > self.observed_up_to {
                    let to_observe = &messages[self.observed_up_to..safe_end];

                    tracing::debug!(
                        messages_to_observe = to_observe.len(),
                        unobserved_bytes = unobserved_bytes,
                        safe_split = safe_end,
                        "Starting observation pass"
                    );

                    match compactor.observe(to_observe).await {
                        Ok(observations) if !observations.is_empty() => {
                            // Append to observation log
                            if !self.observation_log.is_empty() {
                                self.observation_log.push_str("\n\n");
                            }
                            self.observation_log.push_str(&observations);

                            // Drain the observed messages
                            messages.drain(self.observed_up_to..safe_end);
                            // After draining, all remaining messages are unobserved recent ones.
                            // observed_up_to stays at same index (pointing at start of recent msgs)
                            // since we drained everything before it.

                            self.observation_count += 1;

                            tracing::debug!(
                                observation_bytes = observations.len(),
                                messages_drained = safe_end - self.observed_up_to,
                                remaining_messages = messages.len(),
                                log_bytes = self.observation_log.len(),
                                observation_count = self.observation_count,
                                "Observation pass complete"
                            );
                        }
                        Ok(_) => {
                            tracing::warn!("Observer returned empty result, skipping");
                        }
                        Err(e) => {
                            tracing::warn!(error = %e, "Observer failed, skipping observation");
                        }
                    }
                }
            }
        }

        // 2. Check if reflection is needed
        let obs_threshold =
            (self.config.observation_threshold_bytes as f64 * self.config.hysteresis) as usize;

        if self.observation_log.len() > obs_threshold {
            tracing::debug!(
                log_bytes_before = self.observation_log.len(),
                "Starting reflection pass"
            );

            match compactor.reflect(&self.observation_log).await {
                Ok(reflected) if !reflected.is_empty() => {
                    self.observation_log = reflected;
                    self.reflection_count += 1;

                    tracing::debug!(
                        log_bytes_after = self.observation_log.len(),
                        reflection_count = self.reflection_count,
                        "Reflection pass complete"
                    );
                }
                Ok(_) => {
                    tracing::warn!("Reflector returned empty result, keeping current log");
                }
                Err(e) => {
                    tracing::warn!(error = %e, "Reflector failed, keeping current log");
                }
            }
        }

        Ok(())
    }

    /// Get the observation log text (for prompt assembly).
    pub fn observation_log(&self) -> &str {
        &self.observation_log
    }

    /// Get how many messages have been observed (for prompt assembly).
    pub fn observed_up_to(&self) -> usize {
        self.observed_up_to
    }

    /// Get the unobserved (recent) messages from a message list.
    pub fn unobserved_messages<'a>(&self, messages: &'a [Message]) -> &'a [Message] {
        if self.observed_up_to < messages.len() {
            &messages[self.observed_up_to..]
        } else {
            &[]
        }
    }

    /// Total bytes in the observation log.
    pub fn log_bytes(&self) -> usize {
        self.observation_log.len()
    }

    /// Clear all state (for /clear and /reset).
    pub fn clear(&mut self) {
        self.observation_log.clear();
        self.observed_up_to = 0;
        self.observation_count = 0;
        self.reflection_count = 0;
    }
}

/// Find a safe point to split messages that doesn't break tool call sequences.
///
/// A tool call sequence is: assistant message with tool_calls followed by
/// one or more tool result messages. We should never split in the middle.
pub fn find_safe_split_point(messages: &[Message], desired_end: usize) -> usize {
    let clamped = desired_end.min(messages.len());
    let mut end = clamped;

    // Walk backwards to find a position that's not inside a tool call sequence
    while end > 0 {
        let msg = &messages[end - 1];

        // If the message at end-1 is a tool result, we're inside a sequence.
        if msg.tool_call_id.is_some() {
            end -= 1;
            continue;
        }

        // If the message at end-1 is an assistant with tool_calls,
        // the results come after it — exclude this assistant message too.
        if msg.role == Role::Assistant && !msg.tool_calls.is_empty() {
            end -= 1;
            continue;
        }

        // Safe position found
        break;
    }

    if end != clamped {
        tracing::debug!(
            desired = clamped,
            actual = end,
            "Split point adjusted to preserve tool call sequence"
        );
    }

    end
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::message::ToolCall;

    // Helper: create a message with approximately `n` bytes of content
    fn msg_with_bytes(role: Role, n: usize) -> Message {
        let content = "x".repeat(n);
        match role {
            Role::User => Message::user(content.as_str()),
            Role::Assistant => Message::assistant(content.as_str()),
            Role::System => Message::system(content.as_str()),
            Role::Tool => Message::tool_result("tc-0", content.as_str()),
        }
    }

    fn assistant_with_tool_call() -> Message {
        Message::assistant_with_tool_calls(
            "thinking",
            vec![ToolCall::new("tc-1", "read_file", serde_json::json!({"path": "/tmp"}))],
        )
    }

    fn tool_result_msg(id: &str) -> Message {
        Message::tool_result(id, "result content")
    }

    // --- find_safe_split_point ---

    #[test]
    fn test_safe_split_normal_messages() {
        let messages = vec![
            Message::user("hello"),
            Message::assistant("world"),
            Message::user("next"),
            Message::assistant("response"),
        ];
        assert_eq!(find_safe_split_point(&messages, 2), 2);
        assert_eq!(find_safe_split_point(&messages, 3), 3);
    }

    #[test]
    fn test_safe_split_tool_call_sequence() {
        let messages = vec![
            Message::user("hello"),
            assistant_with_tool_call(),
            tool_result_msg("tc-1"),
            Message::assistant("done"),
        ];
        // Desired end = 2 (inside tool sequence) -> should back up to 1
        assert_eq!(find_safe_split_point(&messages, 2), 1);
        // Desired end = 3 (at tool result) -> should back up to 1
        assert_eq!(find_safe_split_point(&messages, 3), 1);
        // Desired end = 4 (after sequence) -> safe at 4
        assert_eq!(find_safe_split_point(&messages, 4), 4);
    }

    #[test]
    fn test_safe_split_all_tool_calls() {
        let messages = vec![
            assistant_with_tool_call(),
            tool_result_msg("tc-1"),
        ];
        // Everything is a tool sequence, returns 0
        assert_eq!(find_safe_split_point(&messages, 2), 0);
    }

    #[test]
    fn test_safe_split_empty() {
        let messages: Vec<Message> = vec![];
        assert_eq!(find_safe_split_point(&messages, 0), 0);
    }

    #[test]
    fn test_safe_split_at_exact_boundary() {
        let messages = vec![
            Message::user("q1"),
            Message::assistant("a1"),
            assistant_with_tool_call(),
            tool_result_msg("tc-1"),
            Message::assistant("final"),
        ];
        // Desired end = 2 is exactly at the assistant message boundary
        assert_eq!(find_safe_split_point(&messages, 2), 2);
    }

    // --- needs_observation ---

    #[test]
    fn test_needs_observation_under_threshold() {
        let config = ObservationConfig {
            message_threshold_bytes: 1000,
            preserve_recent: 2,
            hysteresis: 1.0,
            ..Default::default()
        };
        let om = ObservationalMemory::new(config);
        // 5 messages with 100 bytes each = 500 bytes, preserve_recent=2, so 3 unobserved = 300 bytes
        let messages: Vec<Message> = (0..5).map(|_| msg_with_bytes(Role::User, 100)).collect();
        assert!(!om.needs_observation(&messages));
    }

    #[test]
    fn test_needs_observation_over_threshold() {
        let config = ObservationConfig {
            message_threshold_bytes: 200,
            preserve_recent: 2,
            hysteresis: 1.0,
            ..Default::default()
        };
        let om = ObservationalMemory::new(config);
        // 5 messages with 100 bytes each, preserve_recent=2, so 3 unobserved = 300 > 200
        let messages: Vec<Message> = (0..5).map(|_| msg_with_bytes(Role::User, 100)).collect();
        assert!(om.needs_observation(&messages));
    }

    #[test]
    fn test_needs_observation_with_hysteresis() {
        let config = ObservationConfig {
            message_threshold_bytes: 250,
            preserve_recent: 2,
            hysteresis: 1.2, // threshold becomes 300
            ..Default::default()
        };
        let om = ObservationalMemory::new(config);
        // 3 unobserved messages * 100 = 300 bytes, threshold with hysteresis = 300, not strictly >
        let messages: Vec<Message> = (0..5).map(|_| msg_with_bytes(Role::User, 100)).collect();
        assert!(!om.needs_observation(&messages));
    }

    #[test]
    fn test_needs_observation_few_messages() {
        let config = ObservationConfig {
            message_threshold_bytes: 10,
            preserve_recent: 10,
            hysteresis: 1.0,
            ..Default::default()
        };
        let om = ObservationalMemory::new(config);
        // 5 messages but preserve_recent=10, so unobserved_end = 0
        let messages: Vec<Message> = (0..5).map(|_| msg_with_bytes(Role::User, 100)).collect();
        assert!(!om.needs_observation(&messages));
    }

    // --- needs_reflection ---

    #[test]
    fn test_needs_reflection_under_threshold() {
        let config = ObservationConfig {
            observation_threshold_bytes: 1000,
            hysteresis: 1.0,
            ..Default::default()
        };
        let om = ObservationalMemory::new(config);
        assert!(!om.needs_reflection());
    }

    #[test]
    fn test_needs_reflection_over_threshold() {
        let config = ObservationConfig {
            observation_threshold_bytes: 100,
            hysteresis: 1.0,
            ..Default::default()
        };
        let mut om = ObservationalMemory::new(config);
        om.observation_log = "x".repeat(200);
        assert!(om.needs_reflection());
    }

    // --- compact() with MockCompactor ---

    struct TestCompactor {
        /// None = return the string; Some(msg) = return error with message
        observe_ok: Option<String>,
        observe_err: Option<String>,
        reflect_ok: Option<String>,
        reflect_err: Option<String>,
    }

    impl TestCompactor {
        fn observe_ok(text: &str) -> Self {
            Self {
                observe_ok: Some(text.to_string()),
                observe_err: None,
                reflect_ok: None,
                reflect_err: None,
            }
        }

        fn observe_err(msg: &str) -> Self {
            Self {
                observe_ok: None,
                observe_err: Some(msg.to_string()),
                reflect_ok: None,
                reflect_err: None,
            }
        }

        fn with_reflect_ok(mut self, text: &str) -> Self {
            self.reflect_ok = Some(text.to_string());
            self
        }

        fn with_reflect_err(mut self, msg: &str) -> Self {
            self.reflect_err = Some(msg.to_string());
            self
        }
    }

    #[async_trait]
    impl ContextCompactor for TestCompactor {
        async fn observe(&self, _messages: &[Message]) -> Result<String, Error> {
            if let Some(ref err) = self.observe_err {
                Err(Error::Unknown(err.clone()))
            } else {
                Ok(self.observe_ok.clone().unwrap_or_default())
            }
        }
        async fn reflect(&self, _observation_log: &str) -> Result<String, Error> {
            if let Some(ref err) = self.reflect_err {
                Err(Error::Unknown(err.clone()))
            } else {
                Ok(self.reflect_ok.clone().unwrap_or_default())
            }
        }
    }

    #[tokio::test]
    async fn test_compact_observes_when_threshold_exceeded() {
        let config = ObservationConfig {
            message_threshold_bytes: 200,
            observation_threshold_bytes: 100_000,
            preserve_recent: 2,
            hysteresis: 1.0,
        };
        let mut om = ObservationalMemory::new(config);

        let mut messages: Vec<Message> = (0..6).map(|_| msg_with_bytes(Role::User, 100)).collect();
        let original_len = messages.len();

        let compactor = TestCompactor::observe_ok("## Observations\n- Found something");

        om.compact(&mut messages, &compactor).await.unwrap();

        // Messages should have been drained (6 - 2 preserved = 4 observed, drained)
        assert!(messages.len() < original_len);
        assert_eq!(om.observation_count, 1);
        assert!(om.observation_log.contains("Found something"));
    }

    #[tokio::test]
    async fn test_compact_no_action_below_threshold() {
        let config = ObservationConfig {
            message_threshold_bytes: 50_000,
            observation_threshold_bytes: 200_000,
            preserve_recent: 10,
            hysteresis: 1.0,
        };
        let mut om = ObservationalMemory::new(config);

        let mut messages = vec![
            Message::user("hello"),
            Message::assistant("world"),
        ];

        let compactor = TestCompactor::observe_ok("should not be called");

        om.compact(&mut messages, &compactor).await.unwrap();

        assert_eq!(messages.len(), 2);
        assert_eq!(om.observation_count, 0);
        assert!(om.observation_log.is_empty());
    }

    #[tokio::test]
    async fn test_compact_observer_failure_graceful() {
        let config = ObservationConfig {
            message_threshold_bytes: 200,
            observation_threshold_bytes: 100_000,
            preserve_recent: 2,
            hysteresis: 1.0,
        };
        let mut om = ObservationalMemory::new(config);

        let mut messages: Vec<Message> = (0..6).map(|_| msg_with_bytes(Role::User, 100)).collect();

        let compactor = TestCompactor::observe_err("observer failed");

        // Should not panic
        om.compact(&mut messages, &compactor).await.unwrap();

        // Messages should NOT be drained
        assert_eq!(messages.len(), 6);
        assert_eq!(om.observation_count, 0);
    }

    #[tokio::test]
    async fn test_compact_reflector_triggers_when_log_large() {
        let config = ObservationConfig {
            message_threshold_bytes: 50_000,
            observation_threshold_bytes: 100,
            preserve_recent: 2,
            hysteresis: 1.0,
        };
        let mut om = ObservationalMemory::new(config);
        om.observation_log = "x".repeat(200);

        let mut messages = vec![Message::user("hello"), Message::assistant("world")];

        let compactor = TestCompactor::observe_ok("")
            .with_reflect_ok("compressed log");

        om.compact(&mut messages, &compactor).await.unwrap();

        assert_eq!(om.reflection_count, 1);
        assert_eq!(om.observation_log, "compressed log");
    }

    #[tokio::test]
    async fn test_compact_reflector_failure_preserves_log() {
        let config = ObservationConfig {
            message_threshold_bytes: 50_000,
            observation_threshold_bytes: 100,
            preserve_recent: 2,
            hysteresis: 1.0,
        };
        let mut om = ObservationalMemory::new(config);
        om.observation_log = "x".repeat(200);

        let mut messages = vec![Message::user("hello"), Message::assistant("world")];

        let compactor = TestCompactor::observe_ok("")
            .with_reflect_err("reflector failed");

        om.compact(&mut messages, &compactor).await.unwrap();

        assert_eq!(om.reflection_count, 0);
        assert_eq!(om.observation_log.len(), 200); // Unchanged
    }

    #[tokio::test]
    async fn test_compact_empty_observation_rejected() {
        let config = ObservationConfig {
            message_threshold_bytes: 200,
            observation_threshold_bytes: 100_000,
            preserve_recent: 2,
            hysteresis: 1.0,
        };
        let mut om = ObservationalMemory::new(config);

        let mut messages: Vec<Message> = (0..6).map(|_| msg_with_bytes(Role::User, 100)).collect();

        // observe returns empty string
        let compactor = TestCompactor::observe_ok("");

        om.compact(&mut messages, &compactor).await.unwrap();

        // Messages should NOT be drained because observation was empty
        assert_eq!(messages.len(), 6);
        assert_eq!(om.observation_count, 0);
    }

    #[tokio::test]
    async fn test_compact_preserves_recent_messages() {
        let config = ObservationConfig {
            message_threshold_bytes: 200,
            observation_threshold_bytes: 100_000,
            preserve_recent: 3,
            hysteresis: 1.0,
        };
        let mut om = ObservationalMemory::new(config);

        // Use larger messages so unobserved bytes exceed the threshold
        let mut messages: Vec<Message> = (0..8).map(|_| msg_with_bytes(Role::User, 100)).collect();

        let compactor = TestCompactor::observe_ok("observations recorded");

        om.compact(&mut messages, &compactor).await.unwrap();

        // Should have at least preserve_recent messages remaining
        assert!(messages.len() >= 3);
        assert_eq!(om.observation_count, 1);
    }

    #[tokio::test]
    async fn test_compact_tool_call_boundary_respected() {
        let config = ObservationConfig {
            message_threshold_bytes: 10,
            observation_threshold_bytes: 100_000,
            preserve_recent: 1,
            hysteresis: 1.0,
        };
        let mut om = ObservationalMemory::new(config);

        // Messages: user, assistant+tool, tool_result, assistant, user
        let mut messages = vec![
            Message::user("start"),
            assistant_with_tool_call(),
            tool_result_msg("tc-1"),
            Message::assistant("done with tool"),
            Message::user("follow up"),
        ];

        let compactor = TestCompactor::observe_ok("observed tool usage");

        om.compact(&mut messages, &compactor).await.unwrap();

        // The tool call sequence should not be split
        // Verify that no tool_result message exists without its parent assistant message
        for (i, msg) in messages.iter().enumerate() {
            if msg.tool_call_id.is_some() {
                // There should be an assistant with tool_calls before this
                assert!(i > 0, "Tool result at index 0 without parent");
                let prev = &messages[i - 1];
                assert!(
                    prev.role == Role::Assistant && !prev.tool_calls.is_empty(),
                    "Tool result without preceding assistant+tool_calls"
                );
            }
        }
    }

    // --- clear() ---

    #[test]
    fn test_clear_resets_all_state() {
        let mut om = ObservationalMemory::new(ObservationConfig::default());
        om.observation_log = "some log".to_string();
        om.observed_up_to = 5;
        om.observation_count = 3;
        om.reflection_count = 1;

        om.clear();

        assert!(om.observation_log.is_empty());
        assert_eq!(om.observed_up_to, 0);
        assert_eq!(om.observation_count, 0);
        assert_eq!(om.reflection_count, 0);
    }

    #[tokio::test]
    async fn test_clear_after_observations_and_reflections() {
        let config = ObservationConfig {
            message_threshold_bytes: 100,
            observation_threshold_bytes: 50,
            preserve_recent: 2,
            hysteresis: 1.0,
        };
        let mut om = ObservationalMemory::new(config);

        let mut messages: Vec<Message> = (0..6).map(|_| msg_with_bytes(Role::User, 100)).collect();

        // Large observation text triggers reflection
        let compactor = TestCompactor::observe_ok(&"x".repeat(200))
            .with_reflect_ok("reflected");

        om.compact(&mut messages, &compactor).await.unwrap();
        assert!(om.observation_count > 0 || om.reflection_count > 0);

        om.clear();
        assert!(om.observation_log.is_empty());
        assert_eq!(om.observation_count, 0);
        assert_eq!(om.reflection_count, 0);
    }
}
