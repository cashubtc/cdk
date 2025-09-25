//! Fake Wallet Error

use thiserror::Error;

/// Fake Wallet Error
#[derive(Debug, Error)]
pub enum Error {
    /// Invoice amount not defined
    #[error("Unknown invoice amount")]
    UnknownInvoiceAmount,
    /// Unknown invoice
    #[error("Unknown invoice")]
    UnknownInvoice,
    /// No channel receiver
    #[error("No channel receiver")]
    NoReceiver,
    /// Invoice already paid
    #[error("Invoice already paid")]
    AlreadyPaid,
    /// Serde Error
    #[error(transparent)]
    Serde(#[from] serde_json::Error),
    /// Database Error
    #[error("Database error: {0}")]
    Database(String),
}

impl From<Error> for cdk_common::payment::Error {
    fn from(e: Error) -> Self {
        Self::Lightning(Box::new(e))
    }
}
