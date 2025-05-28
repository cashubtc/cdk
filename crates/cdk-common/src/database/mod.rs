//! CDK Database

#[cfg(feature = "mint")]
pub mod mint;
#[cfg(feature = "wallet")]
mod wallet;

#[cfg(all(feature = "mint", feature = "auth"))]
pub use mint::MintAuthDatabase;
#[cfg(feature = "mint")]
pub use mint::{
    Database as MintDatabase, FiltersDatabase as MintFiltersDatabase,
    KeysDatabase as MintKeysDatabase, ProofsDatabase as MintProofsDatabase,
    QuotesDatabase as MintQuotesDatabase, SignaturesDatabase as MintSignaturesDatabase,
};
#[cfg(feature = "wallet")]
pub use wallet::Database as WalletDatabase;

/// CDK_database error
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// Database Error
    #[error(transparent)]
    Database(Box<dyn std::error::Error + Send + Sync>),
    /// DHKE error
    #[error(transparent)]
    DHKE(#[from] crate::dhke::Error),
    /// NUT00 Error
    #[error(transparent)]
    NUT00(#[from] crate::nuts::nut00::Error),
    /// NUT02 Error
    #[error(transparent)]
    NUT02(#[from] crate::nuts::nut02::Error),
    /// NUT22 Error
    #[error(transparent)]
    #[cfg(feature = "auth")]
    NUT22(#[from] crate::nuts::nut22::Error),
    /// Serde Error
    #[error(transparent)]
    Serde(#[from] serde_json::Error),
    /// Unknown Quote
    #[error("Unknown Quote")]
    UnknownQuote,
    /// Attempt to remove spent proof
    #[error("Attempt to remove spent proof")]
    AttemptRemoveSpentProof,
    /// Attempt to update state of spent proof
    #[error("Attempt to update state of spent proof")]
    AttemptUpdateSpentProof,
    /// Proof not found
    #[error("Proof not found")]
    ProofNotFound,
    /// Invalid keyset
    #[error("Unknown or invalid keyset")]
    InvalidKeysetId,
    #[cfg(feature = "mint")]
    /// Invalid state transition
    #[error("Invalid state transition")]
    InvalidStateTransition(crate::state::Error),
}

#[cfg(feature = "mint")]
impl From<crate::state::Error> for Error {
    fn from(state: crate::state::Error) -> Self {
        match state {
            crate::state::Error::AlreadySpent => Error::AttemptUpdateSpentProof,
            _ => Error::InvalidStateTransition(state),
        }
    }
}
