//! Error types for Nostr keys, NIP-44 and the NIP-17 inbox

use thiserror::Error;

/// Result type for cdk-nostr base operations
pub type Result<T> = std::result::Result<T, Error>;

/// Errors for key handling, NIP-44 encryption and the NIP-17 inbox listener
#[derive(Debug, Error)]
pub enum Error {
    /// The secret key is not valid (bad hex/bech32 or out of range)
    #[error("invalid secret key: {0}")]
    InvalidSecretKey(String),

    /// The public key is not valid
    #[error("invalid public key: {0}")]
    InvalidPublicKey(String),

    /// NIP-44 encryption or decryption failed
    #[error("nip44 error: {0}")]
    Nip44(String),

    /// At least one relay must be configured
    #[error("at least one relay is required")]
    NoRelays,

    /// Failed to add or connect to a relay
    #[error("relay error: {0}")]
    Relay(String),

    /// Failed to subscribe to the relay pool
    #[error("subscription error: {0}")]
    Subscription(String),
}
