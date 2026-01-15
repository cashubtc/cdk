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
    /// their Y value
    async fn update_proofs(
        &self,
        added: Vec<ProofInfo>,
        removed_ys: Vec<PublicKey>,
    ) -> Result<(), Err>;

    /// Update proofs state in storage
    async fn update_proofs_state(&self, ys: Vec<PublicKey>, state: State) -> Result<(), Err>;

    /// Add transaction to storage
    async fn add_transaction(&self, transaction: Transaction) -> Result<(), Err>;

    /// Update mint url
    async fn update_mint_url(
        &self,
        old_mint_url: MintUrl,
        new_mint_url: MintUrl,
    ) -> Result<(), Err>;

    /// Atomically increment Keyset counter and return new value
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

    /// Add a wallet saga to storage.
    ///
    /// The saga should be created with `WalletSaga::new()` which initializes
    /// `version = 0`. This is the starting point for optimistic locking.
    async fn add_saga(&self, saga: wallet::WalletSaga) -> Result<(), Err>;

    /// Get a wallet saga by ID.
    async fn get_saga(&self, id: &uuid::Uuid) -> Result<Option<wallet::WalletSaga>, Err>;

    /// Update a wallet saga with optimistic locking.
    ///
    /// This method implements optimistic locking to handle concurrent access
    /// from multiple wallet instances. The update only succeeds if the saga's
    /// version in the database matches `saga.version - 1` (the version before
    /// the caller incremented it).
    ///
    /// # Returns
    ///
    /// - `Ok(true)` - Update succeeded, this instance "won" the race
    /// - `Ok(false)` - Version mismatch, another instance modified the saga first
    /// - `Err(_)` - Database error (not a version conflict)
    ///
    /// # Usage
    ///
    /// ```ignore
    /// let mut saga = db.get_saga(&id).await?.unwrap();
    /// saga.update_state(NewState); // This increments version
    ///
    /// match db.update_saga(saga).await? {
    ///     true => println!("Update succeeded"),
    ///     false => println!("Another instance modified this saga, skipping"),
    /// }
    /// ```
    ///
    /// # Implementation Notes
    ///
    /// Implementations should use a conditional update like:
    /// ```sql
    /// UPDATE wallet_sagas SET ..., version = ?
    /// WHERE id = ? AND version = ? -- previous version
    /// ```
    /// Return `Ok(true)` if rows_affected > 0, `Ok(false)` otherwise.
    async fn update_saga(&self, saga: wallet::WalletSaga) -> Result<bool, Err>;

    /// Delete a wallet saga.
    ///
    /// This is typically called after a saga completes successfully.
    /// Deletion is best-effort - if it fails, the orphaned saga is harmless
    /// and will be cleaned up on next recovery.
    async fn delete_saga(&self, id: &uuid::Uuid) -> Result<(), Err>;

    /// Get all incomplete sagas (sagas that haven't been deleted yet).
    ///
    /// Used during recovery to find sagas that were interrupted by a crash.
    /// The caller should process each saga and handle version conflicts
    /// gracefully (another instance may be processing the same saga).
    async fn get_incomplete_sagas(&self) -> Result<Vec<wallet::WalletSaga>, Err>;

    /// Reserve proofs for an operation
    /// Sets proofs to Reserved state and marks them as used by the operation
    /// Returns an error if any proofs are already reserved or not in Unspent state
    async fn reserve_proofs(
        &self,
        ys: Vec<PublicKey>,
        operation_id: &uuid::Uuid,
    ) -> Result<(), Err>;

    /// Release proofs reserved by an operation
    /// Sets proofs back to Unspent state and clears the used_by_operation field
    async fn release_proofs(&self, operation_id: &uuid::Uuid) -> Result<(), Err>;

    /// Get proofs reserved by an operation
    async fn get_reserved_proofs(&self, operation_id: &uuid::Uuid) -> Result<Vec<ProofInfo>, Err>;

    /// Reserve a melt quote for an operation.
    ///
    /// Atomically marks the quote as used by the operation. This prevents
    /// concurrent operations from using the same quote.
    ///
    /// # Errors
    ///
    /// Returns `Error::QuoteAlreadyInUse` if the quote is already reserved
    /// by another operation.
    /// Returns `Error::UnknownQuote` if the quote doesn't exist.
    async fn reserve_melt_quote(
        &self,
        quote_id: &str,
        operation_id: &uuid::Uuid,
    ) -> Result<(), Err>;

    /// Release a melt quote reserved by an operation.
    ///
    /// Clears the `used_by_operation` field for the quote, allowing it to be
    /// used by another operation. This is called during saga compensation
    /// or after successful completion.
    async fn release_melt_quote(&self, operation_id: &uuid::Uuid) -> Result<(), Err>;

    /// Reserve a mint quote for an operation.
    ///
    /// Atomically marks the quote as used by the operation. This prevents
    /// concurrent operations from using the same quote.
    ///
    /// # Errors
    ///
    /// Returns `Error::QuoteAlreadyInUse` if the quote is already reserved
    /// by another operation.
    /// Returns `Error::UnknownQuote` if the quote doesn't exist.
    async fn reserve_mint_quote(
        &self,
        quote_id: &str,
        operation_id: &uuid::Uuid,
    ) -> Result<(), Err>;

    /// Release a mint quote reserved by an operation.
    ///
    /// Clears the `used_by_operation` field for the quote, allowing it to be
    /// used by another operation. This is called during saga compensation
    /// or after successful completion.
    async fn release_mint_quote(&self, operation_id: &uuid::Uuid) -> Result<(), Err>;

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
