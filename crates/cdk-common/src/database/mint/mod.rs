//! CDK Database

#[cfg(feature = "federation")]
use std::collections::BTreeMap;
use std::collections::HashMap;
use std::ops::{Deref, DerefMut};

use async_trait::async_trait;
use cashu::quote_id::QuoteId;
use cashu::Amount;

use super::{DbTransactionFinalizer, Error};
#[cfg(feature = "federation")]
use crate::federation::{
    FederationAlephBftFinalizedItem, FederationAlephBftNetworkRetryRecord,
    FederationAlephBftPendingOperationRecord, FederationConfigSnapshot,
    FederationDkgMemberKeysetShare, FederationDkgParticipantCommitment,
    FederationDkgParticipantCommitmentReveal, FederationDkgParticipantState,
    FederationDkgSecretShareRequest, FederationDkgTranscriptSignatureRequest, FederationId,
    FederationJournalEntry, FederationJournalTip, FederationMemberStatusSnapshot,
    FederationOperationId, FederationOperationStatus, FederationPaymentObservationContext,
    FederationPaymentObservationRecord, FederationSessionOutcome, FederationSignatureShareRecord,
    FederationStateDigestCheckpoint, MemberId,
};
use crate::mint::{
    self, MeltQuote, MintKeySetInfo, MintQuote as MintMintQuote, Operation, ProofsWithState,
};
use crate::nuts::{
    BlindSignature, BlindedMessage, CurrencyUnit, Id, MeltQuoteState, Proof, Proofs, PublicKey,
    State,
};
use crate::payment::PaymentIdentifier;

mod auth;

#[cfg(feature = "test")]
pub mod test;

pub use auth::{DynMintAuthDatabase, MintAuthDatabase, MintAuthTransaction};

// Re-export KVStore types from shared module for backward compatibility
pub use super::kvstore::{
    validate_kvstore_params, validate_kvstore_string, KVStore, KVStoreDatabase, KVStoreTransaction,
    KVSTORE_NAMESPACE_KEY_ALPHABET, KVSTORE_NAMESPACE_KEY_MAX_LEN,
};

/// A wrapper indicating that a resource has been acquired with a database lock.
///
/// This type is returned by database operations that lock rows for update
/// (e.g., `SELECT ... FOR UPDATE`). It serves as a compile-time marker that
/// the wrapped resource was properly locked before being returned, ensuring
/// that subsequent modifications are safe from race conditions.
///
/// # Usage
///
/// When you need to modify a database record, first acquire it using a locking
/// query method. The returned `Acquired<T>` guarantees the row is locked for
/// the duration of the transaction.
///
/// ```ignore
/// // Acquire a quote with a row lock
/// let mut quote: Acquired<MintQuote> = tx.get_mint_quote_for_update(&quote_id).await?;
///
/// // Safely modify the quote (row is locked)
/// quote.state = QuoteState::Paid;
///
/// // Persist the changes
/// tx.update_mint_quote(&mut quote).await?;
/// ```
///
/// # Deref Behavior
///
/// `Acquired<T>` implements `Deref` and `DerefMut`, allowing transparent access
/// to the inner value's methods and fields.
#[derive(Debug)]
pub struct Acquired<T> {
    inner: T,
}

impl<T> From<T> for Acquired<T> {
    /// Wraps a value to indicate it has been acquired with a lock.
    ///
    /// This is typically called by database layer implementations after
    /// executing a locking query.
    fn from(value: T) -> Self {
        Acquired { inner: value }
    }
}

impl<T> Acquired<T> {
    /// Consumes the wrapper and returns the inner resource.
    ///
    /// Use this when you need to take ownership of the inner value,
    /// for example when passing it to a function that doesn't accept
    /// `Acquired<T>`.
    pub fn inner(self) -> T {
        self.inner
    }
}

impl<T> Deref for Acquired<T> {
    type Target = T;

    /// Returns a reference to the inner resource.
    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

impl<T> DerefMut for Acquired<T> {
    /// Returns a mutable reference to the inner resource.
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.inner
    }
}

/// Information about a melt request stored in the database
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MeltRequestInfo {
    /// Total amount of all input proofs in the melt request
    pub inputs_amount: Amount<CurrencyUnit>,
    /// Fee amount associated with the input proofs
    pub inputs_fee: Amount<CurrencyUnit>,
    /// Blinded messages for change outputs
    pub change_outputs: Vec<BlindedMessage>,
}

