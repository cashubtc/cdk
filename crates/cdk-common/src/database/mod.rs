//! CDK Database

mod kvstore;

#[cfg(feature = "mint")]
pub mod mint;
#[cfg(feature = "wallet")]
pub mod wallet;

use std::ops::{Deref, DerefMut};

// Re-export shared KVStore types at the top level for both mint and wallet
pub use kvstore::{
    validate_kvstore_params, validate_kvstore_string, KVStore, KVStoreDatabase, KVStoreTransaction,
    KVSTORE_NAMESPACE_KEY_ALPHABET, KVSTORE_NAMESPACE_KEY_MAX_LEN,
};

/// Arc-wrapped KV store for shared ownership
pub type DynKVStore = std::sync::Arc<dyn KVStore<Err = Error> + Send + Sync>;

#[cfg(feature = "mint")]
pub use mint::{
    Database as MintDatabase, DynMintDatabase, KeysDatabase as MintKeysDatabase,
    KeysDatabaseTransaction as MintKeyDatabaseTransaction, ProofsDatabase as MintProofsDatabase,
    ProofsTransaction as MintProofsTransaction, QuotesDatabase as MintQuotesDatabase,
    QuotesTransaction as MintQuotesTransaction, SignaturesDatabase as MintSignaturesDatabase,
    SignaturesTransaction as MintSignatureTransaction, Transaction as MintTransaction,
};
#[cfg(all(feature = "mint", feature = "auth"))]
pub use mint::{DynMintAuthDatabase, MintAuthDatabase, MintAuthTransaction};
#[cfg(feature = "wallet")]
pub use wallet::Database as WalletDatabase;

/// A wrapper indicating that a resource has been acquired with a database lock.
///
/// This type is returned by database operations that lock rows for update
/// (e.g., `SELECT ... FOR UPDATE`). It serves as a compile-time marker that
/// the wrapped resource was properly locked before being returned, ensuring
/// that subsequent modifications are safe from race conditions.
///
/// # Usage
///
/// When you need to modify a database record, first acquire it using a locking
/// query method. The returned `Acquired<T>` guarantees the row is locked for
/// the duration of the transaction.
///
/// ```ignore
/// // Acquire a quote with a row lock
/// let mut quote: Acquired<MintQuote> = tx.get_mint_quote_for_update(&quote_id).await?;
///
/// // Safely modify the quote (row is locked)
/// quote.state = QuoteState::Paid;
///
/// // Persist the changes
/// tx.update_mint_quote(&mut quote).await?;
/// ```
///
/// # Deref Behavior
///
/// `Acquired<T>` implements `Deref` and `DerefMut`, allowing transparent access
/// to the inner value's methods and fields.
#[derive(Debug)]
pub struct Acquired<T> {
    inner: T,
}

impl<T> From<T> for Acquired<T> {
    /// Wraps a value to indicate it has been acquired with a lock.
    ///
    /// This is typically called by database layer implementations after
    /// executing a locking query.
    fn from(value: T) -> Self {
        Acquired { inner: value }
    }
}

impl<T> Acquired<T> {
    /// Consumes the wrapper and returns the inner resource.
    ///
    /// Use this when you need to take ownership of the inner value,
    /// for example when passing it to a function that doesn't accept
    /// `Acquired<T>`.
    pub fn inner(self) -> T {
        self.inner
    }
}

impl<T> Deref for Acquired<T> {
    type Target = T;

    /// Returns a reference to the inner resource.
    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

impl<T> DerefMut for Acquired<T> {
    /// Returns a mutable reference to the inner resource.
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.inner
    }
}

/// Type alias for dynamic Wallet Database
#[cfg(feature = "wallet")]
pub type DynWalletDatabase = std::sync::Arc<dyn WalletDatabase<Error> + Send + Sync>;

// Wallet-specific KVStore type aliases
/// Wallet Key-Value Store trait object
#[cfg(feature = "wallet")]
pub type WalletKVStore = dyn KVStore<Err = Error> + Send + Sync;
/// Arc-wrapped wallet KV store for shared ownership
#[cfg(feature = "wallet")]
pub type DynWalletKVStore = std::sync::Arc<WalletKVStore>;
/// Wallet Key-Value Store Database trait object
#[cfg(feature = "wallet")]
pub type WalletKVStoreDatabase = dyn KVStoreDatabase<Err = Error> + Send + Sync;
/// Wallet Key-Value Store Transaction trait object
#[cfg(feature = "wallet")]
pub type WalletKVStoreTransaction = dyn KVStoreTransaction<Error> + Send + Sync;

/// Data conversion error
#[derive(thiserror::Error, Debug)]
pub enum ConversionError {
    /// Missing columns
    #[error("Not enough elements: expected {0}, got {1}")]
    MissingColumn(usize, usize),

    /// Missing parameter
    #[error("Missing parameter {0}")]
    MissingParameter(String),

    /// Invalid db type
    #[error("Invalid type from db, expected {0} got {1}")]
    InvalidType(String, String),

    /// Invalid data conversion in column
    #[error("Error converting {1}, expecting type {0}")]
    InvalidConversion(String, String),

