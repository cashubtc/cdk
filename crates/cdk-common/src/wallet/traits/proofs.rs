//! WalletProofs - Proof management trait

use super::WalletTypes;

/// Trait for proof management operations
///
/// Provides methods for checking and managing the state of proofs.
#[cfg_attr(target_arch = "wasm32", async_trait::async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait::async_trait)]
pub trait WalletProofs: WalletTypes {
    /// Check if proofs are spent
    ///
    /// Queries the mint to check the state of the provided proofs.
    /// Returns a boolean for each proof indicating if it has been spent.
    ///
    /// # Arguments
    ///
    /// * `proofs` - The proofs to check
    ///
    /// # Returns
    ///
    /// A vector of booleans, where `true` indicates the proof is spent
    async fn check_proofs_spent(&self, proofs: Self::Proofs) -> Result<Vec<bool>, Self::Error>;

    /// Reclaim unspent proofs
    ///
    /// Checks the provided proofs with the mint and reclaims any that
    /// are still unspent by swapping them for fresh proofs.
    ///
    /// # Arguments
    ///
    /// * `proofs` - The proofs to reclaim
    async fn reclaim_unspent(&self, proofs: Self::Proofs) -> Result<(), Self::Error>;
}
