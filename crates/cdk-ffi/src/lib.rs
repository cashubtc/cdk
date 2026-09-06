//! CDK FFI Bindings
//!
//! UniFFI bindings for the CDK Wallet and related types.

#![warn(clippy::unused_async)]
#![allow(missing_docs)]
#![allow(missing_debug_implementations)]

pub mod bip321;
pub mod database;
pub mod error;
pub mod logging;
#[cfg(feature = "nostr")]
pub mod nostr;
#[cfg(feature = "npubcash")]
pub mod npubcash;
#[cfg(feature = "nwc")]
pub mod nwc;
#[cfg(feature = "postgres")]
pub mod postgres;
mod runtime;
pub mod sqlite;
#[cfg(feature = "supabase")]
pub mod supabase;
pub mod token;
pub mod types;
pub mod wallet;
#[cfg(feature = "advanced-wallet")]
pub mod wallet_advanced;
pub mod wallet_api;

pub use database::*;
pub use error::*;
pub use logging::*;
#[cfg(feature = "nostr")]
pub use nostr::*;
#[cfg(feature = "npubcash")]
pub use npubcash::*;
#[cfg(feature = "nwc")]
pub use nwc::*;
pub use types::*;
pub use wallet::{
    generate_mnemonic, mnemonic_to_entropy, RateLimit, Wallet, WalletConfig,
    WalletOpenAdvancedOptions,
};
#[cfg(feature = "advanced-wallet")]
pub use wallet_advanced::*;
pub use wallet_api::*;

uniffi::setup_scaffolding!();
