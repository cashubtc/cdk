//! Error types for the NpubCash SDK

use thiserror::Error;

/// Result type for NpubCash SDK operations
pub type Result<T> = std::result::Result<T, Error>;

/// Error types that can occur when using the NpubCash SDK
#[derive(Debug, Error)]
pub enum Error {
    /// API returned an error response
    #[error("API error ({status}): {message}")]
    Api {
        /// Error message from the API
        message: String,
        /// HTTP status code
        status: u16,
    },

    /// Authentication failed
    #[error("Authentication failed: {0}")]
    Auth(String),

    /// HTTP request failed
    #[error("HTTP request failed: {0}")]
    Http(#[from] reqwest::Error),

    /// JSON serialization/deserialization error
    #[error("JSON serialization error: {0}")]
    Serde(#[from] serde_json::Error),

    /// Invalid URL
    #[error("Invalid URL: {0}")]
    Url(#[from] url::ParseError),

    /// Nostr signing error
    #[error("Nostr signing error: {0}")]
    Nostr(String),

    /// Custom error message
    #[error("{0}")]
    Custom(String),
}
