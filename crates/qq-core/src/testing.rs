//! Test utilities shared across the workspace.
//! Only compiled when running tests or with the `testing` feature.

use async_trait::async_trait;
use std::sync::Mutex;

use crate::error::Error;
use crate::message::{Message, Usage};
use crate::observation::ContextCompactor;
use crate::provider::{CompletionRequest, CompletionResponse, FinishReason, Provider, StreamResult};

/// A mock provider that returns pre-configured responses.
pub struct MockProvider {
    responses: Mutex<Vec<CompletionResponse>>,
    /// Captured requests (for assertion).
    pub captured_requests: Mutex<Vec<CompletionRequest>>,
    pub name: String,
    pub default_model: Option<String>,
}

impl MockProvider {
    pub fn new() -> Self {
        Self {
            responses: Mutex::new(Vec::new()),
            captured_requests: Mutex::new(Vec::new()),
            name: "mock".to_string(),
            default_model: None,
        }
    }

    /// Queue a response to be returned by the next complete() call.
    /// Responses are returned in FIFO order (first queued = first returned).
    pub fn queue_response(&self, content: &str) {
        let response = CompletionResponse {
            message: Message::assistant(content),
            thinking: None,
            usage: Usage::new(0, 0),
            model: "mock-model".to_string(),
            finish_reason: FinishReason::Stop,
        };
        self.responses.lock().unwrap().insert(0, response);
    }

    /// Queue a raw CompletionResponse.
    pub fn queue_raw_response(&self, response: CompletionResponse) {
        self.responses.lock().unwrap().insert(0, response);
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

    async fn complete(&self, request: CompletionRequest) -> Result<CompletionResponse, Error> {
        self.captured_requests.lock().unwrap().push(request);
        match self.responses.lock().unwrap().pop() {
            Some(response) => Ok(response),
            None => Err(Error::Unknown("No mock response queued".to_string())),
        }
    }

    async fn stream(&self, _request: CompletionRequest) -> Result<StreamResult, Error> {
        Err(Error::Unknown(
            "MockProvider does not support streaming".to_string(),
        ))
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
}
