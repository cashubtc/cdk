//! Federation journal, application, custody, and setup database contracts.

use std::collections::BTreeMap;

use async_trait::async_trait;
use cashu::quote_id::QuoteId;
use cashu::Amount;

use super::Error;
use crate::federation::{
    FederationAlephBftFinalizedItem, FederationAlephBftPendingOperationRecord,
    FederationApplicationOperation, FederationApplicationState, FederationCheckpoint,
    FederationCheckpointFrontier, FederationCheckpointId, FederationCheckpointKind,
    FederationCheckpointQuorum, FederationCheckpointSignature, FederationConfigSnapshot,
    FederationDkgActivationRequest, FederationDkgCeremonyRecord, FederationDkgMemberKeysetShare,
    FederationDkgParticipantCommitment, FederationDkgParticipantCommitmentReveal,
    FederationDkgParticipantState, FederationDkgSecretShareRequest,
    FederationDkgTranscriptSignatureRequest, FederationFrostApplicationConfig,
    FederationFrostDkgConfirmationRequest, FederationFrostDkgLocalState,
    FederationFrostDkgRound1Request, FederationFrostDkgRound2Record,
    FederationFrostRootPrivateConfig, FederationFrostRootPublicConfig, FederationFrostRootRecord,
    FederationId, FederationJournalEntry, FederationJournalTip,
    FederationMaterializedStateKeyRecord, FederationOperationConflictKey, FederationOperationId,
    FederationPaymentObservationContext, FederationPaymentObservationRecord,
    FederationSetupActivation, FederationSetupApproval, FederationSetupCoordinatorPhase,
    FederationSetupEnrollment, FederationSetupId, FederationSetupInvitationId,
    FederationSetupInvitationRecord, FederationSetupManifest, FederationSetupParticipantPhase,
    FederationSetupProposal, FederationSetupSession, FederationSetupUrlPolicy,
    FederationSignatureShareRecord, MemberId, VerifiedFederationCheckpointQuorum,
};
use crate::mint::{self, MintQuote as MintMintQuote};
use crate::nuts::{CurrencyUnit, MintQuoteState, PaymentMethod, PublicKey};

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
}

