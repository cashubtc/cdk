use thiserror::Error;

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
    /// Secret Error
    #[error(transparent)]
    CDKSECRET(#[from] cdk::secret::Error),
    /// BIP32 Error
    #[error(transparent)]
    BIP32(#[from] bitcoin::bip32::Error),
    /// Could Not Initialize Db
    #[error("Could not initialize Db")]
    CouldNotInitialize,
}

impl From<Error> for cdk::cdk_database::Error {
    fn from(e: Error) -> Self {
        Self::Database(Box::new(e))
    }
}
