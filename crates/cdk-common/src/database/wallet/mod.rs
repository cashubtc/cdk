//! CDK Database

use std::collections::HashMap;
use std::fmt::Debug;

use async_trait::async_trait;
use cashu::KeySet;

use super::{DbTransactionFinalizer, Error};
use crate::common::ProofInfo;
use crate::database::{KVStoreDatabase, KVStoreTransaction};
use crate::mint_url::MintUrl;
use crate::nuts::{
    CurrencyUnit, Id, KeySetInfo, Keys, MintInfo, PublicKey, SpendingConditions, State,
};
use crate::wallet::{
    self, MintQuote as WalletMintQuote, Transaction, TransactionDirection, TransactionId,
};

#[cfg(feature = "test")]
pub mod test;

/// Easy to use Dynamic Database type alias
pub type DynWalletDatabaseTransaction = Box<dyn DatabaseTransaction<super::Error> + Sync + Send>;

/// Database transaction
///
/// This trait encapsulates all the changes to be done in the wallet
#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
pub trait DatabaseTransaction<Error>:
    KVStoreTransaction<Error> + DbTransactionFinalizer<Err = Error>
{
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
    async fn get_keys(&mut self, id: &Id) -> Result<Option<Keys>, Error>;

    /// Add mint keyset to storage
    async fn add_mint_keysets(
        &mut self,
        mint_url: MintUrl,
        keysets: Vec<KeySetInfo>,
    ) -> Result<(), Error>;

    /// Get mint quote from storage. This function locks the returned minted quote for update
    async fn get_mint_quote(&mut self, quote_id: &str) -> Result<Option<WalletMintQuote>, Error>;

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
pub trait Database<Err>: KVStoreDatabase<Err = Err> + Debug
where
    Err: Into<Error> + From<Error>,
{
    /// Begins a DB transaction
    async fn begin_db_transaction(
        &self,
    ) -> Result<Box<dyn DatabaseTransaction<Err> + Send + Sync>, Err>;

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

    // stores p2pk signing keys for the wallet
    async fn add_p2pk_key(&self, pubkey: &PublicKey, derivation_path: String, derivation_index: u32) -> Result<(), Self::Err>;
    /// Get a stored P2PK signing key by pubkey.
    async fn get_p2pk_key(&self, pubkey: &PublicKey) -> Result<Option<wallet::P2PKSigningKey>, Self::Err>;
    /// List all stored P2PK signing keys.
    async fn list_p2pk_keys(&self) -> Result<Vec<wallet::P2PKSigningKey>, Self::Err>;
}
