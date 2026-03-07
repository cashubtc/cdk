//! Error types for NUT-10: Spending Conditions

use crate::util::hex;
use thiserror::Error;

/// NUT10 Error
#[derive(Debug, Error)]
pub enum Error {
    /// HTLC hash invalid
    #[error("Invalid hash")]
    InvalidHash,
    /// HTLC preimage too large
    #[error("Preimage exceeds maximum size of 32 bytes (64 hex characters)")]
    PreimageTooLarge,
    /// Incorrect secret kind
    #[error("Secret is not a p2pk secret")]
    IncorrectSecretKind,
    /// Incorrect secret kind
    #[error("Witness is not a p2pk witness")]
    IncorrectWitnessKind,
    /// Witness signature is not valid
    #[error("Invalid signature")]
    InvalidSignature,
    /// P2PK Spend conditions not meet
    #[error("P2PK spend conditions are not met")]
    SpendConditionsNotMet,
    /// Witness Signatures not provided
    #[error("Witness signatures not provided")]
    SignaturesNotProvided,
    /// Cannot parse conditions from secret
    #[error("Cannot parse conditions from secret")]
    CannotParseConditions,
    /// From hex error
    #[error(transparent)]
    HexError(#[from] hex::Error),
    /// Serde Json error
    #[error(transparent)]
    SerdeJsonError(#[from] serde_json::Error),
    /// Secret error
    #[error(transparent)]
    Secret(#[from] crate::secret::Error),
    /// NUT01 Error
    #[error(transparent)]
    NUT01(#[from] crate::nuts::nut01::Error),
}
