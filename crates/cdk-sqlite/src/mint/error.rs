//! SQLite Database Error

use thiserror::Error;

/// SQLite Database Error
#[derive(Debug, Error)]
pub enum Error {
    /// SQLX Error
    #[error(transparent)]
    SQLX(#[from] sqlx::Error),
    /// NUT00 Error
    #[error(transparent)]
    CDKNUT00(#[from] cdk::nuts::nut00::Error),
    /// NUT01 Error
    #[error(transparent)]
    CDKNUT01(#[from] cdk::nuts::nut01::Error),
    /// NUT02 Error
    #[error(transparent)]
    CDKNUT02(#[from] cdk::nuts::nut02::Error),
    /// NUT04 Error
    #[error(transparent)]
    CDKNUT04(#[from] cdk::nuts::nut04::Error),
    /// NUT05 Error
    #[error(transparent)]
    CDKNUT05(#[from] cdk::nuts::nut05::Error),
    /// NUT07 Error
    #[error(transparent)]
    CDKNUT07(#[from] cdk::nuts::nut07::Error),
    /// Secret Error
    #[error(transparent)]
    CDKSECRET(#[from] cdk::secret::Error),
    /// BIP32 Error
    #[error(transparent)]
    BIP32(#[from] bitcoin::bip32::Error),
    /// Mint Url Error
    #[error(transparent)]
    MintUrl(#[from] cdk::mint_url::Error),
    /// Could Not Initialize Database
    #[error("Could not initialize database")]
    CouldNotInitialize,
    /// Invalid Database Path
    #[error("Invalid database path")]
    InvalidDbPath,
    /// Invalid bolt11
    #[error("Invalid bolt11")]
    InvalidBolt11,
    /// Serde Error
    #[error(transparent)]
    Serde(#[from] serde_json::Error),
}

impl From<Error> for cdk::cdk_database::Error {
    fn from(e: Error) -> Self {
        Self::Database(Box::new(e))
    }
}
