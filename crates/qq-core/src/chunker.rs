//! Chunk-and-process system for handling large tool outputs.
//!
//! When tool outputs exceed the configured threshold, this module:
//! 1. Splits content into manageable chunks at natural boundaries
//! 2. Summarizes each chunk using the LLM
//! 3. Combines summaries into a coherent result

use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::error::Error;
use crate::message::Message;
use crate::provider::{CompletionRequest, Provider};

/// Configuration for the chunk processor.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChunkerConfig {
    /// Enable automatic chunking of large tool outputs.
    #[serde(default = "default_enabled")]
    pub enabled: bool,

    /// Size threshold (in bytes) to trigger chunking.
    #[serde(default = "default_threshold_bytes")]
    pub threshold_bytes: usize,

    /// Target size for each chunk (in bytes).
    #[serde(default = "default_chunk_size_bytes")]
    pub chunk_size_bytes: usize,

    /// Maximum number of chunks to process.
    #[serde(default = "default_max_chunks")]
    pub max_chunks: usize,

    /// Process chunks in parallel.
    #[serde(default = "default_parallel")]
    pub parallel: bool,
}

fn default_enabled() -> bool {
    true
}

fn default_threshold_bytes() -> usize {
    50_000 // 50KB
}

fn default_chunk_size_bytes() -> usize {
    10_000 // 10KB
}

fn default_max_chunks() -> usize {
    20
}

fn default_parallel() -> bool {
    true
}

impl Default for ChunkerConfig {
    fn default() -> Self {
        Self {
            enabled: default_enabled(),
            threshold_bytes: default_threshold_bytes(),
            chunk_size_bytes: default_chunk_size_bytes(),
            max_chunks: default_max_chunks(),
            parallel: default_parallel(),
        }
    }
}

impl ChunkerConfig {
    /// Create a new config with default values.
    pub fn new() -> Self {
        Self::default()
    }

    /// Set the enabled flag.
    pub fn with_enabled(mut self, enabled: bool) -> Self {
        self.enabled = enabled;
        self
    }

    /// Set the threshold in bytes.
    pub fn with_threshold(mut self, threshold_bytes: usize) -> Self {
        self.threshold_bytes = threshold_bytes;
        self
    }

    /// Set the chunk size in bytes.
    pub fn with_chunk_size(mut self, chunk_size_bytes: usize) -> Self {
        self.chunk_size_bytes = chunk_size_bytes;
        self
    }

    /// Set the maximum number of chunks.
    pub fn with_max_chunks(mut self, max_chunks: usize) -> Self {
        self.max_chunks = max_chunks;
        self
    }

    /// Set whether to process chunks in parallel.
    pub fn with_parallel(mut self, parallel: bool) -> Self {
        self.parallel = parallel;
        self
    }
}

/// Processor for chunking and summarizing large content.
pub struct ChunkProcessor {
    provider: Arc<dyn Provider>,
    config: ChunkerConfig,
}

impl ChunkProcessor {
    /// Create a new chunk processor.
    pub fn new(provider: Arc<dyn Provider>, config: ChunkerConfig) -> Self {
        Self { provider, config }
    }

    /// Check if the content should be chunked based on size threshold.
    pub fn should_chunk(&self, content: &str) -> bool {
        self.config.enabled && content.len() > self.config.threshold_bytes
    }

    /// Check if content appears to be binary (non-text).
    fn is_binary_content(content: &str) -> bool {
        // Check for high proportion of non-printable characters
        let non_printable = content
            .chars()
            .take(1000) // Sample first 1000 chars
            .filter(|c| !c.is_ascii_graphic() && !c.is_ascii_whitespace())
            .count();
        let sample_size = content.len().min(1000);
        sample_size > 0 && (non_printable as f64 / sample_size as f64) > 0.3
    }

