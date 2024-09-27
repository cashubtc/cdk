//! Error for LNbits ln backend

use thiserror::Error;

/// LNbits Error
#[derive(Debug, Error)]
pub enum Error {
    /// Invoice amount not defined
    #[error("Unknown invoice amount")]
    UnknownInvoiceAmount,
    /// Unknown invoice
    #[error("Unknown invoice")]
    UnknownInvoice,
    /// Wrong invoice type
    #[error("Wrong invoice type")]
    WrongRequestType,
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
