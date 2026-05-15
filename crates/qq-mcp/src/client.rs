use std::collections::HashMap;
use std::process::Stdio;

use rmcp::model::{CallToolRequestParams, CallToolResult};
use rmcp::service::RunningService;
use rmcp::transport::child_process::TokioChildProcess;
use rmcp::transport::streamable_http_client::{
    StreamableHttpClientTransport, StreamableHttpClientTransportConfig,
};
use rmcp::{RoleClient, ServiceExt};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::ChildStderr;

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
    ///
    /// Captures the child's stderr (rather than letting it inherit
    /// qq's terminal — see `spawn_stderr_pump` for the why) and
    /// forwards each line through `tracing::info!` so it lands
    /// wherever qq's subscriber points (sink in TUI mode, the
    /// configured file under `--log-file`, stderr in `--no-tui`
    /// mode).
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

        let (process, child_stderr) = TokioChildProcess::builder(cmd)
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| McpError::Connection {
                server: name.clone(),
                source: Box::new(e),
            })?;

        if let Some(stderr) = child_stderr {
            spawn_stderr_pump(name.clone(), stderr);
        }

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

/** Spawn a fire-and-forget tokio task that drains a child process's
stderr and re-emits each line through qq's tracing pipeline.

Without this, MCP server subprocesses inherit qq's stderr (rmcp's
`TokioChildProcessBuilder::new` default is `Stdio::inherit`) and any
`tracing::warn!` they emit — including the `rmcp::service:
response error` lines fired when an upstream HTTP request returns
4xx/5xx — write raw bytes onto the same TTY as the ratatui alternate
screen and corrupt the TUI. Re-routing through `tracing::info!` at
the `mcp_server` target keeps the diagnostics available via
`--log-file` or `RUST_LOG=mcp_server=info` while letting the default
TUI-mode sink writer swallow them.

The handle is intentionally **not** returned. The task self-terminates
when the subprocess exits: the kernel closes the write end of the
stderr pipe, `BufReader::lines().next_line()` returns `Ok(None)`, the
loop falls through, the task completes. The subprocess in turn dies
when `McpClient::shutdown` (or `Drop`) tears down the rmcp transport,
which kills the child via `TokioChildProcess::graceful_shutdown` /
its `ChildWithCleanup` Drop impl. So the pump's lifetime is already
correctly bounded by the connection's, without us tracking it.
*/
fn spawn_stderr_pump(server: String, stderr: ChildStderr) {
    tokio::spawn(async move {
        let mut lines = BufReader::new(stderr).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            tracing::info!(target: "mcp_server", server = %server, "{}", line);
        }
    });
}
