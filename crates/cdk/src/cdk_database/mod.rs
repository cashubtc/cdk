//! CDK Database

use std::collections::HashMap;

use async_trait::async_trait;
use thiserror::Error;

use crate::nuts::{Id, KeySetInfo, Keys, MintInfo, Proofs};
use crate::types::{MeltQuote, MintQuote};
use crate::url::UncheckedUrl;

pub mod wallet_memory;

#[derive(Debug, Error)]
pub enum Error {
    #[error(transparent)]
    Database(Box<dyn std::error::Error + Send + Sync>),
}

#[async_trait]
pub trait WalletDatabase {
    type Err: Into<Error>;

    async fn add_mint(
        &self,
        mint_url: UncheckedUrl,
        mint_info: Option<MintInfo>,
    ) -> Result<(), Self::Err>;
    async fn get_mint(&self, mint_url: UncheckedUrl) -> Result<Option<MintInfo>, Self::Err>;
    async fn get_mints(&self) -> Result<HashMap<UncheckedUrl, Option<MintInfo>>, Self::Err>;

    async fn add_mint_keysets(
        &self,
        mint_url: UncheckedUrl,
        keysets: Vec<KeySetInfo>,
    ) -> Result<(), Self::Err>;
    async fn get_mint_keysets(
        &self,
        mint_url: UncheckedUrl,
    ) -> Result<Option<Vec<KeySetInfo>>, Self::Err>;
    async fn add_mint_quote(&self, quote: MintQuote) -> Result<(), Self::Err>;
    async fn get_mint_quote(&self, quote_id: &str) -> Result<Option<MintQuote>, Self::Err>;
    async fn remove_mint_quote(&self, quote_id: &str) -> Result<(), Self::Err>;

    async fn add_melt_quote(&self, quote: MeltQuote) -> Result<(), Self::Err>;
    async fn get_melt_quote(&self, quote_id: &str) -> Result<Option<MeltQuote>, Self::Err>;
    async fn remove_melt_quote(&self, quote_id: &str) -> Result<(), Self::Err>;

    async fn add_keys(&self, keys: Keys) -> Result<(), Self::Err>;
    async fn get_keys(&self, id: &Id) -> Result<Option<Keys>, Self::Err>;
    async fn remove_keys(&self, id: &Id) -> Result<(), Self::Err>;

    async fn add_proofs(&self, mint_url: UncheckedUrl, proof: Proofs) -> Result<(), Self::Err>;
    async fn get_proofs(&self, mint_url: UncheckedUrl) -> Result<Option<Proofs>, Self::Err>;
    async fn remove_proofs(&self, mint_url: UncheckedUrl, proofs: &Proofs)
        -> Result<(), Self::Err>;

    async fn add_pending_proofs(
        &self,
        mint_url: UncheckedUrl,
        proof: Proofs,
    ) -> Result<(), Self::Err>;
    async fn get_pending_proofs(&self, mint_url: UncheckedUrl)
        -> Result<Option<Proofs>, Self::Err>;
    async fn remove_pending_proofs(
        &self,
        mint_url: UncheckedUrl,
        proofs: &Proofs,
    ) -> Result<(), Self::Err>;

    async fn increment_keyset_counter(&self, keyset_id: &Id, count: u64) -> Result<(), Self::Err>;
    async fn get_keyset_counter(&self, keyset_id: &Id) -> Result<Option<u64>, Self::Err>;
}
