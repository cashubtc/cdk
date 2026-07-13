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

impl HttpError {
    /// Return whether this error is eligible for NUT-19 request replay.
    ///
    /// Only ambiguous transport failures are eligible. HTTP status responses
    /// are never eligible because NUT-19 caches successful responses only; a
    /// failed status therefore does not prove that replaying a side-effecting
    /// request is safe.
    pub fn is_replay_safe(&self) -> bool {
        matches!(self, Self::Connection(_) | Self::Timeout)
    }
}

#[cfg(all(
    feature = "bitreq",
    not(feature = "reqwest"),
    not(target_arch = "wasm32")
))]
impl From<bitreq::Error> for HttpError {
    fn from(err: bitreq::Error) -> Self {
        use std::io;

        use bitreq::Error;

        match err {
            Error::SerdeJsonError(_) => HttpError::Serialization(err.to_string()),
            Error::InvalidUtf8InBody(_) => HttpError::Serialization(err.to_string()),
            Error::InvalidUtf8InResponse => HttpError::Serialization(err.to_string()),
            Error::IoError(io_err) => {
                if io_err.kind() == io::ErrorKind::TimedOut {
                    HttpError::Timeout
                } else if io_err.kind() == io::ErrorKind::ConnectionRefused
                    || io_err.kind() == io::ErrorKind::ConnectionReset
                    || io_err.kind() == io::ErrorKind::ConnectionAborted
                    || io_err.kind() == io::ErrorKind::NotConnected
                {
                    HttpError::Connection(io_err.to_string())
                } else {
                    HttpError::Other(io_err.to_string())
                }
            }
            Error::AddressNotFound => HttpError::Connection(err.to_string()),
            _ => HttpError::Other(err.to_string()),
        }
    }
}

impl From<serde_json::Error> for HttpError {
    fn from(err: serde_json::Error) -> Self {
        HttpError::Serialization(err.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_http_error_status_display() {
        let error = HttpError::Status {
            status: 404,
            message: "Not Found".to_string(),
        };
        assert_eq!(format!("{}", error), "HTTP error (404): Not Found");
    }

    #[test]
    fn test_http_error_connection_display() {
        let error = HttpError::Connection("connection refused".to_string());
        assert_eq!(format!("{}", error), "Connection error: connection refused");
    }

    #[test]
    fn test_http_error_timeout_display() {
        let error = HttpError::Timeout;
        assert_eq!(format!("{}", error), "Request timeout");
    }

    #[test]
    fn test_http_error_serialization_display() {
        let error = HttpError::Serialization("invalid JSON".to_string());
        assert_eq!(format!("{}", error), "Serialization error: invalid JSON");
    }

    #[test]
    fn test_http_error_proxy_display() {
        let error = HttpError::Proxy("proxy unreachable".to_string());
        assert_eq!(format!("{}", error), "Proxy error: proxy unreachable");
    }

    #[test]
    fn test_http_error_build_display() {
        let error = HttpError::Build("invalid config".to_string());
        assert_eq!(format!("{}", error), "Client build error: invalid config");
    }

    #[test]
    fn test_http_error_other_display() {
        let error = HttpError::Other("unknown error".to_string());
        assert_eq!(format!("{}", error), "unknown error");
    }

    #[test]
    fn test_from_serde_json_error() {
        // Create an invalid JSON parse to get a serde_json::Error
        let result: Result<String, _> = serde_json::from_str("not valid json");
        let json_error = result.expect_err("Invalid JSON should produce an error");
        let http_error: HttpError = json_error.into();

        match http_error {
            HttpError::Serialization(msg) => {
                assert!(
                    msg.contains("expected"),
                    "Error message should describe JSON error"
                );
            }
            _ => panic!("Expected HttpError::Serialization"),
        }
    }

    #[test]
    fn test_replay_safe_error_classification() {
        assert!(HttpError::Connection("reset".to_string()).is_replay_safe());
        assert!(HttpError::Timeout.is_replay_safe());

        for status in [400, 408, 429, 500, 503] {
            assert!(!HttpError::Status {
                status,
                message: "request failed".to_string(),
            }
            .is_replay_safe());
        }

        assert!(!HttpError::Serialization("bad JSON".to_string()).is_replay_safe());
        assert!(!HttpError::Other("attestation failed".to_string()).is_replay_safe());
    }
}
