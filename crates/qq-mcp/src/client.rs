use std::collections::HashMap;

use rmcp::model::{CallToolRequestParams, CallToolResult};
use rmcp::service::RunningService;
use rmcp::transport::child_process::TokioChildProcess;
use rmcp::transport::streamable_http_client::{
    StreamableHttpClientTransport, StreamableHttpClientTransportConfig,
};
use rmcp::{RoleClient, ServiceExt};

use crate::error::McpError;

/// A connection to a single MCP server.
pub struct McpClient {
    name: String,
    service: RunningService<RoleClient, ()>,
    tools: Vec<rmcp::model::Tool>,
}

impl McpClient {
    /// Create an McpClient from a pre-built RunningService.
    ///
    /// This is useful for testing with duplex transports where the service
    /// is created outside of the standard connect methods.
    pub async fn from_service(
        name: String,
        service: RunningService<RoleClient, ()>,
    ) -> Result<Self, McpError> {
        let tools = service
            .list_all_tools()
            .await
            .map_err(|e| McpError::Connection {
                server: name.clone(),
                source: Box::new(e),
            })?;

        Ok(Self {
            name,
            service,
            tools,
        })
    }

    /// Connect to an MCP server via stdio (child process).
    pub async fn connect_stdio(
        name: String,
        command: &str,
        args: &[String],
        env: &HashMap<String, String>,
    ) -> Result<Self, McpError> {
        let mut cmd = tokio::process::Command::new(command);
        cmd.args(args);
        for (k, v) in env {
            cmd.env(k, v);
        }

        let process = TokioChildProcess::new(cmd).map_err(|e| McpError::Connection {
            server: name.clone(),
            source: Box::new(e),
        })?;

        let service = ().serve(process).await.map_err(|e| McpError::Connection {
            server: name.clone(),
            source: Box::new(e),
        })?;

        let tools = service
            .list_all_tools()
            .await
            .map_err(|e| McpError::Connection {
                server: name.clone(),
                source: Box::new(e),
            })?;

        Ok(Self {
            name,
            service,
            tools,
        })
    }

    /// Connect to an MCP server via HTTP (Streamable HTTP transport).
    pub async fn connect_http(
        name: String,
        url: &str,
        headers: &HashMap<String, String>,
    ) -> Result<Self, McpError> {
        let mut config = StreamableHttpClientTransportConfig::with_uri(url);
        for (k, v) in headers {
            if let (Ok(header_name), Ok(header_value)) = (
                k.parse::<http::HeaderName>(),
                v.parse::<http::HeaderValue>(),
            ) {
                config.custom_headers.insert(header_name, header_value);
            }
        }

        let transport = StreamableHttpClientTransport::from_config(config);
        let service = ().serve(transport).await.map_err(|e| McpError::Connection {
            server: name.clone(),
            source: Box::new(e),
        })?;

        let tools = service
            .list_all_tools()
            .await
            .map_err(|e| McpError::Connection {
                server: name.clone(),
                source: Box::new(e),
            })?;

        Ok(Self {
            name,
            service,
            tools,
        })
    }

    /// Call a tool on this server.
    pub async fn call_tool(
        &self,
        tool_name: &str,
        arguments: serde_json::Map<String, serde_json::Value>,
    ) -> Result<CallToolResult, McpError> {
        let name = tool_name.to_string();
        self.service
            .call_tool(CallToolRequestParams::new(name).with_arguments(arguments))
            .await
            .map_err(|e| McpError::ToolCall(format!("{}: {}", self.name, e)))
    }

    /// Get the list of tools this server exposes.
    pub fn tools(&self) -> &[rmcp::model::Tool] {
        &self.tools
    }

    /// Get this server's name.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Shut down the connection.
    pub async fn shutdown(self) {
        if let Err(e) = self.service.cancel().await {
            tracing::warn!(server = %self.name, error = ?e, "MCP server shutdown error");
        }
    }
}
