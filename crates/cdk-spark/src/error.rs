//! Error types for CDK Spark integration

use cdk_common::payment;
use thiserror::Error;

/// Errors that can occur when using the Spark Lightning backend
#[derive(Debug, Error)]
pub enum Error {
    /// Spark wallet error
    #[error("Spark wallet error: {0}")]
    SparkWallet(#[from] spark_wallet::SparkWalletError),

    /// Configuration error
    #[error("Configuration error: {0}")]
    Configuration(String),

    /// Invalid mnemonic
    #[error("Invalid mnemonic: {0}")]
    InvalidMnemonic(String),

    /// Invoice parsing error
    #[error("Invoice parsing error: {0}")]
    InvoiceParse(String),

    /// Payment not found
    #[error("Payment not found")]
    PaymentNotFound,

    /// Payment timeout
    #[error("Payment timeout")]
    PaymentTimeout,

    /// Unknown invoice amount
    #[error("Unknown invoice amount")]
    UnknownInvoiceAmount,

    /// Amount conversion error
    #[error("Amount conversion error: {0}")]
    AmountConversion(String),

    /// Invalid payment identifier
    #[error("Invalid payment identifier")]
    InvalidPaymentIdentifier,

    /// Payment stream error
    #[error("Payment stream error: {0}")]
    PaymentStream(String),

    /// BOLT12 not supported in this context
    #[error("BOLT12 offer operation not supported")]
    Bolt12NotSupported,

    /// Network error
    #[error("Network error: {0}")]
    Network(String),

    /// Serialization error
    #[error("Serialization error: {0}")]
    Serialization(#[from] serde_json::Error),

    /// Anyhow error for wrapping other errors
    #[error(transparent)]
    Anyhow(#[from] anyhow::Error),
}

impl From<Error> for payment::Error {
    fn from(e: Error) -> Self {
        match e {
            Error::SparkWallet(ref spark_err) => {
                payment::Error::Anyhow(anyhow::anyhow!("Spark wallet error: {}", spark_err))
            }
            Error::Configuration(ref msg) => {
                payment::Error::Anyhow(anyhow::anyhow!("Configuration error: {}", msg))
            }
            Error::InvalidMnemonic(ref msg) => {
                payment::Error::Anyhow(anyhow::anyhow!("Invalid mnemonic: {}", msg))
            }
            Error::InvoiceParse(ref msg) => {
                payment::Error::Anyhow(anyhow::anyhow!("Invoice parse error: {}", msg))
            }
            Error::PaymentNotFound => payment::Error::Anyhow(anyhow::anyhow!("Payment not found")),
            Error::PaymentTimeout => payment::Error::Anyhow(anyhow::anyhow!("Payment timeout")),
            Error::UnknownInvoiceAmount => {
                payment::Error::Anyhow(anyhow::anyhow!("Unknown invoice amount"))
            }
            Error::AmountConversion(ref msg) => {
                payment::Error::Anyhow(anyhow::anyhow!("Amount conversion error: {}", msg))
            }
            Error::InvalidPaymentIdentifier => {
                payment::Error::Anyhow(anyhow::anyhow!("Invalid payment identifier"))
            }
            Error::PaymentStream(ref msg) => {
                payment::Error::Anyhow(anyhow::anyhow!("Payment stream error: {}", msg))
            }
            Error::Bolt12NotSupported => {
                payment::Error::Anyhow(anyhow::anyhow!("BOLT12 not supported"))
            }
            Error::Network(ref msg) => {
                payment::Error::Anyhow(anyhow::anyhow!("Network error: {}", msg))
            }
            Error::Serialization(ref e) => {
                payment::Error::Anyhow(anyhow::anyhow!("Serialization error: {}", e))
            }
            Error::Anyhow(e) => payment::Error::Anyhow(e),
        }
    }
}

