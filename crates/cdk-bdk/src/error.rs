//! CDK BDK onchain backend errors

use thiserror::Error;
use uuid::Uuid;

/// CDK BDK onchain backend error
#[derive(Debug, Error)]
pub enum Error {
    /// Fee estimation failed
    #[error("Fee estimation failed: {0}")]
    FeeEstimationFailed(String),
    /// Fee estimation unavailable
    #[error("Fee estimation unavailable")]
    FeeEstimationUnavailable,
    /// Start called but tasks are already running
    #[error("Start called but background tasks are already running")]
    AlreadyStarted,
    /// Unsupported payment type for onchain backend
    #[error("Unsupported payment type for onchain backend")]
    UnsupportedOnchain,

    /// JSON error
    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),

    /// Amount conversion error
    #[error("Amount conversion error: {0}")]
    AmountConversion(#[from] cdk_common::amount::Error),

    /// Database error
    #[error("Database error: {0}")]
    Database(#[from] bdk_wallet::rusqlite::Error),

    /// Wallet error
    #[error("Wallet error: {0}")]
    Wallet(String),

    /// Bitcoin RPC error
    #[error("Bitcoin RPC error: {0}")]
    BitcoinRpc(#[from] bdk_bitcoind_rpc::bitcoincore_rpc::Error),

    /// Esplora error
    #[error("Esplora error: {0}")]
    Esplora(String),

    /// Bip32 key derivation error
    #[error("Bip32 key derivation error: {0}")]
    Bip32(#[from] bdk_wallet::bitcoin::bip32::Error),

    /// Key derivation error
    #[error("Key derivation error: {0}")]
    KeyDerivation(#[from] bdk_wallet::keys::KeyError),

    /// Could not sign transaction
    #[error("Could not sign transaction")]
    CouldNotSign,

    /// Path error
    #[error("Path error")]
    Path,

    /// IO error
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    /// KV Store error
    #[error("KV Store error: {0}")]
    KvStore(#[from] cdk_common::database::Error),

    /// Could not find matching output vout in transaction
    #[error("Could not find matching output vout in transaction")]
    VoutNotFound,

    /// Send intent not found in storage
    #[error("Send intent not found: {0}")]
    SendIntentNotFound(Uuid),

    /// Send batch not found in storage
    #[error("Send batch not found: {0}")]
    SendBatchNotFound(Uuid),

    /// Send intent with quote id already exists in storage
    #[error("Send intent already exists for quote id: {0}")]
    DuplicateQuoteId(String),

    /// Batch fee exceeds the combined max fee of all included intents
    #[error("Batch fee {actual_fee} exceeds combined max fee {max_fee}")]
    BatchFeeTooHigh {
        /// Actual transaction fee in sats
        actual_fee: u64,
        /// Maximum combined fee from included intents
        max_fee: u64,
    },

    /// No valid fee allocation exists for the batch
    #[error("No valid fee allocation for batch")]
    NoValidFeeAllocation,

    /// Batch record is missing an output assignment for one of its member intents.
    ///
    /// This indicates a persistence invariant violation: every intent ID listed
    /// in a Signed/Broadcast batch must have a corresponding assignment entry.
    #[error("Batch {batch_id} is missing an output assignment for intent {intent_id}")]
    BatchAssignmentMissing {
        /// Batch that is missing the assignment
        batch_id: Uuid,
        /// Intent with no assignment entry
        intent_id: Uuid,
    },

    /// Receive intent not found in storage
    #[error("Receive intent not found: {0}")]
    ReceiveIntentNotFound(Uuid),

    /// Receive address not found in storage
    #[error("Receive address not found: {0}")]
    ReceiveAddressNotFound(String),

    /// Database
    #[error("Database error")]
    BdkPersist,
}

impl From<Error> for cdk_common::payment::Error {
    fn from(e: Error) -> Self {
        Self::Onchain(Box::new(e))
    }
}
