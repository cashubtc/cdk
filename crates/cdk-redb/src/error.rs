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
    CDKDatabase(#[from] cdk::cdk_database::Error),
    /// CDK Error
    #[error(transparent)]
    CDK(#[from] cdk::error::Error),
    /// NUT02 Error
    #[error(transparent)]
    CDKNUT02(#[from] cdk::nuts::nut02::Error),
    /// NUT00 Error
    #[error(transparent)]
    CDKNUT00(#[from] cdk::nuts::nut00::Error),
    /// Unknown Mint Info
    #[error("Unknown Mint Info")]
    UnknownMintInfo,
    /// Unknown Proof Y
    #[error("Unknown Proof Y")]
    UnknownY,
    /// Unknown Database Version
    #[error("Unknown Database Version")]
    UnknownDatabaseVersion,
}

impl From<Error> for cdk::cdk_database::Error {
    fn from(e: Error) -> Self {
        Self::Database(Box::new(e))
    }
}
