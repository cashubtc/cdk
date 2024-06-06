//! CDK Database

#[cfg(any(feature = "wallet", feature = "mint"))]
use std::collections::HashMap;

#[cfg(any(feature = "wallet", feature = "mint"))]
use async_trait::async_trait;
use thiserror::Error;

#[cfg(feature = "mint")]
use crate::mint::MintKeySetInfo;
#[cfg(feature = "wallet")]
use crate::nuts::State;
#[cfg(feature = "mint")]
use crate::nuts::{BlindSignature, Proof};
#[cfg(any(feature = "wallet", feature = "mint"))]
use crate::nuts::{CurrencyUnit, Id, PublicKey};
#[cfg(feature = "wallet")]
use crate::nuts::{KeySetInfo, Keys, MintInfo, Proofs, SpendingConditions};
#[cfg(feature = "mint")]
use crate::secret::Secret;
#[cfg(feature = "wallet")]
use crate::types::ProofInfo;
#[cfg(any(feature = "wallet", feature = "mint"))]
use crate::types::{MeltQuote, MintQuote};
#[cfg(feature = "wallet")]
use crate::url::UncheckedUrl;

#[cfg(feature = "mint")]
pub mod mint_memory;
#[cfg(feature = "wallet")]
pub mod wallet_memory;

#[cfg(feature = "wallet")]
pub use wallet_memory::WalletMemoryDatabase;

#[derive(Debug, Error)]
pub enum Error {
    #[error(transparent)]
    Database(Box<dyn std::error::Error + Send + Sync>),
    #[error(transparent)]
    Cdk(#[from] crate::error::Error),
    #[error(transparent)]
    NUT01(#[from] crate::nuts::nut00::Error),
}

#[cfg(feature = "wallet")]
#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
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
    async fn get_keyset_by_id(&self, keyset_id: &Id) -> Result<Option<KeySetInfo>, Self::Err>;

    async fn add_mint_quote(&self, quote: MintQuote) -> Result<(), Self::Err>;
    async fn get_mint_quote(&self, quote_id: &str) -> Result<Option<MintQuote>, Self::Err>;
    async fn get_mint_quotes(&self) -> Result<Vec<MintQuote>, Self::Err>;
    async fn remove_mint_quote(&self, quote_id: &str) -> Result<(), Self::Err>;

    async fn add_melt_quote(&self, quote: MeltQuote) -> Result<(), Self::Err>;
    async fn get_melt_quote(&self, quote_id: &str) -> Result<Option<MeltQuote>, Self::Err>;
    async fn remove_melt_quote(&self, quote_id: &str) -> Result<(), Self::Err>;

    async fn add_keys(&self, keys: Keys) -> Result<(), Self::Err>;
    async fn get_keys(&self, id: &Id) -> Result<Option<Keys>, Self::Err>;
    async fn remove_keys(&self, id: &Id) -> Result<(), Self::Err>;

    async fn add_proofs(&self, proof_info: Vec<ProofInfo>) -> Result<(), Self::Err>;
    async fn get_proofs(
        &self,
        mint_url: Option<UncheckedUrl>,
        unit: Option<CurrencyUnit>,
        state: Option<Vec<State>>,
        spending_conditions: Option<Vec<SpendingConditions>>,
    ) -> Result<Option<Vec<ProofInfo>>, Self::Err>;
    async fn remove_proofs(&self, proofs: &Proofs) -> Result<(), Self::Err>;

    async fn set_proof_state(&self, y: PublicKey, state: State) -> Result<(), Self::Err>;

    async fn increment_keyset_counter(&self, keyset_id: &Id, count: u32) -> Result<(), Self::Err>;
    async fn get_keyset_counter(&self, keyset_id: &Id) -> Result<Option<u32>, Self::Err>;

    #[cfg(feature = "nostr")]
    async fn get_nostr_last_checked(
        &self,
        verifying_key: &PublicKey,
    ) -> Result<Option<u32>, Self::Err>;
    #[cfg(feature = "nostr")]
    async fn add_nostr_last_checked(
        &self,
        verifying_key: PublicKey,
        last_checked: u32,
    ) -> Result<(), Self::Err>;
}

#[cfg(feature = "mint")]
#[async_trait]
pub trait MintDatabase {
    type Err: Into<Error> + From<Error>;

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

    async fn add_keyset_info(&self, keyset: MintKeySetInfo) -> Result<(), Self::Err>;
    async fn get_keyset_info(&self, id: &Id) -> Result<Option<MintKeySetInfo>, Self::Err>;
    async fn get_keyset_infos(&self) -> Result<Vec<MintKeySetInfo>, Self::Err>;

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
