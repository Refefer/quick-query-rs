use std::collections::HashMap;
use std::sync::Arc;

use qq_core::Tool as _;
use serde::{Deserialize, Serialize};

use crate::client::McpClient;
use crate::error::McpError;
use crate::tool::McpTool;

/// MCP server transport configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "transport", rename_all = "snake_case")]
pub enum McpServerConfig {
    Stdio {
        command: String,
        #[serde(default)]
        args: Vec<String>,
        #[serde(default)]
        env: HashMap<String, String>,
    },
    Http {
        url: String,
        #[serde(default)]
        headers: HashMap<String, String>,
    },
}

impl McpServerConfig {
    /// Short label for display (e.g., "stdio: npx" or "http").
    pub fn transport_label(&self) -> String {
        match self {
            McpServerConfig::Stdio { command, .. } => format!("stdio: {}", command),
            McpServerConfig::Http { url, .. } => format!("http: {}", url),
        }
    }
}

/// Metadata about a connected MCP server (for display purposes).
#[derive(Debug, Clone)]
pub struct McpServerInfo {
    pub name: String,
    pub transport_label: String,
    pub tool_names: Vec<String>,
}

/// Manages all MCP server connections and their tools.
pub struct McpManager {
    clients: Vec<Arc<McpClient>>,
    tools: Vec<Arc<dyn qq_core::Tool>>,
    server_info: Vec<McpServerInfo>,
}

fn is_valid_server_name(name: &str) -> bool {
    !name.is_empty()
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

impl McpManager {
    /// Connect to all configured MCP servers.
    ///
    /// Servers that fail to connect are logged as warnings and skipped.
    pub async fn connect_all(configs: &HashMap<String, McpServerConfig>) -> Self {
        let mut clients = Vec::new();
        let mut tools: Vec<Arc<dyn qq_core::Tool>> = Vec::new();
        let mut server_info = Vec::new();

        for (name, config) in configs {
            if !is_valid_server_name(name) {
                tracing::warn!(
                    server = %name,
                    "Skipping MCP server: invalid name (must match [a-zA-Z0-9_-]+)"
                );
                continue;
            }

            let transport_label = config.transport_label();

            match connect_server(name.clone(), config).await {
                Ok(client) => {
                    let client = Arc::new(client);
                    let mut tool_names = Vec::new();

                    for mcp_tool in client.tools() {
                        let tool = McpTool::new(name, mcp_tool, Arc::clone(&client));
                        tool_names.push(tool.display_name().to_string());
                        tools.push(Arc::new(tool));
                    }

                    tracing::info!(
                        server = %name,
                        transport = %transport_label,
                        tool_count = tool_names.len(),
                        "Connected to MCP server"
                    );

                    server_info.push(McpServerInfo {
                        name: name.clone(),
                        transport_label,
                        tool_names,
                    });

                    clients.push(client);
                }
                Err(e) => {
                    tracing::warn!(
                        server = %name,
                        error = %e,
                        "Failed to connect to MCP server, skipping"
                    );
                }
            }
        }

        Self {
            clients,
            tools,
            server_info,
        }
    }

    /// All MCP tools for registry registration.
    pub fn tools(&self) -> &[Arc<dyn qq_core::Tool>] {
        &self.tools
    }

    /// Number of connected servers.
    pub fn server_count(&self) -> usize {
        self.clients.len()
    }

    /// Total number of tools across all servers.
    pub fn tool_count(&self) -> usize {
        self.tools.len()
    }

    /// Whether any servers are connected.
    pub fn is_empty(&self) -> bool {
        self.clients.is_empty()
    }

    /// Get server info for display (e.g., /mcp command).
    pub fn server_info(&self) -> &[McpServerInfo] {
        &self.server_info
    }

    /// Shut down all connections.
    pub async fn shutdown(self) {
        for client in self.clients {
            // Try to unwrap the Arc; if other refs exist, skip shutdown
            if let Ok(client) = Arc::try_unwrap(client) {
                client.shutdown().await;
            }
        }
    }
}

async fn connect_server(name: String, config: &McpServerConfig) -> Result<McpClient, McpError> {
    match config {
        McpServerConfig::Stdio { command, args, env } => {
            McpClient::connect_stdio(name, command, args, env).await
        }
        McpServerConfig::Http { url, headers } => {
            McpClient::connect_http(name, url, headers).await
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_valid_server_names() {
        assert!(is_valid_server_name("filesystem"));
        assert!(is_valid_server_name("my-server"));
        assert!(is_valid_server_name("server_1"));
        assert!(is_valid_server_name("A-Z_09"));
        assert!(!is_valid_server_name(""));
        assert!(!is_valid_server_name("bad name"));
        assert!(!is_valid_server_name("bad.name"));
        assert!(!is_valid_server_name("bad/name"));
    }

    #[test]
    fn test_config_serde_stdio() {
        let toml = r#"
transport = "stdio"
command = "npx"
args = ["-y", "@modelcontextprotocol/server-filesystem", "/tmp"]
"#;
        let config: McpServerConfig = toml::from_str(toml).unwrap();
        match config {
            McpServerConfig::Stdio { command, args, .. } => {
                assert_eq!(command, "npx");
                assert_eq!(args.len(), 3);
            }
            _ => panic!("Expected Stdio"),
        }
    }

    #[test]
    fn test_config_serde_http() {
        let toml = r#"
transport = "http"
url = "http://localhost:8000/mcp"
"#;
        let config: McpServerConfig = toml::from_str(toml).unwrap();
        match config {
            McpServerConfig::Http { url, .. } => {
                assert_eq!(url, "http://localhost:8000/mcp");
            }
            _ => panic!("Expected Http"),
        }
    }

    #[test]
    fn test_transport_label() {
        let stdio = McpServerConfig::Stdio {
            command: "npx".to_string(),
            args: vec![],
            env: HashMap::new(),
        };
        assert_eq!(stdio.transport_label(), "stdio: npx");

        let http = McpServerConfig::Http {
            url: "http://localhost:8000/mcp".to_string(),
            headers: HashMap::new(),
        };
        assert_eq!(http.transport_label(), "http: http://localhost:8000/mcp");
    }
}
