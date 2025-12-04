//! SQLite Storage backend for CDK

#![doc = include_str!("../README.md")]
#![warn(missing_docs)]
#![warn(rustdoc::bare_urls)]
#![deny(clippy::unwrap_used)]

pub mod error;
mod migrations;

#[cfg(feature = "wallet")]
pub mod wallet;

#[cfg(feature = "wallet")]
pub use wallet::WalletRedbDatabase;
