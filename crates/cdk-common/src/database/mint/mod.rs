//! CDK Database

use std::collections::HashMap;

/// Valid ASCII characters for namespace and key strings in KV store
pub const KVSTORE_NAMESPACE_KEY_ALPHABET: &str =
    "abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789_-";

/// Maximum length for namespace and key strings in KV store
pub const KVSTORE_NAMESPACE_KEY_MAX_LEN: usize = 120;

/// Validates that a string contains only valid KV store characters and is within length limits
pub fn validate_kvstore_string(s: &str) -> Result<(), Error> {
    if s.len() > KVSTORE_NAMESPACE_KEY_MAX_LEN {
        return Err(Error::KVStoreInvalidKey(format!(
            "{} exceeds maximum length of key characters",
            KVSTORE_NAMESPACE_KEY_MAX_LEN
        )));
    }

    if !s
        .chars()
        .all(|c| KVSTORE_NAMESPACE_KEY_ALPHABET.contains(c))
    {
        return Err(Error::KVStoreInvalidKey("key contains invalid characters. Only ASCII letters, numbers, underscore, and hyphen are allowed".to_string()));
    }

    Ok(())
}

/// Validates namespace and key parameters for KV store operations
pub fn validate_kvstore_params(
    primary_namespace: &str,
    secondary_namespace: &str,
    key: &str,
) -> Result<(), Error> {
    // Validate primary namespace
    validate_kvstore_string(primary_namespace)?;

    // Validate secondary namespace
    validate_kvstore_string(secondary_namespace)?;

    // Validate key
    validate_kvstore_string(key)?;

    // Check empty namespace rules
    if primary_namespace.is_empty() && !secondary_namespace.is_empty() {
        return Err(Error::KVStoreInvalidKey(
            "If primary_namespace is empty, secondary_namespace must also be empty".to_string(),
        ));
    }

    // Check for potential collisions between keys and namespaces in the same namespace
    let namespace_key = format!("{}/{}", primary_namespace, secondary_namespace);
    if key == primary_namespace || key == secondary_namespace || key == namespace_key {
        return Err(Error::KVStoreInvalidKey(format!(
            "Key '{}' conflicts with namespace names",
            key
        )));
    }

    Ok(())
}

use async_trait::async_trait;
use cashu::quote_id::QuoteId;
use cashu::{Amount, MintInfo};
use uuid::Uuid;

use super::Error;
use crate::common::QuoteTTL;
use crate::mint::{self, MintKeySetInfo, MintQuote as MintMintQuote};
use crate::nuts::{
    BlindSignature, CurrencyUnit, Id, MeltQuoteState, Proof, Proofs, PublicKey, State,
};
use crate::payment::PaymentIdentifier;

#[cfg(feature = "auth")]
mod auth;

#[cfg(feature = "test")]
pub mod test;

#[cfg(test)]
mod test_kvstore;

#[cfg(feature = "auth")]
pub use auth::{MintAuthDatabase, MintAuthTransaction};

/// KeysDatabaseWriter
#[async_trait]
pub trait KeysDatabaseTransaction<'a, Error>: DbTransactionFinalizer<Err = Error> {
    /// Add Active Keyset
    async fn set_active_keyset(&mut self, unit: CurrencyUnit, id: Id) -> Result<(), Error>;

    /// Add [`MintKeySetInfo`]
    async fn add_keyset_info(&mut self, keyset: MintKeySetInfo) -> Result<(), Error>;
}

/// Mint Keys Database trait
#[async_trait]
pub trait KeysDatabase {
    /// Mint Keys Database Error
    type Err: Into<Error> + From<Error>;

    /// Beings a transaction
    async fn begin_transaction<'a>(
        &'a self,
    ) -> Result<Box<dyn KeysDatabaseTransaction<'a, Self::Err> + Send + Sync + 'a>, Error>;

    /// Get Active Keyset
    async fn get_active_keyset_id(&self, unit: &CurrencyUnit) -> Result<Option<Id>, Self::Err>;

    /// Get all Active Keyset
    async fn get_active_keysets(&self) -> Result<HashMap<CurrencyUnit, Id>, Self::Err>;

    /// Get [`MintKeySetInfo`]
    async fn get_keyset_info(&self, id: &Id) -> Result<Option<MintKeySetInfo>, Self::Err>;

    /// Get [`MintKeySetInfo`]s
    async fn get_keyset_infos(&self) -> Result<Vec<MintKeySetInfo>, Self::Err>;
}

