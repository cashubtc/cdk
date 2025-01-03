//! CDK Database

#[cfg(feature = "mint")]
pub mod mint_memory;
#[cfg(feature = "wallet")]
pub mod wallet_memory;

/// re-export types
pub use cashu::database::{Error, MintDatabase, WalletDatabase};
#[cfg(feature = "wallet")]
pub use wallet_memory::WalletMemoryDatabase;