/// Result of locking a melt quote and all related quotes atomically.
///
/// This struct is returned by [`QuotesTransaction::lock_melt_quote_and_related`]
/// and contains both the target quote and all quotes sharing the same `request_lookup_id`.
#[derive(Debug)]
pub struct LockedMeltQuotes {
    /// The target quote that was requested, if found
    pub target: Option<Acquired<MeltQuote>>,
    /// All quotes sharing the same `request_lookup_id` (including the target)
    pub all_related: Vec<Acquired<MeltQuote>>,
}

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

    /// Begins a transaction
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
pub trait QuotesTransaction {
    /// Mint Quotes Database Error
    type Err: Into<Error> + From<Error>;

    /// Add melt_request with quote_id, inputs_amount, and inputs_fee
    async fn add_melt_request(
        &mut self,
        quote_id: &QuoteId,
        inputs_amount: Amount<CurrencyUnit>,
        inputs_fee: Amount<CurrencyUnit>,
    ) -> Result<(), Self::Err>;

    /// Add blinded_messages for a quote_id
    async fn add_blinded_messages(
        &mut self,
        quote_id: Option<&QuoteId>,
        blinded_messages: &[BlindedMessage],
        operation: &Operation,
    ) -> Result<(), Self::Err>;

    /// Delete blinded_messages by their blinded secrets
    async fn delete_blinded_messages(
        &mut self,
        blinded_secrets: &[PublicKey],
    ) -> Result<(), Self::Err>;

    /// Get melt_request and associated blinded_messages by quote_id
    async fn get_melt_request_and_blinded_messages(
        &mut self,
        quote_id: &QuoteId,
    ) -> Result<Option<MeltRequestInfo>, Self::Err>;

    /// Delete melt_request and associated blinded_messages by quote_id
    async fn delete_melt_request(&mut self, quote_id: &QuoteId) -> Result<(), Self::Err>;

    /// Get [`MintMintQuote`] and lock it for update in this transaction
    async fn get_mint_quote(
        &mut self,
        quote_id: &QuoteId,
    ) -> Result<Option<Acquired<MintMintQuote>>, Self::Err>;

    /// Get multiple [`MintMintQuote`]s by their IDs and lock them for update in this transaction.
    ///
    /// Returns results in the same order as the input IDs, with `None` for any IDs not found.
    /// This method locks all found quotes to prevent race conditions during concurrent modifications.
    async fn get_mint_quotes_by_ids(
        &mut self,
        quote_ids: &[QuoteId],
    ) -> Result<Vec<Option<Acquired<MintMintQuote>>>, Self::Err>;

    /// Add [`MintMintQuote`]
    async fn add_mint_quote(
        &mut self,
        quote: MintMintQuote,
    ) -> Result<Acquired<MintMintQuote>, Self::Err>;

    /// Persists any pending changes made to the mint quote.
    ///
    /// This method extracts changes accumulated in the quote (via [`mint::MintQuote::take_changes`])
    /// and persists them to the database. Changes may include new payments received or new
    /// issuances recorded against the quote.
    ///
    /// If no changes are pending, this method returns successfully without performing
    /// any database operations.
    ///
    /// # Arguments
    ///
    /// * `quote` - A mutable reference to an acquired (row-locked) mint quote. The quote
    ///   must be locked to ensure transactional consistency when persisting changes.
    ///
    /// # Implementation Notes
    ///
    /// Implementations should call [`mint::MintQuote::take_changes`] to retrieve pending
    /// changes, then persist each payment and issuance record, and finally update the
    /// quote's aggregate counters (`amount_paid`, `amount_issued`) in the database.
    async fn update_mint_quote(
        &mut self,
        quote: &mut Acquired<mint::MintQuote>,
    ) -> Result<(), Self::Err>;

    /// Get [`mint::MeltQuote`] and lock it for update in this transaction
    async fn get_melt_quote(
        &mut self,
        quote_id: &QuoteId,
    ) -> Result<Option<Acquired<mint::MeltQuote>>, Self::Err>;

    /// Add [`mint::MeltQuote`]
    async fn add_melt_quote(&mut self, quote: mint::MeltQuote) -> Result<(), Self::Err>;

