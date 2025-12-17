//! SQLite Storage backend for CDK

#![doc = include_str!("../README.md")]

pub mod error;
mod migrations;

#[cfg(feature = "wallet")]
pub mod wallet;

#[cfg(feature = "wallet")]
pub use wallet::WalletRedbDatabase;
