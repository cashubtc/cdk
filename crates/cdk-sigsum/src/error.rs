//! Error types for the Sigsum client.

use thiserror::Error;

/// Errors that can occur while interacting with a Sigsum log.
#[derive(Debug, Error)]
pub enum Error {
    /// The HTTP request to the log server failed at the transport level.
    #[error("transport error: {0}")]
    Transport(#[from] reqwest::Error),

    /// The log server returned a status code that is not part of the
    /// documented protocol (see Table 1 of the log server protocol spec).
    #[error("unexpected status code {status} from log: {body}")]
    UnexpectedStatus {
        /// HTTP status code returned by the log.
        status: u16,
        /// Human-readable error body returned by the log, if any.
        body: String,
    },

    /// The log's response body could not be parsed as the documented
    /// `Key=Value` ASCII format.
    #[error("malformed response from log: {0}")]
    MalformedResponse(String),

    /// A field expected to be present in a response was missing.
    #[error("missing field `{0}` in log response")]
    MissingField(&'static str),

    /// A hex-encoded field could not be decoded, or decoded to the wrong
    /// length.
    #[error("invalid hex-encoded field `{field}`: {reason}")]
    InvalidHex {
        /// Name of the field that failed to decode.
        field: &'static str,
        /// Description of why decoding failed.
        reason: String,
    },

    /// A signature did not verify.
    #[error("signature verification failed")]
    InvalidSignature,

    /// The requested URL was not a valid base URL for a log.
    #[error("invalid log base URL: {0}")]
    InvalidUrl(#[from] url::ParseError),
}
