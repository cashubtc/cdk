//! CDK Database

use std::collections::HashMap;
use std::fmt::Debug;

use async_trait::async_trait;

use super::Error;
use crate::common::ProofInfo;
use crate::mint_url::MintUrl;
use crate::nuts::{
    CurrencyUnit, Id, KeySetInfo, Keys, MintInfo, PublicKey, SpendingConditions, State,
};
use crate::wallet;
use crate::wallet::MintQuote as WalletMintQuote;

/// Wallet Database trait
#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
pub trait Database: Debug {
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
