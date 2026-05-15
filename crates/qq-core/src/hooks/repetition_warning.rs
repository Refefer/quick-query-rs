/*!
`post_tool` hook that prepends a factual note to repeat tool-call results.

# What this file does

Defines `RepetitionWarningHook`. After a `(tool_name, canonical_args)` pair
has executed once in a run, every subsequent execution of the same pair gets
a one-line prefix attached to its result string:

```text
[note: identical call to `<tool>` previously made in this run]

<original_result>
```

The tool itself still executes; the note is informational. The agent's
policy for what to do with the signal lives in the agent, not in this hook.

# Why post_tool::Replace and not the other dispatch points

- `pre_tool::Block` would substitute a synthetic result *and* count toward
  the agent loop's `blocked_count`. If every call in a turn is blocked, the
  loop returns `RepetitionDetected` and the parent terminates the subagent
  — exactly the failure mode the user is trying to avoid.
- `post_message::Inject` would discard the model's tool calls entirely and
  loop with a synthetic user message. That bakes in two opinions (the call
  shouldn't execute, and the agent should be told how to recover) — both
  too heavy-handed for a 2nd-occurrence informational signal.
- `post_tool::Replace` lets the tool execute normally and only transforms
  the result string. The single assumption is "the agent should see a small
  textual note." That is the minimum we can do while still informing.

# What it assumes

- The same canonical hash function as the in-loop `RepetitionDetector`
  (see `crate::canonical_hash`). Both layers must agree on what counts as
  identical — otherwise an arg-key-order difference could let a loop slip
  past one layer but not the other.
- State is keyed by `agent_id`. Subagent dispatch creates a unique
  `agent_id` per instance (e.g. `pm/coder:coder-agent:3`), so subagents
  don't pollute each other's repetition history.

# Gotchas

- State accumulates across runs of the same `agent_id` within the process
  lifetime. Acceptable today — entries are small and a stale carry-over
  only means a slightly earlier note. Revisit if memory grows.
- The in-loop `RepetitionDetector` (threshold 3) still runs *before* tool
  execution, so on the 3rd identical call it hard-blocks and this hook
  never fires for that call. The note → terminal block escalation is
  driven by the two layers together, not by this hook alone.
*/

use std::collections::HashMap;

use async_trait::async_trait;
use tokio::sync::Mutex;

use crate::canonical_hash::canonical_hash;

use super::{AgentHook, PostToolAction, PostToolContext};

/** A `post_tool` hook that prepends a factual repeat-call note to a tool
result whenever the same `(tool_name, canonical_args)` pair has executed
earlier in the run. Informational only — the tool still executes. */
pub struct RepetitionWarningHook {
    counts: Mutex<HashMap<String, HashMap<u64, usize>>>,
    //           ^ agent_id        ^ canonical_hash -> exec count
}

impl RepetitionWarningHook {
    pub fn new() -> Self {
        Self {
            counts: Mutex::new(HashMap::new()),
        }
    }
}

impl Default for RepetitionWarningHook {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl AgentHook for RepetitionWarningHook {
    fn name(&self) -> &str {
        "repetition-warning"
    }

    async fn post_tool(&self, ctx: &PostToolContext<'_>) -> PostToolAction {
        let hash = canonical_hash(&ctx.tool_call.name, &ctx.tool_call.arguments);

        let count = {
            let mut counts = self.counts.lock().await;
            let inner = counts.entry(ctx.agent_id.to_string()).or_default();
            let entry = inner.entry(hash).or_insert(0);
            *entry += 1;
            *entry
        };

        if count >= 2 {
            PostToolAction::Replace(format!(
                "[note: identical call to `{}` previously made in this run]\n\n{}",
                ctx.tool_call.name, ctx.result,
            ))
        } else {
            PostToolAction::Continue
        }
    }
}
