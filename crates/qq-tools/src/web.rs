//! Web tools for fetching and searching the web.

use async_trait::async_trait;
use reqwest::Client;
use scraper::{Html, Selector};
use serde::{Deserialize, Serialize};

use qq_core::{Error, PropertySchema, Tool, ToolDefinition, ToolOutput, ToolParameters};

// =============================================================================
// Web Search Configuration (Perplexica API)
// =============================================================================

/// Configuration for the Perplexica-based web search
#[derive(Clone, Debug)]
pub struct WebSearchConfig {
    /// Base URL of the Perplexica instance (e.g., "http://localhost:3000")
    pub host: String,
    /// Chat model name (e.g., "gpt-4o-mini")
    pub chat_model: String,
    /// Embedding model name (e.g., "text-embedding-3-large")
    pub embed_model: String,
}

impl WebSearchConfig {
    pub fn new(host: impl Into<String>, chat_model: impl Into<String>, embed_model: impl Into<String>) -> Self {
        Self {
            host: host.into(),
            chat_model: chat_model.into(),
            embed_model: embed_model.into(),
        }
    }
}

// =============================================================================
// Fetch Webpage Tool
// =============================================================================

pub struct FetchWebpageTool {
    client: Client,
}

impl Default for FetchWebpageTool {
    fn default() -> Self {
        Self::new()
    }
}

impl FetchWebpageTool {
    pub fn new() -> Self {
        Self {
            client: Client::builder()
                .user_agent("qq-cli/0.1.0")
                .timeout(std::time::Duration::from_secs(30))
                .build()
                .unwrap_or_default(),
        }
    }
}

#[derive(Deserialize)]
struct FetchWebpageArgs {
    url: String,
    #[serde(default)]
    selector: Option<String>,
}

#[async_trait]
impl Tool for FetchWebpageTool {
    fn name(&self) -> &str {
        "fetch_webpage"
    }

    fn description(&self) -> &str {
        "Fetch a webpage and extract its text content. Optionally filter by CSS selector."
    }

    fn definition(&self) -> ToolDefinition {
        ToolDefinition::new(self.name(), self.description()).with_parameters(
            ToolParameters::new()
                .add_property("url", PropertySchema::string("URL of the webpage to fetch"), true)
                .add_property(
                    "selector",
                    PropertySchema::string("Optional CSS selector to extract specific content (e.g., 'main', 'article', '.content')"),
                    false,
                ),
        )
    }

    async fn execute(&self, arguments: serde_json::Value) -> Result<ToolOutput, Error> {
        let args: FetchWebpageArgs = serde_json::from_value(arguments)
            .map_err(|e| Error::tool("fetch_webpage", format!("Invalid arguments: {}", e)))?;

        // Fetch the page
        let response = self
            .client
            .get(&args.url)
            .send()
            .await
            .map_err(|e| Error::tool("fetch_webpage", format!("Failed to fetch '{}': {}", args.url, e)))?;

        if !response.status().is_success() {
            return Err(Error::tool(
                "fetch_webpage",
                format!("HTTP error {}: {}", response.status(), args.url),
            ));
        }

        let html = response
            .text()
            .await
            .map_err(|e| Error::tool("fetch_webpage", format!("Failed to read response: {}", e)))?;

        // Parse HTML
        let document = Html::parse_document(&html);

        // Extract text based on selector
        let text = if let Some(selector_str) = &args.selector {
            let selector = Selector::parse(selector_str)
                .map_err(|_| Error::tool("fetch_webpage", format!("Invalid selector: {}", selector_str)))?;

            document
                .select(&selector)
                .map(|el| extract_text(&el))
                .collect::<Vec<_>>()
                .join("\n\n")
        } else {
            // Default: try to get main content, fall back to body
            let main_selector = Selector::parse("main, article, .content, #content, .post, .entry").ok();
            let body_selector = Selector::parse("body").ok();

            if let Some(selector) = main_selector {
                let main_content: Vec<_> = document.select(&selector).collect();
                if !main_content.is_empty() {
                    main_content
                        .into_iter()
                        .map(|el| extract_text(&el))
                        .collect::<Vec<_>>()
                        .join("\n\n")
                } else if let Some(body_sel) = body_selector {
                    document
                        .select(&body_sel)
                        .map(|el| extract_text(&el))
                        .collect::<Vec<_>>()
                        .join("\n\n")
                } else {
                    extract_text(&document.root_element())
                }
            } else {
                extract_text(&document.root_element())
            }
        };

        // Clean up the text
        let cleaned = clean_text(&text);

        if cleaned.is_empty() {
            Ok(ToolOutput::success("(No text content found on page)"))
        } else {
            // Truncate if too long
            let max_len = 50000;
            if cleaned.len() > max_len {
                Ok(ToolOutput::success(format!(
                    "{}\n\n... (truncated, {} total characters)",
                    &cleaned[..max_len],
                    cleaned.len()
                )))
            } else {
                Ok(ToolOutput::success(cleaned))
            }
        }
    }
}

