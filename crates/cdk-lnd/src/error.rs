//! LND Errors

use thiserror::Error;
use tonic::Status;

/// LND Error
#[derive(Debug, Error)]
pub enum Error {
    /// Amount Error
    #[error(transparent)]
    Amount(#[from] cdk_common::amount::Error),
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
    /// Payment dispatch outcome is indeterminate
    ///
    /// Returned when a `send_*` call or its status stream fails at the
    /// dispatch boundary: LND may or may not have accepted the payment. This
    /// is deliberately distinct from [`Error::PaymentFailed`], which is only
    /// constructed after every route retry ended in a definitive no-route
    /// result (i.e. pre-dispatch). Backends must keep this variant out of the
    /// terminal-failure classifier so the melt stays indeterminate.
    #[error("LND payment dispatch outcome is indeterminate")]
    AmbiguousDispatch,
    /// Unknown payment status
    #[error("LND unknown payment status")]
    UnknownPaymentStatus,
    /// Missing last hop in route
    #[error("LND missing last hop in route")]
    MissingLastHop,
    /// No route found
    #[error("No route found")]
    NoRoute,
    /// Amount overflow
    #[error("Amount overflow")]
    AmountOverflow,
    /// Errors coming from the backend
    #[error("LND error: `{0}`")]
    LndError(Status),
    /// Errors invalid config
    #[error("LND invalid config: `{0}`")]
    InvalidConfig(String),
    /// Could not read file
    #[error("Could not read file")]
    ReadFile,
    /// Database Error
    #[error("Database error: {0}")]
    Database(String),
}

impl From<Error> for cdk_common::payment::Error {
    fn from(e: Error) -> Self {
        Self::Lightning(Box::new(e))
    }
}
