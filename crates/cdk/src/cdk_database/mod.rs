//! CDK Database

#[cfg(any(feature = "wallet", feature = "mint"))]
use std::collections::HashMap;
use std::fmt::Debug;

#[cfg(any(feature = "wallet", feature = "mint"))]
use async_trait::async_trait;
use thiserror::Error;
#[cfg(feature = "mint")]
use uuid::Uuid;

#[cfg(feature = "mint")]
use crate::mint;
#[cfg(feature = "mint")]
use crate::mint::MintKeySetInfo;
#[cfg(feature = "mint")]
use crate::mint::MintQuote as MintMintQuote;
#[cfg(feature = "wallet")]
use crate::mint_url::MintUrl;
#[cfg(feature = "mint")]
use crate::nuts::MeltBolt11Request;
#[cfg(feature = "mint")]
use crate::nuts::{BlindSignature, MeltQuoteState, MintQuoteState, Proof, Proofs};
#[cfg(any(feature = "wallet", feature = "mint"))]
use crate::nuts::{CurrencyUnit, Id, PublicKey, State};
#[cfg(feature = "wallet")]
use crate::nuts::{KeySetInfo, Keys, MintInfo, SpendingConditions};
#[cfg(feature = "mint")]
use crate::types::LnKey;
#[cfg(feature = "wallet")]
use crate::types::ProofInfo;
#[cfg(feature = "wallet")]
use crate::wallet;
#[cfg(feature = "wallet")]
use crate::wallet::MintQuote as WalletMintQuote;

#[cfg(feature = "mint")]
pub mod mint_memory;
#[cfg(feature = "wallet")]
pub mod wallet_memory;

#[cfg(feature = "wallet")]
pub use wallet_memory::WalletMemoryDatabase;

