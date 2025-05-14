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
    /// Amount overflow
    #[error("Amount overflow")]
    AmountOverflow,
    /// Errors coming from the backend
    #[error("LND error: `{0}`")]
    LndError(Status),
    /// Errors invalid config
    #[error("LND invalid config: `{0}`")]
    InvalidConfig(String),
}

impl From<Error> for cdk_common::payment::Error {
    fn from(e: Error) -> Self {
        Self::Lightning(Box::new(e))
    }
}
