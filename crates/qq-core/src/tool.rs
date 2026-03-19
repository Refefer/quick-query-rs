use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::fmt;
use std::sync::Arc;

use crate::error::Error;

// =============================================================================
// ToolRef — A resolved reference to a single tool
// =============================================================================

/// A typed reference to a single tool, distinguishing internal vs MCP routing.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum ToolRef {
    /// Built-in tool. Wire name = the tool name itself (e.g., "run").
    Internal(String),
    /// MCP tool. Wire name = "mcp__<server>__<tool>".
    Mcp { server: String, tool: String },
}

impl ToolRef {
    /// Flat string for LLM API: `"run"` or `"mcp__ws__web_search"`.
    pub fn wire_name(&self) -> String {
        match self {
            ToolRef::Internal(name) => name.clone(),
            ToolRef::Mcp { server, tool } => format!("mcp__{}__{}", server, tool),
        }
    }

    /// URI-style name for display: `"run"` or `"mcp:ws/web_search"`.
    pub fn display_name(&self) -> String {
        match self {
            ToolRef::Internal(name) => name.clone(),
            ToolRef::Mcp { server, tool } => format!("mcp:{}/{}", server, tool),
        }
    }

    /// Parse a mangled wire name back into a `ToolRef`.
    ///
    /// If the string matches `mcp__<server>__<tool>` (split on first two `__`
    /// segments), it's `Mcp`; otherwise `Internal`.
    pub fn from_wire_name(s: &str) -> Self {
        if let Some(rest) = s.strip_prefix("mcp__") {
            if let Some((server, tool)) = rest.split_once("__") {
                return ToolRef::Mcp {
                    server: server.to_string(),
                    tool: tool.to_string(),
                };
            }
        }
        ToolRef::Internal(s.to_string())
    }

    /// Parse a config/URI-style reference into a `ToolRef`.
    ///
    /// - `"mcp:ws/web_search"` → `Mcp { ws, web_search }`
    /// - `"internal:run"` → `Internal("run")`
    /// - `"run"` (unqualified) → `Internal("run")`
    pub fn from_uri(s: &str) -> Self {
        if let Some(rest) = s.strip_prefix("mcp:") {
            if let Some((server, tool)) = rest.split_once('/') {
                return ToolRef::Mcp {
                    server: server.to_string(),
                    tool: tool.to_string(),
                };
            }
        }
        if let Some(rest) = s.strip_prefix("internal:") {
            return ToolRef::Internal(rest.to_string());
        }
        ToolRef::Internal(s.to_string())
    }
}

impl fmt::Display for ToolRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.display_name())
    }
}

// =============================================================================
// ToolPattern — A pattern that matches one or more tools
// =============================================================================

/// A pattern that matches one or more tools.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ToolPattern {
    /// Match a specific tool.
    Exact(ToolRef),
    /// Match all tools from an MCP server: `"mcp:ws/*"`.
    McpGlob(String),
    /// Match all internal tools: `"internal:*"`.
    AllInternal,
}

impl ToolPattern {
    /// Parse a pattern string into a `ToolPattern`.
    ///
    /// - `"mcp:ws/*"` → `McpGlob("ws")`
    /// - `"internal:*"` → `AllInternal`
    /// - anything else → `Exact(ToolRef::from_uri(s))`
    pub fn parse(s: &str) -> Self {
        if let Some(rest) = s.strip_prefix("mcp:") {
            if let Some(server) = rest.strip_suffix("/*") {
                return ToolPattern::McpGlob(server.to_string());
            }
        }
        if s == "internal:*" {
            return ToolPattern::AllInternal;
        }
        ToolPattern::Exact(ToolRef::from_uri(s))
    }
}

impl fmt::Display for ToolPattern {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ToolPattern::Exact(r) => write!(f, "{}", r),
            ToolPattern::McpGlob(server) => write!(f, "mcp:{}/*", server),
            ToolPattern::AllInternal => write!(f, "internal:*"),
        }
    }
}

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

