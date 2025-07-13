//! SQLite Database Error

use thiserror::Error;

/// SQLite Database Error
#[derive(Debug, Error)]
pub enum Error {
    /// SQLX Error
    #[error(transparent)]
    Sqlite(#[from] rusqlite::Error),

    /// Duplicate entry
    #[error("Record already exists")]
    Duplicate,

    /// Pool error
    #[error(transparent)]
    Pool(#[from] crate::pool::Error<rusqlite::Error>),
    /// Invalid UUID
    #[error("Invalid UUID: {0}")]
    InvalidUuid(String),
    /// QuoteNotFound
    #[error("Quote not found")]
    QuoteNotFound,

    /// Missing named parameter
    #[error("Missing named parameter {0}")]
    MissingParameter(String),

    /// Communication error with the database
    #[error("Internal communication error")]
    Communication,

    /// Invalid response from the database thread
    #[error("Unexpected database response")]
    InvalidDbResponse,

    /// Invalid db type
    #[error("Invalid type from db, expected {0} got {1}")]
    InvalidType(String, String),

    /// Missing columns
    #[error("Not enough elements: expected {0}, got {1}")]
    MissingColumn(usize, usize),

    /// Invalid data conversion in column
    #[error("Error converting {0} to {1}")]
    InvalidConversion(String, String),

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
    /// BIP32 Error
    #[error(transparent)]
    BIP32(#[from] bitcoin::bip32::Error),
    /// Mint Url Error
    #[error(transparent)]
    MintUrl(#[from] cdk_common::mint_url::Error),
    /// Could Not Initialize Database
    #[error("Could not initialize database")]
    CouldNotInitialize,
    /// Invalid Database Path
    #[error("Invalid database path")]
    InvalidDbPath,
    /// Serde Error
    #[error(transparent)]
    Serde(#[from] serde_json::Error),
    /// Unknown Mint Info
    #[error("Unknown mint info")]
    UnknownMintInfo,
    /// Unknown quote TTL
    #[error("Unknown quote TTL")]
    UnknownQuoteTTL,
    /// Unknown config key
    #[error("Unknown config key: {0}")]
    UnknownConfigKey(String),
    /// Proof not found
    #[error("Proof not found")]
    ProofNotFound,
    /// Invalid keyset ID
    #[error("Invalid keyset ID")]
    InvalidKeysetId,
}

impl From<Error> for cdk_common::database::Error {
    fn from(e: Error) -> Self {
        match e {
            Error::Duplicate => Self::Duplicate,
            e => Self::Database(Box::new(e)),
        }
    }
}
