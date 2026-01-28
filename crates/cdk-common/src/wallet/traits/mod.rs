//! Wallet Traits
//!
//! This module defines a composable trait system for wallet implementations.
//! Each trait focuses on a specific capability, allowing flexible and
//! modular wallet implementations.
//!
//! # Trait Hierarchy
//!
//! - [`WalletTypes`] - Base trait defining associated types (required by all others)
//! - [`WalletBalance`] - Balance query operations
//! - [`WalletMintInfo`] - Mint information and keyset operations
//! - [`WalletMint`] - Minting operations (convert payment to tokens)
//! - [`WalletMelt`] - Melting operations (convert tokens to payment)
//! - [`WalletReceive`] - Receiving tokens
//! - [`WalletProofs`] - Proof management
//!
//! # Super-Trait
//!
//! The [`Wallet`] trait combines all capabilities into a single trait,
//! with a blanket implementation for any type that implements all component traits.
//!
//! # Example
//!
//! ```ignore
//! // Function that only needs balance and receive capabilities
//! async fn check_and_receive<W: WalletBalance + WalletReceive>(
//!     wallet: &W,
//!     token: &str,
//! ) -> Result<W::Amount, W::Error> {
//!     let before = wallet.total_balance().await?;
//!     let received = wallet.receive(token).await?;
//!     Ok(received)
//! }
//! ```

mod balance;
mod melt;
mod mint;
mod mint_info;
mod proofs;
mod receive;
mod types;

pub use balance::WalletBalance;
pub use melt::WalletMelt;
pub use mint::WalletMint;
pub use mint_info::WalletMintInfo;
pub use proofs::WalletProofs;
pub use receive::WalletReceive;
pub use types::WalletTypes;

/// Complete wallet trait - composition of all wallet capabilities
///
/// This trait combines all wallet capability traits into a single super-trait.
/// Any type that implements all the component traits automatically implements
/// `Wallet` through a blanket implementation.
///
/// Use this trait when you need the full wallet functionality. For more
/// targeted use cases, prefer using individual traits or combinations of them.
///
/// # Example
///
/// ```ignore
/// use cdk_common::wallet::traits::Wallet;
///
/// async fn full_wallet_operation<W: Wallet>(wallet: &W) -> Result<(), W::Error> {
///     let info = wallet.load_mint_info().await?;
///     let balance = wallet.total_balance().await?;
///     // ... use full wallet functionality
///     Ok(())
/// }
/// ```
pub trait Wallet:
    WalletTypes
    + WalletBalance
    + WalletMintInfo
    + WalletMint
    + WalletMelt
    + WalletReceive
    + WalletProofs
{
}

/// Blanket implementation for Wallet
///
/// Any type that implements all the component traits automatically
/// implements the Wallet super-trait.
impl<T> Wallet for T where
    T: WalletTypes
        + WalletBalance
        + WalletMintInfo
        + WalletMint
        + WalletMelt
        + WalletReceive
        + WalletProofs
{
}
