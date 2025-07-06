//! CDK Database

use std::collections::HashMap;

use async_trait::async_trait;
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
    async fn get_mint_quote(&mut self, quote_id: &Uuid)
        -> Result<Option<MintMintQuote>, Self::Err>;
    /// Add [`MintMintQuote`]
    async fn add_mint_quote(&mut self, quote: MintMintQuote) -> Result<(), Self::Err>;
    /// Increment amount paid [`MintMintQuote`]
    async fn increment_mint_quote_amount_paid(
        &mut self,
        quote_id: &Uuid,
        amount_paid: Amount,
        payment_id: String,
    ) -> Result<Amount, Self::Err>;
    /// Increment amount paid [`MintMintQuote`]
    async fn increment_mint_quote_amount_issued(
        &mut self,
        quote_id: &Uuid,
        amount_issued: Amount,
    ) -> Result<Amount, Self::Err>;
    /// Remove [`MintMintQuote`]
    async fn remove_mint_quote(&mut self, quote_id: &Uuid) -> Result<(), Self::Err>;
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
        quote_id: &Uuid,
        new_request_lookup_id: &PaymentIdentifier,
    ) -> Result<(), Self::Err>;

    /// Update [`mint::MeltQuote`] state
    ///
    /// It is expected for this function to fail if the state is already set to the new state
    async fn update_melt_quote_state(
        &mut self,
        quote_id: &Uuid,
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
    async fn get_mint_quote(&self, quote_id: &Uuid) -> Result<Option<MintMintQuote>, Self::Err>;

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
    async fn get_melt_quote(&self, quote_id: &Uuid) -> Result<Option<mint::MeltQuote>, Self::Err>;
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
        quote_id: Option<Uuid>,
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
        quote_id: &Uuid,
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

/// Base database writer
#[async_trait]
pub trait Transaction<'a, Error>:
    DbTransactionFinalizer<Err = Error>
    + QuotesTransaction<'a, Err = Error>
    + SignaturesTransaction<'a, Err = Error>
    + ProofsTransaction<'a, Err = Error>
{
    /// Set [`QuoteTTL`]
    async fn set_quote_ttl(&mut self, quote_ttl: QuoteTTL) -> Result<(), Error>;

    /// Set [`MintInfo`]
    async fn set_mint_info(&mut self, mint_info: MintInfo) -> Result<(), Error>;
}

/// Mint Database trait
#[async_trait]
pub trait Database<Error>:
    QuotesDatabase<Err = Error> + ProofsDatabase<Err = Error> + SignaturesDatabase<Err = Error>
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
