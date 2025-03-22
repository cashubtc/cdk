//! Cashu shared types and functions.
//!
//! This crate is the base foundation to build things that can interact with the CDK (Cashu
//! Development Kit) and their internal crates.
//!
//! This is meant to contain the shared types, traits and common functions that are used across the
//! internal crates.

pub mod common;
pub mod database;
pub mod error;
#[cfg(feature = "mint")]
pub mod mint;
#[cfg(feature = "mint")]
pub mod payment;
pub mod pub_sub;
pub mod subscription;
pub mod ws;

// re-exporting external crates
pub use bitcoin;
pub use cashu::amount::{self, Amount};
pub use cashu::lightning_invoice::{self, Bolt11Invoice};
pub use cashu::nuts::{self, *};
#[cfg(feature = "wallet")]
pub use cashu::wallet;
pub use cashu::{dhke, ensure_cdk, mint_url, secret, util, SECP256K1};