    /// Retrieves all melt quotes matching a payment lookup identifier and locks them for update.
    ///
    /// This method returns multiple quotes because certain payment methods (notably BOLT12 offers)
    /// can generate multiple payment attempts that share the same lookup identifier. Locking all
    /// related quotes prevents race conditions where concurrent melt operations could interfere
    /// with each other, potentially leading to double-spending or state inconsistencies.
    ///
    /// The returned quotes are locked within the current transaction to ensure safe concurrent
    /// modification. This is essential during melt saga initiation and finalization to guarantee
    /// atomic state transitions across all related quotes.
    ///
    /// # Arguments
    ///
    /// * `request_lookup_id` - The payment identifier used by the Lightning backend to track
    ///   payment state (e.g., payment hash, offer ID, or label).
    async fn get_melt_quotes_by_request_lookup_id(
        &mut self,
        request_lookup_id: &PaymentIdentifier,
    ) -> Result<Vec<Acquired<MeltQuote>>, Self::Err>;

    /// Locks a melt quote and all related quotes sharing the same request_lookup_id atomically.
    ///
    /// This method prevents deadlocks by acquiring all locks in a single query with consistent
    /// ordering, rather than locking the target quote first and then related quotes separately.
    ///
    /// # Deadlock Prevention
    ///
    /// When multiple transactions try to melt quotes sharing the same `request_lookup_id`,
    /// acquiring locks in two steps (first the target quote, then all related quotes) can cause
    /// circular wait deadlocks. This method avoids that by:
    /// 1. Using a subquery to find the `request_lookup_id` for the target quote
    /// 2. Locking ALL quotes with that `request_lookup_id` in one atomic operation
    /// 3. Ordering locks consistently by quote ID
    ///
    /// # Arguments
    ///
    /// * `quote_id` - The ID of the target melt quote
    ///
    /// # Returns
    ///
    /// A [`LockedMeltQuotes`] containing:
    /// - `target`: The target quote (if found)
    /// - `all_related`: All quotes sharing the same `request_lookup_id` (including the target)
    ///
    /// If the quote has no `request_lookup_id`, only the target quote is returned and locked.
    async fn lock_melt_quote_and_related(
        &mut self,
        quote_id: &QuoteId,
    ) -> Result<LockedMeltQuotes, Self::Err>;

    /// Updates the request lookup id for a melt quote.
    ///
    /// Requires an [`Acquired`] melt quote to ensure the row is locked before modification.
    async fn update_melt_quote_request_lookup_id(
        &mut self,
        quote: &mut Acquired<mint::MeltQuote>,
        new_request_lookup_id: &PaymentIdentifier,
    ) -> Result<(), Self::Err>;

    /// Update [`mint::MeltQuote`] state.
    ///
    /// Requires an [`Acquired`] melt quote to ensure the row is locked before modification.
    /// Returns the previous state.
    async fn update_melt_quote_state(
        &mut self,
        quote: &mut Acquired<mint::MeltQuote>,
        new_state: MeltQuoteState,
        payment_proof: Option<String>,
    ) -> Result<MeltQuoteState, Self::Err>;

    /// Get all [`MintMintQuote`]s and lock it for update in this transaction
    async fn get_mint_quote_by_request(
        &mut self,
        request: &str,
    ) -> Result<Option<Acquired<MintMintQuote>>, Self::Err>;

    /// Get all [`MintMintQuote`]s
    async fn get_mint_quote_by_request_lookup_id(
        &mut self,
        request_lookup_id: &PaymentIdentifier,
    ) -> Result<Option<Acquired<MintMintQuote>>, Self::Err>;
}

/// Mint Quote Database trait
#[async_trait]
pub trait QuotesDatabase {
    /// Mint Quotes Database Error
    type Err: Into<Error> + From<Error>;

    /// Get [`MintMintQuote`]
    async fn get_mint_quote(&self, quote_id: &QuoteId) -> Result<Option<MintMintQuote>, Self::Err>;

    /// Get multiple [`MintMintQuote`]s by their IDs.
    ///
    /// Returns results in the same order as the input IDs, with `None` for any IDs not found.
    async fn get_mint_quotes_by_ids(
        &self,
        quote_ids: &[QuoteId],
    ) -> Result<Vec<Option<MintMintQuote>>, Self::Err>;

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
pub trait ProofsTransaction {
    /// Mint Proof Database Error
    type Err: Into<Error> + From<Error>;

