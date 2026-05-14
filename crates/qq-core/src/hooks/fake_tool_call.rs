/*!
`post_message` hook that detects text/XML fake tool calls.

# What this file does

Defines `FakeToolCallHook`. Local models that haven't internalised the
native tool-calling JSON schema sometimes emit text that *looks* like a
tool call (`<tool_call>...</tool_call>`, `<|python_tag|>...`, etc.)
inside the regular response content. Those text snippets never execute.
The hook detects these distinctive markers and asks the model to use
the native tool-calling API instead.

# What it assumes

- The pattern set is intentionally narrow. False positives are worse
  than false negatives here — a misfire would loop the model back to
  retry a perfectly valid response.
- Match is case-insensitive. We lowercase both the candidate content
  and the patterns so the table is the source of truth.

# Gotchas

- A legitimate response that *discusses* tool-calling syntax in prose
  ("the model emits `<tool_call>` blocks") would trigger the hook.
  This is acceptable: the response is malformed enough that asking for
  a clean retry is a reasonable behavior.
- The pattern table lowercases to the canonical forms: detection of
  `<TOOL_CALL>` works because we lowercase the content first.
*/

use async_trait::async_trait;

use super::{AgentHook, PostMessageAction, PostMessageContext};

/// Distinctive markers that indicate the model attempted a tool call as
/// text/XML rather than via the native tool-calling API. Lowercased; the
/// detector also lowercases the candidate content before matching.
const PATTERNS: &[&str] = &[
    "<tool_call>",     // DeepSeek / Qwen
    "<|python_tag|>",  // Llama 3.1+ tool-call sentinel
    "<invoke name=",   // Anthropic XML format leaked into content
    "<function=",      // Some Mistral/Llama variants
    "[tool_call]",     // CodeLlama-style
    "<function_call>", // Generic XML
];

/** A `post_message` hook that detects when the model's response contains
a tool call written as text/XML and injects a synthetic user message
asking it to use the native tool-calling API. */
pub struct FakeToolCallHook;

impl FakeToolCallHook {
    /** Returns `true` iff `content` contains any of the distinctive
    fake-tool-call markers. Case-insensitive. */
    pub fn looks_like_fake_tool_call(content: &str) -> bool {
        let lower = content.to_ascii_lowercase();
        PATTERNS.iter().any(|p| lower.contains(p))
    }
}

#[async_trait]
impl AgentHook for FakeToolCallHook {
    fn name(&self) -> &str {
        "fake-tool-call"
    }

    async fn post_message(&self, ctx: &PostMessageContext<'_>) -> PostMessageAction {
        if !ctx.tool_calls.is_empty() {
            return PostMessageAction::Continue;
        }

        if !Self::looks_like_fake_tool_call(ctx.content) {
            return PostMessageAction::Continue;
        }

        PostMessageAction::Inject(
            "Your last response contained a tool call written as text or XML. \
             Please use the native tool-calling API instead — emit a proper \
             tool_calls field, not text inside your content."
                .into(),
        )
    }
}
