//! HTTP error types

use thiserror::Error;

/// HTTP errors that can occur during requests
#[derive(Debug, Error)]
pub enum HttpError {
    /// HTTP error with status code
    #[error("HTTP error ({status}): {message}")]
    Status {
        /// HTTP status code
        status: u16,
        /// Error message
        message: String,
    },
    /// Connection error
    #[error("Connection error: {0}")]
    Connection(String),
    /// Request timeout
    #[error("Request timeout")]
    Timeout,
    /// Serialization error
    #[error("Serialization error: {0}")]
    Serialization(String),
    /// Proxy error
    #[error("Proxy error: {0}")]
    Proxy(String),
    /// Client build error
    #[error("Client build error: {0}")]
    Build(String),
    /// Other error
    #[error("{0}")]
    Other(String),
}

impl From<reqwest::Error> for HttpError {
    fn from(err: reqwest::Error) -> Self {
        if err.is_timeout() {
            HttpError::Timeout
        } else if err.is_builder() {
            HttpError::Build(err.to_string())
        } else if let Some(status) = err.status() {
            HttpError::Status {
                status: status.as_u16(),
                message: err.to_string(),
            }
        } else {
            // is_connect() is not available on wasm32
            #[cfg(not(target_arch = "wasm32"))]
            if err.is_connect() {
                return HttpError::Connection(err.to_string());
            }
            HttpError::Other(err.to_string())
        }
    }
}

impl From<serde_json::Error> for HttpError {
    fn from(err: serde_json::Error) -> Self {
        HttpError::Serialization(err.to_string())
    }
}
