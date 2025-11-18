//! CDK Database

use std::collections::HashMap;
use std::fmt::Debug;

use async_trait::async_trait;
use cashu::KeySet;

use super::{DbTransactionFinalizer, Error};
use crate::common::ProofInfo;
use crate::mint_url::MintUrl;
use crate::nuts::{
    CurrencyUnit, Id, KeySetInfo, Keys, MintInfo, PublicKey, SpendingConditions, State,
};
use crate::wallet::{
    self, MintQuote as WalletMintQuote, Transaction, TransactionDirection, TransactionId,
};

/// Easy to use Dynamic Database type alias
pub type DynWalletDatabaseTransaction<'a> =
    Box<dyn DatabaseTransaction<'a, super::Error> + Sync + Send + 'a>;

/// Database transaction
///
/// This trait encapsulates all the changes to be done in the wallet
#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
pub trait DatabaseTransaction<'a, Error>: DbTransactionFinalizer<Err = Error> {
    /// Add Mint to storage
    async fn add_mint(
        &mut self,
        mint_url: MintUrl,
        mint_info: Option<MintInfo>,
    ) -> Result<(), Error>;

    /// Remove Mint from storage
    async fn remove_mint(&mut self, mint_url: MintUrl) -> Result<(), Error>;

    /// Update mint url
    async fn update_mint_url(
        &mut self,
        old_mint_url: MintUrl,
        new_mint_url: MintUrl,
    ) -> Result<(), Error>;

    /// Get mint keyset by id
    async fn get_keyset_by_id(&mut self, keyset_id: &Id) -> Result<Option<KeySetInfo>, Error>;

    /// Get [`Keys`] from storage
    async fn get_keys(&mut self, id: &Id) -> Result<Option<Keys>, Self::Err>;

    /// Add mint keyset to storage
    async fn add_mint_keysets(
        &mut self,
        mint_url: MintUrl,
        keysets: Vec<KeySetInfo>,
    ) -> Result<(), Error>;

    /// Get mint quote from storage. This function locks the returned minted quote for update
    async fn get_mint_quote(
        &mut self,
        quote_id: &str,
    ) -> Result<Option<WalletMintQuote>, Self::Err>;

    /// Add mint quote to storage
    async fn add_mint_quote(&mut self, quote: WalletMintQuote) -> Result<(), Error>;

    /// Remove mint quote from storage
    async fn remove_mint_quote(&mut self, quote_id: &str) -> Result<(), Error>;

    /// Get melt quote from storage
    async fn get_melt_quote(&mut self, quote_id: &str) -> Result<Option<wallet::MeltQuote>, Error>;

    /// Add melt quote to storage
    async fn add_melt_quote(&mut self, quote: wallet::MeltQuote) -> Result<(), Error>;

    /// Remove melt quote from storage
    async fn remove_melt_quote(&mut self, quote_id: &str) -> Result<(), Error>;

    /// Add [`Keys`] to storage
    async fn add_keys(&mut self, keyset: KeySet) -> Result<(), Error>;

    /// Remove [`Keys`] from storage
    async fn remove_keys(&mut self, id: &Id) -> Result<(), Error>;

    /// Get proofs from storage and lock them for update
    async fn get_proofs(
        &mut self,
        mint_url: Option<MintUrl>,
        unit: Option<CurrencyUnit>,
        state: Option<Vec<State>>,
        spending_conditions: Option<Vec<SpendingConditions>>,
    ) -> Result<Vec<ProofInfo>, Error>;

    /// Update the proofs in storage by adding new proofs or removing proofs by
    /// their Y value.
    async fn update_proofs(
        &mut self,
        added: Vec<ProofInfo>,
        removed_ys: Vec<PublicKey>,
    ) -> Result<(), Error>;

    /// Update proofs state in storage
    async fn update_proofs_state(&mut self, ys: Vec<PublicKey>, state: State) -> Result<(), Error>;

    /// Atomically increment Keyset counter and return new value
    async fn increment_keyset_counter(&mut self, keyset_id: &Id, count: u32) -> Result<u32, Error>;

    /// Add transaction to storage
    async fn add_transaction(&mut self, transaction: Transaction) -> Result<(), Error>;

    /// Remove transaction from storage
    async fn remove_transaction(&mut self, transaction_id: TransactionId) -> Result<(), Error>;
}

/// Wallet Database trait
#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
pub trait Database: Debug {
    /// Wallet Database Error
    type Err: Into<Error> + From<Error>;

    /// Begins a DB transaction
    async fn begin_db_transaction<'a>(
        &'a self,
    ) -> Result<Box<dyn DatabaseTransaction<'a, Self::Err> + Send + Sync + 'a>, Self::Err>;

    /// Get mint from storage
    async fn get_mint(&self, mint_url: MintUrl) -> Result<Option<MintInfo>, Self::Err>;

    /// Get all mints from storage
    async fn get_mints(&self) -> Result<HashMap<MintUrl, Option<MintInfo>>, Self::Err>;

    /// Get mint keysets for mint url
    async fn get_mint_keysets(
        &self,
        mint_url: MintUrl,
    ) -> Result<Option<Vec<KeySetInfo>>, Self::Err>;

    /// Get mint keyset by id
    async fn get_keyset_by_id(&self, keyset_id: &Id) -> Result<Option<KeySetInfo>, Self::Err>;

    /// Get mint quote from storage
    async fn get_mint_quote(&self, quote_id: &str) -> Result<Option<WalletMintQuote>, Self::Err>;

    /// Get mint quotes from storage
    async fn get_mint_quotes(&self) -> Result<Vec<WalletMintQuote>, Self::Err>;

    /// Get melt quote from storage
    async fn get_melt_quote(&self, quote_id: &str) -> Result<Option<wallet::MeltQuote>, Self::Err>;

    /// Get melt quotes from storage
    async fn get_melt_quotes(&self) -> Result<Vec<wallet::MeltQuote>, Self::Err>;

    /// Get [`Keys`] from storage
    async fn get_keys(&self, id: &Id) -> Result<Option<Keys>, Self::Err>;

    /// Get proofs from storage
    async fn get_proofs(
        &self,
        mint_url: Option<MintUrl>,
        unit: Option<CurrencyUnit>,
        state: Option<Vec<State>>,
        spending_conditions: Option<Vec<SpendingConditions>>,
    ) -> Result<Vec<ProofInfo>, Self::Err>;

    /// Get balance
    async fn get_balance(
        &self,
        mint_url: Option<MintUrl>,
        unit: Option<CurrencyUnit>,
        state: Option<Vec<State>>,
    ) -> Result<u64, Self::Err>;

    /// Get transaction from storage
    async fn get_transaction(
        &self,
        transaction_id: TransactionId,
    ) -> Result<Option<Transaction>, Self::Err>;

    /// List transactions from storage
    async fn list_transactions(
        &self,
        mint_url: Option<MintUrl>,
        direction: Option<TransactionDirection>,
        unit: Option<CurrencyUnit>,
    ) -> Result<Vec<Transaction>, Self::Err>;
}