/// Tool parameter schema. Either a structured schema built with `add_property()` (used by
/// built-in tools) or a raw JSON Schema passthrough (used for MCP tools).
#[derive(Debug, Clone)]
pub enum ToolParameters {
    /// Structured schema built with add_property() -- used by built-in tools
    Structured {
        schema_type: String,
        properties: std::collections::HashMap<String, PropertySchema>,
        required: Vec<String>,
        additional_properties: bool,
    },
    /// Raw JSON Schema passthrough -- used for MCP tools
    Raw(serde_json::Value),
}

impl Serialize for ToolParameters {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            ToolParameters::Structured {
                schema_type,
                properties,
                required,
                additional_properties,
            } => {
                use serde::ser::SerializeMap;
                let mut map = serializer.serialize_map(None)?;
                map.serialize_entry("type", schema_type)?;
                map.serialize_entry("properties", properties)?;
                map.serialize_entry("required", required)?;
                map.serialize_entry("additionalProperties", additional_properties)?;
                map.end()
            }
            ToolParameters::Raw(value) => value.serialize(serializer),
        }
    }
}

impl<'de> Deserialize<'de> for ToolParameters {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = serde_json::Value::deserialize(deserializer)?;
        // Try to parse as a structured schema (has "type" and "properties" fields)
        if let Some(obj) = value.as_object() {
            if obj.contains_key("type") && obj.contains_key("properties") {
                if let (Some(schema_type), Some(properties)) = (
                    obj.get("type").and_then(|v| v.as_str()),
                    obj.get("properties").and_then(|v| v.as_object()),
                ) {
                    let properties: std::collections::HashMap<String, PropertySchema> = properties
                        .iter()
                        .filter_map(|(k, v)| {
                            serde_json::from_value(v.clone()).ok().map(|s| (k.clone(), s))
                        })
                        .collect();
                    let required: Vec<String> = obj
                        .get("required")
                        .and_then(|v| serde_json::from_value(v.clone()).ok())
                        .unwrap_or_default();
                    let additional_properties = obj
                        .get("additionalProperties")
                        .and_then(|v| v.as_bool())
                        .unwrap_or(false);
                    return Ok(ToolParameters::Structured {
                        schema_type: schema_type.to_string(),
                        properties,
                        required,
                        additional_properties,
                    });
                }
            }
        }
        Ok(ToolParameters::Raw(value))
    }
}