/// Extract text from an HTML element, filtering out scripts and styles
fn extract_text(element: &scraper::ElementRef) -> String {
    let mut text = String::new();

    for node in element.descendants() {
        if let Some(el) = node.value().as_element() {
            // Skip script, style, nav, footer, header elements
            let tag = el.name();
            if matches!(tag, "script" | "style" | "nav" | "footer" | "header" | "aside" | "noscript") {
                continue;
            }
        }

        if let Some(t) = node.value().as_text() {
            let trimmed = t.trim();
            if !trimmed.is_empty() {
                if !text.is_empty() && !text.ends_with(' ') && !text.ends_with('\n') {
                    text.push(' ');
                }
                text.push_str(trimmed);
            }
        }
    }

    text
}

/// Clean up extracted text
fn clean_text(text: &str) -> String {
    // Collapse multiple whitespace/newlines
    let mut result = String::new();
    let mut prev_was_whitespace = false;
    let mut newline_count = 0;

    for ch in text.chars() {
        if ch == '\n' {
            newline_count += 1;
            if newline_count <= 2 {
                result.push('\n');
            }
            prev_was_whitespace = true;
        } else if ch.is_whitespace() {
            if !prev_was_whitespace {
                result.push(' ');
                prev_was_whitespace = true;
            }
            newline_count = 0;
        } else {
            result.push(ch);
            prev_was_whitespace = false;
            newline_count = 0;
        }
    }

    result.trim().to_string()
}

// =============================================================================
// Web Search Tool (Perplexica API)
// =============================================================================

pub struct WebSearchTool {
    client: Client,
    config: WebSearchConfig,
}

impl WebSearchTool {
    pub fn new(config: WebSearchConfig) -> Self {
        Self {
            client: Client::builder()
                .user_agent("qq-cli/0.1.0")
                .timeout(std::time::Duration::from_secs(60))
                .build()
                .unwrap_or_default(),
            config,
        }
    }

    /// Get provider IDs for the configured chat and embedding models.
    /// Returns optional providers - if models aren't found, returns None for that provider
    /// and lets the search API handle validation (matching Python behavior).
    async fn get_provider_ids(&self) -> Result<(Option<ModelProvider>, Option<ModelProvider>), Error> {
        let url = format!("{}/api/providers", self.config.host);
        let response = self.client.get(&url).send().await
            .map_err(|e| Error::tool("web_search", format!("Failed to get providers: {}", e)))?;

        if !response.status().is_success() {
            return Err(Error::tool("web_search", format!("Provider API error: {}", response.status())));
        }

        let data: ProvidersResponse = response.json().await
            .map_err(|e| Error::tool("web_search", format!("Failed to parse providers: {}", e)))?;

        let mut chat_provider: Option<ModelProvider> = None;
        let mut embed_provider: Option<ModelProvider> = None;

        for provider in &data.providers {
            // Find chat model
            if chat_provider.is_none() {
                if let Some(model) = provider.chat_models.iter().find(|m| m.name == self.config.chat_model) {
                    chat_provider = Some(ModelProvider {
                        provider_id: provider.id.clone(),
                        key: model.key.clone(),
                    });
                }
            }

            // Find embedding model
            if embed_provider.is_none() {
                if let Some(model) = provider.embedding_models.iter().find(|m| m.name == self.config.embed_model) {
                    embed_provider = Some(ModelProvider {
                        provider_id: provider.id.clone(),
                        key: model.key.clone(),
                    });
                }
            }
        }

        Ok((chat_provider, embed_provider))
    }
}

#[derive(Deserialize)]
struct ProvidersResponse {
    providers: Vec<Provider>,
}

#[derive(Deserialize)]
struct Provider {
    id: String,
    #[serde(rename = "chatModels")]
    chat_models: Vec<Model>,
    #[serde(rename = "embeddingModels")]
    embedding_models: Vec<Model>,
}

#[derive(Deserialize)]
struct Model {
    name: String,
    key: String,
}

#[derive(Clone)]
struct ModelProvider {
    provider_id: String,
    key: String,
}


#[derive(Serialize)]
struct SearchRequest {
    #[serde(rename = "chatModel")]
    chat_model: Option<ChatModelRef>,
    #[serde(rename = "embeddingModel")]
    embedding_model: Option<EmbedModelRef>,
    #[serde(rename = "optimizationMode")]
    optimization_mode: String,
    sources: Vec<String>,
    query: String,
    history: Vec<(String, String)>,
    #[serde(rename = "systemInstructions")]
    system_instructions: String,
    stream: bool,
}

