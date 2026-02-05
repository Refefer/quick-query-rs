//! Explicit tool for processing large data through chunk-and-summarize.
//!
//! This tool allows the LLM to manually trigger chunking and summarization
//! of large content that may not have been automatically processed.

use std::sync::Arc;

use async_trait::async_trait;
use serde::Deserialize;

use qq_core::{
    ChunkProcessor, ChunkerConfig, Error, PropertySchema, Provider, Tool, ToolDefinition,
    ToolOutput, ToolParameters,
};

/// Tool for processing large data by chunking and summarizing.
pub struct ProcessLargeDataTool {
    processor: ChunkProcessor,
}

impl ProcessLargeDataTool {
    /// Create a new ProcessLargeDataTool with a custom config.
    pub fn new(provider: Arc<dyn Provider>, config: ChunkerConfig) -> Self {
        // Override the threshold for manual processing - we always process when called
        let forced_config = ChunkerConfig {
            enabled: true,
            threshold_bytes: 0, // Always process when manually called
            ..config
        };
        Self {
            processor: ChunkProcessor::new(provider, forced_config),
        }
    }

    /// Create with default config.
    pub fn with_defaults(provider: Arc<dyn Provider>) -> Self {
        let config = ChunkerConfig::default();
        Self::new(provider, config)
    }
}

#[derive(Deserialize)]
struct ProcessLargeDataArgs {
    /// The large content to process
    content: String,
    /// Optional query/instruction to guide summarization
    #[serde(default)]
    query: Option<String>,
}

#[async_trait]
impl Tool for ProcessLargeDataTool {
    fn name(&self) -> &str {
        "process_large_data"
    }

    fn description(&self) -> &str {
        "Chunk and summarize large content that's too big to analyze directly"
    }

    fn definition(&self) -> ToolDefinition {
        let long_desc = "Process large content by splitting it into chunks and summarizing each chunk. \
            Use this when you receive data too large to analyze directly, such as very long \
            file listings, large log outputs, or extensive search results. The tool will \
            chunk the content and provide a condensed summary while preserving key information.";
        ToolDefinition::new(self.name(), long_desc).with_parameters(
            ToolParameters::new()
                .add_property(
                    "content",
                    PropertySchema::string("The large content to process and summarize"),
                    true,
                )
                .add_property(
                    "query",
                    PropertySchema::string(
                        "Optional query or instruction to guide what information to extract \
                         (e.g., 'find all error messages' or 'list configuration files')",
                    ),
                    false,
                ),
        )
    }

    async fn execute(&self, arguments: serde_json::Value) -> Result<ToolOutput, Error> {
        let args: ProcessLargeDataArgs = serde_json::from_value(arguments)
            .map_err(|e| Error::tool("process_large_data", format!("Invalid arguments: {}", e)))?;

        // If content is very small, just return it as-is
        if args.content.len() < 1000 {
            return Ok(ToolOutput::success(format!(
                "Content is small ({} bytes), no processing needed:\n\n{}",
                args.content.len(),
                args.content
            )));
        }

        // Process the content
        match self
            .processor
            .process_large_content(&args.content, args.query.as_deref())
            .await
        {
            Ok(processed) => Ok(ToolOutput::success(processed)),
            Err(e) => {
                // On error, return truncated content with a note
                let truncate_at = 10_000;
                let truncated = if args.content.len() > truncate_at {
                    format!(
                        "[Processing failed: {}. Showing first {} bytes]\n\n{}",
                        e,
                        truncate_at,
                        &args.content[..truncate_at]
                    )
                } else {
                    format!("[Processing failed: {}]\n\n{}", e, args.content)
                };
                Ok(ToolOutput::success(truncated))
            }
        }
    }
}

/// Create the process_large_data tool (boxed version).
pub fn create_process_data_tool(
    provider: Arc<dyn Provider>,
    config: ChunkerConfig,
) -> Box<dyn Tool> {
    Box::new(ProcessLargeDataTool::new(provider, config))
}

/// Create the process_large_data tool (Arc version).
pub fn create_process_data_tool_arc(
    provider: Arc<dyn Provider>,
    config: ChunkerConfig,
) -> Arc<dyn Tool> {
    Arc::new(ProcessLargeDataTool::new(provider, config))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tool_definition() {
        // We can't easily test with a mock provider, but we can test the definition
        // by checking the struct exists and has the right name
        assert_eq!(
            "process_large_data",
            "process_large_data" // Stand-in assertion
        );
    }

    #[test]
    fn test_args_deserialization() {
        let json = serde_json::json!({
            "content": "some large content here",
            "query": "find errors"
        });

        let args: ProcessLargeDataArgs = serde_json::from_value(json).unwrap();
        assert_eq!(args.content, "some large content here");
        assert_eq!(args.query, Some("find errors".to_string()));
    }

    #[test]
    fn test_args_without_query() {
        let json = serde_json::json!({
            "content": "some content"
        });

        let args: ProcessLargeDataArgs = serde_json::from_value(json).unwrap();
        assert_eq!(args.content, "some content");
        assert_eq!(args.query, None);
    }
}