/// Mint Quote Database writer trait
#[async_trait]
pub trait QuotesTransaction<'a> {
    /// Mint Quotes Database Error
    type Err: Into<Error> + From<Error>;

    /// Get [`MintMintQuote`] and lock it for update in this transaction
    async fn get_mint_quote(
        &mut self,
        quote_id: &QuoteId,
    ) -> Result<Option<MintMintQuote>, Self::Err>;
    /// Add [`MintMintQuote`]
    async fn add_mint_quote(&mut self, quote: MintMintQuote) -> Result<(), Self::Err>;
    /// Increment amount paid [`MintMintQuote`]
    async fn increment_mint_quote_amount_paid(
        &mut self,
        quote_id: &QuoteId,
        amount_paid: Amount,
        payment_id: String,
    ) -> Result<Amount, Self::Err>;
    /// Increment amount paid [`MintMintQuote`]
    async fn increment_mint_quote_amount_issued(
        &mut self,
        quote_id: &QuoteId,
        amount_issued: Amount,
    ) -> Result<Amount, Self::Err>;
    /// Remove [`MintMintQuote`]
    async fn remove_mint_quote(&mut self, quote_id: &QuoteId) -> Result<(), Self::Err>;
    /// Get [`mint::MeltQuote`] and lock it for update in this transaction
    async fn get_melt_quote(
        &mut self,
        quote_id: &Uuid,
    ) -> Result<Option<mint::MeltQuote>, Self::Err>;
    /// Add [`mint::MeltQuote`]
    async fn add_melt_quote(&mut self, quote: mint::MeltQuote) -> Result<(), Self::Err>;

    /// Updates the request lookup id for a melt quote
    async fn update_melt_quote_request_lookup_id(
        &mut self,
        quote_id: &QuoteId,
        new_request_lookup_id: &PaymentIdentifier,
    ) -> Result<(), Self::Err>;

    /// Update [`mint::MeltQuote`] state
    ///
    /// It is expected for this function to fail if the state is already set to the new state
    async fn update_melt_quote_state(
        &mut self,
        quote_id: &QuoteId,
        new_state: MeltQuoteState,
        payment_proof: Option<String>,
    ) -> Result<(MeltQuoteState, mint::MeltQuote), Self::Err>;
    /// Remove [`mint::MeltQuote`]
    async fn remove_melt_quote(&mut self, quote_id: &Uuid) -> Result<(), Self::Err>;
    /// Get all [`MintMintQuote`]s and lock it for update in this transaction
    async fn get_mint_quote_by_request(
        &mut self,
        request: &str,
    ) -> Result<Option<MintMintQuote>, Self::Err>;

    /// Get all [`MintMintQuote`]s
    async fn get_mint_quote_by_request_lookup_id(
        &mut self,
        request_lookup_id: &PaymentIdentifier,
    ) -> Result<Option<MintMintQuote>, Self::Err>;
}

/// Mint Quote Database trait
#[async_trait]
pub trait QuotesDatabase {
    /// Mint Quotes Database Error
    type Err: Into<Error> + From<Error>;

    /// Get [`MintMintQuote`]
    async fn get_mint_quote(&self, quote_id: &QuoteId) -> Result<Option<MintMintQuote>, Self::Err>;

    /// Get all [`MintMintQuote`]s
    async fn get_mint_quote_by_request(
        &self,
        request: &str,
    ) -> Result<Option<MintMintQuote>, Self::Err>;
    /// Get all [`MintMintQuote`]s
    async fn get_mint_quote_by_request_lookup_id(
        &self,
        request_lookup_id: &PaymentIdentifier,
    ) -> Result<Option<MintMintQuote>, Self::Err>;
    /// Get Mint Quotes
    async fn get_mint_quotes(&self) -> Result<Vec<MintMintQuote>, Self::Err>;
    /// Get [`mint::MeltQuote`]
    async fn get_melt_quote(
        &self,
        quote_id: &QuoteId,
    ) -> Result<Option<mint::MeltQuote>, Self::Err>;
    /// Get all [`mint::MeltQuote`]s
    async fn get_melt_quotes(&self) -> Result<Vec<mint::MeltQuote>, Self::Err>;
}

/// Mint Proof Transaction trait
#[async_trait]
pub trait ProofsTransaction<'a> {
    /// Mint Proof Database Error
    type Err: Into<Error> + From<Error>;

    /// Add  [`Proofs`]
    ///
    /// Adds proofs to the database. The database should error if the proof already exits, with a
    /// `AttemptUpdateSpentProof` if the proof is already spent or a `Duplicate` error otherwise.
    async fn add_proofs(&mut self, proof: Proofs, quote_id: Option<Uuid>) -> Result<(), Self::Err>;
    /// Updates the proofs to a given states and return the previous states
    async fn update_proofs_states(
        &mut self,
        ys: &[PublicKey],
        proofs_state: State,
    ) -> Result<Vec<Option<State>>, Self::Err>;

    /// Remove [`Proofs`]
    async fn remove_proofs(
        &mut self,
        ys: &[PublicKey],
        quote_id: Option<Uuid>,
    ) -> Result<(), Self::Err>;
}

/// Mint Proof Database trait
#[async_trait]
pub trait ProofsDatabase {
    /// Mint Proof Database Error
    type Err: Into<Error> + From<Error>;

