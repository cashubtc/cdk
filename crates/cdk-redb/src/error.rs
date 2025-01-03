//! Redb Error

use std::num::ParseIntError;

use thiserror::Error;

/// Redb Database Error
#[derive(Debug, Error)]
pub enum Error {
    /// Redb Error
    #[error(transparent)]
    Redb(#[from] redb::Error),
    /// Redb Database Error
    #[error(transparent)]
    Database(#[from] redb::DatabaseError),
    /// Redb Transaction Error
    #[error(transparent)]
    Transaction(#[from] redb::TransactionError),
    /// Redb Commit Error
    #[error(transparent)]
    Commit(#[from] redb::CommitError),
    /// Redb Table Error
    #[error(transparent)]
    Table(#[from] redb::TableError),
    /// Redb Storage Error
    #[error(transparent)]
    Storage(#[from] redb::StorageError),
    /// Serde Json Error
    #[error(transparent)]
    Serde(#[from] serde_json::Error),
    /// Parse int Error
    #[error(transparent)]
    ParseInt(#[from] ParseIntError),
    /// CDK Database Error
    #[error(transparent)]
    CDKDatabase(#[from] cashu::database::Error),
    /// CDK Mint Url Error
    #[error(transparent)]
    CDKMintUrl(#[from] cashu::mint_url::Error),
    /// CDK Error
    #[error(transparent)]
    CDK(#[from] cashu::error::Error),
    /// NUT00 Error
    #[error(transparent)]
    CDKNUT00(#[from] cashu::nuts::nut00::Error),
    /// NUT02 Error
    #[error(transparent)]
    CDKNUT02(#[from] cashu::nuts::nut02::Error),
    /// DHKE Error
    #[error(transparent)]
    DHKE(#[from] cashu::dhke::Error),
    /// Unknown Mint Info
    #[error("Unknown mint info")]
    UnknownMintInfo,
    /// Unknown Proof Y
    #[error("Unknown proof Y")]
    UnknownY,
    /// Unknown Database Version
    #[error("Unknown database version")]
    UnknownDatabaseVersion,
}

impl From<Error> for cashu::database::Error {
    fn from(e: Error) -> Self {
        Self::Database(Box::new(e))
    }
}
