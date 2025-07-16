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
    /// Unknown invoice
    #[error("No channel receiver")]
    NoReceiver,
}

impl From<Error> for cdk_common::payment::Error {
    fn from(e: Error) -> Self {
        Self::Lightning(Box::new(e))
    }
}
