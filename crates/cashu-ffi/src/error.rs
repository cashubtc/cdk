//! Errors crossing the FFI boundary.

use cashu::amount::Error as AmountError;
use cashu::dhke::Error as DhkeError;
use cashu::nuts::nut00::Error as Nut00Error;
use cashu::nuts::nut01::Error as Nut01Error;
use cashu::nuts::nut02::Error as Nut02Error;
use cashu::nuts::nut10::Error as Nut10Error;
use cashu::nuts::nut11::Error as Nut11Error;
use cashu::nuts::nut12::Error as Nut12Error;
use cashu::nuts::nut13::Error as Nut13Error;
use thiserror::Error;

/// Errors returned by every fallible export in this crate.
///
/// Variants keep their fields so foreign callers can branch on the cause instead
/// of matching on a formatted string.
#[derive(Debug, Error, uniffi::Error)]
pub enum CashuFfiError {
    /// A hex-encoded argument was not valid hex.
    #[error("field `{field}` is not valid hex: {reason}")]
    InvalidHex {
        /// Name of the offending argument.
        field: String,
        /// Parser message.
        reason: String,
    },
    /// A public key argument could not be parsed.
    #[error("invalid public key in `{field}`: {reason}")]
    InvalidPublicKey {
        /// Name of the offending argument.
        field: String,
        /// Parser message.
        reason: String,
    },
    /// A secret key argument could not be parsed.
    #[error("invalid secret key in `{field}`: {reason}")]
    InvalidSecretKey {
        /// Name of the offending argument.
        field: String,
        /// Parser message.
        reason: String,
    },
    /// A keyset id argument could not be parsed.
    #[error("invalid keyset id `{id}`: {reason}")]
    InvalidKeysetId {
        /// The rejected keyset id.
        id: String,
        /// Parser message.
        reason: String,
    },
    /// NUT-13 requires a 64 byte BIP39 seed.
    #[error("seed must be 64 bytes, got {length}")]
    InvalidSeedLength {
        /// Length actually supplied.
        length: u64,
    },
    /// A restore batch asked for more counters than the crate will derive at once.
    #[error("restore count {count} exceeds the maximum of {max}")]
    InvalidRestoreRange {
        /// Count actually requested.
        count: u32,
        /// Largest count the crate accepts.
        max: u32,
    },
    /// The requested amount could not be split over the available denominations.
    #[error("cannot split amount: {reason}")]
    Split {
        /// Underlying amount error.
        reason: String,
    },
    /// Blinding, unblinding or hash-to-curve failed.
    #[error("dhke failure: {reason}")]
    Dhke {
        /// Underlying dhke error.
        reason: String,
    },
    /// The supplied P2PK options do not describe a spendable output.
    #[error("invalid spending conditions: {reason}")]
    SpendingConditions {
        /// Underlying NUT-10/NUT-11 error.
        reason: String,
    },
    /// A DLEQ proof was malformed. An invalid but well-formed proof returns
    /// `false` instead of an error.
    #[error("malformed dleq proof: {reason}")]
    Dleq {
        /// Underlying NUT-12 error.
        reason: String,
    },
    /// Deterministic derivation failed for the requested counter.
    #[error("nut13 derivation failed at counter {counter}: {reason}")]
    Derivation {
        /// Counter that failed to derive.
        counter: u32,
        /// Underlying NUT-13 error.
        reason: String,
    },
}

impl From<DhkeError> for CashuFfiError {
    fn from(err: DhkeError) -> Self {
        Self::Dhke {
            reason: err.to_string(),
        }
    }
}

impl From<AmountError> for CashuFfiError {
    fn from(err: AmountError) -> Self {
        Self::Split {
            reason: err.to_string(),
        }
    }
}

impl From<Nut00Error> for CashuFfiError {
    fn from(err: Nut00Error) -> Self {
        Self::Split {
            reason: err.to_string(),
        }
    }
}

impl From<Nut01Error> for CashuFfiError {
    fn from(err: Nut01Error) -> Self {
        Self::InvalidPublicKey {
            field: "pubkey".to_string(),
            reason: err.to_string(),
        }
    }
}

impl From<Nut02Error> for CashuFfiError {
    fn from(err: Nut02Error) -> Self {
        Self::InvalidKeysetId {
            id: String::new(),
            reason: err.to_string(),
        }
    }
}

impl From<Nut10Error> for CashuFfiError {
    fn from(err: Nut10Error) -> Self {
        Self::SpendingConditions {
            reason: err.to_string(),
        }
    }
}

impl From<Nut11Error> for CashuFfiError {
    fn from(err: Nut11Error) -> Self {
        Self::SpendingConditions {
            reason: err.to_string(),
        }
    }
}

impl From<Nut12Error> for CashuFfiError {
    fn from(err: Nut12Error) -> Self {
        Self::Dleq {
            reason: err.to_string(),
        }
    }
}

impl From<Nut13Error> for CashuFfiError {
    fn from(err: Nut13Error) -> Self {
        Self::Derivation {
            counter: 0,
            reason: err.to_string(),
        }
    }
}
