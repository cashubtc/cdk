//! WalletReceive - Token receiving trait

use super::WalletTypes;

/// Trait for receiving tokens
///
/// Provides methods for receiving Cashu tokens into the wallet.
/// Receiving involves validating and swapping incoming tokens for
/// fresh proofs controlled by this wallet.
#[cfg_attr(target_arch = "wasm32", async_trait::async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait::async_trait)]
pub trait WalletReceive: WalletTypes {
    /// Receive tokens from an encoded token string
    ///
    /// Parses the encoded token, validates the proofs, and swaps them
    /// for fresh proofs in the wallet.
    ///
    /// # Arguments
    ///
    /// * `encoded_token` - The base64-encoded Cashu token string
    ///
    /// # Returns
    ///
    /// The amount received after any fees
    async fn receive(&self, encoded_token: &str) -> Result<Self::Amount, Self::Error>;
}
