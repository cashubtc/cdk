//! CDK shared types and functions.
//!
//! This crate is the base foundation to build things that can interact with the CDK (Cashu
//! Development Kit) and their internal crates.
//!
//! This is meant to contain the shared types, traits and common functions that are used across the
//! internal crates.

pub mod amount;
pub mod dhke;
pub mod mint;
pub mod mint_url;
pub mod nuts;
pub mod pub_sub;
pub mod secret;
pub mod signatory;
pub mod util;

// re-exporting external crates
pub use bitcoin;
pub use lightning_invoice::{self, Bolt11Invoice};

pub use self::amount::Amount;
pub use self::nuts::*;
pub use self::util::SECP256K1;
