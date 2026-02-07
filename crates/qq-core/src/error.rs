use thiserror::Error;

#[derive(Error, Debug)]
pub enum Error {
    #[error("API error: {message} (status: {status})")]
    Api { status: u16, message: String },

    #[error("Authentication error: {0}")]
    Auth(String),

    #[error("Rate limit exceeded: {0}")]
    RateLimit(String),

    #[error("Invalid request: {0}")]
    InvalidRequest(String),

    #[error("Network error: {0}")]
    Network(String),

    #[error("Serialization error: {0}")]
    Serialization(String),

    #[error("Stream error: {0}")]
    Stream(String),

    #[error("Tool error: {tool} - {message}")]
    Tool { tool: String, message: String },

    #[error("Configuration error: {0}")]
    Config(String),

    #[error("Provider not found: {0}")]
    ProviderNotFound(String),

    #[error("Model not found: {0}")]
    ModelNotFound(String),

    #[error("Timeout: {0}")]
    Timeout(String),

    #[error("Cancelled")]
    Cancelled,

    #[error("Unknown error: {0}")]
    Unknown(String),
}

impl Error {
    pub fn api(status: u16, message: impl Into<String>) -> Self {
        Self::Api {
            status,
            message: message.into(),
        }
    }

    pub fn auth(message: impl Into<String>) -> Self {
        Self::Auth(message.into())
    }

    pub fn rate_limit(message: impl Into<String>) -> Self {
        Self::RateLimit(message.into())
    }

    pub fn invalid_request(message: impl Into<String>) -> Self {
        Self::InvalidRequest(message.into())
    }

    pub fn network(message: impl Into<String>) -> Self {
        Self::Network(message.into())
    }

    pub fn serialization(message: impl Into<String>) -> Self {
        Self::Serialization(message.into())
    }

    pub fn stream(message: impl Into<String>) -> Self {
        Self::Stream(message.into())
    }

    pub fn tool(tool: impl Into<String>, message: impl Into<String>) -> Self {
        Self::Tool {
            tool: tool.into(),
            message: message.into(),
        }
    }

    pub fn config(message: impl Into<String>) -> Self {
        Self::Config(message.into())
    }

    pub fn is_retryable(&self) -> bool {
        matches!(
            self,
            Error::Network(_) | Error::RateLimit(_) | Error::Timeout(_) | Error::Stream(_)
        )
    }

    pub fn is_auth_error(&self) -> bool {
        matches!(self, Error::Auth(_))
    }
}

impl From<serde_json::Error> for Error {
    fn from(err: serde_json::Error) -> Self {
        Error::Serialization(err.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_error_display() {
        let err = Error::api(400, "Bad request");
        assert!(err.to_string().contains("400"));
        assert!(err.to_string().contains("Bad request"));
    }

    #[test]
    fn test_is_retryable() {
        assert!(Error::network("timeout").is_retryable());
        assert!(Error::rate_limit("too many requests").is_retryable());
        assert!(Error::stream("transport error").is_retryable());
        assert!(!Error::auth("invalid key").is_retryable());
    }
}
