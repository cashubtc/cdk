//! WalletMint - Minting operations trait

use super::WalletTypes;

/// Trait for minting operations
///
/// Provides methods for creating mint quotes and minting tokens.
/// Minting is the process of converting external payment (e.g., Lightning)
/// into Cashu tokens.
#[cfg_attr(target_arch = "wasm32", async_trait::async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait::async_trait)]
pub trait WalletMint: WalletTypes {
    /// Create a mint quote
    ///
    /// Requests a quote from the mint for minting a specific amount.
    /// The quote includes a payment request (e.g., Lightning invoice)
    /// that must be paid before tokens can be minted.
    ///
    /// # Arguments
    ///
    /// * `amount` - The amount to mint
    /// * `description` - Optional description for the quote
    async fn mint_quote(
        &self,
        amount: Self::Amount,
        description: Option<String>,
    ) -> Result<Self::MintQuote, Self::Error>;

    /// Mint tokens for a paid quote
    ///
    /// After the payment request from a mint quote has been paid,
    /// this method exchanges the quote for Cashu proofs.
    ///
    /// # Arguments
    ///
    /// * `quote_id` - The ID of the paid quote
    async fn mint(&self, quote_id: &str) -> Result<Self::Proofs, Self::Error>;
}
