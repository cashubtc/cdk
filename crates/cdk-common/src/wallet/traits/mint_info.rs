//! WalletMintInfo - Mint information and keyset operations trait

use super::WalletTypes;

/// Trait for mint information and keyset operations
///
/// Provides methods to query and manage mint metadata including:
/// - Fetching fresh mint information from the server
/// - Loading cached mint information
/// - Managing keysets
#[cfg_attr(target_arch = "wasm32", async_trait::async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait::async_trait)]
pub trait WalletMintInfo: WalletTypes {
    /// Fetch mint information from the mint server
    ///
    /// This always makes a network request to get fresh mint info.
    /// Returns `None` if the mint does not provide info.
    async fn fetch_mint_info(&self) -> Result<Option<Self::MintInfo>, Self::Error>;

    /// Load mint information from cache or fetch if needed
    ///
    /// This may use cached data if available and fresh, otherwise
    /// fetches from the mint server.
    async fn load_mint_info(&self) -> Result<Self::MintInfo, Self::Error>;

    /// Get the active keyset for the wallet's unit
    ///
    /// Returns the currently active keyset with the lowest fees.
    async fn get_active_keyset(&self) -> Result<Self::KeySetInfo, Self::Error>;

    /// Refresh keysets from the mint
    ///
    /// Forces a fresh fetch of keyset information from the mint server.
    async fn refresh_keysets(&self) -> Result<Vec<Self::KeySetInfo>, Self::Error>;
}