    /// Add  [`Proofs`]
    ///
    /// Adds proofs to the database. The database should error if the proof already exits, with a
    /// `AttemptUpdateSpentProof` if the proof is already spent or a `Duplicate` error otherwise.
    async fn add_proofs(
        &mut self,
        proof: Proofs,
        quote_id: Option<QuoteId>,
        operation: &Operation,
    ) -> Result<Acquired<ProofsWithState>, Self::Err>;

    /// Updates the proofs to the given state in the database.
    ///
    /// Also updates the `state` field on the [`ProofsWithState`] wrapper to reflect
    /// the new state after the database update succeeds.
    async fn update_proofs_state(
        &mut self,
        proofs: &mut Acquired<ProofsWithState>,
        new_state: State,
    ) -> Result<(), Self::Err>;

    /// get proofs states
    async fn get_proofs(
        &mut self,
        ys: &[PublicKey],
    ) -> Result<Acquired<ProofsWithState>, Self::Err>;

    /// Remove [`Proofs`]
    async fn remove_proofs(
        &mut self,
        ys: &[PublicKey],
        quote_id: Option<QuoteId>,
    ) -> Result<(), Self::Err>;

    /// Get ys by quote id
    async fn get_proof_ys_by_quote_id(
        &mut self,
        quote_id: &QuoteId,
    ) -> Result<Vec<PublicKey>, Self::Err>;

    /// Get proof ys by operation id
    async fn get_proof_ys_by_operation_id(
        &mut self,
        operation_id: &uuid::Uuid,
    ) -> Result<Vec<PublicKey>, Self::Err>;
}

/// Mint Proof Database trait
#[async_trait]
pub trait ProofsDatabase {
    /// Mint Proof Database Error
    type Err: Into<Error> + From<Error>;

    /// Get [`Proofs`] by ys
    async fn get_proofs_by_ys(&self, ys: &[PublicKey]) -> Result<Vec<Option<Proof>>, Self::Err>;
    /// Get ys by quote id
    async fn get_proof_ys_by_quote_id(
        &self,
        quote_id: &QuoteId,
    ) -> Result<Vec<PublicKey>, Self::Err>;
    /// Get [`Proofs`] state
    async fn get_proofs_states(&self, ys: &[PublicKey]) -> Result<Vec<Option<State>>, Self::Err>;

    /// Get [`Proofs`] by state
    async fn get_proofs_by_keyset_id(
        &self,
        keyset_id: &Id,
    ) -> Result<(Proofs, Vec<Option<State>>), Self::Err>;

    /// Get total proofs redeemed by keyset id
    async fn get_total_redeemed(&self) -> Result<HashMap<Id, Amount>, Self::Err>;

    /// Get proof ys by operation id
    async fn get_proof_ys_by_operation_id(
        &self,
        operation_id: &uuid::Uuid,
    ) -> Result<Vec<PublicKey>, Self::Err>;
}

#[async_trait]
/// Mint Signatures Transaction trait
pub trait SignaturesTransaction {
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

    /// Get blinded secrets (B values) signed for quote
    async fn get_blinded_secrets_for_quote(
        &self,
        quote_id: &QuoteId,
    ) -> Result<Vec<PublicKey>, Self::Err>;

    /// Get total amount issued by keyset id
    async fn get_total_issued(&self) -> Result<HashMap<Id, Amount>, Self::Err>;

    /// Get blinded secrets (B values) by operation id
    async fn get_blinded_secrets_by_operation_id(
        &self,
        operation_id: &uuid::Uuid,
    ) -> Result<Vec<PublicKey>, Self::Err>;
}

#[async_trait]
/// Saga Transaction trait
pub trait SagaTransaction {
    /// Saga Database Error
    type Err: Into<Error> + From<Error>;

    /// Get saga by operation_id
    async fn get_saga(
        &mut self,
        operation_id: &uuid::Uuid,
    ) -> Result<Option<mint::Saga>, Self::Err>;

    /// Add saga
    async fn add_saga(&mut self, saga: &mint::Saga) -> Result<(), Self::Err>;

