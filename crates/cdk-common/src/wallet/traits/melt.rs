//! WalletMelt - Melting operations trait

use super::WalletTypes;

/// Trait for melting operations
///
/// Provides methods for creating melt quotes and melting tokens.
/// Melting is the process of converting Cashu tokens back to external
/// payment (e.g., paying a Lightning invoice).
#[cfg_attr(target_arch = "wasm32", async_trait::async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait::async_trait)]
pub trait WalletMelt: WalletTypes {
    /// The result type returned from a melt operation
    type MeltResult: Clone + Send + Sync;

    /// Create a melt quote
    ///
    /// Requests a quote from the mint for melting tokens to pay a request.
    /// The quote includes the amount required and any fees.
    ///
    /// # Arguments
    ///
    /// * `request` - The payment request to pay (e.g., Lightning invoice)
    async fn melt_quote(&self, request: String) -> Result<Self::MeltQuote, Self::Error>;

    /// Melt tokens to pay a quote
    ///
    /// Uses proofs from the wallet to pay the melt quote's payment request.
    ///
    /// # Arguments
    ///
    /// * `quote_id` - The ID of the melt quote to pay
    async fn melt(&self, quote_id: &str) -> Result<Self::MeltResult, Self::Error>;
}
