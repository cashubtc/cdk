//! CDK Database

use std::collections::HashMap;

use async_trait::async_trait;
use thiserror::Error;

use crate::nuts::{
    BlindSignature, CurrencyUnit, Id, KeySetInfo, Keys, MintInfo, MintKeySet, Proof, Proofs,
    PublicKey,
};
use crate::secret::Secret;
use crate::types::{MeltQuote, MintQuote};
use crate::url::UncheckedUrl;

#[cfg(feature = "mint")]
pub mod mint_memory;
#[cfg(feature = "wallet")]
pub mod wallet_memory;

#[derive(Debug, Error)]
pub enum Error {
    #[error(transparent)]
    Database(Box<dyn std::error::Error + Send + Sync>),
    #[error(transparent)]
    Cdk(#[from] crate::error::Error),
}

#[async_trait]
pub trait WalletDatabase {
    type Err: Into<Error> + From<Error>;

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

#[async_trait]
pub trait MintDatabase {
    type Err: Into<Error>;

    async fn set_mint_info(&self, mint_info: &MintInfo) -> Result<(), Self::Err>;
    async fn get_mint_info(&self) -> Result<MintInfo, Self::Err>;

    async fn add_active_keyset(&self, unit: CurrencyUnit, id: Id) -> Result<(), Self::Err>;
    async fn get_active_keyset_id(&self, unit: &CurrencyUnit) -> Result<Option<Id>, Self::Err>;
    async fn get_active_keysets(&self) -> Result<HashMap<CurrencyUnit, Id>, Self::Err>;

    async fn add_mint_quote(&self, quote: MintQuote) -> Result<(), Self::Err>;
    async fn get_mint_quote(&self, quote_id: &str) -> Result<Option<MintQuote>, Self::Err>;
    async fn get_mint_quotes(&self) -> Result<Vec<MintQuote>, Self::Err>;
    async fn remove_mint_quote(&self, quote_id: &str) -> Result<(), Self::Err>;

    async fn add_melt_quote(&self, quote: MeltQuote) -> Result<(), Self::Err>;
    async fn get_melt_quote(&self, quote_id: &str) -> Result<Option<MeltQuote>, Self::Err>;
    async fn get_melt_quotes(&self) -> Result<Vec<MeltQuote>, Self::Err>;
    async fn remove_melt_quote(&self, quote_id: &str) -> Result<(), Self::Err>;

    async fn add_keyset(&self, keyset: MintKeySet) -> Result<(), Self::Err>;
    async fn get_keyset(&self, id: &Id) -> Result<Option<MintKeySet>, Self::Err>;
    async fn get_keysets(&self) -> Result<Vec<MintKeySet>, Self::Err>;

    async fn add_spent_proof(&self, proof: Proof) -> Result<(), Self::Err>;
    async fn get_spent_proof_by_secret(&self, secret: &Secret) -> Result<Option<Proof>, Self::Err>;
    async fn get_spent_proof_by_y(&self, y: &PublicKey) -> Result<Option<Proof>, Self::Err>;

    async fn add_pending_proof(&self, proof: Proof) -> Result<(), Self::Err>;
    async fn get_pending_proof_by_secret(
        &self,
        secret: &Secret,
    ) -> Result<Option<Proof>, Self::Err>;
    async fn get_pending_proof_by_y(&self, y: &PublicKey) -> Result<Option<Proof>, Self::Err>;
    async fn remove_pending_proof(&self, secret: &Secret) -> Result<(), Self::Err>;

    async fn add_blinded_signature(
        &self,
        blinded_message: PublicKey,
        blinded_signature: BlindSignature,
    ) -> Result<(), Self::Err>;
    async fn get_blinded_signature(
        &self,
        blinded_message: &PublicKey,
    ) -> Result<Option<BlindSignature>, Self::Err>;
    async fn get_blinded_signatures(
        &self,
        blinded_messages: Vec<PublicKey>,
    ) -> Result<Vec<Option<BlindSignature>>, Self::Err>;
}