    /// Update saga state (only updates state and updated_at fields)
    async fn update_saga(
        &mut self,
        operation_id: &uuid::Uuid,
        new_state: mint::SagaStateEnum,
    ) -> Result<(), Self::Err>;

    /// Update saga state and optional finalization metadata.
    async fn update_saga_with_finalization_data(
        &mut self,
        operation_id: &uuid::Uuid,
        new_state: mint::SagaStateEnum,
        finalization_data: Option<&mint::MeltFinalizationData>,
    ) -> Result<(), Self::Err>;

    /// Delete saga
    async fn delete_saga(&mut self, operation_id: &uuid::Uuid) -> Result<(), Self::Err>;
}

#[async_trait]
/// Saga Database trait
pub trait SagaDatabase {
    /// Saga Database Error
    type Err: Into<Error> + From<Error>;

    /// Get the melt saga associated with a melt quote id
    async fn get_melt_saga_by_quote_id(
        &self,
        quote_id: &QuoteId,
    ) -> Result<Option<mint::Saga>, Self::Err>;

    /// Get all incomplete sagas for a given operation kind
    async fn get_incomplete_sagas(
        &self,
        operation_kind: mint::OperationKind,
    ) -> Result<Vec<mint::Saga>, Self::Err>;
}

#[async_trait]
/// Completed Operations Transaction trait
pub trait CompletedOperationsTransaction {
    /// Completed Operations Database Error
    type Err: Into<Error> + From<Error>;

    /// Add completed operation
    async fn add_completed_operation(
        &mut self,
        operation: &mint::Operation,
        fee_by_keyset: &std::collections::HashMap<crate::nuts::Id, crate::Amount>,
    ) -> Result<(), Self::Err>;
}

#[async_trait]
/// Completed Operations Database trait
pub trait CompletedOperationsDatabase {
    /// Completed Operations Database Error
    type Err: Into<Error> + From<Error>;

    /// Get completed operation by operation_id
    async fn get_completed_operation(
        &self,
        operation_id: &uuid::Uuid,
    ) -> Result<Option<mint::Operation>, Self::Err>;

    /// Get completed operations by operation kind
    async fn get_completed_operations_by_kind(
        &self,
        operation_kind: mint::OperationKind,
    ) -> Result<Vec<mint::Operation>, Self::Err>;

    /// Get all completed operations
    async fn get_completed_operations(&self) -> Result<Vec<mint::Operation>, Self::Err>;
}

#[cfg(feature = "federation")]
#[async_trait]
/// Federation journal transaction trait
pub trait FederationJournalTransaction {
    /// Federation journal database error
    type Err: Into<Error> + From<Error>;

    /// Persist the public federation config snapshot for this local database.
    async fn set_federation_config(
        &mut self,
        config: &FederationConfigSnapshot,
    ) -> Result<(), Self::Err>;

    /// Persist one accepted federation journal entry.
    async fn add_federation_journal_entry(
        &mut self,
        entry: &FederationJournalEntry,
    ) -> Result<(), Self::Err>;

    /// Update the local application status for one accepted federation operation.
    async fn update_federation_operation_status(
        &mut self,
        operation_id: FederationOperationId,
        status: FederationOperationStatus,
    ) -> Result<(), Self::Err>;

    /// Atomically persist a deterministic application rejection and rejected status.
    async fn reject_federation_operation(
        &mut self,
        operation_id: FederationOperationId,
        rejection: &crate::error::ErrorResponse,
    ) -> Result<(), Self::Err>;

    /// Persist the highest accepted private REST sequence for one federation member.
    async fn set_federation_member_sequence(
        &mut self,
        member_id: MemberId,
        sequence: u64,
    ) -> Result<(), Self::Err>;

    /// Persist the latest local sync status snapshot for one federation member.
    async fn set_federation_member_status(
        &mut self,
        status: &FederationMemberStatusSnapshot,
    ) -> Result<(), Self::Err>;

    /// Persist one signed federation session outcome.
    async fn add_federation_session_outcome(
        &mut self,
        outcome: &FederationSessionOutcome,
    ) -> Result<(), Self::Err>;

    /// Persist one finalized AlephBFT consensus item.
    async fn add_federation_aleph_bft_finalized_item(
        &mut self,
        item: &FederationAlephBftFinalizedItem,
    ) -> Result<(), Self::Err>;

