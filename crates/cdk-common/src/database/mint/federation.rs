//! Federation journal, application, custody, and setup database contracts.

use std::collections::BTreeMap;

use async_trait::async_trait;
use cashu::quote_id::QuoteId;
use cashu::Amount;

use super::Error;
use crate::federation::{
    validate_federation_setup_invitation_consumption, FederationAlephBftFinalizedItem,
    FederationAlephBftPendingOperationRecord, FederationApplicationOperation,
    FederationApplicationState, FederationCheckpoint, FederationCheckpointFrontier,
    FederationCheckpointId, FederationCheckpointKind, FederationCheckpointQuorum,
    FederationCheckpointSignature, FederationConfigSnapshot, FederationDkgActivationRequest,
    FederationDkgCeremonyRecord, FederationDkgMemberKeysetShare,
    FederationDkgParticipantCommitment, FederationDkgParticipantCommitmentReveal,
    FederationDkgParticipantState, FederationDkgSecretShareRequest,
    FederationDkgTranscriptSignatureRequest, FederationFrostApplicationConfig,
    FederationFrostDkgConfirmationRequest, FederationFrostDkgLocalState,
    FederationFrostDkgRound1Request, FederationFrostDkgRound2Record,
    FederationFrostRootPrivateConfig, FederationFrostRootPublicConfig, FederationFrostRootRecord,
    FederationId, FederationJournalEntry, FederationJournalTip,
    FederationMaterializedStateKeyRecord, FederationMaterializedStateKind,
    FederationMaterializedStateRow, FederationOperationConflictKey, FederationOperationId,
    FederationPaymentObservationContext, FederationPaymentObservationRecord,
    FederationRetentionFloor, FederationSetupActivation, FederationSetupApproval,
    FederationSetupEnrollment, FederationSetupId, FederationSetupInvitationId,
    FederationSetupInvitationRecord, FederationSetupManifest, FederationSetupProposal,
    FederationSetupRecord, FederationSetupSession, FederationSetupStep,
    FederationSignatureShareRecord, FederationStateSnapshotInfo, FederationStateSnapshotRow,
    FederationStateTransferWindowBounds, MemberId, VerifiedFederationCheckpointQuorum,
    VerifiedFederationStateTransferWindow,
};
use crate::mint::{self, MintQuote as MintMintQuote};
use crate::nuts::{
    BlindSignature, CurrencyUnit, MintQuoteState, PaymentMethod, Proof, PublicKey, State,
};

/// Minimal durable mint-quote state used by federation convergence projection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FederationMintQuoteState {
    /// Quote identifier.
    pub id: QuoteId,
    /// Requested amount, when fixed by the payment request.
    pub amount: Option<Amount<CurrencyUnit>>,
    /// Quote currency unit.
    pub unit: CurrencyUnit,
    /// External payment request.
    pub request: String,
    /// Quote expiry timestamp.
    pub expiry: u64,
    /// Total amount observed as paid.
    pub amount_paid: Amount<CurrencyUnit>,
    /// Total amount already issued.
    pub amount_issued: Amount<CurrencyUnit>,
    /// Derived Cashu quote state.
    pub state: MintQuoteState,
    /// Payment method used by the quote.
    pub payment_method: PaymentMethod,
    /// Payment-processor lookup identifier fixed by the accepted operation.
    pub request_lookup_id: crate::payment::PaymentIdentifier,
    /// NUT-20 public key locking the quote, when the request carried one.
    pub pubkey: Option<PublicKey>,
}

/// One stored proof named by the materialized-state key registry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FederationStoredProof {
    /// Proof Y.
    pub y: PublicKey,
    /// Stored proof (secret, C and witness).
    pub proof: Proof,
    /// Stored proof state.
    pub state: State,
    /// Quote the proof was spent against, for melts.
    pub quote_id: Option<String>,
    /// Kind of the mint operation that stored the proof.
    pub operation_kind: Option<String>,
    /// Mint operation that stored the proof.
    pub operation_id: Option<String>,
}

/// One signed blind signature named by the materialized-state key registry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FederationStoredBlindSignature {
    /// Blinded message the signature answers.
    pub blinded_message: PublicKey,
    /// This member's stored signature (its BLS share).
    pub signature: BlindSignature,
    /// Quote the output was signed for, when the operation names one.
    pub quote_id: Option<String>,
}

/// One stored payment observation and, once its quote is terminal, the
/// journal index at which it was retired.
///
/// A retired observation is never read by an operation again: the
/// materialized-state digest of a checkpoint whose retention floor is above
/// that index leaves it out, and history retention deletes it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FederationStoredPaymentObservation {
    /// The observation.
    pub record: FederationPaymentObservationRecord,
    /// Journal index of the operation that retired it.
    pub terminal_operation_index: Option<u64>,
}

