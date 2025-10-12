//! This crate is the base foundation to build things that can interact with the CDK (Cashu
//! Development Kit) and their internal crates.
//!
//! This is meant to contain the shared types, traits and common functions that are used across the
//! internal crates.

#![doc = include_str!("../README.md")]
#![warn(missing_docs)]
#![warn(rustdoc::bare_urls)]

pub mod common;
pub mod database;
pub mod error;
#[cfg(feature = "mint")]
pub mod melt;
#[cfg(feature = "mint")]
pub mod mint;
#[cfg(feature = "mint")]
pub mod payment;
pub mod pub_sub;
#[cfg(feature = "mint")]
pub mod state;
pub mod subscription;
#[cfg(feature = "wallet")]
pub mod wallet;
pub mod ws;
// re-exporting external crates
pub use bitcoin;
pub use cashu::amount::{self, Amount};
pub use cashu::lightning_invoice::{self, Bolt11Invoice};
pub use cashu::nuts::{self, *};
#[cfg(feature = "mint")]
pub use cashu::quote_id::{self, *};
pub use cashu::{dhke, ensure_cdk, mint_url, secret, util, SECP256K1};
pub use error::Error;
/// Re-export parking_lot for reuse
pub use parking_lot;
