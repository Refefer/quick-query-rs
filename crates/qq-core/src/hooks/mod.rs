/*!
Hook trait, context structs, and action enums for the agent loop.

# What this file does

Defines the `AgentHook` extension point used by `Agent::run_once()` to let
external code observe and intervene at four points per iteration:
`pre_message` (before the LLM call), `post_message` (after the response),
`pre_tool` (before each tool execution), and `post_tool` (after).

Each hook returns a per-point action enum. The loop matches exhaustively
and applies the action; hooks never mutate the message vector directly.

# What it assumes

- Hooks are cheap and quick: they run on the agent's hot path between an
  LLM response and the next iteration. Slow hooks block the loop.
- Hooks may be invoked from multiple async tasks across concurrent agent
  runs, so implementations must be `Send + Sync` and own no per-run state
  (or guard it appropriately).

# Gotchas

- Truncation (`finish_reason == Length`) is handled by the agent loop
  *before* `post_message` hooks run. A `PostMessageContext` will never
  carry `Some(FinishReason::Length)`. Hooks see only non-truncated turns.
- The intervention cap is enforced by the loop: once exceeded, hooks are
  not invoked that iteration and the loop falls through to its normal
  "no tool calls → success" branch. This bounds retry without surfacing
  a new error variant.
*/

pub mod empty_response;
pub mod fake_tool_call;
pub mod repetition_warning;

#[cfg(test)]
mod tests;

use async_trait::async_trait;

use crate::message::{FinishReason, Message, ToolCall, Usage};

/** Read-only snapshot passed to `pre_message` hooks just before an LLM
call. Contains the full conversation that's about to be sent. */
pub struct PreMessageContext<'a> {
    pub agent_id: &'a str,
    pub iteration: usize,
    pub messages: &'a [Message],
}

/** Action a `pre_message` hook returns to the agent loop. */
pub enum PreMessageAction {
    /// Proceed with the LLM call as-is.
    Continue,
    /// Append these messages to the working history before the next call.
    Inject(Vec<Message>),
    /// Use this message list for the next LLM call only; the agent's
    /// working history is not mutated.
    Replace(Vec<Message>),
}

/** Read-only snapshot passed to `post_message` hooks immediately after
an LLM response. Carries the parsed content, tool calls, finish reason,
and per-run intervention counter. */
pub struct PostMessageContext<'a> {
    pub agent_id: &'a str,
    pub iteration: usize,
    pub content: &'a str,
    pub tool_calls: &'a [ToolCall],
    /// Present only for reasoning models that emit `<thinking>` content.
    pub thinking: Option<&'a str>,
    /// Present only when the provider surfaced one. Never `Length` —
    /// truncation is handled by the loop before hooks fire.
    pub finish_reason: Option<FinishReason>,
    pub usage: &'a Usage,
    /// Number of consecutive intervening hooks so far in this run.
    /// Informational; the loop already enforces the cap.
    pub consecutive_interventions: u32,
}

/** Action a `post_message` hook returns to the agent loop. */
pub enum PostMessageAction {
    /// Proceed with normal post-response handling.
    Continue,
    /// Push the model's assistant message verbatim, then push this string
    /// as a synthetic user message, then loop. Counts toward the cap.
    Inject(String),
    /// Discard the model's response; substitute this content as the
    /// assistant message and terminate the loop with Success.
    Replace(String),
}

/** Read-only snapshot passed to `pre_tool` hooks immediately before a
tool executes. */
pub struct PreToolContext<'a> {
    pub agent_id: &'a str,
    pub iteration: usize,
    pub tool_call: &'a ToolCall,
}

/** Action a `pre_tool` hook returns to the agent loop. */
pub enum PreToolAction {
    /// Execute the original tool call.
    Continue,
    /// Skip execution; use this string as the tool result.
    Block(String),
    /// Execute this (possibly modified) tool call instead.
    Replace(ToolCall),
}

/** Read-only snapshot passed to `post_tool` hooks immediately after a
tool produces a result. */
pub struct PostToolContext<'a> {
    pub agent_id: &'a str,
    pub iteration: usize,
    pub tool_call: &'a ToolCall,
    pub result: &'a str,
    pub is_error: bool,
}

/** Action a `post_tool` hook returns to the agent loop. */
pub enum PostToolAction {
    /// Use the original result.
    Continue,
    /// Replace the tool result text before it's added to the conversation.
    Replace(String),
}

/** Trait implemented by agent-loop hooks. All four dispatch methods have
a default `Continue` implementation; implementors override only what they
need. `name()` is required and must return a stable identifier used in
logs and telemetry. */
#[async_trait]
pub trait AgentHook: Send + Sync {
    fn name(&self) -> &str;

    async fn pre_message(&self, _ctx: &PreMessageContext<'_>) -> PreMessageAction {
        PreMessageAction::Continue
    }

    async fn post_message(&self, _ctx: &PostMessageContext<'_>) -> PostMessageAction {
        PostMessageAction::Continue
    }

    async fn pre_tool(&self, _ctx: &PreToolContext<'_>) -> PreToolAction {
        PreToolAction::Continue
    }

    async fn post_tool(&self, _ctx: &PostToolContext<'_>) -> PostToolAction {
        PostToolAction::Continue
    }
}

pub use empty_response::EmptyResponseHook;
pub use fake_tool_call::FakeToolCallHook;
pub use repetition_warning::RepetitionWarningHook;