/// CDK_database error
#[derive(Debug, Error)]
pub enum Error {
    /// Database Error
    #[error(transparent)]
    Database(Box<dyn std::error::Error + Send + Sync>),
    /// DHKE error
    #[error(transparent)]
    DHKE(#[from] crate::dhke::Error),
    /// NUT00 Error
    #[error(transparent)]
    NUT00(#[from] crate::nuts::nut00::Error),
    /// NUT02 Error
    #[error(transparent)]
    NUT02(#[from] crate::nuts::nut02::Error),
    /// Serde Error
    #[error(transparent)]
    Serde(#[from] serde_json::Error),
    /// Unknown Quote
    #[error("Unknown Quote")]
    UnknownQuote,
}

/// Wallet Database trait
#[cfg(feature = "wallet")]
#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
pub trait WalletDatabase: Debug {
    /// Wallet Database Error
    type Err: Into<Error> + From<Error>;

    /// Add Mint to storage
    async fn add_mint(
        &self,
        mint_url: MintUrl,
        mint_info: Option<MintInfo>,
    ) -> Result<(), Self::Err>;
    /// Remove Mint from storage
    async fn remove_mint(&self, mint_url: MintUrl) -> Result<(), Self::Err>;
    /// Get mint from storage
    async fn get_mint(&self, mint_url: MintUrl) -> Result<Option<MintInfo>, Self::Err>;
    /// Get all mints from storage
    async fn get_mints(&self) -> Result<HashMap<MintUrl, Option<MintInfo>>, Self::Err>;
    /// Update mint url
    async fn update_mint_url(
        &self,
        old_mint_url: MintUrl,
        new_mint_url: MintUrl,
    ) -> Result<(), Self::Err>;

    /// Add mint keyset to storage
    async fn add_mint_keysets(
        &self,
        mint_url: MintUrl,
        keysets: Vec<KeySetInfo>,
    ) -> Result<(), Self::Err>;
    /// Get mint keysets for mint url
    async fn get_mint_keysets(
        &self,
        mint_url: MintUrl,
    ) -> Result<Option<Vec<KeySetInfo>>, Self::Err>;
    /// Get mint keyset by id
    async fn get_keyset_by_id(&self, keyset_id: &Id) -> Result<Option<KeySetInfo>, Self::Err>;

    /// Add mint quote to storage
    async fn add_mint_quote(&self, quote: WalletMintQuote) -> Result<(), Self::Err>;
    /// Get mint quote from storage
    async fn get_mint_quote(&self, quote_id: &str) -> Result<Option<WalletMintQuote>, Self::Err>;
    /// Get mint quotes from storage
    async fn get_mint_quotes(&self) -> Result<Vec<WalletMintQuote>, Self::Err>;
    /// Remove mint quote from storage
    async fn remove_mint_quote(&self, quote_id: &str) -> Result<(), Self::Err>;

    /// Add melt quote to storage
    async fn add_melt_quote(&self, quote: wallet::MeltQuote) -> Result<(), Self::Err>;
    /// Get melt quote from storage
    async fn get_melt_quote(&self, quote_id: &str) -> Result<Option<wallet::MeltQuote>, Self::Err>;
    /// Remove melt quote from storage
    async fn remove_melt_quote(&self, quote_id: &str) -> Result<(), Self::Err>;

    /// Add [`Keys`] to storage
    async fn add_keys(&self, keys: Keys) -> Result<(), Self::Err>;
    /// Get [`Keys`] from storage
    async fn get_keys(&self, id: &Id) -> Result<Option<Keys>, Self::Err>;
    /// Remove [`Keys`] from storage
    async fn remove_keys(&self, id: &Id) -> Result<(), Self::Err>;

    /// Update the proofs in storage by adding new proofs or removing proofs by
    /// their Y value.
    async fn update_proofs(
        &self,
        added: Vec<ProofInfo>,
        removed_ys: Vec<PublicKey>,
    ) -> Result<(), Self::Err>;
    /// Set proofs as pending in storage. Proofs are identified by their Y
    /// value.
    async fn set_pending_proofs(&self, ys: Vec<PublicKey>) -> Result<(), Self::Err>;
    /// Reserve proofs in storage. Proofs are identified by their Y value.
    async fn reserve_proofs(&self, ys: Vec<PublicKey>) -> Result<(), Self::Err>;
    /// Set proofs as unspent in storage. Proofs are identified by their Y
    /// value.
    async fn set_unspent_proofs(&self, ys: Vec<PublicKey>) -> Result<(), Self::Err>;
    /// Get proofs from storage
    async fn get_proofs(
        &self,
        mint_url: Option<MintUrl>,
        unit: Option<CurrencyUnit>,
        state: Option<Vec<State>>,
        spending_conditions: Option<Vec<SpendingConditions>>,
    ) -> Result<Vec<ProofInfo>, Self::Err>;

    /// Increment Keyset counter
    async fn increment_keyset_counter(&self, keyset_id: &Id, count: u32) -> Result<(), Self::Err>;
    /// Get current Keyset counter
    async fn get_keyset_counter(&self, keyset_id: &Id) -> Result<Option<u32>, Self::Err>;

    /// Get when nostr key was last checked
    async fn get_nostr_last_checked(
        &self,
        verifying_key: &PublicKey,
    ) -> Result<Option<u32>, Self::Err>;
    /// Update last checked time
    async fn add_nostr_last_checked(
        &self,
        verifying_key: PublicKey,
        last_checked: u32,
    ) -> Result<(), Self::Err>;
}

/// Mint Database trait
#[cfg(feature = "mint")]
#[async_trait]
pub trait MintDatabase {
    /// Mint Database Error
    type Err: Into<Error> + From<Error>;

    /// Add Active Keyset
    async fn set_active_keyset(&self, unit: CurrencyUnit, id: Id) -> Result<(), Self::Err>;
    /// Get Active Keyset
    async fn get_active_keyset_id(&self, unit: &CurrencyUnit) -> Result<Option<Id>, Self::Err>;
    /// Get all Active Keyset
    async fn get_active_keysets(&self) -> Result<HashMap<CurrencyUnit, Id>, Self::Err>;

    /// Add [`MintMintQuote`]
    async fn add_mint_quote(&self, quote: MintMintQuote) -> Result<(), Self::Err>;
    /// Get [`MintMintQuote`]
    async fn get_mint_quote(&self, quote_id: &Uuid) -> Result<Option<MintMintQuote>, Self::Err>;
    /// Update state of [`MintMintQuote`]
    async fn update_mint_quote_state(
        &self,
        quote_id: &Uuid,
        state: MintQuoteState,
    ) -> Result<MintQuoteState, Self::Err>;
    /// Get all [`MintMintQuote`]s
    async fn get_mint_quote_by_request(
        &self,
        request: &str,
    ) -> Result<Option<MintMintQuote>, Self::Err>;
    /// Get all [`MintMintQuote`]s
    async fn get_mint_quote_by_request_lookup_id(
        &self,
        request_lookup_id: &str,
    ) -> Result<Option<MintMintQuote>, Self::Err>;
    /// Get Mint Quotes
    async fn get_mint_quotes(&self) -> Result<Vec<MintMintQuote>, Self::Err>;
    /// Remove [`MintMintQuote`]
    async fn remove_mint_quote(&self, quote_id: &Uuid) -> Result<(), Self::Err>;

    /// Add [`mint::MeltQuote`]
    async fn add_melt_quote(&self, quote: mint::MeltQuote) -> Result<(), Self::Err>;
    /// Get [`mint::MeltQuote`]
    async fn get_melt_quote(&self, quote_id: &Uuid) -> Result<Option<mint::MeltQuote>, Self::Err>;
    /// Update [`mint::MeltQuote`] state
    async fn update_melt_quote_state(
        &self,
        quote_id: &Uuid,
        state: MeltQuoteState,
    ) -> Result<MeltQuoteState, Self::Err>;
    /// Get all [`mint::MeltQuote`]s
    async fn get_melt_quotes(&self) -> Result<Vec<mint::MeltQuote>, Self::Err>;
    /// Remove [`mint::MeltQuote`]
    async fn remove_melt_quote(&self, quote_id: &Uuid) -> Result<(), Self::Err>;

    /// Add melt request
    async fn add_melt_request(
        &self,
        melt_request: MeltBolt11Request<Uuid>,
        ln_key: LnKey,
    ) -> Result<(), Self::Err>;
    /// Get melt request
    async fn get_melt_request(
        &self,
        quote_id: &Uuid,
    ) -> Result<Option<(MeltBolt11Request<Uuid>, LnKey)>, Self::Err>;

    /// Add [`MintKeySetInfo`]
    async fn add_keyset_info(&self, keyset: MintKeySetInfo) -> Result<(), Self::Err>;
    /// Get [`MintKeySetInfo`]
    async fn get_keyset_info(&self, id: &Id) -> Result<Option<MintKeySetInfo>, Self::Err>;
    /// Get [`MintKeySetInfo`]s
    async fn get_keyset_infos(&self) -> Result<Vec<MintKeySetInfo>, Self::Err>;

    /// Add spent [`Proofs`]
    async fn add_proofs(&self, proof: Proofs, quote_id: Option<Uuid>) -> Result<(), Self::Err>;
    /// Get [`Proofs`] by ys
    async fn get_proofs_by_ys(&self, ys: &[PublicKey]) -> Result<Vec<Option<Proof>>, Self::Err>;
    /// Get ys by quote id
    async fn get_proof_ys_by_quote_id(&self, quote_id: &Uuid) -> Result<Vec<PublicKey>, Self::Err>;
    /// Get [`Proofs`] state
    async fn get_proofs_states(&self, ys: &[PublicKey]) -> Result<Vec<Option<State>>, Self::Err>;
    /// Get [`Proofs`] state
    async fn update_proofs_states(
        &self,
        ys: &[PublicKey],
        proofs_state: State,
    ) -> Result<Vec<Option<State>>, Self::Err>;
    /// Get [`Proofs`] by state
    async fn get_proofs_by_keyset_id(
        &self,
        keyset_id: &Id,
    ) -> Result<(Proofs, Vec<Option<State>>), Self::Err>;

    /// Add [`BlindSignature`]
    async fn add_blind_signatures(
        &self,
        blinded_messages: &[PublicKey],
        blind_signatures: &[BlindSignature],
        quote_id: Option<Uuid>,
    ) -> Result<(), Self::Err>;
    /// Get [`BlindSignature`]s
    async fn get_blind_signatures(
        &self,
        blinded_messages: &[PublicKey],
    ) -> Result<Vec<Option<BlindSignature>>, Self::Err>;
    /// Get [`BlindSignature`]s for keyset_id
    async fn get_blind_signatures_for_keyset(
        &self,
        keyset_id: &Id,
    ) -> Result<Vec<BlindSignature>, Self::Err>;
    /// Get [`BlindSignature`]s for quote
    async fn get_blind_signatures_for_quote(
        &self,
        quote_id: &Uuid,
    ) -> Result<Vec<BlindSignature>, Self::Err>;
}
