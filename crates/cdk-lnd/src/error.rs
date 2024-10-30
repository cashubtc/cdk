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
    /// Invalid hash
    #[error("Invalid hash")]
    InvalidHash,
    /// Payment failed
    #[error("LND payment failed")]
    PaymentFailed,
    /// Unsupported method
    #[error("Unsupported method")]
    UnsupportedMethod,
    /// Wrong invoice type
    #[error("Wrong invoice type")]
    WrongRequestType,
    /// Unknown payment status
    #[error("LND unknown payment status")]
    UnknownPaymentStatus,
}

impl From<Error> for cdk::cdk_lightning::Error {
    fn from(e: Error) -> Self {
        Self::Lightning(Box::new(e))
    }
}
