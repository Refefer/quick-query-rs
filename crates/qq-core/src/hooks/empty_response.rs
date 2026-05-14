/*!
`post_message` hook that retries on empty model responses.

# What this file does

Defines `EmptyResponseHook`. When the model returns an empty content body
and no tool calls (and the response wasn't truncated), the hook injects a
synthetic user message asking the model to try again.

# What it assumes

- Truncation has already been classified by the agent loop. The hook
  early-returns on `FinishReason::Length` as defense-in-depth, but in
  the normal call path `PostMessageContext::finish_reason` is never
  `Some(Length)`.
- The agent loop enforces the consecutive-intervention cap; the hook
  itself does not need to.

# Gotchas

- "Empty" is judged by `content.trim().is_empty()`. A response that
  contains only whitespace (or only `<thinking>` reasoning, since
  reasoning is delivered separately) is treated as empty.
*/

use async_trait::async_trait;

use crate::message::FinishReason;

use super::{AgentHook, PostMessageAction, PostMessageContext};

/** A `post_message` hook that injects a synthetic user retry when the
model returns an empty content body and no tool calls. */
pub struct EmptyResponseHook;

#[async_trait]
impl AgentHook for EmptyResponseHook {
    fn name(&self) -> &str {
        "empty-response"
    }

    async fn post_message(&self, ctx: &PostMessageContext<'_>) -> PostMessageAction {
        if matches!(ctx.finish_reason, Some(FinishReason::Length)) {
            return PostMessageAction::Continue;
        }

        if !ctx.tool_calls.is_empty() {
            return PostMessageAction::Continue;
        }

        if !ctx.content.trim().is_empty() {
            return PostMessageAction::Continue;
        }

        PostMessageAction::Inject("You sent an empty response, please try again.".into())
    }
}
