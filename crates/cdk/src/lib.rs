//! Rust implementation of the Cashu Protocol

#![warn(missing_docs)]
#![warn(rustdoc::bare_urls)]

pub mod amount;
#[cfg(any(feature = "wallet", feature = "mint"))]
pub mod cdk_database;
#[cfg(feature = "mint")]
pub mod cdk_lightning;
pub mod dhke;
pub mod error;
#[cfg(feature = "mint")]
pub mod mint;
pub mod mint_url;
pub mod nuts;
pub mod secret;
pub mod types;
pub mod util;
#[cfg(feature = "wallet")]
pub mod wallet;

pub mod pub_sub;

pub mod fees;

#[doc(hidden)]
pub use bitcoin::secp256k1;
#[doc(hidden)]
pub use error::Error;
#[doc(hidden)]
pub use lightning_invoice::{self, Bolt11Invoice};
#[cfg(feature = "mint")]
#[doc(hidden)]
pub use mint::Mint;
#[cfg(feature = "wallet")]
#[doc(hidden)]
pub use wallet::{Wallet, WalletSubscription};

#[doc(hidden)]
pub use self::amount::Amount;
#[doc(hidden)]
pub use self::util::SECP256K1;
#[cfg(feature = "wallet")]
#[doc(hidden)]
pub use self::wallet::client::HttpClient;

/// Result
#[doc(hidden)]
pub type Result<T, E = Box<dyn std::error::Error>> = std::result::Result<T, E>;
