//! Error for Blink ln backend

use thiserror::Error;

/// Blink Error
#[derive(Debug, Error)]
pub enum Error {
    /// Invoice amount not defined
    #[error("Unknown invoice amount")]
    UnknownInvoiceAmount,
    /// Unknown invoice
    #[error("Unknown invoice")]
    UnknownInvoice,
    /// Amount overflow
    #[error("Amount overflow")]
    AmountOverflow,
    /// Invalid payment hash
    #[error("Invalid payment hash")]
    InvalidPaymentHash,
    /// GraphQL API error
    #[error("GraphQL error: {0}")]
    GraphQL(String),
    /// Unsupported currency unit
    #[error("Unsupported unit")]
    UnsupportedUnit,
    /// Wallet ID not found
    #[error("Wallet ID not found for the requested unit")]
    WalletIdNotFound,
    /// Currency conversion failed
    #[error("Currency conversion failed")]
    CurrencyConversionFailed,
    /// Reqwest error
    #[error(transparent)]
    Reqwest(#[from] reqwest::Error),
    /// Anyhow error
    #[error(transparent)]
    Anyhow(#[from] anyhow::Error),
}

impl From<Error> for cdk_common::payment::Error {
    fn from(e: Error) -> Self {
        Self::Lightning(Box::new(e))
    }
}
