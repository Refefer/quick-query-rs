use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;

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

    async fn execute(&self, arguments: Value) -> Result<ToolOutput, Error>;
}

pub struct ToolRegistry {
    tools: std::collections::HashMap<String, Box<dyn Tool>>,
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

    pub fn register(&mut self, tool: Box<dyn Tool>) {
        self.tools.insert(tool.name().to_string(), tool);
    }

    pub fn get(&self, name: &str) -> Option<&dyn Tool> {
        self.tools.get(name).map(|t| t.as_ref())
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
}
