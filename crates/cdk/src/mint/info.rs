use tracing::instrument;

use super::{Mint, MintInfo};

impl Mint {
    /// Set Mint Info
    #[instrument(skip_all)]
    pub fn set_mint_info(&self, mint_info: MintInfo) {
        self.config.set_mint_info(mint_info);
    }

    /// Get Mint Info
    #[instrument(skip_all)]
    pub fn mint_info(&self) -> MintInfo {
        self.config.mint_info()
    }
}