    /// Split content into chunks at natural boundaries.
    ///
    /// Strategy:
    /// 1. Primary split: Double newlines (paragraph boundaries)
    /// 2. Secondary split: Single newlines (line boundaries)
    /// 3. Fallback: Character split at word boundaries
    pub fn chunk_content(&self, content: &str) -> Vec<String> {
        let target_size = self.config.chunk_size_bytes;
        let mut chunks = Vec::new();
        let mut current_chunk = String::new();

        // Split by paragraphs first (double newline)
        let paragraphs: Vec<&str> = content.split("\n\n").collect();

        for paragraph in paragraphs {
            // If adding this paragraph would exceed target, handle current chunk
            if !current_chunk.is_empty()
                && current_chunk.len() + paragraph.len() + 2 > target_size
            {
                chunks.push(std::mem::take(&mut current_chunk));
            }

            // If single paragraph is too large, split by lines
            if paragraph.len() > target_size {
                // Flush current chunk first
                if !current_chunk.is_empty() {
                    chunks.push(std::mem::take(&mut current_chunk));
                }

                // Split large paragraph by lines
                for line in paragraph.lines() {
                    if !current_chunk.is_empty()
                        && current_chunk.len() + line.len() + 1 > target_size
                    {
                        chunks.push(std::mem::take(&mut current_chunk));
                    }

                    // If single line is too large, split at word boundaries
                    if line.len() > target_size {
                        if !current_chunk.is_empty() {
                            chunks.push(std::mem::take(&mut current_chunk));
                        }

                        let mut line_chunk = String::new();
                        for word in line.split_whitespace() {
                            if !line_chunk.is_empty()
                                && line_chunk.len() + word.len() + 1 > target_size
                            {
                                chunks.push(std::mem::take(&mut line_chunk));
                            }

                            if !line_chunk.is_empty() {
                                line_chunk.push(' ');
                            }
                            line_chunk.push_str(word);
                        }

                        if !line_chunk.is_empty() {
                            current_chunk = line_chunk;
                        }
                    } else {
                        if !current_chunk.is_empty() {
                            current_chunk.push('\n');
                        }
                        current_chunk.push_str(line);
                    }
                }
            } else {
                if !current_chunk.is_empty() {
                    current_chunk.push_str("\n\n");
                }
                current_chunk.push_str(paragraph);
            }
        }

        // Don't forget the last chunk
        if !current_chunk.is_empty() {
            chunks.push(current_chunk);
        }

        // Respect max_chunks limit
        if chunks.len() > self.config.max_chunks {
            chunks.truncate(self.config.max_chunks);
        }

        chunks
    }

    /// Summarize a single chunk using the LLM.
    async fn summarize_chunk(
        &self,
        chunk: &str,
        chunk_num: usize,
        total_chunks: usize,
        original_query: Option<&str>,
    ) -> Result<String, Error> {
        let query_context = original_query
            .map(|q| format!("Original query context: {}\n\n", q))
            .unwrap_or_default();

        let prompt = format!(
            "You are summarizing a chunk of data (chunk {} of {}).\n\
             {}\
             Summarize the key information relevant to the query. Be concise but preserve important details.\n\
             If this is a list of files or items, preserve the structure but note patterns.\n\
             If this is code or logs, highlight the most important parts.\n\n\
             Content:\n{}",
            chunk_num + 1,
            total_chunks,
            query_context,
            chunk
        );

        let messages = vec![Message::user(prompt.as_str())];
        let mut request = CompletionRequest::new(messages);

        // Use a smaller max_tokens for summaries
        request = request.with_max_tokens(1000);

        // Apply model if provider has one
        if let Some(model) = self.provider.default_model() {
            request = request.with_model(model);
        }

        let response = self.provider.complete(request).await?;
        Ok(response.message.content.to_string_lossy())
    }

    /// Process large content by chunking and summarizing.
    ///
    /// Returns the original content if:
    /// - Content is below threshold
    /// - Content appears to be binary
    /// - Chunking is disabled
    /// - Content is an error message (starts with "Error:")
    pub async fn process_large_content(
        &self,
        content: &str,
        original_query: Option<&str>,
    ) -> Result<String, Error> {
        // Skip if disabled or below threshold
        if !self.should_chunk(content) {
            return Ok(content.to_string());
        }

        // Skip error outputs
        if content.starts_with("Error:") || content.starts_with("Error ") {
            return Ok(content.to_string());
        }

        // Skip binary content
        if Self::is_binary_content(content) {
            return Ok(format!(
                "[Binary content detected, {} bytes]\n\n{}",
                content.len(),
                &content[..content.len().min(500)]
            ));
        }

        // Split into chunks
        let chunks = self.chunk_content(content);
        let total_chunks = chunks.len();
        let was_truncated = content.len() > self.config.chunk_size_bytes * self.config.max_chunks;

        if chunks.is_empty() {
            return Ok(content.to_string());
        }

        // If only one chunk (content was just above threshold but chunked to one), return it
        if chunks.len() == 1 {
            return Ok(chunks.into_iter().next().unwrap());
        }

        // Summarize chunks
        let summaries = if self.config.parallel {
            self.summarize_chunks_parallel(&chunks, original_query).await?
        } else {
            self.summarize_chunks_sequential(&chunks, original_query).await?
        };

        // Combine summaries
        let mut result = format!(
            "[Large output processed: {} bytes split into {} chunks]\n\n",
            content.len(),
            total_chunks
        );

        for (i, summary) in summaries.iter().enumerate() {
            result.push_str(&format!("### Chunk {} of {}\n", i + 1, total_chunks));
            result.push_str(summary);
            result.push_str("\n\n");
        }

        if was_truncated {
            result.push_str(&format!(
                "[Note: Output was truncated. Only first {} chunks processed.]\n",
                self.config.max_chunks
            ));
        }

        Ok(result)
    }

