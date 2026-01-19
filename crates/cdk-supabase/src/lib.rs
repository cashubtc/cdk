mod error;
#[cfg(feature = "wallet")]
pub mod wallet;

pub use error::Error;

#[cfg(feature = "wallet")]
pub use wallet::SupabaseWalletDatabase;
