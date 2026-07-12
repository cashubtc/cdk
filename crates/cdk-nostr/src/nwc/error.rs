//! Error types for the Nostr Wallet Connect service

use thiserror::Error;

/// Result type for the NWC service
pub type Result<T> = std::result::Result<T, Error>;

/// Errors that can occur while running the NWC wallet service
#[derive(Debug, Error)]
pub enum Error {
    /// No relays were configured for the service
    #[error("at least one relay is required")]
    NoRelays,

    /// Failed to add or connect to a relay
    #[error("relay error: {0}")]
    Relay(String),

    /// Failed to build, sign, or publish a Nostr event
    #[error("nostr event error: {0}")]
    Event(String),

    /// Failed to subscribe to the relay pool
    #[error("subscription error: {0}")]
    Subscription(String),

    /// NIP-47 protocol error (encoding/decoding)
    #[error("nip47 protocol error: {0}")]
    Protocol(String),

    /// Encryption or decryption of an event payload failed
    #[error("encryption error: {0}")]
    Encryption(String),

    /// JSON serialization/deserialization error
    #[error("json error: {0}")]
    Serde(#[from] serde_json::Error),
}
