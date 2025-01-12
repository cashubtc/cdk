//! Rust implementation of the Cashu Protocol

#![warn(missing_docs)]
#![warn(rustdoc::bare_urls)]

#[cfg(any(feature = "wallet", feature = "mint"))]
pub mod cdk_database;

#[cfg(feature = "mint")]
pub mod mint;
#[cfg(feature = "wallet")]
pub mod wallet;

pub mod pub_sub;

/// Re-export amount type
#[doc(hidden)]
pub use cdk_common::{
    amount, common as types, dhke,
    error::{self, Error},
    lightning as cdk_lightning, lightning_invoice, mint_url, nuts, secret, subscription, util, ws,
    Amount, Bolt11Invoice,
};

pub mod fees;

#[doc(hidden)]
pub use bitcoin::secp256k1;
#[cfg(feature = "mint")]
#[doc(hidden)]
pub use mint::Mint;
#[cfg(feature = "wallet")]
#[doc(hidden)]
pub use wallet::{Wallet, WalletSubscription};

#[doc(hidden)]
pub use self::util::SECP256K1;
#[cfg(feature = "wallet")]
#[doc(hidden)]
pub use self::wallet::client::HttpClient;

/// Result
#[doc(hidden)]
pub type Result<T, E = Box<dyn std::error::Error>> = std::result::Result<T, E>;
