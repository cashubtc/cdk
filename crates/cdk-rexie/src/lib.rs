#[cfg(all(feature = "wallet", target_arch = "wasm32"))]
pub mod wallet;

#[cfg(all(feature = "wallet", target_arch = "wasm32"))]
pub use wallet::RexieWalletDatabase;
