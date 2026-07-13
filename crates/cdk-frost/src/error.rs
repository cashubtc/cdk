//! Error types for the FROST key boundary.

use thiserror::Error;

/// Error returned by canonical package, application, nonce, or signing checks.
#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum Error {
    /// An encoded value is malformed or non-canonical.
    #[error("malformed {kind}: {reason}")]
    MalformedInput {
        /// Kind of value being decoded.
        kind: &'static str,
        /// Validation failure.
        reason: String,
    },
    /// An encoded value exceeds its protocol limit.
    #[error("{kind} exceeds maximum size of {max} bytes")]
    OversizedInput {
        /// Kind of value being decoded.
        kind: &'static str,
        /// Maximum accepted size.
        max: usize,
    },
    /// A participant is absent, duplicated, or does not match the local share.
    #[error("invalid participant {participant}: {reason}")]
    InvalidParticipant {
        /// Numeric participant identifier.
        participant: u16,
        /// Validation failure.
        reason: String,
    },
    /// A threshold or signer count is invalid.
    #[error("insufficient threshold material: required {required}, provided {provided}")]
    InsufficientThreshold {
        /// Required signer count.
        required: u16,
        /// Provided signer count.
        provided: usize,
    },
    /// An application is unknown, duplicated, or has incompatible metadata.
    #[error("application boundary rejected request: {0}")]
    Application(String),
    /// A request does not belong to the expected signing session.
    #[error("wrong signing session")]
    WrongSession,
    /// A nonce commitment is invalid, unknown, or already consumed.
    #[error("invalid nonce commitment: {0}")]
    InvalidCommitment(String),
    /// A signature share failed validation.
    #[error("invalid signature share: {0}")]
    InvalidShare(String),
    /// The aggregate signature failed independent verification.
    #[error("aggregate signature verification failed")]
    AggregateVerification,
    /// The selected library rejected a cryptographic operation.
    #[error("FROST operation failed: {0}")]
    Frost(String),
    /// Internal invariants failed closed.
    #[error("internal FROST boundary failure: {0}")]
    Internal(String),
}

impl Error {
    pub(crate) fn malformed(kind: &'static str, reason: impl ToString) -> Self {
        Self::MalformedInput {
            kind,
            reason: reason.to_string(),
        }
    }
}
