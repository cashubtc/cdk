//! CDK Database

#[cfg(feature = "mint")]
mod mint;
#[cfg(feature = "wallet")]
mod wallet;

#[cfg(feature = "mint")]
pub use mint::Database as MintDatabase;
#[cfg(feature = "mint")]
pub use mint::MintAuthDatabase;
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
    /// NUT00 Error
    #[error(transparent)]
    NUTXX1(#[from] crate::nuts::nutxx1::Error),
    /// Serde Error
    #[error(transparent)]
    Serde(#[from] serde_json::Error),
    /// Unknown Quote
    #[error("Unknown Quote")]
    UnknownQuote,
}
