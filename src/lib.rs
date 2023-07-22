pub mod amount;
#[cfg(feature = "wallet")]
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

#[cfg(all(feature = "wallet", not(target_arch = "wasm32")))]
pub mod client;
#[cfg(all(feature = "wallet", target_arch = "wasm32"))]
pub mod wasm_client;

#[cfg(all(feature = "wallet", target_arch = "wasm32"))]
pub use wasm_client::Client;

#[cfg(all(feature = "wallet", not(target_arch = "wasm32")))]
pub use client::Client;

pub use amount::Amount;
pub use bitcoin::hashes::sha256::Hash as Sha256;
pub use lightning_invoice;
pub use lightning_invoice::Invoice;

pub type Result<T, E = Box<dyn std::error::Error>> = std::result::Result<T, E>;
