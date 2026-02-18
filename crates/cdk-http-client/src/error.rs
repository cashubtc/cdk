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

#[cfg(feature = "reqwest")]
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
            HttpError::Other(err.to_string())
        }
    }
}

#[cfg(feature = "bitreq")]
impl From<bitreq::Error> for HttpError {
    fn from(err: bitreq::Error) -> Self {
        use bitreq::Error;
        use std::io;

        match err {
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
}
