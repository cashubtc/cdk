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
    /// Amount overflow
    #[error("Amount overflow")]
    AmountOverflow,
    /// Invalid payment hash
    #[error("Invalid payment hash")]
    InvalidPaymentHash,
    /// Anyhow error
    #[error(transparent)]
    Anyhow(#[from] anyhow::Error),
}

impl From<Error> for cdk_common::payment::Error {
    fn from(e: Error) -> Self {
        Self::Lightning(Box::new(e))
    }
}
