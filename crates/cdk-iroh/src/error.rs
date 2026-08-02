//! Redacted transport errors.

use thiserror::Error;

/// Errors produced by endpoint lifecycle and the HTTP-over-Iroh bridge.
#[derive(Debug, Error)]
pub enum Error {
    /// Endpoint construction failed.
    #[error("Iroh endpoint construction failed")]
    Endpoint,
    /// An Iroh URL is invalid.
    #[error("invalid Iroh URL: {reason}")]
    InvalidUrl {
        /// Non-sensitive validation reason.
        reason: &'static str,
    },
    /// An HTTP request attempts to override transport-owned framing.
    #[error("invalid Iroh HTTP request: {reason}")]
    InvalidRequest {
        /// Non-sensitive validation reason.
        reason: &'static str,
    },
    /// The URL uses a scheme this operation cannot handle.
    #[error("unsupported URL scheme `{scheme}`")]
    UnsupportedScheme {
        /// Rejected URL scheme.
        scheme: String,
    },
    /// Connecting to an authenticated endpoint failed.
    #[error("Iroh connection to peer {peer} failed")]
    Connect {
        /// Short, bounded peer fingerprint.
        peer: String,
    },
    /// A transport operation timed out.
    #[error("Iroh {operation} timed out")]
    Timeout {
        /// Stable operation name without address or request data.
        operation: &'static str,
    },
    /// Opening or using a request stream failed.
    #[error("Iroh request stream failed")]
    Stream,
    /// HTTP framing failed.
    #[error("HTTP-over-Iroh framing failed")]
    Http,
    /// Request serialization failed.
    #[error("Iroh request serialization failed")]
    Serialization,
    /// The request body exceeds the configured limit.
    #[error("Iroh request body too large: actual={actual}, max={max}")]
    RequestTooLarge {
        /// Serialized request bytes.
        actual: usize,
        /// Maximum admitted bytes.
        max: usize,
    },
    /// The response body exceeds the configured or caller-provided limit.
    #[error("Iroh response body too large: actual={actual}, max={max}")]
    ResponseTooLarge {
        /// Bytes observed before rejection.
        actual: usize,
        /// Maximum admitted bytes.
        max: usize,
    },
    /// A bounded admission resource is saturated.
    #[error("Iroh {resource} admission limit reached")]
    Admission {
        /// Stable bounded resource name.
        resource: &'static str,
    },
    /// Server shutdown did not drain in time.
    #[error("Iroh server shutdown timed out")]
    ShutdownTimeout,
    /// A supervised server task failed.
    #[error("Iroh server task failed")]
    Server,
}

impl From<Error> for cdk_http_client::HttpError {
    fn from(error: Error) -> Self {
        use cdk_http_client::HttpError;

        match error {
            Error::Timeout { .. } => HttpError::Timeout,
            Error::ResponseTooLarge { .. } => HttpError::Other(error.to_string()),
            Error::Serialization => {
                HttpError::Serialization("Iroh request serialization failed".to_string())
            }
            Error::UnsupportedScheme { .. }
            | Error::InvalidUrl { .. }
            | Error::InvalidRequest { .. } => HttpError::Other(error.to_string()),
            Error::RequestTooLarge { .. } => HttpError::Other(error.to_string()),
            _ => HttpError::Connection(error.to_string()),
        }
    }
}
