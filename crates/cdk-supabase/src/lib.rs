//! Supabase database backend for CDK
//!
//! This crate provides Supabase-based database implementations for the CDK wallet.

mod error;
#[cfg(feature = "wallet")]
/// Wallet database implementation for Supabase
pub mod wallet;

pub use error::Error;
#[cfg(feature = "wallet")]
pub use wallet::SupabaseWalletDatabase;
