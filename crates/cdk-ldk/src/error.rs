#[derive(thiserror::Error, Debug)]
pub enum Error {
    #[error(transparent)]
    Bip32(#[from] bitcoin::bip32::Error),
    #[error("Block source error")]
    BlockSource(lightning_block_sync::BlockSourceError),
    #[error(transparent)]
    Commit(#[from] redb::CommitError),
    #[error(transparent)]
    Database(#[from] redb::DatabaseError),
    #[error("Decode error")]
    Decode(lightning::ln::msgs::DecodeError),
    #[error("Invalid path")]
    InvalidPath,
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error("{0}")]
    Ldk(String),
    #[error(transparent)]
    ParseInt(#[from] std::num::ParseIntError),
    #[error(transparent)]
    Rpc(#[from] bitcoincore_rpc::Error),
    #[error(transparent)]
    Storage(#[from] redb::StorageError),
    #[error(transparent)]
    SystemTime(#[from] std::time::SystemTimeError),
    #[error(transparent)]
    Table(#[from] redb::TableError),
    #[error(transparent)]
    Transaction(#[from] redb::TransactionError),
}

impl From<lightning_block_sync::BlockSourceError> for Error {
    fn from(e: lightning_block_sync::BlockSourceError) -> Self {
        Self::BlockSource(e)
    }
}

impl From<lightning::ln::msgs::DecodeError> for Error {
    fn from(e: lightning::ln::msgs::DecodeError) -> Self {
        Self::Decode(e)
    }
}
