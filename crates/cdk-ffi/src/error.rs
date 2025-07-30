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

    /// Invalid amount
    #[error("Invalid amount")]
    InvalidAmount,

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
    #[error("Invalid token")]
    InvalidToken,

    /// Keyset unknown
    #[error("Keyset unknown")]
    KeysetUnknown,

    /// Unit not supported
    #[error("Unit not supported")]
    UnitNotSupported,

    /// Wallet error
    #[error("Wallet error: {msg}")]
    Wallet { msg: String },
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

impl From<anyhow::Error> for FfiError {
    fn from(err: anyhow::Error) -> Self {
        FfiError::Generic {
            msg: err.to_string(),
        }
    }
}
