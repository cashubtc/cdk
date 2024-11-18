//! Error for Strike ln backend

use thiserror::Error;

/// Strike Error
#[derive(Debug, Error)]
pub enum Error {
    /// Invoice amount not defined
    #[error("Unknown invoice amount")]
    UnknownInvoiceAmount,
    /// Unknown invoice
    #[error("Unknown invoice")]
    UnknownInvoice,
    /// Strikers error
    #[error(transparent)]
    StrikeRs(#[from] strike_rs::Error),
    /// Unsupported method
    #[error("Unsupported method")]
    UnsupportedMethod,
    /// Anyhow error
    #[error(transparent)]
    Anyhow(#[from] anyhow::Error),
}

impl From<Error> for cdk::cdk_lightning::Error {
    fn from(e: Error) -> Self {
        Self::Lightning(Box::new(e))
    }
}
