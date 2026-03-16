use std::sync::Arc;

use async_trait::async_trait;
use serde_json::Value;

use qq_core::{Error, Tool, ToolDefinition, ToolOutput, ToolParameters, ToolRef};
use rmcp::model::{RawContent, ResourceContents};

use crate::client::McpClient;

/// Wraps a single MCP tool as a qq_core::Tool.
pub struct McpTool {
    /// Namespaced name: "mcp__<server>__<tool>"
    namespaced_name: String,
    /// Original MCP tool name (for call_tool)
    mcp_tool_name: String,
    /// Server name
    server_name: String,
    /// Human-readable display name: "mcp:<server>/<tool>"
    display_name_str: String,
    /// Tool description
    description: String,
    /// Raw JSON Schema from the MCP server
    input_schema: Value,
    /// Connection to the MCP server
    client: Arc<McpClient>,
}

impl McpTool {
    pub fn new(server_name: &str, mcp_tool: &rmcp::model::Tool, client: Arc<McpClient>) -> Self {
        let namespaced_name = format!("mcp__{}__{}", server_name, mcp_tool.name);
        let display_name_str = format!("mcp:{}/{}", server_name, mcp_tool.name);
        let description = mcp_tool
            .description
            .as_deref()
            .unwrap_or("MCP tool")
            .to_string();

        // Convert Arc<serde_json::Map<String, Value>> to Value::Object
        let input_schema = Value::Object(mcp_tool.input_schema.as_ref().clone());

        Self {
            namespaced_name,
            mcp_tool_name: mcp_tool.name.to_string(),
            server_name: server_name.to_string(),
            display_name_str,
            description,
            input_schema,
            client,
        }
    }

    /// Get a typed `ToolRef` for this MCP tool.
    pub fn tool_ref(&self) -> ToolRef {
        ToolRef::Mcp {
            server: self.server_name.clone(),
            tool: self.mcp_tool_name.clone(),
        }
    }
}

#[async_trait]
impl Tool for McpTool {
    fn name(&self) -> &str {
        &self.namespaced_name
    }

    fn description(&self) -> &str {
        &self.description
    }

    fn display_name(&self) -> &str {
        &self.display_name_str
    }

    fn definition(&self) -> ToolDefinition {
        ToolDefinition::new(&self.namespaced_name, &self.description)
            .with_parameters(ToolParameters::from_raw(self.input_schema.clone()))
    }

    async fn execute(&self, arguments: Value) -> Result<ToolOutput, Error> {
        let args = match arguments {
            Value::Object(map) => map,
            Value::Null => serde_json::Map::new(),
            other => {
                return Ok(ToolOutput::error(format!(
                    "Expected object arguments, got: {}",
                    other
                )));
            }
        };

        match self.client.call_tool(&self.mcp_tool_name, args).await {
            Ok(result) => {
                let is_error = result.is_error.unwrap_or(false);
                let text = result
                    .content
                    .iter()
                    .map(|c| match &c.raw {
                        RawContent::Text(t) => t.text.to_string(),
                        RawContent::Image(i) => {
                            format!("[Image: {}, {} bytes base64]", i.mime_type, i.data.len())
                        }
                        RawContent::Resource(r) => match &r.resource {
                            ResourceContents::TextResourceContents { text, .. } => text.clone(),
                            ResourceContents::BlobResourceContents { blob, .. } => {
                                format!("[Binary resource: {} bytes]", blob.len())
                            }
                        },
                        RawContent::Audio(_) => "[Audio content]".to_string(),
                        _ => "[Unknown content type]".to_string(),
                    })
                    .collect::<Vec<_>>()
                    .join("\n");

                Ok(ToolOutput::with_content(
                    vec![qq_core::TypedContent::Text { text }],
                    is_error,
                ))
            }
            Err(e) => Ok(ToolOutput::error(format!("MCP tool call failed: {}", e))),
        }
    }
}
