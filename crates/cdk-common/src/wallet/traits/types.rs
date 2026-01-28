//! WalletTypes - Base trait defining all wallet associated types

/// Base trait defining all wallet associated types
///
/// This trait provides the foundation for all wallet implementations by defining
/// the associated types used throughout the wallet trait system. All other wallet
/// traits require `WalletTypes` as a supertrait.
///
/// # Associated Types
///
/// - `Amount`: The amount type used for token values
/// - `Proofs`: Collection of cryptographic proofs
/// - `Proof`: A single cryptographic proof
/// - `MintQuote`: Quote information for minting operations
/// - `MeltQuote`: Quote information for melting operations
/// - `Token`: Cashu token representation
/// - `CurrencyUnit`: Currency unit (e.g., sat, msat)
/// - `MintUrl`: URL of the mint
/// - `MintInfo`: Information about the mint
/// - `KeySetInfo`: Keyset metadata
/// - `Error`: Error type for wallet operations
pub trait WalletTypes: Send + Sync {
    /// Amount type for token values
    type Amount: Clone + Send + Sync;
    /// Collection of proofs type
    type Proofs: Clone + Send + Sync;
    /// Single proof type
    type Proof: Clone + Send + Sync;
    /// Mint quote type
    type MintQuote: Clone + Send + Sync;
    /// Melt quote type
    type MeltQuote: Clone + Send + Sync;
    /// Token type
    type Token: Clone + Send + Sync;
    /// Currency unit type
    type CurrencyUnit: Clone + Send + Sync;
    /// Mint URL type
    type MintUrl: Clone + Send + Sync;
    /// Mint info type
    type MintInfo: Clone + Send + Sync;
    /// Keyset info type
    type KeySetInfo: Clone + Send + Sync;
    /// Error type for wallet operations
    type Error: std::error::Error + Send + Sync + 'static;

    /// Get the mint URL
    fn mint_url(&self) -> Self::MintUrl;

    /// Get the currency unit
    fn unit(&self) -> Self::CurrencyUnit;
}
