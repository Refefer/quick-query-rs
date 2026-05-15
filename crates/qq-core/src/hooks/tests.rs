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
use crate::hooks::repetition_warning::RepetitionWarningHook;
use crate::hooks::{
    AgentHook, PostMessageAction, PostMessageContext, PostToolAction, PostToolContext,
};
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

// -------------------------------------------------------------------
// RepetitionWarningHook
// -------------------------------------------------------------------

fn tool_call(name: &str, args: serde_json::Value) -> ToolCall {
    ToolCall {
        id: format!("call_{name}"),
        name: name.into(),
        arguments: args,
    }
}

fn post_tool_ctx<'a>(
    agent_id: &'a str,
    tool_call: &'a ToolCall,
    result: &'a str,
) -> PostToolContext<'a> {
    PostToolContext {
        agent_id,
        iteration: 0,
        tool_call,
        result,
        is_error: false,
    }
}

#[tokio::test]
async fn repetition_warning_first_call_passes_through() {
    let hook = RepetitionWarningHook::new();
    let call = tool_call("read", serde_json::json!({"path": "foo.rs"}));
    let ctx = post_tool_ctx("agent-1", &call, "original result");

    let action = hook.post_tool(&ctx).await;

    assert!(matches!(action, PostToolAction::Continue));
}

#[tokio::test]
async fn repetition_warning_second_identical_call_replaces_with_note() {
    let hook = RepetitionWarningHook::new();
    let call = tool_call("read", serde_json::json!({"path": "foo.rs"}));

    let ctx1 = post_tool_ctx("agent-1", &call, "first");
    let _ = hook.post_tool(&ctx1).await;

    let ctx2 = post_tool_ctx("agent-1", &call, "second");
    let action = hook.post_tool(&ctx2).await;

    match action {
        PostToolAction::Replace(s) => {
            assert!(s.starts_with("[note:"), "expected note prefix, got: {s}");
            assert!(s.contains("read"), "note should name the tool: {s}");
            assert!(s.contains("second"), "note should preserve original result: {s}");
        }
        _ => panic!("expected Replace, got Continue"),
    }
}

#[tokio::test]
async fn repetition_warning_third_call_still_replaces() {
    let hook = RepetitionWarningHook::new();
    let call = tool_call("read", serde_json::json!({"path": "foo.rs"}));

    for label in &["first", "second", "third"] {
        let ctx = post_tool_ctx("agent-1", &call, label);
        let action = hook.post_tool(&ctx).await;
        if *label == "first" {
            assert!(matches!(action, PostToolAction::Continue));
        } else {
            assert!(
                matches!(action, PostToolAction::Replace(_)),
                "expected Replace on {label} call"
            );
        }
    }
}

#[tokio::test]
async fn repetition_warning_different_args_independent() {
    let hook = RepetitionWarningHook::new();
    let call1 = tool_call("read", serde_json::json!({"path": "foo.rs"}));
    let call2 = tool_call("read", serde_json::json!({"path": "bar.rs"}));

    let ctx1 = post_tool_ctx("agent-1", &call1, "r1");
    let _ = hook.post_tool(&ctx1).await;

    let ctx2 = post_tool_ctx("agent-1", &call2, "r2");
    let action = hook.post_tool(&ctx2).await;

    assert!(
        matches!(action, PostToolAction::Continue),
        "different args should be tracked independently"
    );
}

#[tokio::test]
async fn repetition_warning_different_tools_independent() {
    let hook = RepetitionWarningHook::new();
    let call1 = tool_call("read", serde_json::json!({"path": "foo.rs"}));
    let call2 = tool_call("write", serde_json::json!({"path": "foo.rs"}));

    let ctx1 = post_tool_ctx("agent-1", &call1, "r1");
    let _ = hook.post_tool(&ctx1).await;

    let ctx2 = post_tool_ctx("agent-1", &call2, "r2");
    let action = hook.post_tool(&ctx2).await;

    assert!(
        matches!(action, PostToolAction::Continue),
        "different tool names should be tracked independently"
    );
}

#[tokio::test]
async fn repetition_warning_arg_key_order_independent() {
    let hook = RepetitionWarningHook::new();
    let call1 = tool_call("run", serde_json::json!({"a": 1, "b": 2}));
    let call2 = tool_call("run", serde_json::json!({"b": 2, "a": 1}));

    let ctx1 = post_tool_ctx("agent-1", &call1, "r1");
    let _ = hook.post_tool(&ctx1).await;

    let ctx2 = post_tool_ctx("agent-1", &call2, "r2");
    let action = hook.post_tool(&ctx2).await;

    assert!(
        matches!(action, PostToolAction::Replace(_)),
        "canonical hash must treat key-reordered objects as identical"
    );
}

#[tokio::test]
async fn repetition_warning_isolates_per_agent_id() {
    let hook = RepetitionWarningHook::new();
    let call = tool_call("read", serde_json::json!({"path": "foo.rs"}));

    let ctx1 = post_tool_ctx("agent-1", &call, "from-1");
    let _ = hook.post_tool(&ctx1).await;

    let ctx2 = post_tool_ctx("agent-2", &call, "from-2");
    let action = hook.post_tool(&ctx2).await;

    assert!(
        matches!(action, PostToolAction::Continue),
        "agent-2's first call should not see agent-1's history"
    );
}

#[tokio::test]
async fn repetition_warning_fires_on_error_results_too() {
    let hook = RepetitionWarningHook::new();
    let call = tool_call("run", serde_json::json!({"cmd": "false"}));

    let ctx1 = PostToolContext {
        agent_id: "agent-1",
        iteration: 0,
        tool_call: &call,
        result: "Error: command failed",
        is_error: true,
    };
    let _ = hook.post_tool(&ctx1).await;

    let ctx2 = PostToolContext {
        agent_id: "agent-1",
        iteration: 1,
        tool_call: &call,
        result: "Error: command failed",
        is_error: true,
    };
    let action = hook.post_tool(&ctx2).await;

    assert!(
        matches!(action, PostToolAction::Replace(_)),
        "repeating a failing call should still trigger the note"
    );
}
