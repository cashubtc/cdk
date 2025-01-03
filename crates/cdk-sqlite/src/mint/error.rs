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
    CDKNUT00(#[from] cashu::nuts::nut00::Error),
    /// NUT01 Error
    #[error(transparent)]
    CDKNUT01(#[from] cashu::nuts::nut01::Error),
    /// NUT02 Error
    #[error(transparent)]
    CDKNUT02(#[from] cashu::nuts::nut02::Error),
    /// NUT04 Error
    #[error(transparent)]
    CDKNUT04(#[from] cashu::nuts::nut04::Error),
    /// NUT05 Error
    #[error(transparent)]
    CDKNUT05(#[from] cashu::nuts::nut05::Error),
    /// NUT07 Error
    #[error(transparent)]
    CDKNUT07(#[from] cashu::nuts::nut07::Error),
    /// Secret Error
    #[error(transparent)]
    CDKSECRET(#[from] cashu::secret::Error),
    /// BIP32 Error
    #[error(transparent)]
    BIP32(#[from] bitcoin::bip32::Error),
    /// Mint Url Error
    #[error(transparent)]
    MintUrl(#[from] cashu::mint_url::Error),
    /// Could Not Initialize Database
    #[error("Could not initialize database")]
    CouldNotInitialize,
    /// Invalid Database Path
    #[error("Invalid database path")]
    InvalidDbPath,
    /// Serde Error
    #[error(transparent)]
    Serde(#[from] serde_json::Error),
}

impl From<Error> for cashu::database::Error {
    fn from(e: Error) -> Self {
        Self::Database(Box::new(e))
    }
}
