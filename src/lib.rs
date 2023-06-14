pub mod cashu_wallet;
pub mod client;
pub mod dhke;
pub mod error;
pub mod keyset;
pub mod serde_utils;
pub mod types;
pub mod utils;

pub use bitcoin::Amount;
pub use lightning_invoice::Invoice;

pub type Result<T, E = Box<dyn std::error::Error>> = std::result::Result<T, E>;