/// Every stored row the materialized-state digest reads, read through one
/// connection or transaction.
///
/// Rows are keyed by the materialized-state key registry exactly as the
/// digest enumerates them, so a missing row is visible to the caller.
#[derive(Debug, Default)]
pub struct FederationMaterializedStateSource {
    /// FROST root public configs.
    pub frost_root_public_configs: Vec<FederationFrostRootPublicConfig>,
    /// FROST application configs.
    pub frost_application_configs: Vec<FederationFrostApplicationConfig>,
    /// The materialized-state key registry.
    pub state_keys: Vec<FederationMaterializedStateKeyRecord>,
    /// Completed operations named by the registry that exist.
    pub completed_operations: Vec<mint::Operation>,
    /// Mint quotes named by the registry that exist.
    pub mint_quotes: Vec<FederationMintQuoteState>,
    /// Melt quotes named by the registry that exist.
    pub melt_quotes: Vec<mint::MeltQuote>,
    /// Every stored payment observation, in consensus order, retired ones
    /// included.
    pub payment_observations: Vec<FederationStoredPaymentObservation>,
    /// Proofs named by the registry that exist.
    pub proofs: Vec<FederationStoredProof>,
    /// Signed blind signatures named by the registry that exist.
    pub blind_signatures: Vec<FederationStoredBlindSignature>,
    /// Every member's signature shares for the named blinded messages.
    pub signature_shares: Vec<FederationSignatureShareRecord>,
    /// Application-owned projection rows (the Bark projection), when the
    /// backend stores one.
    pub application_rows: Vec<FederationMaterializedStateRow>,
}

/// Scalar size metadata for one local AlephBFT session backup.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct FederationAlephBftBackupMetadata {
    /// Number of persisted backup chunks.
    pub chunk_count: u64,
    /// Total persisted backup bytes.
    pub total_bytes: u64,
}

