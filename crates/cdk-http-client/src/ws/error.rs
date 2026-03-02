//! WebSocket error types

/// Errors that can occur during WebSocket operations
#[derive(Debug, thiserror::Error)]
pub enum WsError {
    /// Failed to establish a WebSocket connection
    #[error("WebSocket connection error: {0}")]
    Connection(String),
    /// Failed to send a WebSocket message
    #[error("WebSocket send error: {0}")]
    Send(String),
    /// Failed to receive a WebSocket message
    #[error("WebSocket receive error: {0}")]
    Receive(String),
}
