//! LND Errors

use thiserror::Error;

/// LND Error
#[derive(Debug, Error)]
pub enum Error {
    /// Invoice amount not defined
    #[error("Unknown invoice amount")]
    UnknownInvoiceAmount,
    /// Unknown invoice
    #[error("Unknown invoice")]
    UnknownInvoice,
    /// Connection error
    #[error("LND connection error")]
    Connection,
    /// Payment failed
    #[error("LND payment failed")]
    PaymentFailed,
}

impl From<Error> for cdk::cdk_lightning::Error {
    fn from(e: Error) -> Self {
        Self::Lightning(Box::new(e))
    }
}