    /// Persist finalized AlephBFT consensus items.
    async fn add_federation_aleph_bft_finalized_items(
        &mut self,
        items: &[FederationAlephBftFinalizedItem],
    ) -> Result<(), Self::Err> {
        for item in items {
            self.add_federation_aleph_bft_finalized_item(item).await?;
        }

        Ok(())
    }

    /// Append raw AlephBFT backup bytes for one local session.
    async fn append_federation_aleph_bft_backup_bytes(
        &mut self,
        session: u64,
        bytes: &[u8],
    ) -> Result<(), Self::Err>;

    /// Persist the latest active AlephBFT session marker.
    async fn set_federation_aleph_bft_active_session(
        &mut self,
        session: u64,
    ) -> Result<(), Self::Err>;

    /// Persist one local AlephBFT operation admitted to the pending mempool.
    async fn add_federation_aleph_bft_pending_operation(
        &mut self,
        operation: &FederationAlephBftPendingOperationRecord,
    ) -> Result<(), Self::Err>;

    /// Remove one local AlephBFT pending operation after it is decided.
    async fn remove_federation_aleph_bft_pending_operation(
        &mut self,
        operation_id: FederationOperationId,
    ) -> Result<(), Self::Err>;

    /// Persist one materialized state digest checkpoint.
    async fn add_federation_state_digest_checkpoint(
        &mut self,
        checkpoint: &FederationStateDigestCheckpoint,
    ) -> Result<(), Self::Err>;

    /// Retain only the newest materialized state digest checkpoints.
    async fn prune_federation_state_digest_checkpoints(
        &mut self,
        keep_latest: usize,
    ) -> Result<(), Self::Err>;

    /// Persist one payment observation extracted from an accepted federation operation.
    async fn add_federation_payment_observation(
        &mut self,
        observation: &FederationPaymentObservationRecord,
    ) -> Result<(), Self::Err>;

    /// Persist one local federation threshold signature share.
    async fn add_federation_signature_share(
        &mut self,
        share: &FederationSignatureShareRecord,
    ) -> Result<(), Self::Err>;

    /// Persist local federation threshold signature shares.
    async fn add_federation_signature_shares(
        &mut self,
        shares: &[FederationSignatureShareRecord],
    ) -> Result<(), Self::Err> {
        for share in shares {
            self.add_federation_signature_share(share).await?;
        }

        Ok(())
    }

    /// Persist one DKG pre-reveal commitment.
    async fn add_federation_dkg_commitment(
        &mut self,
        commitment: &FederationDkgParticipantCommitment,
    ) -> Result<(), Self::Err>;

    /// Persist one DKG public commitment reveal.
    async fn add_federation_dkg_reveal(
        &mut self,
        reveal: &FederationDkgParticipantCommitmentReveal,
    ) -> Result<(), Self::Err>;

    /// Persist one private DKG secret-share contribution.
    async fn add_federation_dkg_secret_share_contribution(
        &mut self,
        contribution: &FederationDkgSecretShareRequest,
    ) -> Result<(), Self::Err>;

    /// Persist one local restartable DKG participant state.
    async fn add_federation_dkg_participant_state(
        &mut self,
        state: &FederationDkgParticipantState,
    ) -> Result<(), Self::Err>;

    /// Persist one completed local member DKG keyset share.
    async fn add_federation_dkg_member_keyset_share(
        &mut self,
        share: &FederationDkgMemberKeysetShare,
    ) -> Result<(), Self::Err>;

    /// Persist one member signature over a DKG/setup transcript hash.
    async fn add_federation_dkg_transcript_signature(
        &mut self,
        signature: &FederationDkgTranscriptSignatureRequest,
    ) -> Result<(), Self::Err>;

    /// Get payment observations for one quote and context inside this transaction.
    async fn get_federation_payment_observations(
        &mut self,
        context: FederationPaymentObservationContext,
        quote: &str,
    ) -> Result<Vec<FederationPaymentObservationRecord>, Self::Err>;
}

#[cfg(feature = "federation")]
#[async_trait]
/// Federation journal database trait
pub trait FederationJournalDatabase {
    /// Federation journal database error
    type Err: Into<Error> + From<Error>;

