//! Rexie Indexdb database

#![warn(missing_docs)]
#![warn(rustdoc::bare_urls)]

#[cfg(all(feature = "wallet", target_arch = "wasm32"))]
pub mod wallet;

#[cfg(all(feature = "wallet", target_arch = "wasm32"))]
pub use wallet::WalletRexieDatabase;
