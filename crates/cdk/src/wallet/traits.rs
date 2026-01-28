//! Wallet trait implementations
//!
//! This module implements the wallet traits from `cdk_common::wallet::traits`
//! for the CDK Wallet struct.

use cdk_common::wallet::traits::{
    WalletBalance, WalletMelt, WalletMint, WalletMintInfo, WalletProofs, WalletReceive, WalletTypes,
};

use crate::amount::SplitTarget;
use crate::mint_url::MintUrl;
use crate::nuts::{CurrencyUnit, KeySetInfo, MintInfo, Proof, Proofs, State};
use crate::types::Melted;
use crate::wallet::{MeltQuote, MintQuote, ReceiveOptions};
use crate::{Amount, Error, Wallet};

impl WalletTypes for Wallet {
    type Amount = Amount;
    type Proofs = Proofs;
    type Proof = Proof;
    type MintQuote = MintQuote;
    type MeltQuote = MeltQuote;
    type Token = crate::nuts::Token;
    type CurrencyUnit = CurrencyUnit;
    type MintUrl = MintUrl;
    type MintInfo = MintInfo;
    type KeySetInfo = KeySetInfo;
    type Error = Error;

    fn mint_url(&self) -> Self::MintUrl {
        self.mint_url.clone()
    }

    fn unit(&self) -> Self::CurrencyUnit {
        self.unit.clone()
    }
}

#[cfg_attr(target_arch = "wasm32", async_trait::async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait::async_trait)]
impl WalletBalance for Wallet {
    async fn total_balance(&self) -> Result<Self::Amount, Self::Error> {
        self.total_balance().await
    }

    async fn total_pending_balance(&self) -> Result<Self::Amount, Self::Error> {
        self.total_pending_balance().await
    }

    async fn total_reserved_balance(&self) -> Result<Self::Amount, Self::Error> {
        self.total_reserved_balance().await
    }
}

#[cfg_attr(target_arch = "wasm32", async_trait::async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait::async_trait)]
impl WalletMintInfo for Wallet {
    async fn fetch_mint_info(&self) -> Result<Option<Self::MintInfo>, Self::Error> {
        self.fetch_mint_info().await
    }

    async fn load_mint_info(&self) -> Result<Self::MintInfo, Self::Error> {
        self.load_mint_info().await
    }

    async fn get_active_keyset(&self) -> Result<Self::KeySetInfo, Self::Error> {
        self.get_active_keyset().await
    }

    async fn refresh_keysets(&self) -> Result<Vec<Self::KeySetInfo>, Self::Error> {
        self.refresh_keysets().await
    }
}

#[cfg_attr(target_arch = "wasm32", async_trait::async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait::async_trait)]
impl WalletMint for Wallet {
    async fn mint_quote(
        &self,
        amount: Self::Amount,
        description: Option<String>,
    ) -> Result<Self::MintQuote, Self::Error> {
        self.mint_quote(amount, description).await
    }

    async fn mint(&self, quote_id: &str) -> Result<Self::Proofs, Self::Error> {
        self.mint(quote_id, SplitTarget::default(), None).await
    }
}

#[cfg_attr(target_arch = "wasm32", async_trait::async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait::async_trait)]
impl WalletMelt for Wallet {
    type MeltResult = Melted;

    async fn melt_quote(&self, request: String) -> Result<Self::MeltQuote, Self::Error> {
        self.melt_quote(request, None).await
    }

    async fn melt(&self, quote_id: &str) -> Result<Self::MeltResult, Self::Error> {
        self.melt(quote_id).await
    }
}

#[cfg_attr(target_arch = "wasm32", async_trait::async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait::async_trait)]
impl WalletReceive for Wallet {
    async fn receive(&self, encoded_token: &str) -> Result<Self::Amount, Self::Error> {
        self.receive(encoded_token, ReceiveOptions::default()).await
    }
}

#[cfg_attr(target_arch = "wasm32", async_trait::async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait::async_trait)]
impl WalletProofs for Wallet {
    async fn check_proofs_spent(&self, proofs: Self::Proofs) -> Result<Vec<bool>, Self::Error> {
        let proof_states = self.check_proofs_spent(proofs).await?;
        Ok(proof_states
            .into_iter()
            .map(|ps| matches!(ps.state, State::Spent | State::PendingSpent))
            .collect())
    }

    async fn reclaim_unspent(&self, proofs: Self::Proofs) -> Result<(), Self::Error> {
        self.reclaim_unspent(proofs).await
    }
}
