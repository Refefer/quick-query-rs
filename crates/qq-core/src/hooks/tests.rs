/*!
Unit tests for the built-in hooks.

# What this file does

Exercises `EmptyResponseHook` and `FakeToolCallHook` directly through
their `post_message` methods, verifying the action enum returned for
each input shape (empty, content-only, fake-tool-call markers,
truncation, etc.).

# What it assumes

- The action enums are `PartialEq`-free (we match on them rather than
  asserting equality), since their payloads include free-form strings
  whose exact wording we don't lock down here.

# Gotchas

- These tests don't exercise the agent loop; loop-level integration
  tests live in `qq-core/src/agent.rs::tests` and use `MockProvider`.
*/

use crate::hooks::empty_response::EmptyResponseHook;
use crate::hooks::fake_tool_call::FakeToolCallHook;
use crate::hooks::{AgentHook, PostMessageAction, PostMessageContext};
use crate::message::{FinishReason, ToolCall, Usage};

fn ctx<'a>(
    content: &'a str,
    tool_calls: &'a [ToolCall],
    usage: &'a Usage,
) -> PostMessageContext<'a> {
    PostMessageContext {
        agent_id: "test-agent",
        iteration: 0,
        content,
        tool_calls,
        thinking: None,
        finish_reason: None,
        usage,
        consecutive_interventions: 0,
    }
}

fn ctx_with_finish<'a>(
    content: &'a str,
    tool_calls: &'a [ToolCall],
    usage: &'a Usage,
    finish_reason: Option<FinishReason>,
) -> PostMessageContext<'a> {
    PostMessageContext {
        agent_id: "test-agent",
        iteration: 0,
        content,
        tool_calls,
        thinking: None,
        finish_reason,
        usage,
        consecutive_interventions: 0,
    }
}

#[tokio::test]
async fn empty_response_hook_fires_on_empty_content() {
    let usage = Usage::default();
    let calls: Vec<ToolCall> = Vec::new();
    let context = ctx("", &calls, &usage);
    let action = EmptyResponseHook.post_message(&context).await;

    assert!(matches!(action, PostMessageAction::Inject(_)));
}

#[tokio::test]
async fn empty_response_hook_fires_on_whitespace_only() {
    let usage = Usage::default();
    let calls: Vec<ToolCall> = Vec::new();
    let context = ctx("   \n\t  \n", &calls, &usage);
    let action = EmptyResponseHook.post_message(&context).await;

    assert!(matches!(action, PostMessageAction::Inject(_)));
}

#[tokio::test]
async fn empty_response_hook_passes_with_content() {
    let usage = Usage::default();
    let calls: Vec<ToolCall> = Vec::new();
    let context = ctx("Here is the answer.", &calls, &usage);
    let action = EmptyResponseHook.post_message(&context).await;

    assert!(matches!(action, PostMessageAction::Continue));
}

#[tokio::test]
async fn empty_response_hook_passes_when_tool_calls_present() {
    let usage = Usage::default();
    let calls = vec![ToolCall {
        id: "call_1".into(),
        name: "run".into(),
        arguments: serde_json::json!({}),
    }];
    let context = ctx("", &calls, &usage);
    let action = EmptyResponseHook.post_message(&context).await;

    assert!(matches!(action, PostMessageAction::Continue));
}

#[tokio::test]
async fn empty_response_hook_passes_on_truncation() {
    let usage = Usage::default();
    let calls: Vec<ToolCall> = Vec::new();
    let context = ctx_with_finish("", &calls, &usage, Some(FinishReason::Length));
    let action = EmptyResponseHook.post_message(&context).await;

    assert!(matches!(action, PostMessageAction::Continue));
}

#[tokio::test]
async fn fake_tool_call_hook_detects_each_pattern() {
    let cases: &[&str] = &[
        "<tool_call>{\"name\":\"run\"}</tool_call>",
        "<|python_tag|>run({})",
        "<invoke name=\"run\">arg</invoke>",
        "<function=run>{}</function>",
        "[tool_call]run{}[/tool_call]",
        "<function_call>{}</function_call>",
    ];
    let usage = Usage::default();
    let calls: Vec<ToolCall> = Vec::new();

    for case in cases {
        let context = ctx(case, &calls, &usage);
        let action = FakeToolCallHook.post_message(&context).await;

        assert!(
            matches!(action, PostMessageAction::Inject(_)),
            "expected Inject for pattern: {case}"
        );
    }
}

#[tokio::test]
async fn fake_tool_call_hook_case_insensitive() {
    let usage = Usage::default();
    let calls: Vec<ToolCall> = Vec::new();
    let context = ctx("<TOOL_CALL>{}</TOOL_CALL>", &calls, &usage);
    let action = FakeToolCallHook.post_message(&context).await;

    assert!(matches!(action, PostMessageAction::Inject(_)));
}

#[tokio::test]
async fn fake_tool_call_hook_passes_normal_content() {
    let usage = Usage::default();
    let calls: Vec<ToolCall> = Vec::new();
    let context = ctx(
        "I read the file and found three matches in src/main.rs.",
        &calls,
        &usage,
    );
    let action = FakeToolCallHook.post_message(&context).await;

    assert!(matches!(action, PostMessageAction::Continue));
}

#[tokio::test]
async fn fake_tool_call_hook_passes_when_real_tool_calls_present() {
    let usage = Usage::default();
    let calls = vec![ToolCall {
        id: "call_1".into(),
        name: "run".into(),
        arguments: serde_json::json!({}),
    }];
    let context = ctx("<tool_call>{}</tool_call>", &calls, &usage);
    let action = FakeToolCallHook.post_message(&context).await;

    assert!(matches!(action, PostMessageAction::Continue));
}
