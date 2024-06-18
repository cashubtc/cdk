pub mod error;
mod migrations;

#[cfg(feature = "mint")]
pub mod mint;
#[cfg(feature = "wallet")]
pub mod wallet;

#[cfg(feature = "mint")]
pub use mint::MintRedbDatabase;
#[cfg(feature = "wallet")]
pub use wallet::RedbWalletDatabase;
