//! LND Errors

use fedimint_tonic_lnd::tonic::Status;
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
    /// Errors coming from the backend
    #[error("LND error: `{0}`")]
    LndError(Status),
}

impl From<Error> for cdk::cdk_lightning::Error {
    fn from(e: Error) -> Self {
        Self::Lightning(Box::new(e))
    }
}
