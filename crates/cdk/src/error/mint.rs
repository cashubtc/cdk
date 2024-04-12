use thiserror::Error;

use crate::nuts::nut01;

#[derive(Debug, Error)]
pub enum Error {
    #[error("No key for amount")]
    AmountKey,
    #[error("Amount miss match")]
    Amount,
    #[error("Token Already Spent")]
    TokenSpent,
    /// Secp256k1 error
    #[error(transparent)]
    Secp256k1(#[from] bitcoin::secp256k1::Error),
    /// NUT01 error
    #[error(transparent)]
    NUT01(#[from] nut01::Error),
    #[error("`Token not verified`")]
    TokenNotVerifed,
    #[error("Invoice amount undefined")]
    InvoiceAmountUndefined,
    /// Duplicate Proofs sent in request
    #[error("Duplicate proofs")]
    DuplicateProofs,
    /// Keyset id not active
    #[error("Keyset id is not active")]
    InactiveKeyset,
    /// Keyset is not known
    #[error("Unknown Keyset")]
    UnknownKeySet,
    #[error(transparent)]
    Secret(#[from] crate::secret::Error),
    #[error(transparent)]
    Cashu(#[from] super::Error),
    #[error("`{0}`")]
    CustomError(String),
}
