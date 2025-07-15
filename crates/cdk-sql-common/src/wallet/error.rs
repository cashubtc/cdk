//! SQLite Wallet Error

use thiserror::Error;

/// SQL Wallet Error
#[derive(Debug, Error)]
pub enum Error {
    /// SQLX Error
    #[error(transparent)]
    Sqlite(#[from] rusqlite::Error),
    /// Pool error
    #[error(transparent)]
    Pool(#[from] crate::pool::Error<rusqlite::Error>),

    /// Missing columns
    #[error("Not enough elements: expected {0}, got {1}")]
    MissingColumn(usize, usize),

    /// Invalid db type
    #[error("Invalid type from db, expected {0} got {1}")]
    InvalidType(String, String),

    /// Invalid data conversion in column
    #[error("Error converting {0} to {1}")]
    InvalidConversion(String, String),

    /// Serde Error
    #[error(transparent)]
    Serde(#[from] serde_json::Error),
    /// CDK Error
    #[error(transparent)]
    CDK(#[from] cdk_common::Error),
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
    /// NUT23 Error
    #[error(transparent)]
    CDKNUT23(#[from] cdk_common::nuts::nut23::Error),
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
