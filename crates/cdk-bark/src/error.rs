//! Error types for the bark mint payment backend.

use cdk_common::payment;

/// Errors produced by [`crate::BarkMintPayment`].
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// An error returned by the underlying `bark` wallet.
    #[error("bark error: {0}")]
    Bark(#[source] anyhow::Error),
    /// The notification stream has already been taken by another caller.
    #[error("payment event stream already taken")]
    StreamAlreadyTaken,
    /// A BOLT11 invoice was missing an amount.
    #[error("invoice has no amount")]
    UnknownInvoiceAmount,
    /// A payment hash is unknown to the wallet.
    #[error("unknown invoice")]
    UnknownInvoice,
    /// A `PaymentIdentifier` variant unsupported by this backend.
    #[error("unsupported payment identifier")]
    UnsupportedIdentifier,
}

impl From<Error> for payment::Error {
    fn from(err: Error) -> Self {
        payment::Error::Anyhow(anyhow::anyhow!(err))
    }
}