/// One immutable DKG record.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FederationDkgRecord<'a> {
    /// One pre-reveal commitment.
    Commitment(&'a FederationDkgParticipantCommitment),
    /// One public commitment reveal.
    Reveal(&'a FederationDkgParticipantCommitmentReveal),
    /// One private secret-share contribution.
    SecretShareContribution(&'a FederationDkgSecretShareRequest),
    /// One local restartable participant state.
    ParticipantState(&'a FederationDkgParticipantState),
    /// One completed local member keyset share.
    MemberKeysetShare(&'a FederationDkgMemberKeysetShare),
    /// One member signature over a DKG/setup transcript hash.
    TranscriptSignature(&'a FederationDkgTranscriptSignatureRequest),
    /// One member's finalized-config activation confirmation.
    Activation(&'a FederationDkgActivationRequest),
    /// One FROST DKG record.
    Frost(FederationFrostDkgRecord<'a>),
}

/// One FROST DKG record.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FederationFrostDkgRecord<'a> {
    /// One distinct round-one broadcast.
    Round1(&'a FederationFrostDkgRound1Request),
    /// One confidential round-two contribution.
    Round2(&'a FederationFrostDkgRound2Record),
    /// Encrypted local DKG state, stored before phase advancement.
    LocalState(&'a FederationFrostDkgLocalState),
    /// The completed canonical root result.
    Root(&'a FederationFrostRootRecord),
    /// One signed canonical root confirmation.
    Confirmation(&'a FederationFrostDkgConfirmationRequest),
}

/// Every durable FROST DKG record, by kind, each in ceremony, sender and
/// receiver order.
#[derive(Debug, Clone, Default)]
pub struct FederationFrostDkgRecords {
    /// Round-one broadcasts.
    pub round1: Vec<FederationFrostDkgRound1Request>,
    /// Round-two contributions.
    pub round2: Vec<FederationFrostDkgRound2Record>,
    /// Encrypted local states: round-one states, then round-two states.
    pub local_states: Vec<FederationFrostDkgLocalState>,
    /// Completed canonical root records.
    pub roots: Vec<FederationFrostRootRecord>,
    /// Signed canonical root confirmations.
    pub confirmations: Vec<FederationFrostDkgConfirmationRequest>,
}

/// A range of consensus positions (journal indexes, finalized-item indexes or
/// backup chunk indexes), read in ascending order.
///
/// The default range reads every stored row.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct FederationIndexRange {
    /// First position read.
    pub start: u64,
    /// First position not read; `None` reads to the end.
    pub end: Option<u64>,
    /// Maximum number of rows read; `None` reads every row in the range.
    pub limit: Option<usize>,
}

impl FederationIndexRange {
    /// Every stored row.
    pub const ALL: Self = Self {
        start: 0,
        end: None,
        limit: None,
    };

    /// Every stored row from `start` on.
    #[must_use]
    pub const fn starting_at(start: u64) -> Self {
        Self {
            start,
            end: None,
            limit: None,
        }
    }

    /// At most `limit` rows in `[start, end)`.
    #[must_use]
    pub const fn page(start: u64, end: u64, limit: usize) -> Self {
        Self {
            start,
            end: Some(end),
            limit: Some(limit),
        }
    }

    /// At most `limit` rows from `start` on.
    #[must_use]
    pub const fn limited(start: u64, limit: usize) -> Self {
        Self {
            start,
            end: None,
            limit: Some(limit),
        }
    }
}

/// How a journal entry is looked up.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FederationJournalEntryKey<'a> {
    /// The latest entry with this operation id.
    ///
    /// An operation id repeats only after its earlier entry has left the
    /// operation validity window, so the latest entry is the one that can
    /// still guard duplicates.
    Operation(FederationOperationId),
    /// The latest entry reserving this conflict key.
    ConflictKey(&'a FederationOperationConflictKey),
}

/// Which stored checkpoint, or the quorum assembled for it, to read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FederationCheckpointSelector {
    /// The checkpoint with this id.
    Id(FederationCheckpointId),
    /// The newest one of this kind.
    Latest(FederationCheckpointKind),
    /// The scheduled one with the sequence after this one.
    ///
    /// Candidate capture is independent of signature collection. Quorums must
    /// still be assembled and promoted in lineage order.
    NextScheduled(u64),
    /// The locally trusted one.
    Trusted,
    /// The one whose snapshot this member installed by state transfer.
    ///
    /// Journal positions below it hold no local entries: the installed state
    /// stands for them.
    StateTransferBase,
}

/// Which signature shares to read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FederationSignatureShareKey {
    /// Every member's share for one output of one accepted operation, in
    /// member order.
    Output {
        /// Accepted operation.
        operation_id: FederationOperationId,
        /// Output position in the operation.
        output_index: u32,
    },
    /// Every share for one blinded message, in operation, output and member
    /// order.
    BlindedMessage(PublicKey),
}

/// Which payment observations to read. Each selection is in consensus order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FederationPaymentObservationSelection<'a> {
    /// The live (not retired) observations of one quote.
    Live {
        /// Observation context.
        context: FederationPaymentObservationContext,
        /// Quote identifier.
        quote: &'a str,
    },
    /// Every stored observation of one quote, retired ones included, for
    /// audit.
    History {
        /// Observation context.
        context: FederationPaymentObservationContext,
        /// Quote identifier.
        quote: &'a str,
    },
    /// Every live observation.
    AllLive,
}

impl From<MintMintQuote> for FederationMintQuoteState {
    fn from(quote: MintMintQuote) -> Self {
        let amount_paid = quote.amount_paid();
        let amount_issued = quote.amount_issued();
        let state = quote.state();
        Self {
            id: quote.id,
            amount: quote.amount,
            unit: quote.unit,
            request: quote.request,
            expiry: quote.expiry,
            amount_paid,
            amount_issued,
            state,
            payment_method: quote.payment_method,
            request_lookup_id: quote.request_lookup_id,
            pubkey: quote.pubkey,
        }
    }
}

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

    /// Get the latest accepted journal entry for `key` inside this
    /// transaction, ordered before `before` when it is given.
    async fn get_federation_journal_entry(
        &mut self,
        key: FederationJournalEntryKey<'_>,
        before: Option<u64>,
    ) -> Result<Option<FederationJournalEntry>, Self::Err>;

    /// Persist one legal post-consensus application state of the latest entry with an operation ID.
    async fn set_federation_operation_application_state(
        &mut self,
        operation_id: FederationOperationId,
        application: &FederationApplicationState,
    ) -> Result<(), Self::Err>;

    /// Persist the highest accepted effectful DKG sequence for one federation member.
    async fn set_federation_dkg_member_sequence(
        &mut self,
        member_id: MemberId,
        sequence: u64,
    ) -> Result<(), Self::Err>;

    /// Persist a local or peer-derived checkpoint candidate at the exact current durable frontier.
    async fn add_federation_checkpoint_candidate(
        &mut self,
        checkpoint: &FederationCheckpoint,
    ) -> Result<(), Self::Err>;

    /// Record a successful local full-state audit for an already stored checkpoint.
    ///
    /// The caller must have recomputed the complete materialized-state digest while
    /// holding its application mutation lock. The checkpoint must equal the current
    /// durable frontier.
    async fn record_federation_materialized_state_audit(
        &mut self,
        checkpoint: &FederationCheckpoint,
    ) -> Result<(), Self::Err>;

    /// Persist one immutable local or remote checkpoint signature.
    async fn add_federation_checkpoint_signature(
        &mut self,
        signature: &FederationCheckpointSignature,
    ) -> Result<(), Self::Err>;

    /// Persist one immutable assembled checkpoint quorum.
    ///
    /// Cryptographic quorum verification remains the caller's responsibility;
    /// persistence validates immutable checkpoint and signer identities.
    async fn add_federation_checkpoint_quorum(
        &mut self,
        quorum: &FederationCheckpointQuorum,
    ) -> Result<(), Self::Err>;

    /// Atomically promote a caller-verified quorum no further than durable local state.
    async fn promote_verified_federation_checkpoint(
        &mut self,
        quorum: &VerifiedFederationCheckpointQuorum,
    ) -> Result<(), Self::Err>;

    /// Store the frozen state rows of a scheduled checkpoint this member has
    /// just captured, and drop snapshots older than the latest
    /// [`crate::federation::FEDERATION_STATE_SNAPSHOT_RETAINED`] scheduled ones.
    ///
    /// The rows must hash to the checkpoint's materialized-state digest.
    async fn add_federation_state_snapshot(
        &mut self,
        checkpoint: &FederationCheckpoint,
        rows: &[FederationStateSnapshotRow],
    ) -> Result<(), Self::Err>;

    /// Make a caller-verified checkpoint the frontier of a member whose state
    /// was installed from that checkpoint's snapshot in this transaction, and
    /// store the checkpoint's verified validity window as history.
    ///
    /// The member must hold no journal, finalized items or frontier yet. The
    /// checkpoint becomes the trusted, audited frontier and the state-transfer
    /// base. The window's finalized items and journal entries (with their
    /// application digests and reserved conflict keys) are stored below it,
    /// so duplicate and conflict checks see them as its peers do.
    async fn install_federation_state_transfer_frontier(
        &mut self,
        quorum: &VerifiedFederationCheckpointQuorum,
        window: &VerifiedFederationStateTransferWindow,
    ) -> Result<(), Self::Err>;

    /// Advance the retention floor and delete one bounded batch below it.
    ///
    /// The floor is the scheduled checkpoint before the latest one covered by
    /// the trusted checkpoint, capped at the start of the latest one's
    /// validity window and at `keep_journal_from`, the earliest journal entry
    /// that live state still reads. It never moves back. Each call deletes at
    /// most `limit` journal entries (with their conflict keys), `limit`
    /// finalized items and `limit` scheduled checkpoints below the floor
    /// checkpoint, and returns how many rows it deleted. The floor checkpoint
    /// and the trusted and audited checkpoints are kept.
    async fn prune_federation_history(
        &mut self,
        limit: u64,
        keep_journal_from: Option<u64>,
    ) -> Result<u64, Self::Err>;

    /// Delete one bounded batch of retired terminal rows below `floor`:
    /// payment observations retired below it, and the transitions and
    /// observations of Bark actions retired below it.
    ///
    /// `floor` is the retention floor's journal index; rows retired below it
    /// are excluded from every later checkpoint's materialized-state digest.
    /// Returns how many rows were deleted.
    async fn prune_federation_terminal_rows(
        &mut self,
        floor: u64,
        limit: u64,
    ) -> Result<u64, Self::Err>;

    /// Wipe this member's federation consensus state and mint business rows
    /// so it starts as an empty member and rejoins by state transfer.
    ///
    /// Deleted: the journal and its conflict keys, finalized items, AlephBFT
    /// backups, pending operations and session state, checkpoints with their
    /// signatures, quorums, snapshots and frontier (the retention floor and
    /// state-transfer base with it), the materialized-state key registry,
    /// payment observations, signature shares, the consensus projections and
    /// the Bark projection, the federation payment schedule and Bark replay
    /// markers, and the mint's quotes, proofs, blind signatures, melt
    /// requests, sagas, completed operations and per-keyset totals.
    ///
    /// Kept: the federation config snapshot, DKG and setup records, FROST key
    /// material, the mint's keysets and its configuration. Files outside the
    /// database (the FROST nonce store) are not touched. Returns how many rows
    /// were deleted.
    async fn reset_federation_member_state(&mut self) -> Result<u64, Self::Err>;

    /// Durably latch a materialized-state audit mismatch.
    ///
    /// This operation is intentionally one-way. Ordinary startup, peer
    /// agreement, and checkpoint promotion cannot clear the latch.
    async fn latch_federation_materialized_state_audit_mismatch(&mut self)
        -> Result<(), Self::Err>;

    /// Persist finalized AlephBFT consensus items in accepted item order.
    ///
    /// Storing an identical item again is accepted; a different item at a
    /// stored position is a conflict.
    async fn add_federation_aleph_bft_finalized_items(
        &mut self,
        items: &[FederationAlephBftFinalizedItem],
    ) -> Result<(), Self::Err>;

    /// Append raw AlephBFT backup bytes for one local session.
    async fn append_federation_aleph_bft_backup_bytes(
        &mut self,
        session: u64,
        bytes: &[u8],
    ) -> Result<(), Self::Err>;

    /// Atomically replace all raw AlephBFT backup chunks for one local
    /// session.
    async fn replace_federation_aleph_bft_backup_chunks(
        &mut self,
        session: u64,
        chunks: &[Vec<u8>],
    ) -> Result<(), Self::Err>;

    /// Delete local AlephBFT backup chunks for sessions older than `session`.
    ///
    /// The active session backup is always retained even if the requested
    /// pruning boundary is newer.
    async fn prune_federation_aleph_bft_backups_before(
        &mut self,
        session: u64,
    ) -> Result<(), Self::Err>;

    /// Persist the latest active AlephBFT session marker and atomically rebind
    /// validated pending operations from older sessions to the effective
    /// monotonic active session.
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
}

#[async_trait]
/// Federation post-consensus mint application transaction trait.
pub trait FederationApplicationTransaction {
    /// Federation application database error.
    type Err: Into<Error> + From<Error>;

    /// Persist one payment observation of an accepted federation operation,
    /// or install one from a state transfer with the journal index at which
    /// it was retired (`None` while it is live).
    ///
    /// An observation is identified by its operation id, which hashes its
    /// content. A later entry with the same id (re-ordered after its validity
    /// window) keeps the first ordering position. The apply path never
    /// records an observation of a final quote, whose earlier observations
    /// are retired.
    async fn add_federation_payment_observation(
        &mut self,
        observation: &FederationPaymentObservationRecord,
        terminal_operation_index: Option<u64>,
    ) -> Result<(), Self::Err>;

    /// Retire every live observation of one terminal quote at
    /// `operation_index`, the journal index of the operation applying now.
    ///
    /// Returns how many observations were retired.
    async fn retire_federation_payment_observations(
        &mut self,
        context: FederationPaymentObservationContext,
        quote: &str,
        operation_index: u64,
    ) -> Result<u64, Self::Err>;

    /// Record that `quote` became final at `operation_index` in the
    /// application projection (the Bark action bound to it), so a bound
    /// action in a final phase retires. Without an application projection
    /// this does nothing.
    async fn settle_federation_application_quote(
        &mut self,
        quote: &str,
        operation_index: u64,
    ) -> Result<(), Self::Err>;

    /// Persist local federation threshold signature shares.
    ///
    /// Storing an identical share again keeps its first accepted position.
    async fn add_federation_signature_shares(
        &mut self,
        shares: &[FederationSignatureShareRecord],
    ) -> Result<(), Self::Err>;

    /// Get the selected payment observations inside this transaction.
    async fn get_federation_payment_observations(
        &mut self,
        selection: FederationPaymentObservationSelection<'_>,
    ) -> Result<Vec<FederationStoredPaymentObservation>, Self::Err>;

    /// Read every stored row the materialized-state digest covers.
    async fn get_federation_materialized_state_source(
        &mut self,
    ) -> Result<FederationMaterializedStateSource, Self::Err>;

    /// Write materialized-state key registry records as a state transfer
    /// installs them.
    async fn add_federation_materialized_state_keys(
        &mut self,
        records: &[FederationMaterializedStateKeyRecord],
    ) -> Result<(), Self::Err>;

    /// Install application-owned projection rows (the Bark projection) from a
    /// state transfer. The rows are the ones the materialized-state source
    /// returns in `application_rows`.
    async fn install_federation_application_rows(
        &mut self,
        rows: &[FederationMaterializedStateRow],
    ) -> Result<(), Self::Err>;

    /// Deterministically apply one accepted extension operation to its projection.
    async fn apply_federation_application_operation(
        &mut self,
        entry: &FederationJournalEntry,
        operation: &FederationApplicationOperation,
    ) -> Result<(), Self::Err>;
}

#[async_trait]
/// Federation DKG ceremony transaction trait.
pub trait FederationDkgTransaction {
    /// Federation DKG database error.
    type Err: Into<Error> + From<Error>;

    /// Persist one active DKG ceremony lifecycle record idempotently.
    async fn add_federation_dkg_ceremony(
        &mut self,
        ceremony: &FederationDkgCeremonyRecord,
    ) -> Result<(), Self::Err>;

    /// Compare and set one DKG ceremony lifecycle transition.
    async fn transition_federation_dkg_ceremony(
        &mut self,
        expected: &FederationDkgCeremonyRecord,
        updated: &FederationDkgCeremonyRecord,
    ) -> Result<(), Self::Err>;

    /// Persist one immutable DKG record exactly once, accepting exact replay.
    async fn add_federation_dkg_record(
        &mut self,
        record: FederationDkgRecord<'_>,
    ) -> Result<(), Self::Err>;
}

#[async_trait]
/// Federation FROST wallet configuration transaction trait.
pub trait FederationFrostWalletTransaction {
    /// Federation FROST wallet database error.
    type Err: Into<Error> + From<Error>;

    /// Persist one immutable public root snapshot.
    async fn add_federation_frost_root_public_config(
        &mut self,
        config: &FederationFrostRootPublicConfig,
    ) -> Result<(), Self::Err>;

    /// Persist this member's sealed private root snapshot.
    async fn add_federation_frost_root_private_config(
        &mut self,
        config: &FederationFrostRootPrivateConfig,
    ) -> Result<(), Self::Err>;

    /// Persist one immutable application-key snapshot.
    async fn add_federation_frost_application_config(
        &mut self,
        config: &FederationFrostApplicationConfig,
    ) -> Result<(), Self::Err>;
}

#[async_trait]
/// Multi-operator federation setup transaction trait.
pub trait FederationSetupTransaction {
    /// Federation setup database error.
    type Err: Into<Error> + From<Error>;

    /// Persist one local setup session exactly once, accepting exact replay.
    async fn add_federation_setup_session(
        &mut self,
        session: &FederationSetupSession,
    ) -> Result<(), Self::Err>;

    /// Compare and set one legal local setup-session transition.
    async fn transition_federation_setup_session(
        &mut self,
        expected: &FederationSetupSession,
        updated: &FederationSetupSession,
    ) -> Result<(), Self::Err>;

    /// Persist secret-free invitation metadata exactly once.
    async fn add_federation_setup_invitation(
        &mut self,
        invitation: &FederationSetupInvitationRecord,
    ) -> Result<(), Self::Err>;

    /// Compare and set one invitation state transition.
    async fn transition_federation_setup_invitation(
        &mut self,
        expected: &FederationSetupInvitationRecord,
        updated: &FederationSetupInvitationRecord,
    ) -> Result<(), Self::Err>;

    /// Atomically expire one unconsumed invitation and persist its replacement.
    async fn replace_federation_setup_invitation(
        &mut self,
        expected: &FederationSetupInvitationRecord,
        replacement: &FederationSetupInvitationRecord,
    ) -> Result<(), Self::Err>;

    /// Persist one immutable setup record exactly once, accepting exact
    /// replay.
    async fn add_federation_setup_record(
        &mut self,
        record: FederationSetupRecord<'_>,
    ) -> Result<(), Self::Err>;

    /// Atomically insert an enrollment and consume its invitation.
    async fn consume_federation_setup_invitation(
        &mut self,
        expected: &FederationSetupInvitationRecord,
        consumed: &FederationSetupInvitationRecord,
        enrollment: &FederationSetupEnrollment,
    ) -> Result<(), Self::Err> {
        validate_federation_setup_invitation_consumption(expected, consumed, enrollment)
            .map_err(|error| Self::Err::from(Error::Federation(error)))?;
        self.add_federation_setup_record(FederationSetupRecord::Enrollment(enrollment))
            .await?;
        self.transition_federation_setup_invitation(expected, consumed)
            .await
    }

    /// Atomically store the record of one local session step and advance the
    /// local setup session from `expected` to `updated`.
    async fn advance_federation_setup_session(
        &mut self,
        expected: &FederationSetupSession,
        updated: &FederationSetupSession,
        step: FederationSetupStep<'_>,
    ) -> Result<(), Self::Err> {
        step.validate_transition(expected, updated)
            .map_err(|error| Self::Err::from(Error::Federation(error)))?;
        self.add_federation_setup_record(step.record()).await?;
        self.transition_federation_setup_session(expected, updated)
            .await
    }
}

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

    /// Get accepted federation journal entries in `range`, in consensus order.
    async fn get_federation_journal_entries(
        &self,
        range: FederationIndexRange,
    ) -> Result<Vec<FederationJournalEntry>, Self::Err>;

    /// Get scalar metadata for the latest accepted federation journal entry.
    async fn get_federation_journal_tip(&self) -> Result<Option<FederationJournalTip>, Self::Err>;

    /// Get the latest accepted journal entry for `key`, ordered before
    /// `before` when it is given.
    async fn get_federation_journal_entry(
        &self,
        key: FederationJournalEntryKey<'_>,
        before: Option<u64>,
    ) -> Result<Option<FederationJournalEntry>, Self::Err>;

    /// Get persisted effectful DKG sequence watermarks by member.
    async fn get_federation_dkg_member_sequences(
        &self,
    ) -> Result<BTreeMap<MemberId, u64>, Self::Err>;

    /// Get bounded scalar checkpoint and rolling-digest metadata.
    async fn get_federation_checkpoint_frontier(
        &self,
    ) -> Result<FederationCheckpointFrontier, Self::Err>;

    /// Get one persisted checkpoint payload.
    async fn get_federation_checkpoint(
        &self,
        selector: FederationCheckpointSelector,
    ) -> Result<Option<FederationCheckpoint>, Self::Err>;

    /// Get all persisted signatures for one checkpoint ID.
    async fn get_federation_checkpoint_signatures(
        &self,
        checkpoint_id: FederationCheckpointId,
    ) -> Result<Vec<FederationCheckpointSignature>, Self::Err>;

    /// Get one persisted assembled checkpoint quorum.
    ///
    /// Callers must verify the returned proof against the active public
    /// config (and re-verify the trusted one before using it at startup).
    async fn get_federation_checkpoint_quorum(
        &self,
        selector: FederationCheckpointSelector,
    ) -> Result<Option<FederationCheckpointQuorum>, Self::Err>;

    /// Get the stored state snapshots, oldest scheduled sequence first.
    async fn get_federation_state_snapshots(
        &self,
    ) -> Result<Vec<FederationStateSnapshotInfo>, Self::Err>;

    /// Get up to `limit` rows of one stored state snapshot from `from_row_index`.
    async fn get_federation_state_snapshot_rows(
        &self,
        checkpoint_id: FederationCheckpointId,
        from_row_index: u64,
        limit: usize,
    ) -> Result<Vec<FederationStateSnapshotRow>, Self::Err>;

    /// Where the validity window before `checkpoint` starts in this member's
    /// history, and the digests just before it.
    async fn get_federation_state_transfer_window_bounds(
        &self,
        checkpoint: &FederationCheckpoint,
    ) -> Result<FederationStateTransferWindowBounds, Self::Err>;

    /// Get the retention floor: this member holds no journal entries or
    /// finalized items below it.
    async fn get_federation_retention_floor(
        &self,
    ) -> Result<Option<FederationRetentionFloor>, Self::Err>;

    /// The journal index below which `checkpoint`'s materialized-state digest
    /// leaves out terminal entities.
    ///
    /// It is the retention floor `checkpoint` sets once it is the latest
    /// scheduled checkpoint, before live-state pins: the latest scheduled
    /// checkpoint below it, or the start of `checkpoint`'s validity window if
    /// that is earlier, and zero when no scheduled checkpoint lies below it.
    /// It depends only on the checkpoint sequence and finalized items, so every
    /// member computes the same value, and it is never below any retention
    /// floor a member recorded before capturing `checkpoint`.
    async fn get_federation_materialized_state_floor(
        &self,
        checkpoint: &FederationCheckpoint,
    ) -> Result<u64, Self::Err>;

    /// Get persisted AlephBFT finalized items in `range`, in accepted item
    /// order.
    async fn get_federation_aleph_bft_finalized_items(
        &self,
        range: FederationIndexRange,
    ) -> Result<Vec<FederationAlephBftFinalizedItem>, Self::Err>;

    /// Get the persisted AlephBFT backup chunks of one local session whose
    /// chunk index is in `range`, in chunk order.
    async fn get_federation_aleph_bft_backup_chunks(
        &self,
        session: u64,
        range: FederationIndexRange,
    ) -> Result<Vec<Vec<u8>>, Self::Err>;

    /// Get scalar chunk and byte totals for one local AlephBFT session backup.
    async fn get_federation_aleph_bft_backup_metadata(
        &self,
        session: u64,
    ) -> Result<FederationAlephBftBackupMetadata, Self::Err>;

    /// Get the latest active AlephBFT session marker.
    async fn get_federation_aleph_bft_active_session(&self) -> Result<Option<u64>, Self::Err>;

    /// Get local AlephBFT pending operations for one session.
    async fn get_federation_aleph_bft_pending_operations(
        &self,
        session: u64,
    ) -> Result<Vec<FederationAlephBftPendingOperationRecord>, Self::Err>;
}

#[async_trait]
/// Federation post-consensus mint application database trait.
pub trait FederationApplicationDatabase {
    /// Federation application database error.
    type Err: Into<Error> + From<Error>;

    /// Get the selected payment observations.
    async fn get_federation_payment_observations(
        &self,
        selection: FederationPaymentObservationSelection<'_>,
    ) -> Result<Vec<FederationStoredPaymentObservation>, Self::Err>;

    /// Get the registry records of one kind of consensus-authoritative
    /// materialized row key, in row-key order.
    async fn get_federation_materialized_state_keys(
        &self,
        kind: FederationMaterializedStateKind,
    ) -> Result<Vec<FederationMaterializedStateKeyRecord>, Self::Err>;

    /// Get melt quotes needed for convergence projection by quote ID.
    ///
    /// Only matching rows are returned; callers reconstruct deterministic
    /// ordering and missing-row markers from their input key set.
    async fn get_federation_melt_quotes(
        &self,
        quote_ids: &[QuoteId],
    ) -> Result<Vec<mint::MeltQuote>, Self::Err>;

    /// Read every stored row the materialized-state digest covers.
    async fn get_federation_materialized_state_source(
        &self,
    ) -> Result<FederationMaterializedStateSource, Self::Err>;

    /// Get the signature shares `key` names.
    async fn get_federation_signature_shares(
        &self,
        key: FederationSignatureShareKey,
    ) -> Result<Vec<FederationSignatureShareRecord>, Self::Err>;
}

#[async_trait]
/// Federation DKG ceremony database trait.
pub trait FederationDkgDatabase {
    /// Federation DKG database error.
    type Err: Into<Error> + From<Error>;

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

    /// Get all persisted DKG finalized-config activation confirmations.
    async fn get_federation_dkg_activations(
        &self,
    ) -> Result<Vec<FederationDkgActivationRequest>, Self::Err>;

    /// Get all durable DKG ceremony lifecycle records.
    async fn get_federation_dkg_ceremonies(
        &self,
    ) -> Result<Vec<FederationDkgCeremonyRecord>, Self::Err>;

    /// Get every durable FROST DKG record, by kind.
    async fn get_federation_frost_dkg_records(
        &self,
    ) -> Result<FederationFrostDkgRecords, Self::Err>;
}

#[async_trait]
/// Federation FROST wallet configuration database trait.
pub trait FederationFrostWalletDatabase {
    /// Federation FROST wallet database error.
    type Err: Into<Error> + From<Error>;

    /// Load all immutable public root snapshots.
    async fn get_federation_frost_root_public_configs(
        &self,
    ) -> Result<Vec<FederationFrostRootPublicConfig>, Self::Err>;

    /// Load all participant-local sealed root snapshots.
    async fn get_federation_frost_root_private_configs(
        &self,
    ) -> Result<Vec<FederationFrostRootPrivateConfig>, Self::Err>;

    /// Load all immutable application-key snapshots.
    async fn get_federation_frost_application_configs(
        &self,
    ) -> Result<Vec<FederationFrostApplicationConfig>, Self::Err>;
}

#[async_trait]
/// Multi-operator federation setup read database trait.
pub trait FederationSetupDatabase {
    /// Federation setup database error.
    type Err: Into<Error> + From<Error>;

    /// Load one local setup session.
    async fn get_federation_setup_session(
        &self,
        setup_id: FederationSetupId,
    ) -> Result<Option<FederationSetupSession>, Self::Err>;

    /// Load all invitation records for one setup in member/identifier order.
    async fn get_federation_setup_invitations(
        &self,
        setup_id: FederationSetupId,
    ) -> Result<Vec<FederationSetupInvitationRecord>, Self::Err>;

    /// Load one invitation record by public identifier.
    async fn get_federation_setup_invitation(
        &self,
        setup_id: FederationSetupId,
        invitation_id: FederationSetupInvitationId,
    ) -> Result<Option<FederationSetupInvitationRecord>, Self::Err>;

    /// Load all signed enrollments for one setup in member order.
    async fn get_federation_setup_enrollments(
        &self,
        setup_id: FederationSetupId,
    ) -> Result<Vec<FederationSetupEnrollment>, Self::Err>;

    /// Load the immutable proposal, when published.
    async fn get_federation_setup_proposal(
        &self,
        setup_id: FederationSetupId,
    ) -> Result<Option<FederationSetupProposal>, Self::Err>;

    /// Load this member's durable one-proposal approval fence.
    async fn get_federation_setup_approval_fence(
        &self,
        setup_id: FederationSetupId,
        member_id: MemberId,
    ) -> Result<Option<FederationSetupApproval>, Self::Err>;

    /// Load coordinator-received approvals in member order.
    async fn get_federation_setup_approvals(
        &self,
        setup_id: FederationSetupId,
    ) -> Result<Vec<FederationSetupApproval>, Self::Err>;

    /// Load the complete certified manifest, when available.
    async fn get_federation_setup_manifest(
        &self,
        setup_id: FederationSetupId,
    ) -> Result<Option<FederationSetupManifest>, Self::Err>;

    /// Load local member activation references in member order.
    async fn get_federation_setup_activations(
        &self,
        setup_id: FederationSetupId,
    ) -> Result<Vec<FederationSetupActivation>, Self::Err>;
}
