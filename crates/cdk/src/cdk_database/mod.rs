//! CDK Database

#[cfg(feature = "mint")]
pub mod mint_memory;
#[cfg(feature = "wallet")]
pub mod wallet_memory;

/// re-export types
pub use cdk_common::database::{
    Error, MintDatabase, Transaction, TransactionDirection, TransactionId, WalletDatabase,
    WalletProofDatabase, WalletTransactionDatabase,
};
#[cfg(feature = "wallet")]
pub use wallet_memory::WalletMemoryDatabase;
