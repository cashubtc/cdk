extern crate core;

#[cfg(any(feature = "mint", feature = "wallet"))]
pub use bip39::Mnemonic;
pub use bitcoin::hashes::sha256::Hash as Sha256;
pub use bitcoin::secp256k1;
pub use lightning_invoice::{self, Bolt11Invoice};

pub mod amount;
pub mod cdk_database;
pub mod dhke;
pub mod error;
#[cfg(feature = "mint")]
pub mod mint;
pub mod nuts;
pub mod secret;
pub mod types;
pub mod url;
pub mod util;
#[cfg(feature = "wallet")]
pub mod wallet;

pub use self::amount::Amount;
pub use self::url::UncheckedUrl;
pub use self::util::SECP256K1;
#[cfg(feature = "wallet")]
pub use self::wallet::client::HttpClient;

pub type Result<T, E = Box<dyn std::error::Error>> = std::result::Result<T, E>;