    /// Get the persisted public federation config snapshot.
    async fn get_federation_config(
        &self,
        federation_id: FederationId,
    ) -> Result<Option<FederationConfigSnapshot>, Self::Err>;

    /// Get all accepted federation journal entries in consensus order.
    async fn get_federation_journal_entries(
        &self,
    ) -> Result<Vec<FederationJournalEntry>, Self::Err>;

    /// Get scalar metadata for the latest accepted federation journal entry.
    async fn get_federation_journal_tip(&self) -> Result<Option<FederationJournalTip>, Self::Err>;

    /// Get one accepted federation journal entry by operation ID.
    async fn get_federation_journal_entry_by_operation_id(
        &self,
        operation_id: FederationOperationId,
    ) -> Result<Option<FederationJournalEntry>, Self::Err>;

    /// Get accepted federation journal entries starting at `next_index`.
    async fn get_federation_journal_entries_from(
        &self,
        next_index: u64,
    ) -> Result<Vec<FederationJournalEntry>, Self::Err>;

    /// Get persisted private REST sequence watermarks by member.
    async fn get_federation_member_sequences(&self) -> Result<BTreeMap<MemberId, u64>, Self::Err>;

    /// Get the latest local sync status snapshot for one federation member.
    async fn get_federation_member_status(
        &self,
        member_id: MemberId,
    ) -> Result<Option<FederationMemberStatusSnapshot>, Self::Err>;

    /// Get all persisted signed federation session outcomes.
    async fn get_federation_session_outcomes(
        &self,
    ) -> Result<Vec<FederationSessionOutcome>, Self::Err>;

    /// Get persisted signed federation session outcomes starting at `from_index`.
    async fn get_federation_session_outcomes_from(
        &self,
        from_index: u64,
    ) -> Result<Vec<FederationSessionOutcome>, Self::Err>;

    /// Get all persisted AlephBFT finalized items in accepted item order.
    async fn get_federation_aleph_bft_finalized_items(
        &self,
    ) -> Result<Vec<FederationAlephBftFinalizedItem>, Self::Err>;

    /// Get persisted AlephBFT finalized items starting at `accepted_item_index`.
    async fn get_federation_aleph_bft_finalized_items_from(
        &self,
        accepted_item_index: u64,
    ) -> Result<Vec<FederationAlephBftFinalizedItem>, Self::Err>;

    /// Get raw AlephBFT backup bytes for one local session.
    async fn get_federation_aleph_bft_backup_bytes(
        &self,
        session: u64,
    ) -> Result<Option<Vec<u8>>, Self::Err>;

    /// Get the latest active AlephBFT session marker.
    async fn get_federation_aleph_bft_active_session(&self) -> Result<Option<u64>, Self::Err>;

    /// Get local AlephBFT pending operations for one session.
    async fn get_federation_aleph_bft_pending_operations(
        &self,
        session: u64,
    ) -> Result<Vec<FederationAlephBftPendingOperationRecord>, Self::Err>;

    /// Persist one outbound AlephBFT network-message retry row.
    async fn add_federation_aleph_bft_network_retry(
        &self,
        retry: &FederationAlephBftNetworkRetryRecord,
    ) -> Result<(), Self::Err>;

    /// Remove one outbound AlephBFT network-message retry row.
    async fn remove_federation_aleph_bft_network_retry(
        &self,
        retry_id: &str,
    ) -> Result<(), Self::Err>;

    /// Get outbound AlephBFT network-message retries for one session.
    async fn get_federation_aleph_bft_network_retries(
        &self,
        session: u64,
    ) -> Result<Vec<FederationAlephBftNetworkRetryRecord>, Self::Err>;

    /// Get all persisted materialized state digest checkpoints in consensus order.
    async fn get_federation_state_digest_checkpoints(
        &self,
    ) -> Result<Vec<FederationStateDigestCheckpoint>, Self::Err>;

    /// Get persisted materialized state digest checkpoints starting at `next_index`.
    async fn get_federation_state_digest_checkpoints_from(
        &self,
        next_index: u64,
    ) -> Result<Vec<FederationStateDigestCheckpoint>, Self::Err>;