#[derive(Serialize)]
struct ChatModelRef {
    #[serde(rename = "providerId")]
    provider_id: String,
    key: String,
}

#[derive(Serialize)]
struct EmbedModelRef {
    #[serde(rename = "providerId")]
    provider_id: String,
    key: String,
}

#[derive(Deserialize)]
struct SearchResponse {
    message: String,
    sources: Vec<SearchSource>,
}

#[derive(Deserialize)]
struct SearchSource {
    content: String,
    metadata: SourceMetadata,
}

#[derive(Deserialize)]
struct SourceMetadata {
    title: String,
    url: String,
}

#[derive(Deserialize)]
struct WebSearchArgs {
    query: String,
}

#[async_trait]
impl Tool for WebSearchTool {
    fn name(&self) -> &str {
        "web_search"
    }

    fn description(&self) -> &str {
        "Search the web using natural language queries. Returns a synthesized answer with sources."
    }

    fn definition(&self) -> ToolDefinition {
        ToolDefinition::new(self.name(), self.description()).with_parameters(
            ToolParameters::new()
                .add_property("query", PropertySchema::string("The search query (can be natural language)"), true),
        )
    }

    async fn execute(&self, arguments: serde_json::Value) -> Result<ToolOutput, Error> {
        let args: WebSearchArgs = serde_json::from_value(arguments)
            .map_err(|e| Error::tool("web_search", format!("Invalid arguments: {}", e)))?;

        // Get provider IDs (may be None if models not found - let API handle validation)
        let (chat_provider, embed_provider) = self.get_provider_ids().await?;

        // Build search request with optional model refs (matching Python behavior)
        let request = SearchRequest {
            chat_model: chat_provider.map(|p| ChatModelRef {
                provider_id: p.provider_id,
                key: p.key,
            }),
            embedding_model: embed_provider.map(|p| EmbedModelRef {
                provider_id: p.provider_id,
                key: p.key,
            }),
            optimization_mode: "speed".to_string(),
            sources: vec!["web".to_string()],
            query: args.query.clone(),
            history: vec![
                ("human".to_string(), "Hi, how are you?".to_string()),
                ("assistant".to_string(), "I am doing well, how can I help you today?".to_string()),
            ],
            system_instructions: "Provide high level details.".to_string(),
            stream: false,
        };

        // Perform search
        let url = format!("{}/api/search", self.config.host);
        let response = self.client
            .post(&url)
            .json(&request)
            .send()
            .await
            .map_err(|e| Error::tool("web_search", format!("Search request failed: {}", e)))?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(Error::tool("web_search", format!("Search API error {}: {}", status, body)));
        }

        let result: SearchResponse = response.json().await
            .map_err(|e| Error::tool("web_search", format!("Failed to parse search response: {}", e)))?;

        // Format output
        let mut output = result.message;

        if !result.sources.is_empty() {
            output.push_str("\n\n## Sources\n");
            for source in result.sources {
                output.push_str(&format!("- [{}]({})\n", source.metadata.title, source.metadata.url));
            }
        }

        Ok(ToolOutput::success(output))
    }
}

// =============================================================================
// Factory functions
// =============================================================================

use std::sync::Arc;

/// Create all web tools (boxed version)
pub fn create_web_tools() -> Vec<Box<dyn Tool>> {
    vec![Box::new(FetchWebpageTool::new())]
}

/// Create all web tools (Arc version)
pub fn create_web_tools_arc() -> Vec<Arc<dyn Tool>> {
    vec![Arc::new(FetchWebpageTool::new())]
}

/// Create web tools with optional search capability
pub fn create_web_tools_with_search(search_config: Option<WebSearchConfig>) -> Vec<Arc<dyn Tool>> {
    let mut tools: Vec<Arc<dyn Tool>> = vec![Arc::new(FetchWebpageTool::new())];

    if let Some(config) = search_config {
        tools.push(Arc::new(WebSearchTool::new(config)));
    }

    tools
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_clean_text() {
        let input = "  Hello   world  \n\n\n\n  Test  ";
        let cleaned = clean_text(input);
        // Multiple spaces become single space, multiple newlines collapse to max 2
        assert!(cleaned.contains("Hello"));
        assert!(cleaned.contains("world"));
        assert!(cleaned.contains("Test"));
        assert!(!cleaned.contains("    ")); // No excessive spaces
    }

    #[test]
    fn test_extract_text() {
        let html = Html::parse_document("<html><body><p>Hello</p><script>evil()</script><p>World</p></body></html>");
        let text = extract_text(&html.root_element());
        assert!(text.contains("Hello"));
        assert!(text.contains("World"));
        // Script content should be filtered
    }
}
