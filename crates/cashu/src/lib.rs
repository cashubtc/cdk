extern crate core;

pub use bitcoin::hashes::sha256::Hash as Sha256;
pub use bitcoin::secp256k1;
pub use lightning_invoice::{self, Bolt11Invoice};

pub mod amount;
#[cfg(any(feature = "wallet", feature = "mint"))]
pub mod dhke;
pub mod error;
pub mod nuts;
pub mod secret;
pub mod serde_utils;
pub mod types;
pub mod url;
pub mod util;

pub use self::amount::Amount;
pub use self::util::SECP256K1;

pub type Result<T, E = Box<dyn std::error::Error>> = std::result::Result<T, E>;
