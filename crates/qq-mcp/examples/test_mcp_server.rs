use rmcp::{
    ServerHandler, ServiceExt,
    handler::server::{router::tool::ToolRouter, wrapper::Parameters},
    model::{ServerCapabilities, ServerInfo},
    schemars, tool, tool_handler, tool_router,
    transport::io::stdio,
};

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
struct EchoRequest {
    #[schemars(description = "The message to echo back")]
    message: String,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
struct AddRequest {
    #[schemars(description = "First number")]
    a: i32,
    #[schemars(description = "Second number")]
    b: i32,
}

#[derive(Debug, Clone)]
struct TestMcpServer {
    tool_router: ToolRouter<Self>,
}

impl TestMcpServer {
    fn new() -> Self {
        Self {
            tool_router: Self::tool_router(),
        }
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

fn main() {
    let rt = tokio::runtime::Runtime::new().unwrap();
    rt.block_on(async {
        let server = TestMcpServer::new();
        let service = server.serve(stdio()).await.unwrap();
        let _ = service.waiting().await;
    });
}
