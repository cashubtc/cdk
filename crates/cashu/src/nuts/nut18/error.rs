//! Error types for NUT-18: Payment Requests

use thiserror::Error;

/// NUT18 Error
#[derive(Debug, Error)]
pub enum Error {
    /// Invalid Prefix
    #[error("Invalid Prefix")]
    InvalidPrefix,
    /// Ciborium error
    #[error(transparent)]
    CiboriumError(#[from] ciborium::de::Error<std::io::Error>),
    /// Base64 error
    #[error(transparent)]
    Base64Error(#[from] bitcoin::base64::DecodeError),
    /// NUT-26 bech32m encoding error
    #[error(transparent)]
    Nut26Error(#[from] crate::nuts::nut26::Error),
}