/// Scalar size metadata for one local AlephBFT session backup.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct FederationAlephBftBackupMetadata {
    /// Number of persisted backup chunks.
    pub chunk_count: u64,
    /// Total persisted backup bytes.
    pub total_bytes: u64,
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

    /// Persist one legal post-consensus application state.
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

    /// Prune superseded checkpoint candidates and covered recovery proofs.
    ///
    /// Scheduled quorums and checkpoints referenced by scalar frontier state
    /// are always retained. The pruning decision and deletions run in the
    /// caller's transaction.
    async fn prune_federation_checkpoints(&mut self) -> Result<u64, Self::Err>;

    /// Durably latch a materialized-state audit mismatch.
    ///
    /// This operation is intentionally one-way. Ordinary startup, peer
    /// agreement, and checkpoint promotion cannot clear the latch.
    async fn latch_federation_materialized_state_audit_mismatch(&mut self)
        -> Result<(), Self::Err>;

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

    /// Get payment observations for one quote and context inside this transaction.
    async fn get_federation_payment_observations(
        &mut self,
        context: FederationPaymentObservationContext,
        quote: &str,
    ) -> Result<Vec<FederationPaymentObservationRecord>, Self::Err>;

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

    /// Persist one member's finalized-config activation confirmation.
    async fn add_federation_dkg_activation(
        &mut self,
        activation: &FederationDkgActivationRequest,
    ) -> Result<(), Self::Err>;

    /// Persist one distinct FROST DKG round-one broadcast.
    async fn add_federation_frost_dkg_round1(
        &mut self,
        request: &FederationFrostDkgRound1Request,
    ) -> Result<(), Self::Err>;

    /// Persist one confidential FROST DKG round-two contribution.
    async fn add_federation_frost_dkg_round2(
        &mut self,
        record: &FederationFrostDkgRound2Record,
    ) -> Result<(), Self::Err>;

    /// Persist encrypted local FROST DKG state before phase advancement.
    async fn set_federation_frost_dkg_local_state(
        &mut self,
        state: &FederationFrostDkgLocalState,
    ) -> Result<(), Self::Err>;

    /// Persist the completed canonical FROST root result.
    async fn add_federation_frost_root_record(
        &mut self,
        record: &FederationFrostRootRecord,
    ) -> Result<(), Self::Err>;

    /// Persist one signed canonical FROST root confirmation.
    async fn add_federation_frost_dkg_confirmation(
        &mut self,
        confirmation: &FederationFrostDkgConfirmationRequest,
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

    /// Persist one signed enrollment exactly once per setup member and invitation.
    async fn add_federation_setup_enrollment(
        &mut self,
        enrollment: &FederationSetupEnrollment,
    ) -> Result<(), Self::Err>;

    /// Persist the one immutable coordinator-signed proposal.
    async fn add_federation_setup_proposal(
        &mut self,
        proposal: &FederationSetupProposal,
    ) -> Result<(), Self::Err>;

    /// Persist the participant-local one-proposal approval fence.
    async fn add_federation_setup_approval_fence(
        &mut self,
        approval: &FederationSetupApproval,
    ) -> Result<(), Self::Err>;

    /// Persist one approval received by the coordinator.
    async fn add_federation_setup_approval(
        &mut self,
        approval: &FederationSetupApproval,
    ) -> Result<(), Self::Err>;

    /// Persist the one complete certified manifest.
    async fn add_federation_setup_manifest(
        &mut self,
        manifest: &FederationSetupManifest,
    ) -> Result<(), Self::Err>;

    /// Persist one member's final config activation reference.
    async fn add_federation_setup_activation(
        &mut self,
        activation: &FederationSetupActivation,
    ) -> Result<(), Self::Err>;

    /// Atomically insert an enrollment and consume its invitation.
    async fn consume_federation_setup_invitation(
        &mut self,
        expected: &FederationSetupInvitationRecord,
        consumed: &FederationSetupInvitationRecord,
        enrollment: &FederationSetupEnrollment,
    ) -> Result<(), Self::Err> {
        expected
            .validate_transition_to(consumed)
            .map_err(|error| Self::Err::from(Error::Federation(error)))?;
        enrollment
            .validate(FederationSetupUrlPolicy::AllowInsecureLocalhost)
            .map_err(|error| Self::Err::from(Error::Federation(error)))?;
        if expected.setup_id != enrollment.setup_id
            || expected.invitation_id != enrollment.invitation_id
            || expected.member_id != enrollment.member_id
            || consumed.state != crate::federation::FederationSetupInvitationState::Consumed
        {
            return Err(Self::Err::from(Error::Federation(
                crate::federation::FederationError::InvalidSetup {
                    reason: "setup enrollment does not match consumed invitation".to_string(),
                },
            )));
        }
        self.add_federation_setup_enrollment(enrollment).await?;
        self.transition_federation_setup_invitation(expected, consumed)
            .await
    }

    /// Atomically persist the local signed enrollment and advance to enrolled.
    async fn record_federation_setup_enrollment(
        &mut self,
        expected: &FederationSetupSession,
        updated: &FederationSetupSession,
        enrollment: &FederationSetupEnrollment,
    ) -> Result<(), Self::Err> {
        expected
            .validate_transition_to(updated)
            .map_err(|error| Self::Err::from(Error::Federation(error)))?;
        enrollment
            .validate(FederationSetupUrlPolicy::AllowInsecureLocalhost)
            .map_err(|error| Self::Err::from(Error::Federation(error)))?;
        if expected.participant_phase != FederationSetupParticipantPhase::IdentityCreated
            || updated.participant_phase != FederationSetupParticipantPhase::Enrolled
            || expected.setup_id != enrollment.setup_id
            || expected.local_role.member_id() != enrollment.member_id
        {
            return Err(Self::Err::from(Error::Federation(
                crate::federation::FederationError::InvalidSetup {
                    reason: "setup enrollment does not match the local enrollment transition"
                        .to_string(),
                },
            )));
        }
        self.add_federation_setup_enrollment(enrollment).await?;
        self.transition_federation_setup_session(expected, updated)
            .await
    }

    /// Atomically persist an immutable proposal and advance coordinator state.
    async fn publish_federation_setup_proposal(
        &mut self,
        expected: &FederationSetupSession,
        updated: &FederationSetupSession,
        proposal: &FederationSetupProposal,
    ) -> Result<(), Self::Err> {
        expected
            .validate_transition_to(updated)
            .map_err(|error| Self::Err::from(Error::Federation(error)))?;
        proposal
            .validate(FederationSetupUrlPolicy::AllowInsecureLocalhost)
            .map_err(|error| Self::Err::from(Error::Federation(error)))?;
        if expected.setup_id != proposal.body.setup_id
            || expected.coordinator_phase
                != Some(FederationSetupCoordinatorPhase::CollectingEnrollments)
            || updated.coordinator_phase != Some(FederationSetupCoordinatorPhase::ProposalPublished)
            || expected.participant_phase != FederationSetupParticipantPhase::Enrolled
            || updated.participant_phase != FederationSetupParticipantPhase::Enrolled
        {
            return Err(Self::Err::from(Error::Federation(
                crate::federation::FederationError::InvalidSetup {
                    reason: "setup proposal does not match the coordinator publication transition"
                        .to_string(),
                },
            )));
        }
        self.add_federation_setup_proposal(proposal).await?;
        self.transition_federation_setup_session(expected, updated)
            .await
    }

    /// Atomically persist the exact proposal reviewed by this local member.
    async fn review_federation_setup_proposal(
        &mut self,
        expected: &FederationSetupSession,
        updated: &FederationSetupSession,
        proposal: &FederationSetupProposal,
    ) -> Result<(), Self::Err> {
        expected
            .validate_transition_to(updated)
            .map_err(|error| Self::Err::from(Error::Federation(error)))?;
        proposal
            .validate(FederationSetupUrlPolicy::AllowInsecureLocalhost)
            .map_err(|error| Self::Err::from(Error::Federation(error)))?;
        if expected.participant_phase != FederationSetupParticipantPhase::Enrolled
            || updated.participant_phase != FederationSetupParticipantPhase::ProposalReviewed
            || expected.setup_id != proposal.body.setup_id
            || !proposal
                .body
                .enrollments
                .contains_key(&expected.local_role.member_id())
        {
            return Err(Self::Err::from(Error::Federation(
                crate::federation::FederationError::InvalidSetup {
                    reason: "setup proposal does not match the local review transition".to_string(),
                },
            )));
        }
        self.add_federation_setup_proposal(proposal).await?;
        self.transition_federation_setup_session(expected, updated)
            .await
    }

    /// Atomically persist the local approval fence before advancing to approved.
    async fn fence_federation_setup_approval(
        &mut self,
        expected: &FederationSetupSession,
        updated: &FederationSetupSession,
        proposal: &FederationSetupProposal,
        approval: &FederationSetupApproval,
    ) -> Result<(), Self::Err> {
        expected
            .validate_transition_to(updated)
            .map_err(|error| Self::Err::from(Error::Federation(error)))?;
        proposal
            .validate(FederationSetupUrlPolicy::AllowInsecureLocalhost)
            .map_err(|error| Self::Err::from(Error::Federation(error)))?;
        approval
            .validate(proposal)
            .map_err(|error| Self::Err::from(Error::Federation(error)))?;
        if updated.participant_phase != FederationSetupParticipantPhase::Approved
            || expected.setup_id != proposal.body.setup_id
            || expected.local_role.member_id() != approval.member_id
        {
            return Err(Self::Err::from(Error::Federation(
                crate::federation::FederationError::InvalidSetup {
                    reason: "setup approval fence does not match local session".to_string(),
                },
            )));
        }
        self.add_federation_setup_approval_fence(approval).await?;
        self.transition_federation_setup_session(expected, updated)
            .await
    }

    /// Atomically persist a certified manifest and advance one local lifecycle.
    async fn store_federation_setup_manifest(
        &mut self,
        expected: &FederationSetupSession,
        updated: &FederationSetupSession,
        manifest: &FederationSetupManifest,
    ) -> Result<(), Self::Err> {
        expected
            .validate_transition_to(updated)
            .map_err(|error| Self::Err::from(Error::Federation(error)))?;
        manifest
            .validate(FederationSetupUrlPolicy::AllowInsecureLocalhost)
            .map_err(|error| Self::Err::from(Error::Federation(error)))?;
        let coordinator_certified = expected.coordinator_phase
            == Some(FederationSetupCoordinatorPhase::ProposalPublished)
            && updated.coordinator_phase == Some(FederationSetupCoordinatorPhase::Certified);
        let participant_certified = expected.participant_phase
            == FederationSetupParticipantPhase::Approved
            && updated.participant_phase == FederationSetupParticipantPhase::Certified;
        if expected.setup_id != manifest.proposal.body.setup_id
            || (!coordinator_certified && !participant_certified)
        {
            return Err(Self::Err::from(Error::Federation(
                crate::federation::FederationError::InvalidSetup {
                    reason: "setup manifest does not match a certification transition".to_string(),
                },
            )));
        }
        self.add_federation_setup_manifest(manifest).await?;
        self.transition_federation_setup_session(expected, updated)
            .await
    }

    /// Atomically persist the final config reference and activate this member.
    async fn activate_federation_setup(
        &mut self,
        expected: &FederationSetupSession,
        updated: &FederationSetupSession,
        activation: &FederationSetupActivation,
    ) -> Result<(), Self::Err> {
        expected
            .validate_transition_to(updated)
            .map_err(|error| Self::Err::from(Error::Federation(error)))?;
        activation
            .validate()
            .map_err(|error| Self::Err::from(Error::Federation(error)))?;
        if updated.participant_phase != FederationSetupParticipantPhase::Activated
            || expected.setup_id != activation.setup_id
            || expected.local_role.member_id() != activation.member_id
        {
            return Err(Self::Err::from(Error::Federation(
                crate::federation::FederationError::InvalidSetup {
                    reason: "setup activation does not match local session".to_string(),
                },
            )));
        }
        self.add_federation_setup_activation(activation).await?;
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

    /// Get the first accepted federation journal entry reserving a conflict key.
    async fn get_federation_journal_entry_by_conflict_key(
        &self,
        conflict_key: &FederationOperationConflictKey,
    ) -> Result<Option<FederationJournalEntry>, Self::Err>;

    /// Get accepted federation journal entries starting at `next_index`.
    async fn get_federation_journal_entries_from(
        &self,
        next_index: u64,
    ) -> Result<Vec<FederationJournalEntry>, Self::Err>;

    /// Get a bounded accepted-journal range in consensus order.
    async fn get_federation_journal_entries_range(
        &self,
        next_index: u64,
        end_index_exclusive: u64,
        limit: usize,
    ) -> Result<Vec<FederationJournalEntry>, Self::Err>;

    /// Get persisted effectful DKG sequence watermarks by member.
    async fn get_federation_dkg_member_sequences(
        &self,
    ) -> Result<BTreeMap<MemberId, u64>, Self::Err>;

    /// Get bounded scalar checkpoint and rolling-digest metadata.
    async fn get_federation_checkpoint_frontier(
        &self,
    ) -> Result<FederationCheckpointFrontier, Self::Err>;

    /// Get one persisted checkpoint payload by ID.
    async fn get_federation_checkpoint(
        &self,
        checkpoint_id: FederationCheckpointId,
    ) -> Result<Option<FederationCheckpoint>, Self::Err>;

    /// Get the newest persisted candidate of one checkpoint kind.
    async fn get_latest_federation_checkpoint(
        &self,
        kind: FederationCheckpointKind,
    ) -> Result<Option<FederationCheckpoint>, Self::Err>;

    /// Get the next scheduled candidate after the supplied sequence.
    ///
    /// Candidate capture is independent of signature collection. Quorums must
    /// still be assembled and promoted in lineage order.
    async fn get_next_federation_scheduled_checkpoint(
        &self,
        scheduled_checkpoint_sequence: u64,
    ) -> Result<Option<FederationCheckpoint>, Self::Err>;

    /// Get persisted checkpoint payloads after a finalized-item cursor.
    async fn get_federation_checkpoints_after(
        &self,
        finalized_next_item_index: u64,
    ) -> Result<Vec<FederationCheckpoint>, Self::Err>;

    /// Get all persisted signatures for one checkpoint ID.
    async fn get_federation_checkpoint_signatures(
        &self,
        checkpoint_id: FederationCheckpointId,
    ) -> Result<Vec<FederationCheckpointSignature>, Self::Err>;

    /// Get one persisted assembled quorum by checkpoint ID.
    async fn get_federation_checkpoint_quorum(
        &self,
        checkpoint_id: FederationCheckpointId,
    ) -> Result<Option<FederationCheckpointQuorum>, Self::Err>;

    /// Get the newest assembled quorum of one checkpoint kind.
    ///
    /// Callers must verify the returned proof against the active public config.
    async fn get_latest_federation_checkpoint_quorum(
        &self,
        kind: FederationCheckpointKind,
    ) -> Result<Option<FederationCheckpointQuorum>, Self::Err>;

    /// Get the first assembled scheduled quorum after `scheduled_checkpoint_sequence`.
    ///
    /// Callers must verify the returned proof against the active public config.
    async fn get_next_federation_scheduled_checkpoint_quorum(
        &self,
        scheduled_checkpoint_sequence: u64,
    ) -> Result<Option<FederationCheckpointQuorum>, Self::Err>;

    /// Get the locally trusted assembled checkpoint quorum.
    ///
    /// Callers must re-verify the bounded proof before using it at startup.
    async fn get_trusted_federation_checkpoint_quorum(
        &self,
    ) -> Result<Option<FederationCheckpointQuorum>, Self::Err>;

    /// Get all persisted AlephBFT finalized items in accepted item order.
    async fn get_federation_aleph_bft_finalized_items(
        &self,
    ) -> Result<Vec<FederationAlephBftFinalizedItem>, Self::Err>;

    /// Get persisted AlephBFT finalized items starting at `accepted_item_index`.
    async fn get_federation_aleph_bft_finalized_items_from(
        &self,
        accepted_item_index: u64,
    ) -> Result<Vec<FederationAlephBftFinalizedItem>, Self::Err>;

    /// Get a bounded finalized-item range in accepted-item order.
    async fn get_federation_aleph_bft_finalized_items_range(
        &self,
        accepted_item_index: u64,
        end_item_index_exclusive: u64,
        limit: usize,
    ) -> Result<Vec<FederationAlephBftFinalizedItem>, Self::Err>;

    /// Get raw AlephBFT backup bytes for one local session.
    async fn get_federation_aleph_bft_backup_bytes(
        &self,
        session: u64,
    ) -> Result<Option<Vec<u8>>, Self::Err>;

    /// Get individually persisted AlephBFT backup chunks for one local session.
    ///
    /// Backends that stored legacy backups as one blob may return that blob as
    /// a single chunk. The default preserves compatibility for those backends.
    async fn get_federation_aleph_bft_backup_chunks(
        &self,
        session: u64,
    ) -> Result<Vec<Vec<u8>>, Self::Err> {
        Ok(self
            .get_federation_aleph_bft_backup_bytes(session)
            .await?
            .into_iter()
            .collect())
    }

    /// Get a bounded range of AlephBFT backup chunks for one local session.
    async fn get_federation_aleph_bft_backup_chunks_from(
        &self,
        session: u64,
        from_chunk: u64,
        limit: usize,
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

    /// Get payment observations for one quote and context in consensus order.
    async fn get_federation_payment_observations(
        &self,
        context: FederationPaymentObservationContext,
        quote: &str,
    ) -> Result<Vec<FederationPaymentObservationRecord>, Self::Err>;

    /// Get all payment observations in consensus order.
    async fn get_all_federation_payment_observations(
        &self,
    ) -> Result<Vec<FederationPaymentObservationRecord>, Self::Err>;

    /// Get the direct registry of consensus-authoritative materialized row keys.
    async fn get_federation_materialized_state_keys(
        &self,
    ) -> Result<Vec<FederationMaterializedStateKeyRecord>, Self::Err>;

    /// Get a complete application-owned projection encoded as bounded JSON.
    async fn get_federation_application_projection_snapshot(
        &self,
        application_id: crate::federation::FederationFrostApplicationId,
    ) -> Result<serde_json::Value, Self::Err>;

    /// Get one application-owned action without loading unrelated projection history.
    ///
    /// The application defines the canonical 32-byte action identifier and JSON record.
    /// Unsupported applications and missing actions return `None`.
    async fn get_federation_application_action(
        &self,
        application_id: crate::federation::FederationFrostApplicationId,
        action_id: [u8; 32],
    ) -> Result<Option<serde_json::Value>, Self::Err>;

    /// Get minimal mint-quote state for convergence projection in input order.
    async fn get_federation_mint_quote_states(
        &self,
        quote_ids: &[QuoteId],
    ) -> Result<Vec<Option<FederationMintQuoteState>>, Self::Err>;

    /// Get melt quotes needed for convergence projection by quote ID.
    ///
    /// Only matching rows are returned; callers reconstruct deterministic
    /// ordering and missing-row markers from their input key set.
    async fn get_federation_melt_quotes(
        &self,
        quote_ids: &[QuoteId],
    ) -> Result<Vec<mint::MeltQuote>, Self::Err>;

    /// Get completed operations needed for convergence projection by operation ID.
    ///
    /// Only matching rows are returned; callers reconstruct deterministic
    /// ordering and missing-row markers from their input key set.
    async fn get_federation_completed_operations(
        &self,
        operation_ids: &[uuid::Uuid],
    ) -> Result<Vec<mint::Operation>, Self::Err>;

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

    /// Get all durable FROST DKG round-one broadcasts.
    async fn get_federation_frost_dkg_round1(
        &self,
    ) -> Result<Vec<FederationFrostDkgRound1Request>, Self::Err>;

    /// Get all durable FROST DKG round-two contributions.
    async fn get_federation_frost_dkg_round2(
        &self,
    ) -> Result<Vec<FederationFrostDkgRound2Record>, Self::Err>;

    /// Get encrypted local FROST DKG phase records.
    async fn get_federation_frost_dkg_local_states(
        &self,
    ) -> Result<Vec<FederationFrostDkgLocalState>, Self::Err>;

    /// Get completed canonical FROST root records.
    async fn get_federation_frost_root_records(
        &self,
    ) -> Result<Vec<FederationFrostRootRecord>, Self::Err>;

    /// Get signed canonical FROST root confirmations.
    async fn get_federation_frost_dkg_confirmations(
        &self,
    ) -> Result<Vec<FederationFrostDkgConfirmationRequest>, Self::Err>;
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
