//! Error for phoenixd ln backend

use thiserror::Error;

/// Phoenixd Error
#[derive(Debug, Error)]
pub enum Error {
    /// Invoice amount not defined
    #[error("Unknown invoice amount")]
    UnknownInvoiceAmount,
    /// Unknown invoice
    #[error("Unknown invoice")]
    UnknownInvoice,
    /// Unsupported unit
    #[error("Unit Unsupported")]
    UnsupportedUnit,
    /// phd error
    #[error(transparent)]
    Phd(#[from] phoenixd_rs::Error),
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
