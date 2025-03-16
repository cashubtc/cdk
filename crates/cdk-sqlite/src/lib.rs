//! SQLite storage backend for cdk

#![warn(missing_docs)]
#![warn(rustdoc::bare_urls)]

mod common;

#[cfg(feature = "mint")]
pub mod mint;
#[cfg(feature = "wallet")]
pub mod wallet;

#[cfg(feature = "mint")]
pub use mint::MintSqliteDatabase;
#[cfg(feature = "wallet")]
pub use wallet::WalletSqliteDatabase;
