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
    /// Unknown payment status
    #[error("LND unknown payment status")]
    UnknownPaymentStatus,
    /// Missing last hop in route
    #[error("LND missing last hop in route")]
    MissingLastHop,
    /// No MPP record in last hop
    #[error("LND missing MPP record in last hop")]
    MissingMppRecord,
}

impl From<Error> for cdk::cdk_lightning::Error {
    fn from(e: Error) -> Self {
        Self::Lightning(Box::new(e))
    }
}