    /// Get [`Proofs`] by ys
    async fn get_proofs_by_ys(&self, ys: &[PublicKey]) -> Result<Vec<Option<Proof>>, Self::Err>;
    /// Get ys by quote id
    async fn get_proof_ys_by_quote_id(&self, quote_id: &Uuid) -> Result<Vec<PublicKey>, Self::Err>;
    /// Get [`Proofs`] state
    async fn get_proofs_states(&self, ys: &[PublicKey]) -> Result<Vec<Option<State>>, Self::Err>;
    /// Get [`Proofs`] by state
    async fn get_proofs_by_keyset_id(
        &self,
        keyset_id: &Id,
    ) -> Result<(Proofs, Vec<Option<State>>), Self::Err>;
}

#[async_trait]
/// Mint Signatures Transaction trait
pub trait SignaturesTransaction<'a> {
    /// Mint Signature Database Error
    type Err: Into<Error> + From<Error>;

    /// Add [`BlindSignature`]
    async fn add_blind_signatures(
        &mut self,
        blinded_messages: &[PublicKey],
        blind_signatures: &[BlindSignature],
        quote_id: Option<QuoteId>,
    ) -> Result<(), Self::Err>;

    /// Get [`BlindSignature`]s
    async fn get_blind_signatures(
        &mut self,
        blinded_messages: &[PublicKey],
    ) -> Result<Vec<Option<BlindSignature>>, Self::Err>;
}

#[async_trait]
/// Mint Signatures Database trait
pub trait SignaturesDatabase {
    /// Mint Signature Database Error
    type Err: Into<Error> + From<Error>;

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
        quote_id: &QuoteId,
    ) -> Result<Vec<BlindSignature>, Self::Err>;
}

#[async_trait]
/// Commit and Rollback
pub trait DbTransactionFinalizer {
    /// Mint Signature Database Error
    type Err: Into<Error> + From<Error>;

    /// Commits all the changes into the database
    async fn commit(self: Box<Self>) -> Result<(), Self::Err>;

    /// Rollbacks the write transaction
    async fn rollback(self: Box<Self>) -> Result<(), Self::Err>;
}

/// Key-Value Store Transaction trait
#[async_trait]
pub trait KVStoreTransaction<'a, Error>: DbTransactionFinalizer<Err = Error> {
    /// Read value from key-value store
    async fn kv_read(
        &mut self,
        primary_namespace: &str,
        secondary_namespace: &str,
        key: &str,
    ) -> Result<Option<Vec<u8>>, Error>;

    /// Write value to key-value store
    async fn kv_write(
        &mut self,
        primary_namespace: &str,
        secondary_namespace: &str,
        key: &str,
        value: &[u8],
    ) -> Result<(), Error>;

    /// Remove value from key-value store
    async fn kv_remove(
        &mut self,
        primary_namespace: &str,
        secondary_namespace: &str,
        key: &str,
    ) -> Result<(), Error>;

    /// List keys in a namespace
    async fn kv_list(
        &mut self,
        primary_namespace: &str,
        secondary_namespace: &str,
    ) -> Result<Vec<String>, Error>;
}

/// Base database writer
#[async_trait]
pub trait Transaction<'a, Error>:
    DbTransactionFinalizer<Err = Error>
    + QuotesTransaction<'a, Err = Error>
    + SignaturesTransaction<'a, Err = Error>
    + ProofsTransaction<'a, Err = Error>
    + KVStoreTransaction<'a, Error>
{
    /// Set [`QuoteTTL`]
    async fn set_quote_ttl(&mut self, quote_ttl: QuoteTTL) -> Result<(), Error>;

    /// Set [`MintInfo`]
    async fn set_mint_info(&mut self, mint_info: MintInfo) -> Result<(), Error>;
}

/// Key-Value Store Database trait
#[async_trait]
pub trait KVStoreDatabase {
    /// KV Store Database Error
    type Err: Into<Error> + From<Error>;

    /// Read value from key-value store
    async fn kv_read(
        &self,
        primary_namespace: &str,
        secondary_namespace: &str,
        key: &str,
    ) -> Result<Option<Vec<u8>>, Self::Err>;

    /// List keys in a namespace
    async fn kv_list(
        &self,
        primary_namespace: &str,
        secondary_namespace: &str,
    ) -> Result<Vec<String>, Self::Err>;
}

/// Key-Value Store Database trait
#[async_trait]
pub trait KVStore: KVStoreDatabase {
    /// Beings a KV transaction
    async fn begin_transaction<'a>(
        &'a self,
    ) -> Result<Box<dyn KVStoreTransaction<'a, Self::Err> + Send + Sync + 'a>, Error>;
}

/// Mint Database trait
#[async_trait]
pub trait Database<Error>:
    KVStoreDatabase<Err = Error>
    + QuotesDatabase<Err = Error>
    + ProofsDatabase<Err = Error>
    + SignaturesDatabase<Err = Error>
{
    /// Beings a transaction
    async fn begin_transaction<'a>(
        &'a self,
    ) -> Result<Box<dyn Transaction<'a, Error> + Send + Sync + 'a>, Error>;

    /// Get [`MintInfo`]
    async fn get_mint_info(&self) -> Result<MintInfo, Error>;

    /// Get [`QuoteTTL`]
    async fn get_quote_ttl(&self) -> Result<QuoteTTL, Error>;
}