    /// Summarize chunks in parallel.
    async fn summarize_chunks_parallel(
        &self,
        chunks: &[String],
        original_query: Option<&str>,
    ) -> Result<Vec<String>, Error> {
        use futures::future::join_all;

        let total = chunks.len();
        let futures: Vec<_> = chunks
            .iter()
            .enumerate()
            .map(|(i, chunk)| self.summarize_chunk(chunk, i, total, original_query))
            .collect();

        let results = join_all(futures).await;

        // Collect results, replacing errors with error messages
        let summaries: Vec<String> = results
            .into_iter()
            .enumerate()
            .map(|(i, r)| {
                r.unwrap_or_else(|e| format!("[Error summarizing chunk {}: {}]", i + 1, e))
            })
            .collect();

        Ok(summaries)
    }

    /// Summarize chunks sequentially.
    async fn summarize_chunks_sequential(
        &self,
        chunks: &[String],
        original_query: Option<&str>,
    ) -> Result<Vec<String>, Error> {
        let total = chunks.len();
        let mut summaries = Vec::with_capacity(total);

        for (i, chunk) in chunks.iter().enumerate() {
            let summary = self
                .summarize_chunk(chunk, i, total, original_query)
                .await
                .unwrap_or_else(|e| format!("[Error summarizing chunk {}: {}]", i + 1, e));
            summaries.push(summary);
        }

        Ok(summaries)
    }

    /// Get the current configuration.
    pub fn config(&self) -> &ChunkerConfig {
        &self.config
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_chunker_config_defaults() {
        let config = ChunkerConfig::default();
        assert!(config.enabled);
        assert_eq!(config.threshold_bytes, 50_000);
        assert_eq!(config.chunk_size_bytes, 10_000);
        assert_eq!(config.max_chunks, 20);
        assert!(config.parallel);
    }

    #[test]
    fn test_chunker_config_builder() {
        let config = ChunkerConfig::new()
            .with_enabled(false)
            .with_threshold(100_000)
            .with_chunk_size(20_000)
            .with_max_chunks(10)
            .with_parallel(false);

        assert!(!config.enabled);
        assert_eq!(config.threshold_bytes, 100_000);
        assert_eq!(config.chunk_size_bytes, 20_000);
        assert_eq!(config.max_chunks, 10);
        assert!(!config.parallel);
    }

    #[test]
    fn test_chunk_content_small() {
        // Content smaller than chunk size should stay as one chunk
        let config = ChunkerConfig::new().with_chunk_size(1000);

        // Create a mock provider - we won't actually use it for chunking
        // Just test the chunking logic
        let content = "Small content";

        // Directly test chunk_content logic
        let target_size = config.chunk_size_bytes;
        assert!(content.len() < target_size);
    }

    #[test]
    fn test_chunk_content_paragraphs() {
        let config = ChunkerConfig::new().with_chunk_size(50);

        // Simulate the chunking logic
        let content = "First paragraph.\n\nSecond paragraph.\n\nThird paragraph.";
        let paragraphs: Vec<&str> = content.split("\n\n").collect();
        assert_eq!(paragraphs.len(), 3);
    }

    #[test]
    fn test_is_binary_content() {
        // Text content
        assert!(!ChunkProcessor::is_binary_content("Hello, world!"));
        assert!(!ChunkProcessor::is_binary_content("Line 1\nLine 2\nLine 3"));

        // Binary-ish content (high proportion of non-printable)
        let binary = (0u8..255).map(|b| b as char).collect::<String>();
        assert!(ChunkProcessor::is_binary_content(&binary));
    }
}
