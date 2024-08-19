//! Greenlight Errors

use thiserror::Error;

/// Greenlight Error
#[derive(Debug, Error)]
pub enum Error {
    /// Invoice amount not defined
    #[error("Unknown invoice amount")]
    UnknownInvoiceAmount,
    /// Wrong CLN response
    #[error("Wrong cln response")]
    WrongClnResponse,
    /// Unknown invoice
    #[error("Unknown invoice")]
    UnknownInvoice,
    /// Anyhow Error
    #[error(transparent)]
    Anyhow(#[from] anyhow::Error),
}

impl From<Error> for cdk::cdk_lightning::Error {
    fn from(e: Error) -> Self {
        Self::Lightning(Box::new(e))
    }
}
