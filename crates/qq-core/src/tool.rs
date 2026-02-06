use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::sync::Arc;

use crate::error::Error;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolDefinition {
    pub name: String,
    pub description: String,
    pub parameters: ToolParameters,
}

impl ToolDefinition {
    pub fn new(name: impl Into<String>, description: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            description: description.into(),
            parameters: ToolParameters::default(),
        }
    }

    pub fn with_parameters(mut self, parameters: ToolParameters) -> Self {
        self.parameters = parameters;
        self
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolParameters {
    #[serde(rename = "type")]
    pub schema_type: String,
    #[serde(default)]
    pub properties: std::collections::HashMap<String, PropertySchema>,
    #[serde(default)]
    pub required: Vec<String>,
    #[serde(rename = "additionalProperties", default)]
    pub additional_properties: bool,
}

impl Default for ToolParameters {
    fn default() -> Self {
        Self {
            schema_type: "object".to_string(),
            properties: std::collections::HashMap::new(),
            required: Vec::new(),
            additional_properties: false,
        }
    }
}

impl ToolParameters {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn add_property(
        mut self,
        name: impl Into<String>,
        schema: PropertySchema,
        required: bool,
    ) -> Self {
        let name = name.into();
        self.properties.insert(name.clone(), schema);
        if required {
            self.required.push(name);
        }
        self
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PropertySchema {
    #[serde(rename = "type")]
    pub schema_type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(rename = "enum", skip_serializing_if = "Option::is_none")]
    pub enum_values: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub items: Option<Box<PropertySchema>>,
}

impl PropertySchema {
    pub fn string(description: impl Into<String>) -> Self {
        Self {
            schema_type: "string".to_string(),
            description: Some(description.into()),
            enum_values: None,
            default: None,
            items: None,
        }
    }

    pub fn integer(description: impl Into<String>) -> Self {
        Self {
            schema_type: "integer".to_string(),
            description: Some(description.into()),
            enum_values: None,
            default: None,
            items: None,
        }
    }

    pub fn number(description: impl Into<String>) -> Self {
        Self {
            schema_type: "number".to_string(),
            description: Some(description.into()),
            enum_values: None,
            default: None,
            items: None,
        }
    }

    pub fn boolean(description: impl Into<String>) -> Self {
        Self {
            schema_type: "boolean".to_string(),
            description: Some(description.into()),
            enum_values: None,
            default: None,
            items: None,
        }
    }

    pub fn array(description: impl Into<String>, items: PropertySchema) -> Self {
        Self {
            schema_type: "array".to_string(),
            description: Some(description.into()),
            enum_values: None,
            default: None,
            items: Some(Box::new(items)),
        }
    }

    pub fn enum_string(description: impl Into<String>, values: Vec<String>) -> Self {
        Self {
            schema_type: "string".to_string(),
            description: Some(description.into()),
            enum_values: Some(values),
            default: None,
            items: None,
        }
    }

    pub fn with_default(mut self, default: Value) -> Self {
        self.default = Some(default);
        self
    }
}

#[derive(Debug, Clone)]
pub struct ToolOutput {
    pub content: String,
    pub is_error: bool,
}

impl ToolOutput {
    pub fn success(content: impl Into<String>) -> Self {
        Self {
            content: content.into(),
            is_error: false,
        }
    }

    pub fn error(content: impl Into<String>) -> Self {
        Self {
            content: content.into(),
            is_error: true,
        }
    }
}

#[async_trait]
pub trait Tool: Send + Sync {
    fn name(&self) -> &str;

    fn description(&self) -> &str;

    fn definition(&self) -> ToolDefinition;

    /// Whether this tool performs blocking (CPU-bound or synchronous I/O) work.
    ///
    /// When true, `execute_tool_dispatch` routes execution through
    /// `tokio::task::spawn_blocking` to keep the async executor free.
    fn is_blocking(&self) -> bool {
        false
    }

    async fn execute(&self, arguments: Value) -> Result<ToolOutput, Error>;
}

#[derive(Clone)]
pub struct ToolRegistry {
    tools: std::collections::HashMap<String, Arc<dyn Tool>>,
}

impl Default for ToolRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl ToolRegistry {
    pub fn new() -> Self {
        Self {
            tools: std::collections::HashMap::new(),
        }
    }

    pub fn register(&mut self, tool: Arc<dyn Tool>) {
        self.tools.insert(tool.name().to_string(), tool);
    }

    /// Register a boxed tool (convenience for backward compatibility)
    pub fn register_boxed(&mut self, tool: Box<dyn Tool>) {
        let arc: Arc<dyn Tool> = Arc::from(tool);
        self.tools.insert(arc.name().to_string(), arc);
    }

    pub fn get(&self, name: &str) -> Option<&dyn Tool> {
        self.tools.get(name).map(|t| t.as_ref())
    }

    /// Get a cloned Arc reference to a tool
    pub fn get_arc(&self, name: &str) -> Option<Arc<dyn Tool>> {
        self.tools.get(name).cloned()
    }

    pub fn definitions(&self) -> Vec<ToolDefinition> {
        self.tools.values().map(|t| t.definition()).collect()
    }

    pub fn names(&self) -> Vec<&str> {
        self.tools.keys().map(|s| s.as_str()).collect()
    }

    pub fn is_empty(&self) -> bool {
        self.tools.is_empty()
    }

    pub fn len(&self) -> usize {
        self.tools.len()
    }

    /// Create a subset registry containing only the specified tools.
    ///
    /// Tools not found in the registry are silently ignored.
    pub fn subset(&self, tool_names: &[String]) -> Self {
        let mut new_registry = Self::new();
        for name in tool_names {
            if let Some(tool) = self.tools.get(name) {
                new_registry.tools.insert(name.clone(), Arc::clone(tool));
            }
        }
        new_registry
    }

    /// Create a subset registry from a slice of &str tool names.
    pub fn subset_from_strs(&self, tool_names: &[&str]) -> Self {
        let owned: Vec<String> = tool_names.iter().map(|s| s.to_string()).collect();
        self.subset(&owned)
    }
}

/// Execute a tool, routing blocking tools through `spawn_blocking`.
///
/// Non-blocking tools run directly on the async executor. Blocking tools
/// (where `is_blocking()` returns true) are dispatched to a blocking thread
/// via `tokio::task::spawn_blocking`, using `Handle::block_on` to drive the
/// async `execute()` method to completion on that thread.
pub async fn execute_tool_dispatch(
    tool: Arc<dyn Tool>,
    arguments: Value,
) -> Result<ToolOutput, crate::Error> {
    if tool.is_blocking() {
        let handle = tokio::runtime::Handle::current();
        tokio::task::spawn_blocking(move || handle.block_on(tool.execute(arguments)))
            .await
            .map_err(|e| crate::Error::Unknown(format!("Blocking tool task failed: {}", e)))?
    } else {
        tool.execute(arguments).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tool_definition() {
        let def = ToolDefinition::new("read_file", "Read contents of a file")
            .with_parameters(
                ToolParameters::new()
                    .add_property("path", PropertySchema::string("Path to the file"), true),
            );

        assert_eq!(def.name, "read_file");
        assert!(def.parameters.required.contains(&"path".to_string()));
    }

    #[test]
    fn test_property_schema() {
        let schema = PropertySchema::string("A test string");
        assert_eq!(schema.schema_type, "string");

        let enum_schema = PropertySchema::enum_string(
            "A choice",
            vec!["a".to_string(), "b".to_string()],
        );
        assert_eq!(enum_schema.enum_values.unwrap().len(), 2);
    }

    #[test]
    fn test_tool_output() {
        let success = ToolOutput::success("done");
        assert!(!success.is_error);

        let error = ToolOutput::error("failed");
        assert!(error.is_error);
    }

    /// A test tool that is non-blocking (default).
    struct NonBlockingTool;

    #[async_trait]
    impl Tool for NonBlockingTool {
        fn name(&self) -> &str { "non_blocking" }
        fn description(&self) -> &str { "test" }
        fn definition(&self) -> ToolDefinition {
            ToolDefinition::new("non_blocking", "test")
        }
        async fn execute(&self, _arguments: Value) -> Result<ToolOutput, crate::Error> {
            Ok(ToolOutput::success("non_blocking_result"))
        }
    }

    /// A test tool that declares itself as blocking.
    struct BlockingTool;

    #[async_trait]
    impl Tool for BlockingTool {
        fn name(&self) -> &str { "blocking" }
        fn description(&self) -> &str { "test" }
        fn definition(&self) -> ToolDefinition {
            ToolDefinition::new("blocking", "test")
        }
        fn is_blocking(&self) -> bool { true }
        async fn execute(&self, _arguments: Value) -> Result<ToolOutput, crate::Error> {
            Ok(ToolOutput::success("blocking_result"))
        }
    }

    #[test]
    fn test_is_blocking_default() {
        let tool = NonBlockingTool;
        assert!(!tool.is_blocking());
    }

    #[test]
    fn test_is_blocking_override() {
        let tool = BlockingTool;
        assert!(tool.is_blocking());
    }

    #[tokio::test]
    async fn test_execute_tool_dispatch_non_blocking() {
        let tool: Arc<dyn Tool> = Arc::new(NonBlockingTool);
        let result = execute_tool_dispatch(tool, Value::Null).await.unwrap();
        assert_eq!(result.content, "non_blocking_result");
        assert!(!result.is_error);
    }

    #[tokio::test]
    async fn test_execute_tool_dispatch_blocking() {
        let tool: Arc<dyn Tool> = Arc::new(BlockingTool);
        let result = execute_tool_dispatch(tool, Value::Null).await.unwrap();
        assert_eq!(result.content, "blocking_result");
        assert!(!result.is_error);
    }
}