    /// Get payment observations for one quote and context in consensus order.
    async fn get_federation_payment_observations(
        &self,
        context: FederationPaymentObservationContext,
        quote: &str,
    ) -> Result<Vec<FederationPaymentObservationRecord>, Self::Err>;

    /// Get signature shares for one accepted operation output.
    async fn get_federation_signature_shares_for_output(
        &self,
        operation_id: FederationOperationId,
        output_index: u32,
    ) -> Result<Vec<FederationSignatureShareRecord>, Self::Err>;

    /// Get signature shares for one blinded message.
    async fn get_federation_signature_shares_for_blinded_message(
        &self,
        blinded_message: PublicKey,
    ) -> Result<Vec<FederationSignatureShareRecord>, Self::Err>;

    /// Get all persisted DKG pre-reveal commitments.
    async fn get_federation_dkg_commitments(
        &self,
    ) -> Result<Vec<FederationDkgParticipantCommitment>, Self::Err>;

    /// Get all persisted DKG public commitment reveals.
    async fn get_federation_dkg_reveals(
        &self,
    ) -> Result<Vec<FederationDkgParticipantCommitmentReveal>, Self::Err>;

    /// Get all persisted private DKG secret-share contributions.
    async fn get_federation_dkg_secret_share_contributions(
        &self,
    ) -> Result<Vec<FederationDkgSecretShareRequest>, Self::Err>;

    /// Get all persisted local restartable DKG participant states.
    async fn get_federation_dkg_participant_states(
        &self,
    ) -> Result<Vec<FederationDkgParticipantState>, Self::Err>;

    /// Get all persisted completed local member DKG keyset shares.
    async fn get_federation_dkg_member_keyset_shares(
        &self,
    ) -> Result<Vec<FederationDkgMemberKeysetShare>, Self::Err>;

    /// Get all persisted member signatures over DKG/setup transcript hashes.
    async fn get_federation_dkg_transcript_signatures(
        &self,
    ) -> Result<Vec<FederationDkgTranscriptSignatureRequest>, Self::Err>;
}

#[cfg(not(feature = "federation"))]
/// Base database writer
pub trait Transaction<Error>:
    DbTransactionFinalizer<Err = Error>
    + QuotesTransaction<Err = Error>
    + SignaturesTransaction<Err = Error>
    + ProofsTransaction<Err = Error>
    + KVStoreTransaction<Error>
    + SagaTransaction<Err = Error>
    + CompletedOperationsTransaction<Err = Error>
{
}

#[cfg(feature = "federation")]
/// Base database writer
pub trait Transaction<Error>:
    DbTransactionFinalizer<Err = Error>
    + QuotesTransaction<Err = Error>
    + SignaturesTransaction<Err = Error>
    + ProofsTransaction<Err = Error>
    + KVStoreTransaction<Error>
    + SagaTransaction<Err = Error>
    + CompletedOperationsTransaction<Err = Error>
    + FederationJournalTransaction<Err = Error>
{
}

#[cfg(not(feature = "federation"))]
/// Mint Database trait
#[async_trait]
pub trait Database<Error>:
    KVStoreDatabase<Err = Error>
    + QuotesDatabase<Err = Error>
    + ProofsDatabase<Err = Error>
    + SignaturesDatabase<Err = Error>
    + SagaDatabase<Err = Error>
    + CompletedOperationsDatabase<Err = Error>
{
    /// Begins a transaction
    async fn begin_transaction(&self) -> Result<Box<dyn Transaction<Error> + Send + Sync>, Error>;
}

#[cfg(feature = "federation")]
/// Mint Database trait
#[async_trait]
pub trait Database<Error>:
    KVStoreDatabase<Err = Error>
    + QuotesDatabase<Err = Error>
    + ProofsDatabase<Err = Error>
    + SignaturesDatabase<Err = Error>
    + SagaDatabase<Err = Error>
    + CompletedOperationsDatabase<Err = Error>
    + FederationJournalDatabase<Err = Error>
{
    /// Begins a transaction
    async fn begin_transaction(&self) -> Result<Box<dyn Transaction<Error> + Send + Sync>, Error>;
}

/// Type alias for Mint Database
pub type DynMintDatabase = std::sync::Arc<dyn Database<Error> + Send + Sync>;

/// Type alias for Mint Transaction
pub type DynMintTransaction = Box<dyn Transaction<Error> + Send + Sync>;
