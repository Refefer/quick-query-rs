//! Test utilities shared across the workspace.
//! Only compiled when running tests or with the `testing` feature.

use async_trait::async_trait;
use std::sync::Mutex;

use crate::error::Error;
use crate::message::{FinishReason, Message, StreamChunk, Usage};
use crate::observation::ContextCompactor;
use crate::provider::{CompletionRequest, CompletionResponse, Provider, StreamResult};

/// A mock provider that returns pre-configured responses.
pub struct MockProvider {
    responses: Mutex<Vec<CompletionResponse>>,
    /// Each entry is the chunk sequence to replay for one stream() call. FIFO.
    stream_chunks: Mutex<Vec<Vec<StreamChunk>>>,
    /// Captured requests (for assertion).
    pub captured_requests: Mutex<Vec<CompletionRequest>>,
    pub name: String,
    pub default_model: Option<String>,
    pub context_window: Option<u32>,
}

impl MockProvider {
    pub fn new() -> Self {
        Self {
            responses: Mutex::new(Vec::new()),
            stream_chunks: Mutex::new(Vec::new()),
            captured_requests: Mutex::new(Vec::new()),
            name: "mock".to_string(),
            default_model: None,
            context_window: None,
        }
    }

    /// Set the context window so tests can exercise the
    /// "context window full vs max_tokens cap" branch in the agent loop.
    pub fn with_context_window(mut self, tokens: u32) -> Self {
        self.context_window = Some(tokens);
        self
    }

    /// Queue a response to be returned by the next complete() call.
    /// Responses are returned in FIFO order (first queued = first returned).
    pub fn queue_response(&self, content: &str) {
        self.queue_response_with_finish(content, FinishReason::Stop);
    }

    /// Queue a response with an explicit finish_reason. Useful for testing
    /// truncation handling (`FinishReason::Length`).
    pub fn queue_response_with_finish(&self, content: &str, finish_reason: FinishReason) {
        let response = CompletionResponse {
            message: Message::assistant(content),
            thinking: None,
            usage: Usage::new(0, 0),
            model: "mock-model".to_string(),
            finish_reason,
        };
        self.responses.lock().unwrap().insert(0, response);
    }

    /// Queue a raw CompletionResponse.
    pub fn queue_raw_response(&self, response: CompletionResponse) {
        self.responses.lock().unwrap().insert(0, response);
    }

    /// Queue a stream — the next `stream()` call replays these chunks in order.
    /// FIFO across calls.
    pub fn queue_stream(&self, chunks: Vec<StreamChunk>) {
        self.stream_chunks.lock().unwrap().insert(0, chunks);
    }

    /// Get the number of captured requests.
    pub fn request_count(&self) -> usize {
        self.captured_requests.lock().unwrap().len()
    }

    /// Get the last captured request.
    pub fn last_request(&self) -> Option<CompletionRequest> {
        self.captured_requests.lock().unwrap().last().cloned()
    }
}

impl Default for MockProvider {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Provider for MockProvider {
    fn name(&self) -> &str {
        &self.name
    }

    fn default_model(&self) -> Option<&str> {
        self.default_model.as_deref()
    }

    fn context_window(&self) -> Option<u32> {
        self.context_window
    }

    async fn complete(&self, request: CompletionRequest) -> Result<CompletionResponse, Error> {
        self.captured_requests.lock().unwrap().push(request);
        match self.responses.lock().unwrap().pop() {
            Some(response) => Ok(response),
            None => Err(Error::Unknown("No mock response queued".to_string())),
        }
    }

    async fn stream(&self, request: CompletionRequest) -> Result<StreamResult, Error> {
        self.captured_requests.lock().unwrap().push(request);
        let chunks = self.stream_chunks.lock().unwrap().pop().unwrap_or_else(|| {
            // No queued stream — emit a minimal "Stop" stream so callers don't hang.
            vec![StreamChunk::Done {
                usage: None,
                finish_reason: Some(FinishReason::Stop),
            }]
        });
        let stream = futures::stream::iter(chunks.into_iter().map(Ok));
        Ok(Box::pin(stream))
    }
}

/// A mock context compactor for testing ObservationalMemory.
pub struct MockCompactor {
    observe_responses: Mutex<Vec<Result<String, Error>>>,
    reflect_responses: Mutex<Vec<Result<String, Error>>>,
    pub observe_calls: Mutex<Vec<Vec<Message>>>,
    pub reflect_calls: Mutex<Vec<String>>,
}

impl MockCompactor {
    pub fn new() -> Self {
        Self {
            observe_responses: Mutex::new(Vec::new()),
            reflect_responses: Mutex::new(Vec::new()),
            observe_calls: Mutex::new(Vec::new()),
            reflect_calls: Mutex::new(Vec::new()),
        }
    }

    /// Queue a response for the next observe() call (FIFO).
    pub fn queue_observe(&self, response: Result<String, Error>) {
        self.observe_responses.lock().unwrap().insert(0, response);
    }

    /// Queue a response for the next reflect() call (FIFO).
    pub fn queue_reflect(&self, response: Result<String, Error>) {
        self.reflect_responses.lock().unwrap().insert(0, response);
    }
}

impl Default for MockCompactor {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl ContextCompactor for MockCompactor {
    async fn observe(&self, messages: &[Message]) -> Result<String, Error> {
        self.observe_calls
            .lock()
            .unwrap()
            .push(messages.to_vec());
        self.observe_responses
            .lock()
            .unwrap()
            .pop()
            .unwrap_or(Err(Error::Unknown(
                "No mock observe response queued".to_string(),
            )))
    }

    async fn reflect(&self, log: &str) -> Result<String, Error> {
        self.reflect_calls
            .lock()
            .unwrap()
            .push(log.to_string());
        self.reflect_responses
            .lock()
            .unwrap()
            .pop()
            .unwrap_or(Err(Error::Unknown(
                "No mock reflect response queued".to_string(),
            )))
    }

    async fn observe_with_prior(
        &self,
        messages: &[Message],
        _prior_observations: Option<&str>,
    ) -> Result<String, Error> {
        self.observe(messages).await
    }
}
