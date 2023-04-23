use bitcoin::Amount;

use crate::{
    cashu_mint::CashuMint,
    error::Error,
    types::{MintKeys, Proof, ProofsStatus, RequestMintResponse},
};

pub struct CashuWallet {
    pub mint: CashuMint,
    pub keys: MintKeys,
}

impl CashuWallet {
    pub fn new(mint: CashuMint, keys: MintKeys) -> Self {
        Self { mint, keys }
    }

    /// Check if a proof is spent
    pub async fn check_proofs_spent(&self, proofs: Vec<Proof>) -> Result<ProofsStatus, Error> {
        let spendable = self.mint.check_spendable(&proofs).await?;

        let (spendable, spent): (Vec<_>, Vec<_>) = proofs
            .iter()
            .zip(spendable.spendable.iter())
            .partition(|(_, &b)| b);

        Ok(ProofsStatus {
            spendable: spendable.into_iter().map(|(s, _)| s).cloned().collect(),
            spent: spent.into_iter().map(|(s, _)| s).cloned().collect(),
        })
    }

    /// Request Mint
    pub async fn request_mint(&self, amount: Amount) -> Result<RequestMintResponse, Error> {
        self.mint.request_mint(amount).await
    }

    /// Check fee
    pub async fn check_fee(&self, invoice: lightning_invoice::Invoice) -> Result<Amount, Error> {
        Ok(self.mint.check_fees(invoice).await?.fee)
    }
}