    /// Mint Url Error
    #[error(transparent)]
    MintUrl(#[from] crate::mint_url::Error),

    /// NUT00 Error
    #[error(transparent)]
    CDKNUT00(#[from] crate::nuts::nut00::Error),

    /// NUT01 Error
    #[error(transparent)]
    CDKNUT01(#[from] crate::nuts::nut01::Error),

    /// NUT02 Error
    #[error(transparent)]
    CDKNUT02(#[from] crate::nuts::nut02::Error),

    /// NUT04 Error
    #[error(transparent)]
    CDKNUT04(#[from] crate::nuts::nut04::Error),

    /// NUT05 Error
    #[error(transparent)]
    CDKNUT05(#[from] crate::nuts::nut05::Error),

    /// NUT07 Error
    #[error(transparent)]
    CDKNUT07(#[from] crate::nuts::nut07::Error),

    /// NUT23 Error
    #[error(transparent)]
    CDKNUT23(#[from] crate::nuts::nut23::Error),

    /// Secret Error
    #[error(transparent)]
    CDKSECRET(#[from] crate::secret::Error),

    /// Serde Error
    #[error(transparent)]
    Serde(#[from] serde_json::Error),

    /// BIP32 Error
    #[error(transparent)]
    BIP32(#[from] bitcoin::bip32::Error),

    /// Generic error
    #[error(transparent)]
    Generic(#[from] Box<crate::Error>),
}

impl From<crate::Error> for ConversionError {
    fn from(err: crate::Error) -> Self {
        ConversionError::Generic(Box::new(err))
    }
}

/// CDK_database error
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// Database Error
    #[error(transparent)]
    Database(Box<dyn std::error::Error + Send + Sync>),

    /// Duplicate entry
    #[error("Duplicate entry")]
    Duplicate,

    /// Locked resource
    #[error("Locked resource")]
    Locked,

    /// Amount overflow
    #[error("Amount overflow")]
    AmountOverflow,
    /// Amount zero
    #[error("Amount zero")]
    AmountZero,

    /// DHKE error
    #[error(transparent)]
    DHKE(#[from] crate::dhke::Error),
    /// NUT00 Error
    #[error(transparent)]
    NUT00(#[from] crate::nuts::nut00::Error),
    /// NUT01 Error
    #[error(transparent)]
    NUT01(#[from] crate::nuts::nut01::Error),
    /// NUT02 Error
    #[error(transparent)]
    NUT02(#[from] crate::nuts::nut02::Error),
    /// NUT22 Error
    #[error(transparent)]
    #[cfg(feature = "auth")]
    NUT22(#[from] crate::nuts::nut22::Error),
    /// NUT04 Error
    #[error(transparent)]
    NUT04(#[from] crate::nuts::nut04::Error),
    /// Quote ID Error
    #[error(transparent)]
    #[cfg(feature = "mint")]
    QuoteId(#[from] crate::quote_id::QuoteIdError),
    /// Serde Error
    #[error(transparent)]
    Serde(#[from] serde_json::Error),
    /// Unknown Quote
    #[error("Unknown Quote")]
    UnknownQuote,
    /// Attempt to remove spent proof
    #[error("Attempt to remove spent proof")]
    AttemptRemoveSpentProof,
    /// Attempt to update state of spent proof
    #[error("Attempt to update state of spent proof")]
    AttemptUpdateSpentProof,
    /// Proof not found
    #[error("Proof not found")]
    ProofNotFound,
    /// Invalid keyset
    #[error("Unknown or invalid keyset")]
    InvalidKeysetId,
    #[cfg(feature = "mint")]
    /// Invalid state transition
    #[error("Invalid state transition")]
    InvalidStateTransition(crate::state::Error),

    /// Invalid connection settings
    #[error("Invalid credentials {0}")]
    InvalidConnectionSettings(String),

    /// Unexpected database response
    #[error("Invalid database response")]
    InvalidDbResponse,

    /// Internal error
    #[error("Internal {0}")]
    Internal(String),

    /// Data conversion error
    #[error(transparent)]
    Conversion(#[from] ConversionError),

    /// Missing Placeholder value
    #[error("Missing placeholder value {0}")]
    MissingPlaceholder(String),

    /// Unknown quote ttl
    #[error("Unknown quote ttl")]
    UnknownQuoteTTL,

    /// Invalid UUID
    #[error("Invalid UUID: {0}")]
    InvalidUuid(String),

    /// QuoteNotFound
    #[error("Quote not found")]
    QuoteNotFound,

    /// KV Store invalid key or namespace
    #[error("Invalid KV store key or namespace: {0}")]
    KVStoreInvalidKey(String),
}

#[cfg(feature = "mint")]
impl From<crate::state::Error> for Error {
    fn from(state: crate::state::Error) -> Self {
        match state {
            crate::state::Error::AlreadySpent => Error::AttemptUpdateSpentProof,
            _ => Error::InvalidStateTransition(state),
        }
    }
}

#[async_trait::async_trait]
/// Commit and Rollback
pub trait DbTransactionFinalizer {
    /// Mint Signature Database Error
    type Err: Into<Error> + From<Error>;

    /// Commits all the changes into the database
    async fn commit(self: Box<Self>) -> Result<(), Self::Err>;

    /// Rollbacks the write transaction
    async fn rollback(self: Box<Self>) -> Result<(), Self::Err>;
}
