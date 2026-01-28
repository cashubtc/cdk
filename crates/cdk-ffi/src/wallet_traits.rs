//! Wallet trait implementations for FFI Wallet
//!
//! This module implements the wallet traits from `cdk_common::wallet::traits`
//! for the FFI Wallet struct.

use std::str::FromStr;

use cdk_common::wallet::traits::{
    WalletBalance, WalletMelt, WalletMint, WalletMintInfo, WalletProofs, WalletReceive, WalletTypes,
};

use crate::error::FfiError;
use crate::token::Token;
use crate::types::{
    Amount, CurrencyUnit, KeySetInfo, MeltQuote, Melted, MintInfo, MintQuote, MintUrl, Proof,
    Proofs,
};
use crate::wallet::Wallet;

impl WalletTypes for Wallet {
    type Amount = Amount;
    type Proofs = Proofs;
    type Proof = Proof;
    type MintQuote = MintQuote;
    type MeltQuote = MeltQuote;
    type Token = Token;
    type CurrencyUnit = CurrencyUnit;
    type MintUrl = MintUrl;
    type MintInfo = MintInfo;
    type KeySetInfo = KeySetInfo;
    type Error = FfiError;

    fn mint_url(&self) -> Self::MintUrl {
        self.mint_url()
    }

    fn unit(&self) -> Self::CurrencyUnit {
        self.unit()
    }
}

#[async_trait::async_trait]
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

#[async_trait::async_trait]
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

#[async_trait::async_trait]
impl WalletMint for Wallet {
    async fn mint_quote(
        &self,
        amount: Self::Amount,
        description: Option<String>,
    ) -> Result<Self::MintQuote, Self::Error> {
        self.mint_quote(amount, description).await
    }

    async fn mint(&self, quote_id: &str) -> Result<Self::Proofs, Self::Error> {
        self.mint(quote_id.to_string(), crate::types::SplitTarget::None, None)
            .await
    }
}

#[async_trait::async_trait]
impl WalletMelt for Wallet {
    type MeltResult = Melted;

    async fn melt_quote(&self, request: String) -> Result<Self::MeltQuote, Self::Error> {
        self.melt_quote(request, None).await
    }

    async fn melt(&self, quote_id: &str) -> Result<Self::MeltResult, Self::Error> {
        self.melt(quote_id.to_string()).await
    }
}

#[async_trait::async_trait]
impl WalletReceive for Wallet {
    async fn receive(&self, encoded_token: &str) -> Result<Self::Amount, Self::Error> {
        // Parse the token string into a Token
        let token = std::sync::Arc::new(Token::from_str(encoded_token)?);
        self.receive(token, crate::types::ReceiveOptions::default())
            .await
    }
}

#[async_trait::async_trait]
impl WalletProofs for Wallet {
    async fn check_proofs_spent(&self, proofs: Self::Proofs) -> Result<Vec<bool>, Self::Error> {
        self.check_proofs_spent(proofs).await
    }

    async fn reclaim_unspent(&self, proofs: Self::Proofs) -> Result<(), Self::Error> {
        self.reclaim_unspent(proofs).await
    }
}
