#[derive(Debug, thiserror::Error)]
pub enum McpError {
    #[error("MCP connection failed for '{server}': {source}")]
    Connection {
        server: String,
        source: Box<dyn std::error::Error + Send + Sync>,
    },
    #[error("MCP tool call failed: {0}")]
    ToolCall(String),
    #[error("Invalid server name '{0}': must match [a-zA-Z0-9_-]+")]
    InvalidServerName(String),
}
