//! CDK Database

use std::collections::HashMap;
use std::fmt::Debug;

use async_trait::async_trait;
use cashu::KeySet;

use super::Error;
use crate::common::ProofInfo;
use crate::database::KVStoreDatabase;
use crate::mint_url::MintUrl;
use crate::nuts::{
    CurrencyUnit, Id, KeySetInfo, Keys, MintInfo, PublicKey, SpendingConditions, State,
};
use crate::wallet::{
    self, MintQuote as WalletMintQuote, Transaction, TransactionDirection, TransactionId,
};

#[cfg(feature = "test")]
pub mod test;

/// Wallet Database trait
#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
pub trait Database<Err>: KVStoreDatabase<Err = Err> + Debug
where
    Err: Into<Error> + From<Error>,
{
    /// Get mint from storage
    async fn get_mint(&self, mint_url: MintUrl) -> Result<Option<MintInfo>, Err>;

    /// Get all mints from storage
    async fn get_mints(&self) -> Result<HashMap<MintUrl, Option<MintInfo>>, Err>;

    /// Get mint keysets for mint url
    async fn get_mint_keysets(&self, mint_url: MintUrl) -> Result<Option<Vec<KeySetInfo>>, Err>;

    /// Get mint keyset by id
    async fn get_keyset_by_id(&self, keyset_id: &Id) -> Result<Option<KeySetInfo>, Err>;

    /// Get mint quote from storage
    async fn get_mint_quote(&self, quote_id: &str) -> Result<Option<WalletMintQuote>, Err>;

    /// Get mint quotes from storage
    async fn get_mint_quotes(&self) -> Result<Vec<WalletMintQuote>, Err>;
    /// Get unissued mint quotes from storage
    /// Returns bolt11 quotes where nothing has been issued yet (amount_issued = 0) and all bolt12 quotes.
    /// Includes unpaid bolt11 quotes to allow checking with the mint if they've been paid (wallet state may be outdated).
    async fn get_unissued_mint_quotes(&self) -> Result<Vec<WalletMintQuote>, Err>;

    /// Get melt quote from storage
    async fn get_melt_quote(&self, quote_id: &str) -> Result<Option<wallet::MeltQuote>, Err>;

    /// Get melt quotes from storage
    async fn get_melt_quotes(&self) -> Result<Vec<wallet::MeltQuote>, Err>;

    /// Get [`Keys`] from storage
    async fn get_keys(&self, id: &Id) -> Result<Option<Keys>, Err>;

    /// Get proofs from storage
    async fn get_proofs(
        &self,
        mint_url: Option<MintUrl>,
        unit: Option<CurrencyUnit>,
        state: Option<Vec<State>>,
        spending_conditions: Option<Vec<SpendingConditions>>,
    ) -> Result<Vec<ProofInfo>, Err>;

    /// Get proofs by Y values
    async fn get_proofs_by_ys(&self, ys: Vec<PublicKey>) -> Result<Vec<ProofInfo>, Err>;

    /// Get balance
    async fn get_balance(
        &self,
        mint_url: Option<MintUrl>,
        unit: Option<CurrencyUnit>,
        state: Option<Vec<State>>,
    ) -> Result<u64, Err>;

    /// Get transaction from storage
    async fn get_transaction(
        &self,
        transaction_id: TransactionId,
    ) -> Result<Option<Transaction>, Err>;

    /// List transactions from storage
    async fn list_transactions(
        &self,
        mint_url: Option<MintUrl>,
        direction: Option<TransactionDirection>,
        unit: Option<CurrencyUnit>,
    ) -> Result<Vec<Transaction>, Err>;

    /// Update the proofs in storage by adding new proofs or removing proofs by
    /// their Y value (without transaction)
    async fn update_proofs(
        &self,
        added: Vec<ProofInfo>,
        removed_ys: Vec<PublicKey>,
    ) -> Result<(), Err>;

    /// Update proofs state in storage (without transaction)
    async fn update_proofs_state(&self, ys: Vec<PublicKey>, state: State) -> Result<(), Err>;

    /// Add transaction to storage (without transaction)
    async fn add_transaction(&self, transaction: Transaction) -> Result<(), Err>;

    /// Update mint url (without transaction)
    async fn update_mint_url(
        &self,
        old_mint_url: MintUrl,
        new_mint_url: MintUrl,
    ) -> Result<(), Err>;

    /// Atomically increment Keyset counter and return new value (without transaction)
    async fn increment_keyset_counter(&self, keyset_id: &Id, count: u32) -> Result<u32, Err>;

    /// Add Mint to storage
    async fn add_mint(&self, mint_url: MintUrl, mint_info: Option<MintInfo>) -> Result<(), Err>;

    /// Remove Mint from storage
    async fn remove_mint(&self, mint_url: MintUrl) -> Result<(), Err>;

    /// Add mint keyset to storage
    async fn add_mint_keysets(
        &self,
        mint_url: MintUrl,
        keysets: Vec<KeySetInfo>,
    ) -> Result<(), Err>;

    /// Add mint quote to storage
    async fn add_mint_quote(&self, quote: WalletMintQuote) -> Result<(), Err>;

    /// Remove mint quote from storage
    async fn remove_mint_quote(&self, quote_id: &str) -> Result<(), Err>;

    /// Add melt quote to storage
    async fn add_melt_quote(&self, quote: wallet::MeltQuote) -> Result<(), Err>;

    /// Remove melt quote from storage
    async fn remove_melt_quote(&self, quote_id: &str) -> Result<(), Err>;

    /// Add [`Keys`] to storage
    async fn add_keys(&self, keyset: KeySet) -> Result<(), Err>;

    /// Remove [`Keys`] from storage
    async fn remove_keys(&self, id: &Id) -> Result<(), Err>;

    /// Remove transaction from storage
    async fn remove_transaction(&self, transaction_id: TransactionId) -> Result<(), Err>;

    // KV Store write methods (non-transactional)

    /// Write a value to the key-value store
    async fn kv_write(
        &self,
        primary_namespace: &str,
        secondary_namespace: &str,
        key: &str,
        value: &[u8],
    ) -> Result<(), Err>;

    /// Remove a value from the key-value store
    async fn kv_remove(
        &self,
        primary_namespace: &str,
        secondary_namespace: &str,
        key: &str,
    ) -> Result<(), Err>;
}
