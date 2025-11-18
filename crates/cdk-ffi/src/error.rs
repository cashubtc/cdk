//! FFI Error types

use cdk::Error as CdkError;

/// FFI Error type that wraps CDK errors for cross-language use
#[derive(Debug, thiserror::Error, uniffi::Error)]
#[uniffi(flat_error)]
pub enum FfiError {
    /// Generic error with message
    #[error("CDK Error: {msg}")]
    Generic { msg: String },

    /// Amount overflow
    #[error("Amount overflow")]
    AmountOverflow,

    /// Division by zero
    #[error("Division by zero")]
    DivisionByZero,

    /// Amount error
    #[error("Amount error: {msg}")]
    Amount { msg: String },

    /// Payment failed
    #[error("Payment failed")]
    PaymentFailed,

    /// Payment pending
    #[error("Payment pending")]
    PaymentPending,

    /// Insufficient funds
    #[error("Insufficient funds")]
    InsufficientFunds,

    /// Database error
    #[error("Database error: {msg}")]
    Database { msg: String },

    /// Network error
    #[error("Network error: {msg}")]
    Network { msg: String },

    /// Invalid token
    #[error("Invalid token: {msg}")]
    InvalidToken { msg: String },

    /// Wallet error
    #[error("Wallet error: {msg}")]
    Wallet { msg: String },

    /// Keyset unknown
    #[error("Keyset unknown")]
    KeysetUnknown,

    /// Unit not supported
    #[error("Unit not supported")]
    UnitNotSupported,

    /// Runtime task join error
    #[error("Runtime task join error: {msg}")]
    RuntimeTaskJoin { msg: String },

    /// Invalid mnemonic phrase
    #[error("Invalid mnemonic: {msg}")]
    InvalidMnemonic { msg: String },

    /// URL parsing error
    #[error("Invalid URL: {msg}")]
    InvalidUrl { msg: String },

    /// Hex format error
    #[error("Invalid hex format: {msg}")]
    InvalidHex { msg: String },

    /// Cryptographic key parsing error
    #[error("Invalid cryptographic key: {msg}")]
    InvalidCryptographicKey { msg: String },

    /// Serialization/deserialization error
    #[error("Serialization error: {msg}")]
    Serialization { msg: String },
}

impl From<CdkError> for FfiError {
    fn from(err: CdkError) -> Self {
        match err {
            CdkError::AmountOverflow => FfiError::AmountOverflow,
            CdkError::PaymentFailed => FfiError::PaymentFailed,
            CdkError::PaymentPending => FfiError::PaymentPending,
            CdkError::InsufficientFunds => FfiError::InsufficientFunds,
            CdkError::UnsupportedUnit => FfiError::UnitNotSupported,
            CdkError::KeysetUnknown(_) => FfiError::KeysetUnknown,
            _ => FfiError::Generic {
                msg: err.to_string(),
            },
        }
    }
}

impl From<cdk::amount::Error> for FfiError {
    fn from(err: cdk::amount::Error) -> Self {
        FfiError::Amount {
            msg: err.to_string(),
        }
    }
}

impl From<cdk::nuts::nut00::Error> for FfiError {
    fn from(err: cdk::nuts::nut00::Error) -> Self {
        FfiError::Generic {
            msg: err.to_string(),
        }
    }
}

impl From<serde_json::Error> for FfiError {
    fn from(err: serde_json::Error) -> Self {
        FfiError::Serialization {
            msg: err.to_string(),
        }
    }
}
