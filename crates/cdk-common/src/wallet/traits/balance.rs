//! WalletBalance - Balance operations trait

use super::WalletTypes;

/// Trait for wallet balance operations
///
/// Provides methods to query the wallet's balance in different states:
/// - Total unspent balance available for spending
/// - Pending balance (awaiting confirmation)
/// - Reserved balance (locked for specific operations)
#[cfg_attr(target_arch = "wasm32", async_trait::async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait::async_trait)]
pub trait WalletBalance: WalletTypes {
    /// Get the total unspent balance
    ///
    /// Returns the sum of all unspent proofs available for spending.
    async fn total_balance(&self) -> Result<Self::Amount, Self::Error>;

    /// Get the total pending balance
    ///
    /// Returns the sum of all proofs in pending state (awaiting confirmation).
    async fn total_pending_balance(&self) -> Result<Self::Amount, Self::Error>;

    /// Get the total reserved balance
    ///
    /// Returns the sum of all proofs reserved for specific operations.
    async fn total_reserved_balance(&self) -> Result<Self::Amount, Self::Error>;
}
