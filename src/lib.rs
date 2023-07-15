pub mod amount;
#[cfg(feature = "wallet")]
pub mod client;
pub mod dhke;
pub mod error;
#[cfg(feature = "mint")]
pub mod mint;
pub mod nuts;
pub mod serde_utils;
pub mod types;
pub mod utils;
#[cfg(feature = "wallet")]
pub mod wallet;

pub use amount::Amount;
pub use bitcoin::hashes::sha256::Hash as Sha256;
pub use lightning_invoice;
pub use lightning_invoice::Invoice;

pub type Result<T, E = Box<dyn std::error::Error>> = std::result::Result<T, E>;
