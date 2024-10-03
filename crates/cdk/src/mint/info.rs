use tracing::instrument;

use crate::mint_url::MintUrl;

use super::{Mint, MintInfo};

impl Mint {
    /// Set Mint Url
    #[instrument(skip_all)]
    pub fn set_mint_url(&mut self, mint_url: MintUrl) {
        self.mint_url = mint_url;
    }

    /// Get Mint Url
    #[instrument(skip_all)]
    pub fn get_mint_url(&self) -> &MintUrl {
        &self.mint_url
    }

    /// Set Mint Info
    #[instrument(skip_all)]
    pub fn set_mint_info(&mut self, mint_info: MintInfo) {
        self.mint_info = mint_info;
    }

    /// Get Mint Info
    #[instrument(skip_all)]
    pub fn mint_info(&self) -> &MintInfo {
        &self.mint_info
    }
}
