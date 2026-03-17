use std::collections::HashMap;

use qq_mcp::McpClient;

/// Get the path to the test_mcp_server example binary.
///
/// Assumes the binary was pre-built (or will be built by cargo test).
fn test_server_command() -> (String, Vec<String>) {
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".to_string());
    let args = vec![
        "run".to_string(),
        "--example".to_string(),
        "test_mcp_server".to_string(),
        "-p".to_string(),
        "qq-mcp".to_string(),
        "--quiet".to_string(),
    ];
    (cargo, args)
}

#[tokio::test]
async fn test_stdio_connect() {
    let (command, args) = test_server_command();
    let client = McpClient::connect_stdio(
        "stdio-test".to_string(),
        &command,
        &args,
        &HashMap::new(),
    )
    .await
    .expect("should connect to stdio test server");

    let tools = client.tools();
    assert_eq!(tools.len(), 2, "expected echo + add");

    let names: Vec<&str> = tools.iter().map(|t| t.name.as_ref()).collect();
    assert!(names.contains(&"echo"));
    assert!(names.contains(&"add"));

    client.shutdown().await;
}

#[tokio::test]
async fn test_stdio_call_tool() {
    let (command, args) = test_server_command();
    let client = McpClient::connect_stdio(
        "stdio-test".to_string(),
        &command,
        &args,
        &HashMap::new(),
    )
    .await
    .expect("should connect to stdio test server");

    let mut echo_args = serde_json::Map::new();
    echo_args.insert("message".into(), serde_json::Value::String("stdio works".into()));

    let result = client.call_tool("echo", echo_args).await.unwrap();
    let text: String = result
        .content
        .iter()
        .filter_map(|c| match &c.raw {
            rmcp::model::RawContent::Text(t) => Some(t.text.to_string()),
            _ => None,
        })
        .collect();

    assert_eq!(text, "stdio works");

    let mut add_args = serde_json::Map::new();
    add_args.insert("a".into(), serde_json::json!(10));
    add_args.insert("b".into(), serde_json::json!(20));

    let result = client.call_tool("add", add_args).await.unwrap();
    let text: String = result
        .content
        .iter()
        .filter_map(|c| match &c.raw {
            rmcp::model::RawContent::Text(t) => Some(t.text.to_string()),
            _ => None,
        })
        .collect();

    assert_eq!(text, "30");

    client.shutdown().await;
}
