//! WebSocket error types

/// Errors that can occur during WebSocket operations.
#[derive(Debug, thiserror::Error)]
pub enum WsError {
    /// A temporary network or connection failure.
    #[error("transient WebSocket error: {0}")]
    Transient(String),
    /// The remote endpoint does not support WebSocket subscriptions.
    #[error("WebSocket subscriptions are not supported: {0}")]
    NotSupported(String),
    /// A permanent configuration, authentication, TLS, or protocol failure.
    #[error("terminal WebSocket error: {0}")]
    Terminal(String),
}

#[cfg(not(target_arch = "wasm32"))]
impl WsError {
    pub(crate) fn from_tungstenite(error: tokio_tungstenite::tungstenite::Error) -> Self {
        use tokio_tungstenite::tungstenite::Error;

        let status = match &error {
            Error::Http(response) => Some(response.status().as_u16()),
            _ => None,
        };
        let message = error.to_string();

        match status {
            Some(404 | 405 | 501) => Self::NotSupported(message),
            Some(408 | 429 | 500..=599) => Self::Transient(message),
            Some(_) => Self::Terminal(message),
            None => match error {
                Error::ConnectionClosed | Error::AlreadyClosed | Error::Io(_) => {
                    Self::Transient(message)
                }
                Error::Tls(_)
                | Error::Capacity(_)
                | Error::Protocol(_)
                | Error::WriteBufferFull(_)
                | Error::Utf8
                | Error::AttackAttempt
                | Error::Url(_)
                | Error::Http(_)
                | Error::HttpFormat(_) => Self::Terminal(message),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn classifies_http_upgrade_statuses() {
        use tokio_tungstenite::tungstenite::http::{Response, StatusCode};
        use tokio_tungstenite::tungstenite::Error;

        let error_for_status = |status| {
            Error::Http(
                Response::builder()
                    .status(status)
                    .body(None)
                    .expect("valid HTTP response"),
            )
        };

        assert!(matches!(
            WsError::from_tungstenite(error_for_status(StatusCode::NOT_FOUND)),
            WsError::NotSupported(_)
        ));
        assert!(matches!(
            WsError::from_tungstenite(error_for_status(StatusCode::UNAUTHORIZED)),
            WsError::Terminal(_)
        ));
        assert!(matches!(
            WsError::from_tungstenite(error_for_status(StatusCode::SERVICE_UNAVAILABLE)),
            WsError::Transient(_)
        ));
    }
}
