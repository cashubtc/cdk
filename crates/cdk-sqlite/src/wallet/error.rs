//! SQLite Wallet Error

use thiserror::Error;

/// SQLite Wallet Error
#[derive(Debug, Error)]
pub enum Error {
    /// SQLX Error
    #[error(transparent)]
    SQLX(#[from] sqlx::Error),
    /// SQLX Migration Error
    #[error(transparent)]
    SQLXMigration(#[from] sqlx::migrate::MigrateError),
    /// Serde Error
    #[error(transparent)]
    Serde(#[from] serde_json::Error),
    /// NUT00 Error
    #[error(transparent)]
    CDKNUT00(#[from] cdk_common::nuts::nut00::Error),
    /// NUT01 Error
    #[error(transparent)]
    CDKNUT01(#[from] cdk_common::nuts::nut01::Error),
    /// NUT02 Error
    #[error(transparent)]
    CDKNUT02(#[from] cdk_common::nuts::nut02::Error),
    /// NUT04 Error
    #[error(transparent)]
    CDKNUT04(#[from] cdk_common::nuts::nut04::Error),
    /// NUT05 Error
    #[error(transparent)]
    CDKNUT05(#[from] cdk_common::nuts::nut05::Error),
    /// NUT07 Error
    #[error(transparent)]
    CDKNUT07(#[from] cdk_common::nuts::nut07::Error),
    /// Secret Error
    #[error(transparent)]
    CDKSECRET(#[from] cdk_common::secret::Error),
    /// Mint Url
    #[error(transparent)]
    MintUrl(#[from] cdk_common::mint_url::Error),
    /// BIP32 Error
    #[error(transparent)]
    BIP32(#[from] bitcoin::bip32::Error),
    /// Could Not Initialize Database
    #[error("Could not initialize database")]
    CouldNotInitialize,
    /// Invalid Database Path
    #[error("Invalid database path")]
    InvalidDbPath,
}

impl From<Error> for cdk_common::database::Error {
    fn from(e: Error) -> Self {
        Self::Database(Box::new(e))
    }
}
