mod common;

use std::sync::Arc;

use qq_core::{Tool, ToolRegistry};
use qq_mcp::McpTool;

use common::test_server::create_duplex_client;

// ---------------------------------------------------------------------------
// Connection & tool discovery
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_client_connect_and_list_tools() {
    let (client, _handle) = create_duplex_client("test").await;

    let tools = client.tools();
    assert_eq!(tools.len(), 2, "expected echo + add");

    let names: Vec<&str> = tools.iter().map(|t| t.name.as_ref()).collect();
    assert!(names.contains(&"echo"), "missing echo tool");
    assert!(names.contains(&"add"), "missing add tool");
}

// ---------------------------------------------------------------------------
// Raw client tool calls
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_client_call_echo() {
    let (client, _handle) = create_duplex_client("test").await;

    let mut args = serde_json::Map::new();
    args.insert("message".into(), serde_json::Value::String("hello".into()));

    let result = client.call_tool("echo", args).await.unwrap();
    let text: String = result
        .content
        .iter()
        .filter_map(|c| match &c.raw {
            rmcp::model::RawContent::Text(t) => Some(t.text.to_string()),
            _ => None,
        })
        .collect();

    assert_eq!(text, "hello");
    assert!(!result.is_error.unwrap_or(false));
}

#[tokio::test]
async fn test_client_call_add() {
    let (client, _handle) = create_duplex_client("test").await;

    let mut args = serde_json::Map::new();
    args.insert("a".into(), serde_json::json!(3));
    args.insert("b".into(), serde_json::json!(4));

    let result = client.call_tool("add", args).await.unwrap();
    let text: String = result
        .content
        .iter()
        .filter_map(|c| match &c.raw {
            rmcp::model::RawContent::Text(t) => Some(t.text.to_string()),
            _ => None,
        })
        .collect();

    assert_eq!(text, "7");
}

// ---------------------------------------------------------------------------
// McpTool wrapping
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_mcp_tool_wrapping() {
    let (client, _handle) = create_duplex_client("myserver").await;
    let client = Arc::new(client);

    let mcp_tools: Vec<McpTool> = client
        .tools()
        .iter()
        .map(|t| McpTool::new("myserver", t, Arc::clone(&client)))
        .collect();

    assert_eq!(mcp_tools.len(), 2);

    // Check namespaced names
    let names: Vec<&str> = mcp_tools.iter().map(|t| t.name()).collect();
    assert!(names.contains(&"mcp__myserver__echo"));
    assert!(names.contains(&"mcp__myserver__add"));

    // Check definitions have descriptions
    for tool in &mcp_tools {
        let def = tool.definition();
        assert!(!def.description.is_empty(), "tool {} has empty description", tool.name());
    }
}

// ---------------------------------------------------------------------------
// McpTool::execute
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_mcp_tool_execute() {
    let (client, _handle) = create_duplex_client("srv").await;
    let client = Arc::new(client);

    let echo_mcp = client.tools().iter().find(|t| t.name.as_ref() == "echo").unwrap().clone();
    let echo_tool = McpTool::new("srv", &echo_mcp, Arc::clone(&client));

    let result = echo_tool
        .execute(serde_json::json!({"message": "world"}))
        .await
        .unwrap();

    assert!(!result.is_error);
    let text = result
        .content
        .iter()
        .filter_map(|c| match c {
            qq_core::TypedContent::Text { text } => Some(text.as_str()),
            _ => None,
        })
        .collect::<String>();
    assert_eq!(text, "world");
}

#[tokio::test]
async fn test_mcp_tool_execute_invalid_args() {
    let (client, _handle) = create_duplex_client("srv").await;
    let client = Arc::new(client);

    let echo_mcp = client.tools().iter().find(|t| t.name.as_ref() == "echo").unwrap().clone();
    let echo_tool = McpTool::new("srv", &echo_mcp, Arc::clone(&client));

    // Pass a non-object value
    let result = echo_tool
        .execute(serde_json::json!("not an object"))
        .await
        .unwrap();

    assert!(result.is_error, "expected error for non-object args");
}

// ---------------------------------------------------------------------------
// ToolRegistry integration
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_tool_registry_with_mcp() {
    let (client, _handle) = create_duplex_client("ws").await;
    let client = Arc::new(client);

    let mut registry = ToolRegistry::new();
    for mcp_tool in client.tools() {
        let tool = McpTool::new("ws", mcp_tool, Arc::clone(&client));
        registry.register(Arc::new(tool));
    }

    // Resolve glob ref
    let resolved = registry.resolve_tool_refs(&["mcp:ws/*".into()]);
    assert_eq!(resolved.len(), 2);
    assert!(resolved.contains(&"mcp__ws__echo".to_string()));
    assert!(resolved.contains(&"mcp__ws__add".to_string()));

    // Resolve specific ref
    let resolved = registry.resolve_tool_refs(&["mcp:ws/echo".into()]);
    assert_eq!(resolved, vec!["mcp__ws__echo".to_string()]);
}

#[tokio::test]
async fn test_tool_registry_subset() {
    let (client, _handle) = create_duplex_client("ws").await;
    let client = Arc::new(client);

    let mut registry = ToolRegistry::new();
    for mcp_tool in client.tools() {
        let tool = McpTool::new("ws", mcp_tool, Arc::clone(&client));
        registry.register(Arc::new(tool));
    }

    // Resolve then subset
    let resolved = registry.resolve_tool_refs(&["mcp:ws/echo".into()]);
    let subset = registry.subset(&resolved);

    assert_eq!(subset.len(), 1);
    assert!(subset.get("mcp__ws__echo").is_some());
    assert!(subset.get("mcp__ws__add").is_none());
}

#[tokio::test]
async fn test_tool_limits_resolution() {
    let (client, _handle) = create_duplex_client("ws").await;
    let client = Arc::new(client);

    let mut registry = ToolRegistry::new();
    for mcp_tool in client.tools() {
        let tool = McpTool::new("ws", mcp_tool, Arc::clone(&client));
        registry.register(Arc::new(tool));
    }

    let mut limits = std::collections::HashMap::new();
    limits.insert("mcp:ws/*".to_string(), 3usize);

    let resolved = registry.resolve_tool_limits(limits);
    assert_eq!(resolved.len(), 2);
    assert_eq!(resolved.get("mcp__ws__echo"), Some(&3));
    assert_eq!(resolved.get("mcp__ws__add"), Some(&3));
}
