//! CDK Database

use std::collections::HashMap;
use std::fmt::Debug;

use async_trait::async_trait;
use bitcoin::bip32::DerivationPath;
use cashu::KeySet;

use super::Error;
use crate::mint_url::MintUrl;
use crate::nuts::{
    CurrencyUnit, Id, KeySetInfo, Keys, MintInfo, PublicKey, SpendingConditions, State,
};
use crate::wallet::{
    self, MintQuote as WalletMintQuote, ProofInfo, Transaction, TransactionDirection, TransactionId,
};

#[cfg(feature = "test")]
pub mod test;

/// Wallet Database trait
#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
pub trait Database<Err>: Debug
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
    /// their Y value
    async fn update_proofs(
        &self,
        added: Vec<ProofInfo>,
        removed_ys: Vec<PublicKey>,
    ) -> Result<(), Err>;

    /// Atomically persist a validated general-restoration batch.
    ///
    /// For every supplied receive saga ID/version, require a matching
    /// `SwapRequested` receive, clear its atomic-settlement provenance, and
    /// increment its version. Return false without writes on any conflict.
    /// Insert missing non-Spent proofs without resetting existing states;
    /// a mint Pending result may restrict an existing Unspent proof to Pending.
    /// never reinsert `persisted_ys` consumed since the caller's snapshot.
    /// Remove proofs reported Spent. Saga invalidation and all proof changes
    /// must commit together, including on cancellation or error. Advance
    /// keyset counters past every supplied derivation index in that transaction.
    /// Before any writes, validate all `saga_versions` (including ProofsPending
    /// receives) and lock/compare `keyset_counter` for `keyset_id`. The caller
    /// reads that counter before the saga snapshot. A conflict returns false
    /// without writes, including when an intervening operation has settled.
    async fn restore_proofs(
        &self,
        proofs: Vec<ProofInfo>,
        persisted_ys: Vec<PublicKey>,
        receive_sagas: HashMap<uuid::Uuid, u32>,
        saga_versions: HashMap<uuid::Uuid, u32>,
        keyset_id: Id,
        keyset_counter: u32,
    ) -> Result<bool, Err>;

    /// Atomically finish a receive after its complete outputs have been validated.
    ///
    /// Only apply when the receive saga is still `SwapRequested` with `version`
    /// and its incoming transaction is pending or completed. Insert missing outputs without
    /// overwriting existing proofs, remove the input proofs owned by this saga, mark the transaction
    /// completed, and delete the saga in the same transaction.
    /// `persisted_ys` identifies previously persisted outputs: do not reinsert
    /// these proofs if another operation has consumed them since they were read.
    /// Return `false` without writes if the saga or incoming transaction no longer matches.
    /// No partial settlement may become visible, including on error.
    async fn finalize_receive(
        &self,
        saga_id: &uuid::Uuid,
        version: u32,
        outputs: Vec<ProofInfo>,
        persisted_ys: Vec<PublicKey>,
    ) -> Result<bool, Err>;

    /// Update proofs state in storage
    async fn update_proofs_state(&self, ys: Vec<PublicKey>, state: State) -> Result<(), Err>;

    /// Add transaction to storage. An upsert must not change the status of an
    /// existing terminal transaction.
    async fn add_transaction(&self, transaction: Transaction) -> Result<(), Err>;

    /// Update mint url
    async fn update_mint_url(
        &self,
        old_mint_url: MintUrl,
        new_mint_url: MintUrl,
    ) -> Result<(), Err>;

    /// Atomically increment Keyset counter and return new value
    async fn increment_keyset_counter(&self, keyset_id: &Id, count: u32) -> Result<u32, Err>;

    /// Atomically increment a namespaced derivation counter and return its new value.
    async fn increment_derivation_counter(&self, namespace: &str, count: u32) -> Result<u32, Err>;

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

    /// Add a wallet saga to storage.
    ///
    /// The saga should be created with `WalletSaga::new()` which initializes
    /// `version = 0`. This is the starting point for optimistic locking.
    async fn add_saga(&self, saga: wallet::WalletSaga) -> Result<(), Err>;

    /// Get a wallet saga by ID.
    async fn get_saga(&self, id: &uuid::Uuid) -> Result<Option<wallet::WalletSaga>, Err>;

    /// Update a wallet saga with optimistic locking.
    ///
    /// Returns `Ok(true)` if the update succeeded (version match), or `Ok(false)`
    /// if another instance modified the saga first (version mismatch).
    async fn update_saga(&self, saga: wallet::WalletSaga) -> Result<bool, Err>;

    /// Delete a wallet saga.
    async fn delete_saga(&self, id: &uuid::Uuid) -> Result<(), Err>;

    /// Get all incomplete sagas.
    async fn get_incomplete_sagas(&self) -> Result<Vec<wallet::WalletSaga>, Err>;

    /// Reserve proofs for an operation
    async fn reserve_proofs(
        &self,
        ys: Vec<PublicKey>,
        operation_id: &uuid::Uuid,
    ) -> Result<(), Err>;

    /// Release proofs reserved by an operation.
    ///
    /// Only proofs owned by `operation_id` in [`State::Reserved`] or
    /// [`State::Pending`] may be changed to [`State::Unspent`]. Implementations
    /// must preserve spent proofs and proofs owned by another operation.
    async fn release_proofs(&self, operation_id: &uuid::Uuid) -> Result<(), Err>;

    /// Get proofs reserved by an operation
    async fn get_reserved_proofs(&self, operation_id: &uuid::Uuid) -> Result<Vec<ProofInfo>, Err>;

    /// Reserve a melt quote for an operation.
    async fn reserve_melt_quote(
        &self,
        quote_id: &str,
        operation_id: &uuid::Uuid,
    ) -> Result<(), Err>;

    /// Release a melt quote reserved by an operation.
    async fn release_melt_quote(&self, operation_id: &uuid::Uuid) -> Result<(), Err>;

    /// Reserve a mint quote for an operation.
    async fn reserve_mint_quote(
        &self,
        quote_id: &str,
        operation_id: &uuid::Uuid,
    ) -> Result<(), Err>;

    /// Release a mint quote reserved by an operation.
    async fn release_mint_quote(&self, operation_id: &uuid::Uuid) -> Result<(), Err>;

    /// Read a value from the key-value store
    async fn kv_read(
        &self,
        primary_namespace: &str,
        secondary_namespace: &str,
        key: &str,
    ) -> Result<Option<Vec<u8>>, Err>;

    /// List keys in a namespace
    async fn kv_list(
        &self,
        primary_namespace: &str,
        secondary_namespace: &str,
    ) -> Result<Vec<String>, Err>;

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

    // P2PK signing key methods

    /// Store a P2PK signing key for the wallet
    async fn add_p2pk_key(
        &self,
        pubkey: &PublicKey,
        derivation_path: DerivationPath,
        derivation_index: u32,
    ) -> Result<(), Err>;

    /// Get a stored P2PK signing key by pubkey.
    async fn get_p2pk_key(&self, pubkey: &PublicKey)
        -> Result<Option<wallet::P2PKSigningKey>, Err>;

    /// List all stored P2PK signing keys.
    async fn list_p2pk_keys(&self) -> Result<Vec<wallet::P2PKSigningKey>, Err>;

    /// Tries to get the latest p2pk key generated
    async fn latest_p2pk(&self) -> Result<Option<wallet::P2PKSigningKey>, Err>;
}
