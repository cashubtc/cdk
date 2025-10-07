//! Redb Error

use std::num::ParseIntError;

use thiserror::Error;

/// Redb Database Error
#[derive(Debug, Error)]
pub enum Error {
    /// Redb Error
    #[error(transparent)]
    Redb(#[from] Box<redb::Error>),
    /// Redb Database Error
    #[error(transparent)]
    Database(#[from] Box<redb::DatabaseError>),
    /// Redb Transaction Error
    #[error(transparent)]
    Transaction(#[from] Box<redb::TransactionError>),
    /// Redb Commit Error
    #[error(transparent)]
    Commit(#[from] Box<redb::CommitError>),
    /// Redb Table Error
    #[error(transparent)]
    Table(#[from] Box<redb::TableError>),
    /// Redb Storage Error
    #[error(transparent)]
    Storage(#[from] Box<redb::StorageError>),
    /// Upgrade Transaction Error
    #[error(transparent)]
    Upgrade(#[from] Box<redb::UpgradeError>),
    /// Serde Json Error
    #[error(transparent)]
    Serde(#[from] serde_json::Error),
    /// Parse int Error
    #[error(transparent)]
    ParseInt(#[from] ParseIntError),
    /// CDK Database Error
    #[error(transparent)]
    CDKDatabase(#[from] cdk_common::database::Error),
    /// CDK Mint Url Error
    #[error(transparent)]
    CDKMintUrl(#[from] cdk_common::mint_url::Error),
    /// CDK Error
    #[error(transparent)]
    CDK(#[from] cdk_common::error::Error),
    /// IO Error
    #[error(transparent)]
    Io(#[from] std::io::Error),
    /// NUT00 Error
    #[error(transparent)]
    CDKNUT00(#[from] cdk_common::nuts::nut00::Error),
    /// NUT02 Error
    #[error(transparent)]
    CDKNUT02(#[from] cdk_common::nuts::nut02::Error),
    /// DHKE Error
    #[error(transparent)]
    DHKE(#[from] cdk_common::dhke::Error),
    /// Unknown Mint Info
    #[error("Unknown mint info")]
    UnknownMintInfo,
    /// Unknown quote ttl
    #[error("Unknown quote ttl")]
    UnknownQuoteTTL,
    /// Unknown Proof Y
    #[error("Unknown proof Y")]
    UnknownY,
    /// Unknown Quote
    #[error("Unknown quote")]
    UnknownQuote,
    /// Unknown Database Version
    #[error("Unknown database version")]
    UnknownDatabaseVersion,
    /// Duplicate
    #[error("Duplicate")]
    Duplicate,
}

impl From<Error> for cdk_common::database::Error {
    fn from(e: Error) -> Self {
        Self::Database(Box::new(e))
    }
}

// Implement From for boxed redb errors
impl From<redb::Error> for Error {
    fn from(e: redb::Error) -> Self {
        Self::Redb(Box::new(e))
    }
}

impl From<redb::DatabaseError> for Error {
    fn from(e: redb::DatabaseError) -> Self {
        Self::Database(Box::new(e))
    }
}

impl From<redb::TransactionError> for Error {
    fn from(e: redb::TransactionError) -> Self {
        Self::Transaction(Box::new(e))
    }
}

impl From<redb::CommitError> for Error {
    fn from(e: redb::CommitError) -> Self {
        Self::Commit(Box::new(e))
    }
}

impl From<redb::TableError> for Error {
    fn from(e: redb::TableError) -> Self {
        Self::Table(Box::new(e))
    }
}

impl From<redb::StorageError> for Error {
    fn from(e: redb::StorageError) -> Self {
        Self::Storage(Box::new(e))
    }
}

impl From<redb::UpgradeError> for Error {
    fn from(e: redb::UpgradeError) -> Self {
        Self::Upgrade(Box::new(e))
    }
}
