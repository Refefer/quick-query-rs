#![allow(dead_code)]

use rmcp::{
    ServerHandler, ServiceExt,
    handler::server::{router::tool::ToolRouter, wrapper::Parameters},
    model::{ServerCapabilities, ServerInfo},
    schemars, tool, tool_handler, tool_router,
};

use qq_mcp::McpClient;

// -- Request types --

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct EchoRequest {
    #[schemars(description = "The message to echo back")]
    pub message: String,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct AddRequest {
    #[schemars(description = "First number")]
    pub a: i32,
    #[schemars(description = "Second number")]
    pub b: i32,
}

// -- Test MCP Server --

#[derive(Debug, Clone)]
pub struct TestMcpServer {
    tool_router: ToolRouter<Self>,
}

impl TestMcpServer {
    pub fn new() -> Self {
        Self {
            tool_router: Self::tool_router(),
        }
    }
}

impl Default for TestMcpServer {
    fn default() -> Self {
        Self::new()
    }
}

#[tool_router]
impl TestMcpServer {
    #[tool(description = "Echo back the given message")]
    fn echo(&self, Parameters(EchoRequest { message }): Parameters<EchoRequest>) -> String {
        message
    }

    #[tool(description = "Add two numbers together")]
    fn add(&self, Parameters(AddRequest { a, b }): Parameters<AddRequest>) -> String {
        (a + b).to_string()
    }
}

#[tool_handler(router = self.tool_router)]
impl ServerHandler for TestMcpServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
            .with_instructions("A test MCP server with echo and add tools")
    }
}

// -- Duplex helper --

/// Create an McpClient connected to a TestMcpServer via in-process duplex transport.
///
/// Returns the client and a join handle for the server task.
pub async fn create_duplex_client(
    server_name: &str,
) -> (McpClient, tokio::task::JoinHandle<()>) {
    let (server_io, client_io) = tokio::io::duplex(4096);

    // Split for server side
    let (server_read, server_write) = tokio::io::split(server_io);
    let server = TestMcpServer::new();
    let server_handle = tokio::spawn(async move {
        let service = server.serve((server_read, server_write)).await.unwrap();
        // Keep server alive until cancelled
        let _ = service.waiting().await;
    });

    // Split for client side
    let (client_read, client_write) = tokio::io::split(client_io);
    let service = ()
        .serve((client_read, client_write))
        .await
        .expect("client service should start");

    let client = McpClient::from_service(server_name.to_string(), service)
        .await
        .expect("client should connect and list tools");

    (client, server_handle)
}
