//! # Cashu
//! 
//! A Rust implementation of the [Cashu](https://github.com/cashubtc) protocol, providing the core functionality for Cashu e-cash operations.
//! 
//! ## Overview
//! 
//! Cashu is a privacy-focused, token-based digital cash system built on the Bitcoin Lightning Network. This crate implements the core Cashu protocol as defined in the [Cashu NUTs (Notation, Usage, and Terminology)](https://github.com/cashubtc/nuts/).
//! 
//! ## Features
//! 
//! - **Cryptographic Operations**: Secure blind signatures and verification
//! - **Token Management**: Creation, validation, and manipulation of Cashu tokens
//! - **NUTs Implementation**: Support for the core Cashu protocol specifications
//! - **Type-safe API**: Strongly-typed interfaces for working with Cashu primitives
//! 
//! ## Usage
//! 
//! ```rust,no_run
//! use cashu::amount::Amount;
//! use cashu::nuts::nut00::Token;
//! use std::str::FromStr;
//! 
//! // Parse a Cashu token from a string
//! let token_str = "cashuAeyJ0b2tlbiI6W3sibWludCI6Imh0dHBzOi8vZXhhbXBsZS5jb20iLCJwcm9vZnMiOlt7ImlkIjoiMDAwMDAwMDAwMDAwMDAwMCIsImFtb3VudCI6MX1dfV19";
//! let token = Token::from_str(token_str).expect("Valid token");
//! 
//! // Get the total amount
//! let amount: Amount = token.amount();
//! println!("Token amount: {}", amount);
//! ```

#![warn(missing_docs)]
#![warn(rustdoc::bare_urls)]

pub mod amount;
pub mod dhke;
pub mod mint_url;
pub mod nuts;
pub mod secret;
pub mod util;
#[cfg(feature = "wallet")]
pub mod wallet;

pub use lightning_invoice::{self, Bolt11Invoice};

pub use self::amount::Amount;
pub use self::mint_url::MintUrl;
pub use self::nuts::*;
pub use self::util::SECP256K1;

#[doc(hidden)]
#[macro_export]
macro_rules! ensure_cdk {
    ($cond:expr, $err:expr) => {
        if !$cond {
            return Err($err);
        }
    };
}
