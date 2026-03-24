//! Error types for NUT-10: Spending Conditions

use thiserror::Error;

use crate::nuts::{nut01, nut11, nut14};
use crate::util::hex;

/// NUT10 Error
#[derive(Debug, Error)]
pub enum Error {
    /// Unknown Kind
    #[error("Kind not found")]
    KindNotFound,
    /// Tag value not found
    #[error("Tag value not found")]
    TagValueNotFound,
    /// Incorrect secret kind
    #[error("Incorrect secret kind")]
    IncorrectSecretKind,
    /// Spend conditions not met
    #[error("Spend conditions are not met")]
    SpendConditionsNotMet,

    /// From hex error
    #[error(transparent)]
    HexError(#[from] hex::Error),
    /// Parse int error
    #[error(transparent)]
    ParseInt(#[from] std::num::ParseIntError),
    /// Serde Json error
    #[error(transparent)]
    SerdeJsonError(#[from] serde_json::Error),
    /// Secret error
    #[error(transparent)]
    Secret(#[from] crate::secret::Error),

    /// NUT01 Error
    #[error(transparent)]
    NUT01(#[from] nut01::Error),
    /// NUT11 Error
    #[error(transparent)]
    NUT11(#[from] nut11::Error),
    /// NUT14 Error
    #[error(transparent)]
    NUT14(#[from] nut14::Error),
}
