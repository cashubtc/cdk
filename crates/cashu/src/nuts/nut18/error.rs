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
    /// A requested nutroot leaf cannot be reproduced byte for byte
    #[error("A requested nutroot leaf cannot be reproduced")]
    NutrootLeaf,
    /// A NUMS request without leaves locks funds nobody can spend
    #[error("A script-only (NUMS) request must list leaves")]
    NutrootNoLeaves,
    /// A received proof does not match the nutroot request
    #[error("Payment does not match the nutroot request: {0}")]
    NutrootMismatch(&'static str),
    /// Slot derivation error
    #[error(transparent)]
    Nut28(#[from] crate::nuts::nut28::Error),
    /// Spend info error
    #[error(transparent)]
    SpendInfo(#[from] crate::nuts::nut00::spend_info::Error),
}