impl Default for ToolParameters {
    fn default() -> Self {
        Self::Structured {
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

    /// Create from a raw JSON Schema value (used for MCP tools).
    pub fn from_raw(value: serde_json::Value) -> Self {
        ToolParameters::Raw(value)
    }

    pub fn add_property(
        mut self,
        name: impl Into<String>,
        schema: PropertySchema,
        required: bool,
    ) -> Self {
        match &mut self {
            ToolParameters::Structured {
                properties: props,
                required: req,
                ..
            } => {
                let name = name.into();
                props.insert(name.clone(), schema);
                if required {
                    req.push(name);
                }
            }
            ToolParameters::Raw(_) => {
                panic!("Cannot add_property to a Raw ToolParameters");
            }
        }
        self
    }

    /// Access the required fields (for testing/inspection). Returns empty slice for Raw.
    pub fn required(&self) -> &[String] {
        match self {
            ToolParameters::Structured { required, .. } => required,
            ToolParameters::Raw(_) => &[],
        }
    }

    /// Access the properties map (for testing/inspection). Returns None for Raw.
    pub fn properties(&self) -> Option<&std::collections::HashMap<String, PropertySchema>> {
        match self {
            ToolParameters::Structured { properties, .. } => Some(properties),
            ToolParameters::Raw(_) => None,
        }
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
    pub content: Vec<crate::message::TypedContent>,
    pub is_error: bool,
}

impl ToolOutput {
    pub fn success(content: impl Into<crate::message::TypedContent>) -> Self {
        Self {
            content: vec![content.into()],
            is_error: false,
        }
    }

    pub fn error(content: impl Into<crate::message::TypedContent>) -> Self {
        Self {
            content: vec![content.into()],
            is_error: true,
        }
    }

    pub fn with_content(content: Vec<crate::message::TypedContent>, is_error: bool) -> Self {
        Self { content, is_error }
    }

    /// Extract text content, concatenating text parts only.
    pub fn text_content(&self) -> String {
        self.content
            .iter()
            .filter_map(|c| match c {
                crate::message::TypedContent::Text { text } => Some(text.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("")
    }
}

#[async_trait]
pub trait Tool: Send + Sync {
    fn name(&self) -> &str;

    /// Short summary for user display (e.g., /tools command).
    fn description(&self) -> &str;

    /// Human-readable display name. Default: same as name() (wire name).
    fn display_name(&self) -> &str {
        self.name()
    }

    /// Rich description sent to LLMs as part of the tool definition.
    /// Includes usage examples, dos/don'ts, and behavioral guidance.
    /// Default: falls back to `description()`.
    fn tool_description(&self) -> &str {
        self.description()
    }

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

    /// Resolve config-style tool references to actual registry tool names.
    ///
    /// Supported formats:
    /// - `internal:run` — built-in tool (resolves to `run`)
    /// - `internal:*` — glob: all non-MCP tools in the registry
    /// - `mcp:server/tool` — MCP tool (resolves to `mcp__server__tool`)
    /// - `mcp:server/*` — glob: all tools from the named MCP server
    ///
    /// Unqualified names (no `:` scheme) are passed through as-is for use by
    /// internal agent code that builds tool lists programmatically.
    pub fn resolve_tool_refs(&self, refs: &[String]) -> Vec<String> {
        let mut result = Vec::new();
        for r in refs {
            if let Some(rest) = r.strip_prefix("mcp:") {
                if let Some(server) = rest.strip_suffix("/*") {
                    // Glob: mcp:server/* → all mcp__server__* tools
                    let prefix = format!("mcp__{}__", server);
                    for name in self.tools.keys() {
                        if name.starts_with(&prefix) && !result.contains(name) {
                            result.push(name.clone());
                        }
                    }
                } else if let Some((server, tool)) = rest.split_once('/') {
                    // Specific: mcp:server/tool → mcp__server__tool
                    let mangled = format!("mcp__{}__{}",  server, tool);
                    if !result.contains(&mangled) {
                        result.push(mangled);
                    }
                }
            } else if let Some(rest) = r.strip_prefix("internal:") {
                if rest == "*" {
                    // Glob: internal:* → all non-MCP tools
                    for name in self.tools.keys() {
                        if !name.starts_with("mcp__") && !result.contains(name) {
                            result.push(name.clone());
                        }
                    }
                } else if !result.contains(&rest.to_string()) {
                    result.push(rest.to_string());
                }
            } else {
                // Unqualified name — pass through (used by internal agent code)
                if !result.contains(r) {
                    result.push(r.clone());
                }
            }
        }
        result
    }

    /// Resolve config-style tool limit keys to actual registry tool names.
    ///
    /// Uses the same resolution rules as `resolve_tool_refs`. Glob patterns
    /// expand to all matching tools, each receiving the same limit value.
    pub fn resolve_tool_limits(
        &self,
        limits: std::collections::HashMap<String, usize>,
    ) -> std::collections::HashMap<String, usize> {
        let mut resolved = std::collections::HashMap::new();
        for (key, value) in limits {
            for name in self.resolve_tool_refs(&[key]) {
                resolved.insert(name, value);
            }
        }
        resolved
    }

    /// Resolve typed `ToolPattern`s to actual registry tool names.
    pub fn resolve_patterns(&self, patterns: &[ToolPattern]) -> Vec<String> {
        let mut result = Vec::new();
        for pattern in patterns {
            match pattern {
                ToolPattern::Exact(ref_) => {
                    let name = ref_.wire_name();
                    if !result.contains(&name) {
                        result.push(name);
                    }
                }
                ToolPattern::McpGlob(server) => {
                    let prefix = format!("mcp__{}__", server);
                    for name in self.tools.keys() {
                        if name.starts_with(&prefix) && !result.contains(name) {
                            result.push(name.clone());
                        }
                    }
                }
                ToolPattern::AllInternal => {
                    for name in self.tools.keys() {
                        if !name.starts_with("mcp__") && !result.contains(name) {
                            result.push(name.clone());
                        }
                    }
                }
            }
        }
        result
    }

    /// Resolve typed `ToolPattern` limit keys to actual registry tool names.
    pub fn resolve_pattern_limits(
        &self,
        limits: std::collections::HashMap<ToolPattern, usize>,
    ) -> std::collections::HashMap<String, usize> {
        let mut resolved = std::collections::HashMap::new();
        for (pattern, value) in limits {
            for name in self.resolve_patterns(&[pattern]) {
                resolved.insert(name, value);
            }
        }
        resolved
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
        assert!(def.parameters.required().contains(&"path".to_string()));
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
        assert_eq!(success.text_content(), "done");

        let error = ToolOutput::error("failed");
        assert!(error.is_error);
        assert_eq!(error.text_content(), "failed");
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
    fn test_tool_description_defaults_to_description() {
        let tool = NonBlockingTool;
        assert_eq!(tool.tool_description(), tool.description());
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
        assert_eq!(result.text_content(), "non_blocking_result");
        assert!(!result.is_error);
    }

    #[tokio::test]
    async fn test_execute_tool_dispatch_blocking() {
        let tool: Arc<dyn Tool> = Arc::new(BlockingTool);
        let result = execute_tool_dispatch(tool, Value::Null).await.unwrap();
        assert_eq!(result.text_content(), "blocking_result");
        assert!(!result.is_error);
    }

    /// Helper to create a named stub tool for registry tests.
    struct NamedTool(&'static str);
    #[async_trait]
    impl Tool for NamedTool {
        fn name(&self) -> &str { self.0 }
        fn description(&self) -> &str { "test" }
        fn definition(&self) -> ToolDefinition { ToolDefinition::new(self.0, "test") }
        async fn execute(&self, _: Value) -> Result<ToolOutput, crate::Error> {
            Ok(ToolOutput::success("ok"))
        }
    }

    fn registry_with(names: &[&'static str]) -> ToolRegistry {
        let mut reg = ToolRegistry::new();
        for name in names {
            reg.register(Arc::new(NamedTool(name)));
        }
        reg
    }

    #[test]
    fn test_resolve_internal_specific() {
        let reg = registry_with(&["run", "read_file"]);
        let resolved = reg.resolve_tool_refs(&["internal:run".into()]);
        assert_eq!(resolved, vec!["run"]);
    }

    #[test]
    fn test_resolve_internal_glob() {
        let reg = registry_with(&["run", "read_file", "mcp__ws__web_search"]);
        let mut resolved = reg.resolve_tool_refs(&["internal:*".into()]);
        resolved.sort();
        assert_eq!(resolved, vec!["read_file", "run"]);
        assert!(!resolved.contains(&"mcp__ws__web_search".to_string()));
    }

    #[test]
    fn test_resolve_mcp_specific() {
        let reg = registry_with(&["mcp__ws__web_search", "mcp__ws__fetch_webpage"]);
        let resolved = reg.resolve_tool_refs(&["mcp:ws/web_search".into()]);
        assert_eq!(resolved, vec!["mcp__ws__web_search"]);
    }

    #[test]
    fn test_resolve_mcp_glob() {
        let reg = registry_with(&[
            "mcp__ws__web_search",
            "mcp__ws__fetch_webpage",
            "mcp__other__something",
            "run",
        ]);
        let mut resolved = reg.resolve_tool_refs(&["mcp:ws/*".into()]);
        resolved.sort();
        assert_eq!(resolved, vec!["mcp__ws__fetch_webpage", "mcp__ws__web_search"]);
    }

    #[test]
    fn test_resolve_mixed() {
        let reg = registry_with(&[
            "run", "read_file",
            "mcp__ws__web_search", "mcp__ws__fetch_webpage",
        ]);
        let resolved = reg.resolve_tool_refs(&[
            "internal:run".into(),
            "mcp:ws/*".into(),
            "internal:read_file".into(),
        ]);
        assert!(resolved.contains(&"run".to_string()));
        assert!(resolved.contains(&"read_file".to_string()));
        assert!(resolved.contains(&"mcp__ws__web_search".to_string()));
        assert!(resolved.contains(&"mcp__ws__fetch_webpage".to_string()));
        assert_eq!(resolved.len(), 4);
    }

    #[test]
    fn test_resolve_no_duplicates() {
        let reg = registry_with(&["mcp__ws__web_search"]);
        let resolved = reg.resolve_tool_refs(&[
            "mcp:ws/web_search".into(),
            "mcp:ws/*".into(),
        ]);
        assert_eq!(resolved, vec!["mcp__ws__web_search"]);
    }

    #[test]
    fn test_resolve_tool_limits() {
        let reg = registry_with(&[
            "run", "mcp__ws__web_search", "mcp__ws__fetch_webpage",
        ]);
        let mut limits = std::collections::HashMap::new();
        limits.insert("run".into(), 10);
        limits.insert("mcp:ws/web_search".into(), 2);
        limits.insert("mcp:ws/fetch_webpage".into(), 5);

        let resolved = reg.resolve_tool_limits(limits);
        assert_eq!(resolved.get("run"), Some(&10));
        assert_eq!(resolved.get("mcp__ws__web_search"), Some(&2));
        assert_eq!(resolved.get("mcp__ws__fetch_webpage"), Some(&5));
        assert_eq!(resolved.len(), 3);
    }

    #[test]
    fn test_resolve_tool_limits_glob() {
        let reg = registry_with(&["mcp__ws__web_search", "mcp__ws__fetch_webpage"]);
        let mut limits = std::collections::HashMap::new();
        limits.insert("mcp:ws/*".into(), 3);

        let resolved = reg.resolve_tool_limits(limits);
        assert_eq!(resolved.get("mcp__ws__web_search"), Some(&3));
        assert_eq!(resolved.get("mcp__ws__fetch_webpage"), Some(&3));
    }

    // =========================================================================
    // ToolRef tests
    // =========================================================================

    #[test]
    fn test_tool_ref_internal_round_trip() {
        let r = ToolRef::Internal("run".to_string());
        assert_eq!(r.wire_name(), "run");
        assert_eq!(r.display_name(), "run");
        assert_eq!(ToolRef::from_wire_name("run"), r);
    }

    #[test]
    fn test_tool_ref_mcp_round_trip() {
        let r = ToolRef::Mcp {
            server: "ws".to_string(),
            tool: "web_search".to_string(),
        };
        assert_eq!(r.wire_name(), "mcp__ws__web_search");
        assert_eq!(r.display_name(), "mcp:ws/web_search");
        assert_eq!(ToolRef::from_wire_name("mcp__ws__web_search"), r);
    }

    #[test]
    fn test_tool_ref_display() {
        let internal = ToolRef::Internal("run".to_string());
        assert_eq!(format!("{}", internal), "run");

        let mcp = ToolRef::Mcp {
            server: "ws".to_string(),
            tool: "web_search".to_string(),
        };
        assert_eq!(format!("{}", mcp), "mcp:ws/web_search");
    }

    #[test]
    fn test_tool_ref_from_uri() {
        assert_eq!(
            ToolRef::from_uri("mcp:ws/web_search"),
            ToolRef::Mcp {
                server: "ws".to_string(),
                tool: "web_search".to_string(),
            }
        );
        assert_eq!(
            ToolRef::from_uri("internal:run"),
            ToolRef::Internal("run".to_string())
        );
        assert_eq!(
            ToolRef::from_uri("run"),
            ToolRef::Internal("run".to_string())
        );
    }

    #[test]
    fn test_tool_ref_from_wire_name_internal() {
        // Strings that don't match mcp__X__Y stay internal
        assert_eq!(
            ToolRef::from_wire_name("read_file"),
            ToolRef::Internal("read_file".to_string())
        );
        // "mcp__" alone (no second __) is internal
        assert_eq!(
            ToolRef::from_wire_name("mcp__incomplete"),
            ToolRef::Internal("mcp__incomplete".to_string())
        );
    }

    // =========================================================================
    // ToolPattern tests
    // =========================================================================

    #[test]
    fn test_tool_pattern_from_str() {
        assert_eq!(
            ToolPattern::parse("mcp:ws/*"),
            ToolPattern::McpGlob("ws".to_string())
        );
        assert_eq!(ToolPattern::parse("internal:*"), ToolPattern::AllInternal);
        assert_eq!(
            ToolPattern::parse("run"),
            ToolPattern::Exact(ToolRef::Internal("run".to_string()))
        );
        assert_eq!(
            ToolPattern::parse("mcp:ws/web_search"),
            ToolPattern::Exact(ToolRef::Mcp {
                server: "ws".to_string(),
                tool: "web_search".to_string(),
            })
        );
    }

    #[test]
    fn test_tool_pattern_display() {
        assert_eq!(format!("{}", ToolPattern::McpGlob("ws".into())), "mcp:ws/*");
        assert_eq!(format!("{}", ToolPattern::AllInternal), "internal:*");
        assert_eq!(
            format!("{}", ToolPattern::Exact(ToolRef::Internal("run".into()))),
            "run"
        );
    }

    // =========================================================================
    // resolve_patterns tests
    // =========================================================================

    #[test]
    fn test_resolve_patterns_exact_internal() {
        let reg = registry_with(&["run", "read_file"]);
        let resolved = reg.resolve_patterns(&[
            ToolPattern::Exact(ToolRef::Internal("run".to_string())),
        ]);
        assert_eq!(resolved, vec!["run"]);
    }

    #[test]
    fn test_resolve_patterns_exact_mcp() {
        let reg = registry_with(&["mcp__ws__web_search"]);
        let resolved = reg.resolve_patterns(&[
            ToolPattern::Exact(ToolRef::Mcp {
                server: "ws".to_string(),
                tool: "web_search".to_string(),
            }),
        ]);
        assert_eq!(resolved, vec!["mcp__ws__web_search"]);
    }

    #[test]
    fn test_resolve_patterns_mcp_glob() {
        let reg = registry_with(&[
            "mcp__ws__web_search",
            "mcp__ws__fetch_webpage",
            "mcp__other__something",
            "run",
        ]);
        let mut resolved = reg.resolve_patterns(&[ToolPattern::McpGlob("ws".to_string())]);
        resolved.sort();
        assert_eq!(resolved, vec!["mcp__ws__fetch_webpage", "mcp__ws__web_search"]);
    }

    #[test]
    fn test_resolve_patterns_all_internal() {
        let reg = registry_with(&["run", "read_file", "mcp__ws__web_search"]);
        let mut resolved = reg.resolve_patterns(&[ToolPattern::AllInternal]);
        resolved.sort();
        assert_eq!(resolved, vec!["read_file", "run"]);
    }

    #[test]
    fn test_resolve_patterns_no_duplicates() {
        let reg = registry_with(&["mcp__ws__web_search"]);
        let resolved = reg.resolve_patterns(&[
            ToolPattern::Exact(ToolRef::Mcp {
                server: "ws".to_string(),
                tool: "web_search".to_string(),
            }),
            ToolPattern::McpGlob("ws".to_string()),
        ]);
        assert_eq!(resolved, vec!["mcp__ws__web_search"]);
    }

    #[test]
    fn test_resolve_patterns_empty_glob() {
        // Glob for a server that doesn't exist resolves to nothing
        let reg = registry_with(&["run"]);
        let resolved = reg.resolve_patterns(&[ToolPattern::McpGlob("nonexistent".to_string())]);
        assert!(resolved.is_empty());
    }

    #[test]
    fn test_tool_display_name_default() {
        let tool = NonBlockingTool;
        assert_eq!(tool.display_name(), tool.name());
    }
}
