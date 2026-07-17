//! Incremental verification of finalized federation history suffixes.

use std::collections::VecDeque;

use super::{
    FederationAlephBftFinalizedItem, FederationAlephBftRecoveryCursor,
    FederationAlephBftUnitPayload, FederationApplicationDigest, FederationApplicationFrontier,
    FederationApplicationState, FederationCatchUpOperation, FederationCheckpointFrontier,
    FederationCommittedFrontier, FederationConfig, FederationConsensusRecoveryBase,
    FederationConsensusRuntime, FederationError, FederationFinalizedFrontier,
    FederationFinalizedItemDigest, FederationJournalEntry, FederationJournalFrontier, MemberId,
    Result, VerifiedFederationCheckpointQuorum,
};

#[derive(Debug, Clone)]
enum FederationSuffixJournalClaim {
    Full(Box<FederationJournalEntry>),
    Compact(FederationCatchUpOperation),
}

impl FederationSuffixJournalClaim {
    fn application(&self) -> &FederationApplicationState {
        match self {
            Self::Full(entry) => &entry.application,
            Self::Compact(entry) => &entry.application,
        }
    }

    fn operation_id(&self) -> super::FederationOperationId {
        match self {
            Self::Full(entry) => entry.operation_id,
            Self::Compact(entry) => entry.operation_id,
        }
    }

    fn overlays_runtime_application(&self) -> bool {
        matches!(self, Self::Full(_)) && self.application().is_terminal()
    }

    fn ensure_matches(&self, derived: &FederationJournalEntry) -> Result<()> {
        let projection = match self {
            Self::Full(entry) => FederationCatchUpOperation::from(entry.as_ref()),
            Self::Compact(entry) => entry.clone(),
        };
        projection.ensure_matches_journal_entry(derived)?;

        if let Self::Full(entry) = self {
            if entry.envelope.canonical_bytes()? != derived.envelope.canonical_bytes()?
                || entry.conflict_keys != derived.conflict_keys
                || entry.consensus_evidence != derived.consensus_evidence
            {
                return Err(FederationError::AlephBftFinalizedItemRecoveryMismatch {
                    reason: format!(
                        "journal body or consensus evidence mismatch at operation index {}",
                        entry.index
                    ),
                });
            }
        }

        let derived_consensus_rejection = matches!(
            derived.application,
            FederationApplicationState::ConsensusRejected {}
        );
        let claimed_consensus_rejection = matches!(
            self.application(),
            FederationApplicationState::ConsensusRejected {}
        );
        if derived_consensus_rejection != claimed_consensus_rejection {
            return Err(FederationError::AlephBftFinalizedItemRecoveryMismatch {
                reason: format!(
                    "journal conflict outcome mismatch at operation index {}",
                    derived.index
                ),
            });
        }
        Ok(())
    }
}

/// Incremental verifier for one genesis- or checkpoint-rooted history suffix.
///
/// The verifier retains at most one unclosed AlephBFT unit. A page boundary does
/// not advance its committed frontier until the following unit or final target
/// proves that the buffered unit is complete.
#[derive(Debug)]
pub struct FederationSuffixVerifier {
    runtime: FederationConsensusRuntime,
    active_session: u64,
    base_journal_next_index: u64,
    journal_frontier: FederationJournalFrontier,
    finalized_frontier: FederationFinalizedFrontier,
    application_frontier: FederationApplicationFrontier,
    application_gap_seen: bool,
    pending_unit: Vec<FederationAlephBftFinalizedItem>,
    claims: VecDeque<FederationSuffixJournalClaim>,
    matched_claims: usize,
    finalized_input_started: bool,
}

impl FederationSuffixVerifier {
    /// Start verification from an explicit authenticated recovery base.
    pub fn new(
        config: &FederationConfig,
        member_id: MemberId,
        active_session: u64,
        base: FederationConsensusRecoveryBase<'_>,
    ) -> Result<Self> {
        config.validate()?;
        Self::new_for_validated_config(config, member_id, active_session, base)
    }

    /// Start verification using a federation config already validated at its trust boundary.
    pub fn new_for_validated_config(
        config: &FederationConfig,
        member_id: MemberId,
        active_session: u64,
        base: FederationConsensusRecoveryBase<'_>,
    ) -> Result<Self> {
        let runtime = FederationConsensusRuntime::from_recovery_base_for_validated_config(
            config,
            member_id,
            active_session,
            base,
        )?;
        let (base_journal_next_index, journal_frontier, finalized_frontier, application_frontier) =
            match base {
                FederationConsensusRecoveryBase::Genesis => (
                    0,
                    FederationJournalFrontier {
                        next_index: 0,
                        order_digest: super::FederationJournalDigest::genesis(),
                    },
                    FederationFinalizedFrontier {
                        next_item_index: 0,
                        item_digest: FederationFinalizedItemDigest::genesis(),
                    },
                    FederationApplicationFrontier {
                        next_index: 0,
                        digest: FederationApplicationDigest::genesis(),
                    },
                ),
                FederationConsensusRecoveryBase::Checkpoint(checkpoint) => {
                    let checkpoint = checkpoint.checkpoint();
                    (
                        checkpoint.journal_next_index,
                        FederationJournalFrontier {
                            next_index: checkpoint.journal_next_index,
                            order_digest: checkpoint.journal_order_digest,
                        },
                        FederationFinalizedFrontier {
                            next_item_index: checkpoint.finalized_next_item_index,
                            item_digest: checkpoint.finalized_item_digest,
                        },
                        FederationApplicationFrontier {
                            next_index: checkpoint.journal_next_index,
                            digest: checkpoint.application_digest,
                        },
                    )
                }
            };
        Ok(Self {
            runtime,
            active_session,
            base_journal_next_index,
            journal_frontier,
            finalized_frontier,
            application_frontier,
            application_gap_seen: false,
            pending_unit: Vec::new(),
            claims: VecDeque::new(),
            matched_claims: 0,
            finalized_input_started: false,
        })
    }

    /// Resume page verification from a transactionally persisted durable runtime.
    ///
    /// The runtime already contains the verified suffix after `base_journal_next_index`.
    /// This avoids replaying that suffix for every later page while the supplied
    /// rolling frontier preserves the journal, finalized, and application digests.
    pub fn from_durable_runtime(
        runtime: FederationConsensusRuntime,
        active_session: u64,
        base_journal_frontier: FederationJournalFrontier,
        frontier: FederationCommittedFrontier,
    ) -> Result<Self> {
        frontier.validate_shape()?;
        if !frontier.is_fully_terminal() {
            return Err(suffix_mismatch(
                "durable checkpoint catch-up runtime has a non-terminal application suffix",
            ));
        }
        let retained_journal_entries = u64::try_from(runtime.journal().entries().len())
            .map_err(|_| suffix_mismatch("retained journal length does not fit in u64"))?;
        let runtime_journal_base = runtime
            .journal()
            .next_index()
            .checked_sub(retained_journal_entries)
            .ok_or_else(|| suffix_mismatch("retained journal cursor underflow"))?;
        if runtime.journal().next_index() != frontier.journal.next_index
            || runtime_journal_base > base_journal_frontier.next_index
            || runtime
                .journal()
                .order_digest_before(base_journal_frontier.next_index)
                != Some(base_journal_frontier.order_digest)
            || runtime.journal().latest_order_digest() != frontier.journal.order_digest
        {
            return Err(suffix_mismatch(
                "durable checkpoint catch-up runtime does not match its journal frontier",
            ));
        }
        let status = runtime.aleph_bft_status()?;
        if status.next_item_index != frontier.finalized.next_item_index
            || runtime.aleph_bft_recovery_cursor()? != frontier.aleph_bft
        {
            return Err(suffix_mismatch(
                "durable checkpoint catch-up runtime does not match its finalized frontier",
            ));
        }
        let matched_claims = runtime.journal().entries().len();
        Ok(Self {
            runtime,
            active_session,
            base_journal_next_index: base_journal_frontier.next_index,
            journal_frontier: frontier.journal,
            finalized_frontier: frontier.finalized,
            application_frontier: frontier.application,
            application_gap_seen: false,
            pending_unit: Vec::new(),
            claims: VecDeque::new(),
            matched_claims,
            finalized_input_started: false,
        })
    }

    /// Register bounded pre-checkpoint entries referenced by suffix operations.
    pub fn register_historical_entries(
        &mut self,
        entries: impl IntoIterator<Item = FederationJournalEntry>,
    ) -> Result<()> {
        if self.finalized_input_started {
            return Err(suffix_mismatch(
                "historical duplicate and conflict guards arrived after finalized history",
            ));
        }
        for entry in entries {
            if entry.index >= self.base_journal_next_index {
                return Err(suffix_mismatch(
                    "historical duplicate or conflict guard is not before the suffix base",
                ));
            }
            self.runtime.register_historical_journal_entry(entry)?;
        }
        Ok(())
    }

    /// Consume another consecutive finalized-item chunk.
    pub fn push_finalized_items(
        &mut self,
        items: &[FederationAlephBftFinalizedItem],
    ) -> Result<()> {
        self.finalized_input_started |= !items.is_empty();
        for item in items {
            let pending_len = u64::try_from(self.pending_unit.len()).map_err(|_| {
                suffix_mismatch("pending finalized unit length does not fit in u64")
            })?;
            let expected_item_index = self
                .finalized_frontier
                .next_item_index
                .checked_add(pending_len)
                .ok_or_else(|| suffix_mismatch("finalized item cursor overflow"))?;
            if item.accepted_item_index != expected_item_index {
                return Err(suffix_mismatch(format!(
                    "non-contiguous finalized item: expected {expected_item_index}, found {}",
                    item.accepted_item_index
                )));
            }

            if let Some(first) = self.pending_unit.first() {
                let same_unit = first.session == item.session
                    && first.unit_submitting_member == item.unit_submitting_member
                    && first.unit_index == item.unit_index;
                if !same_unit {
                    self.commit_pending_unit()?;
                }
            }
            let expected_in_unit = u64::try_from(self.pending_unit.len())
                .map_err(|_| suffix_mismatch("unit item count does not fit in u64"))?;
            if item.item_index_in_unit != expected_in_unit {
                return Err(suffix_mismatch(format!(
                    "non-contiguous item within finalized unit: expected {expected_in_unit}, found {}",
                    item.item_index_in_unit
                )));
            }
            self.pending_unit.push(item.clone());
        }
        Ok(())
    }

    /// Consume exact durable journal rows corresponding to finalized operations.
    pub fn push_journal_entries(&mut self, entries: &[FederationJournalEntry]) -> Result<()> {
        self.claims.extend(
            entries
                .iter()
                .cloned()
                .map(Box::new)
                .map(FederationSuffixJournalClaim::Full),
        );
        self.match_available_claims()
    }

    /// Consume compact peer journal projections corresponding to finalized operations.
    pub fn push_catch_up_operations(
        &mut self,
        entries: &[FederationCatchUpOperation],
    ) -> Result<()> {
        self.claims.extend(
            entries
                .iter()
                .cloned()
                .map(FederationSuffixJournalClaim::Compact),
        );
        self.match_available_claims()
    }

    /// Finish against exact durable local history frontiers.
    pub fn finish_for_durable_frontier(
        self,
        target: &FederationCheckpointFrontier,
    ) -> Result<VerifiedFederationSuffix> {
        if target.materialized_state_audit_mismatch {
            return Err(FederationError::CheckpointInvalid {
                reason: "materialized-state audit mismatch is latched".to_string(),
            });
        }
        self.finish(
            target.journal_frontier(),
            target.finalized_frontier(),
            target.application_frontier(),
            None,
        )
    }

    /// Finish against every signed frontier in a verified target checkpoint.
    pub fn finish_for_checkpoint(
        self,
        target: &VerifiedFederationCheckpointQuorum,
    ) -> Result<VerifiedFederationSuffix> {
        let target = target.checkpoint().committed_frontier();
        self.finish(
            target.journal,
            target.finalized,
            target.application,
            Some(target.aleph_bft),
        )
    }

    /// Finish one authenticated prefix page bounded by a verified checkpoint.
    ///
    /// The returned suffix may end before the target but can never cross any
    /// target cursor or watermark. Callers must use [`Self::finish_for_checkpoint`]
    /// before promoting the checkpoint.
    pub fn finish_for_checkpoint_prefix(
        self,
        target: &VerifiedFederationCheckpointQuorum,
    ) -> Result<VerifiedFederationSuffix> {
        let verified = self.finish_current()?;
        target
            .checkpoint()
            .committed_frontier()
            .ensure_covers(verified.frontier())?;
        Ok(verified)
    }

    fn finish(
        self,
        target_journal: FederationJournalFrontier,
        target_finalized: FederationFinalizedFrontier,
        target_application: FederationApplicationFrontier,
        target_aleph_bft: Option<FederationAlephBftRecoveryCursor>,
    ) -> Result<VerifiedFederationSuffix> {
        let verified = self.finish_current()?;
        let frontier = verified.frontier();
        if frontier.journal != target_journal
            || frontier.finalized != target_finalized
            || frontier.application != target_application
            || target_aleph_bft
                .as_ref()
                .is_some_and(|target| *target != frontier.aleph_bft)
        {
            return Err(suffix_mismatch(
                "verified suffix does not reach the exact target frontier",
            ));
        }
        Ok(verified)
    }

    fn finish_current(mut self) -> Result<VerifiedFederationSuffix> {
        self.commit_pending_unit()?;
        self.match_available_claims()?;
        if !self.claims.is_empty() || self.matched_claims != self.runtime.journal().entries().len()
        {
            return Err(suffix_mismatch(
                "finalized operations and journal entries do not have one-to-one correspondence",
            ));
        }

        let runtime_session = self.runtime.aleph_bft_status()?.session;
        if self.active_session > runtime_session {
            self.runtime.begin_aleph_bft_session(self.active_session)?;
        }
        let status = self.runtime.aleph_bft_status()?;
        let aleph_bft = FederationAlephBftRecoveryCursor {
            session: status.session,
            next_unit_finalization_index: status.next_unit_finalization_index,
            next_unit_index_by_member: self.runtime.next_unit_index_by_member().clone(),
        };
        let frontier = FederationCommittedFrontier {
            journal: self.journal_frontier,
            finalized: self.finalized_frontier,
            application: self.application_frontier,
            aleph_bft,
        };
        frontier.validate_shape()?;
        Ok(VerifiedFederationSuffix {
            runtime: self.runtime,
            frontier,
            terminal_application_count: self
                .application_frontier
                .next_index
                .saturating_sub(self.base_journal_next_index),
        })
    }

    fn commit_pending_unit(&mut self) -> Result<()> {
        if self.pending_unit.is_empty() {
            return Ok(());
        }
        let pending = std::mem::take(&mut self.pending_unit);
        let first = pending
            .first()
            .expect("non-empty pending unit has a first item");
        if pending.iter().any(|item| {
            item.session != first.session
                || item.unit_finalization_index != first.unit_finalization_index
                || item.unit_submitting_member != first.unit_submitting_member
                || item.unit_index != first.unit_index
                || item.unit_body_hash != first.unit_body_hash
        }) {
            return Err(suffix_mismatch(
                "finalized unit provenance changed within one unit body",
            ));
        }
        let runtime_session = self.runtime.aleph_bft_status()?.session;
        if first.session > runtime_session {
            self.runtime.begin_aleph_bft_session(first.session)?;
        } else if first.session < runtime_session {
            return Err(FederationError::AlephBftUnitSessionMismatch {
                expected: runtime_session,
                found: first.session,
            });
        }
        let payload = FederationAlephBftUnitPayload::new(
            self.runtime.federation_id(),
            first.unit_submitting_member,
            first.session,
            first.unit_index,
            pending.iter().map(|item| item.item.clone()).collect(),
        )?;
        if payload.body_hash != first.unit_body_hash {
            return Err(suffix_mismatch(
                "finalized unit body hash does not match its ordered item body",
            ));
        }

        let finalized_start = self.runtime.aleph_bft_finalized_items()?.len();
        self.runtime.finalize_aleph_bft_unit(payload)?;
        let generated = &self.runtime.aleph_bft_finalized_items()?[finalized_start..];
        if generated.len() != pending.len() {
            return Err(suffix_mismatch(
                "live finalization produced a different finalized item count",
            ));
        }
        for (generated, supplied) in generated.iter().zip(&pending) {
            generated.ensure_matches(supplied)?;
            self.finalized_frontier.item_digest =
                self.finalized_frontier.item_digest.append(generated)?;
            self.finalized_frontier.next_item_index = self
                .finalized_frontier
                .next_item_index
                .checked_add(1)
                .ok_or_else(|| suffix_mismatch("finalized item cursor overflow"))?;
        }
        self.journal_frontier.next_index = self.runtime.journal().next_index();
        self.journal_frontier.order_digest = self.runtime.journal().latest_order_digest();
        self.match_available_claims()
    }

    fn match_available_claims(&mut self) -> Result<()> {
        while let Some(claim) = self.claims.front() {
            let Some(derived) = self.runtime.journal().entries().get(self.matched_claims) else {
                break;
            };
            claim.ensure_matches(derived)?;
            let application = claim.application().clone();
            if application.is_terminal() && !self.application_gap_seen {
                self.application_frontier.digest = self
                    .application_frontier
                    .digest
                    .append_terminal(derived.index, derived.operation_id, &application)?;
                self.application_frontier.next_index = self
                    .application_frontier
                    .next_index
                    .checked_add(1)
                    .ok_or_else(|| suffix_mismatch("application cursor overflow"))?;
            } else if !application.is_terminal() {
                self.application_gap_seen = true;
            }
            if claim.overlays_runtime_application() {
                self.runtime
                    .update_application_state(claim.operation_id(), application)?;
            }
            self.claims.pop_front();
            self.matched_claims = self.matched_claims.saturating_add(1);
        }
        Ok(())
    }
}

/// Opaque proof that one suffix reached an exact durable or signed target.
#[derive(Debug)]
pub struct VerifiedFederationSuffix {
    runtime: FederationConsensusRuntime,
    frontier: FederationCommittedFrontier,
    terminal_application_count: u64,
}

impl VerifiedFederationSuffix {
    /// Return the exact verified committed frontier.
    #[must_use]
    pub const fn frontier(&self) -> &FederationCommittedFrontier {
        &self.frontier
    }

    /// Return the number of suffix operations in the terminal application prefix.
    #[must_use]
    pub const fn terminal_application_count(&self) -> u64 {
        self.terminal_application_count
    }

    /// Return the verified suffix journal rows.
    #[must_use]
    pub fn journal_entries(&self) -> &[FederationJournalEntry] {
        self.runtime.journal().entries()
    }

    /// Return the verified finalized suffix items.
    pub fn finalized_items(&self) -> Result<&[FederationAlephBftFinalizedItem]> {
        self.runtime.aleph_bft_finalized_items()
    }

    pub(crate) fn into_runtime(self) -> FederationConsensusRuntime {
        self.runtime
    }
}

fn suffix_mismatch(reason: impl Into<String>) -> FederationError {
    FederationError::AlephBftFinalizedItemRecoveryMismatch {
        reason: reason.into(),
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::str::FromStr;

    use serde_json::json;

    use super::*;
    use crate::mint_url::MintUrl;
    use crate::SecretKey;

    use super::super::{
        FederationAddressAllocationOperation, FederationAddressKeychain, FederationConsensusItem,
        FederationConsensusModuleFact, FederationFrostApplicationId, FederationId,
        FederationMaterializedStateDigest, FederationOperation, FederationOperationEnvelope,
        FederationOperationId, MemberConfigPrivate, MemberConfigPublic, ThresholdParams,
    };

    fn member_id(value: u16) -> MemberId {
        MemberId::new(value).expect("member id")
    }

    fn genesis_verifier(config: &FederationConfig) -> Result<FederationSuffixVerifier> {
        FederationSuffixVerifier::new(
            config,
            member_id(1),
            0,
            FederationConsensusRecoveryBase::Genesis,
        )
    }

    fn test_config() -> (FederationConfig, MemberConfigPrivate) {
        let first_secret = SecretKey::from_slice(&[41; 32]).expect("first identity secret");
        let second_secret = SecretKey::from_slice(&[42; 32]).expect("second identity secret");
        let members = BTreeMap::from([
            (
                member_id(1),
                MemberConfigPublic {
                    member_id: member_id(1),
                    public_mint_url: MintUrl::from_str("https://mint1.example.com")
                        .expect("mint url"),
                    federation_api_url: MintUrl::from_str("https://fed1.example.com")
                        .expect("federation url"),
                    identity_public_key: first_secret.public_key(),
                },
            ),
            (
                member_id(2),
                MemberConfigPublic {
                    member_id: member_id(2),
                    public_mint_url: MintUrl::from_str("https://mint2.example.com")
                        .expect("mint url"),
                    federation_api_url: MintUrl::from_str("https://fed2.example.com")
                        .expect("federation url"),
                    identity_public_key: second_secret.public_key(),
                },
            ),
        ]);
        let config = FederationConfig::new(
            ThresholdParams::new(2, 2, 2).expect("thresholds"),
            members,
            Vec::new(),
        )
        .expect("federation config");
        let private = MemberConfigPrivate {
            federation_id: Some(config.federation_id),
            member_id: member_id(1),
            identity_secret_key: first_secret,
            keyset_secret_shares: BTreeMap::new(),
            setup_transcript_hash: None,
        };
        (config, private)
    }

    fn operation(
        config: &FederationConfig,
        private: &MemberConfigPrivate,
        quote: &str,
        application: u8,
    ) -> FederationOperationEnvelope {
        let operation = FederationAddressAllocationOperation::new(
            quote,
            FederationFrostApplicationId::from_bytes([application; 32]).expect("application id"),
            1,
            FederationAddressKeychain::External,
        )
        .expect("address allocation operation");
        FederationOperationEnvelope::new(
            config.federation_id,
            FederationOperation::AddressAllocation(operation),
        )
        .authorize(private)
        .expect("authorized operation")
    }

    fn module_fact(name: &str) -> FederationConsensusItem {
        FederationConsensusItem::ModuleFact(FederationConsensusModuleFact::new(
            "suffix-test",
            name,
            json!({"name": name}),
        ))
    }

    struct Fixture {
        config: FederationConfig,
        finalized: Vec<FederationAlephBftFinalizedItem>,
        journal: Vec<FederationJournalEntry>,
        target: FederationCheckpointFrontier,
    }

    fn fixture() -> Fixture {
        let (config, private) = test_config();
        let mut runtime =
            FederationConsensusRuntime::authenticated_aleph_bft(&config, member_id(1), 0)
                .expect("runtime");
        for (unit_index, quote) in [(0, "first"), (1, "second")] {
            let payload = FederationAlephBftUnitPayload::new(
                config.federation_id,
                member_id(1),
                0,
                unit_index,
                vec![
                    FederationConsensusItem::operation(operation(
                        &config,
                        &private,
                        quote,
                        u8::try_from(unit_index + 1).expect("application byte"),
                    )),
                    module_fact(quote),
                ],
            )
            .expect("unit payload");
            runtime
                .finalize_aleph_bft_unit(payload)
                .expect("finalize unit");
        }
        let operation_ids = runtime
            .journal()
            .entries()
            .iter()
            .map(|entry| entry.operation_id)
            .collect::<Vec<_>>();
        for operation_id in operation_ids {
            runtime
                .update_application_state(operation_id, FederationApplicationState::Applied {})
                .expect("terminal application");
        }
        let finalized = runtime
            .aleph_bft_finalized_items()
            .expect("finalized items")
            .to_vec();
        let journal = runtime.journal().entries().to_vec();
        let finalized_item_digest = finalized
            .iter()
            .try_fold(FederationFinalizedItemDigest::genesis(), |digest, item| {
                digest.append(item)
            })
            .expect("finalized digest");
        let application_digest = journal
            .iter()
            .try_fold(FederationApplicationDigest::genesis(), |digest, entry| {
                digest.append_terminal(entry.index, entry.operation_id, &entry.application)
            })
            .expect("application digest");
        let target = FederationCheckpointFrontier {
            journal_next_index: runtime.journal().next_index(),
            journal_order_digest: runtime.journal().latest_order_digest(),
            finalized_next_item_index: u64::try_from(finalized.len()).expect("finalized count"),
            finalized_item_digest,
            application_next_index: runtime.journal().next_index(),
            application_digest,
            trusted_checkpoint_id: None,
            latest_audited_checkpoint_id: None,
            latest_materialized_state_digest: Some(FederationMaterializedStateDigest::genesis()),
            materialized_state_audit_mismatch: false,
        };
        Fixture {
            config,
            finalized,
            journal,
            target,
        }
    }

    fn verify_full(fixture: &Fixture, chunks: &[usize]) -> Result<VerifiedFederationSuffix> {
        let mut verifier = genesis_verifier(&fixture.config)?;
        let mut start = 0;
        for end in chunks {
            verifier.push_finalized_items(&fixture.finalized[start..*end])?;
            start = *end;
        }
        verifier.push_finalized_items(&fixture.finalized[start..])?;
        verifier.push_journal_entries(&fixture.journal)?;
        verifier.finish_for_durable_frontier(&fixture.target)
    }

    #[test]
    fn full_and_compact_paths_verify_the_same_frontier() {
        let fixture = fixture();
        let full = verify_full(&fixture, &[]).expect("full suffix");
        let mut compact = genesis_verifier(&fixture.config).expect("compact verifier");
        compact
            .push_finalized_items(&fixture.finalized)
            .expect("compact finalized items");
        compact
            .push_catch_up_operations(
                &fixture
                    .journal
                    .iter()
                    .map(FederationCatchUpOperation::from)
                    .collect::<Vec<_>>(),
            )
            .expect("compact journal");
        let compact = compact
            .finish_for_durable_frontier(&fixture.target)
            .expect("compact suffix");

        assert_eq!(full.frontier(), compact.frontier());
        let full = FederationConsensusRuntime::recover_verified_suffix(full);
        let compact = FederationConsensusRuntime::recover_verified_suffix(compact);
        assert_eq!(
            full.aleph_bft_status().expect("full status"),
            compact.aleph_bft_status().expect("compact status")
        );
        assert_eq!(
            full.journal()
                .entries()
                .iter()
                .map(|entry| (entry.index, entry.operation_id, entry.order_digest))
                .collect::<Vec<_>>(),
            compact
                .journal()
                .entries()
                .iter()
                .map(|entry| (entry.index, entry.operation_id, entry.order_digest))
                .collect::<Vec<_>>()
        );
        assert_eq!(
            serde_json::to_vec(
                full.aleph_bft_finalized_items()
                    .expect("full finalized items")
            )
            .expect("full finalized JSON"),
            serde_json::to_vec(
                compact
                    .aleph_bft_finalized_items()
                    .expect("compact finalized items")
            )
            .expect("compact finalized JSON")
        );
    }

    #[test]
    fn chunk_boundaries_do_not_change_verified_frontier() {
        let fixture = fixture();
        let whole = verify_full(&fixture, &[]).expect("whole suffix");
        for boundaries in [vec![1], vec![2], vec![1, 2, 3], vec![3]] {
            let chunked = verify_full(&fixture, &boundaries).expect("chunked suffix");
            assert_eq!(whole.frontier(), chunked.frontier());
        }
        let mut paged = genesis_verifier(&fixture.config).expect("paged verifier");
        for finalized_page in fixture.finalized.chunks(1) {
            paged
                .push_finalized_items(finalized_page)
                .expect("finalized page");
        }
        for journal_page in fixture.journal.chunks(1) {
            paged
                .push_journal_entries(journal_page)
                .expect("journal page");
        }
        let paged = paged
            .finish_for_durable_frontier(&fixture.target)
            .expect("paged suffix");
        assert_eq!(whole.frontier(), paged.frontier());
    }

    #[test]
    fn durable_resume_accepts_runtime_retaining_pre_checkpoint_history() {
        let fixture = fixture();
        let verified = verify_full(&fixture, &[]).expect("verified durable history");
        let runtime = FederationConsensusRuntime::recover_verified_suffix(verified);
        let committed_frontier = fixture
            .target
            .committed_frontier(
                runtime
                    .aleph_bft_recovery_cursor()
                    .expect("runtime recovery cursor"),
            )
            .expect("committed frontier");
        let checkpoint_journal_frontier = FederationJournalFrontier {
            next_index: 1,
            order_digest: fixture.journal[0].order_digest,
        };

        let resumed = FederationSuffixVerifier::from_durable_runtime(
            runtime,
            0,
            checkpoint_journal_frontier,
            committed_frontier,
        )
        .expect("resume from a checkpoint inside retained live history")
        .finish_for_durable_frontier(&fixture.target)
        .expect("finish retained live history");

        assert_eq!(
            resumed
                .journal_entries()
                .iter()
                .map(|entry| (entry.index, entry.operation_id, entry.order_digest))
                .collect::<Vec<_>>(),
            fixture
                .journal
                .iter()
                .map(|entry| (entry.index, entry.operation_id, entry.order_digest))
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn partial_unit_is_rejected_without_a_verified_result() {
        let fixture = fixture();
        let mut verifier = genesis_verifier(&fixture.config).expect("verifier");
        verifier
            .push_finalized_items(&fixture.finalized[..1])
            .expect("buffer partial unit");
        assert!(matches!(
            verifier.finish_for_durable_frontier(&fixture.target),
            Err(FederationError::AlephBftFinalizedItemRecoveryMismatch { .. })
        ));
    }

    #[test]
    fn finalized_commitment_mutations_fail_both_adapters_at_the_same_stage() {
        let fixture = fixture();
        let mut mutations = Vec::new();
        let mut item = fixture.finalized.clone();
        item[0].accepted_item_index = item[0].accepted_item_index.saturating_add(1);
        mutations.push(item);
        let mut item = fixture.finalized.clone();
        item[0].session = item[0].session.saturating_add(1);
        mutations.push(item);
        let mut item = fixture.finalized.clone();
        item[0].unit_finalization_index = item[0].unit_finalization_index.saturating_add(1);
        mutations.push(item);
        let mut item = fixture.finalized.clone();
        item[0].unit_submitting_member = member_id(2);
        mutations.push(item);
        let mut item = fixture.finalized.clone();
        item[0].unit_index = item[0].unit_index.saturating_add(1);
        mutations.push(item);
        let mut item = fixture.finalized.clone();
        item[0].item_index_in_unit = item[0].item_index_in_unit.saturating_add(1);
        mutations.push(item);
        let mut item = fixture.finalized.clone();
        item[0].operation_id = Some(FederationOperationId::from_bytes([91; 32]));
        mutations.push(item);
        let mut item = fixture.finalized.clone();
        item[0].status = super::super::FederationAlephBftFinalizedItemStatus::DuplicateOperation;
        mutations.push(item);
        let mut item = fixture.finalized.clone();
        item[0].unit_body_hash = super::super::FederationAlephBftUnitBodyHash::from_bytes([92; 32]);
        mutations.push(item);
        let mut item = fixture.finalized.clone();
        item[0].item = module_fact("changed");
        mutations.push(item);

        for mutation in mutations {
            let mut full = genesis_verifier(&fixture.config).expect("full verifier");
            let full_error = full
                .push_finalized_items(&mutation)
                .and_then(|()| full.push_journal_entries(&fixture.journal))
                .and_then(|()| {
                    full.finish_for_durable_frontier(&fixture.target)
                        .map(|_| ())
                })
                .expect_err("full mutation must fail");

            let mut compact = genesis_verifier(&fixture.config).expect("compact verifier");
            let compact_error = compact
                .push_finalized_items(&mutation)
                .and_then(|()| {
                    compact.push_catch_up_operations(
                        &fixture
                            .journal
                            .iter()
                            .map(FederationCatchUpOperation::from)
                            .collect::<Vec<_>>(),
                    )
                })
                .and_then(|()| {
                    compact
                        .finish_for_durable_frontier(&fixture.target)
                        .map(|_| ())
                })
                .expect_err("compact mutation must fail");
            assert_eq!(
                std::mem::discriminant(&full_error),
                std::mem::discriminant(&compact_error)
            );
        }
    }

    #[test]
    fn journal_commitment_mutations_fail_both_adapters_at_the_same_stage() {
        let fixture = fixture();
        let mut mutations = Vec::new();
        let mut entry = FederationCatchUpOperation::from(&fixture.journal[0]);
        entry.index = entry.index.saturating_add(1);
        mutations.push(entry);
        let mut entry = FederationCatchUpOperation::from(&fixture.journal[0]);
        entry.operation_id = FederationOperationId::from_bytes([93; 32]);
        mutations.push(entry);
        let mut entry = FederationCatchUpOperation::from(&fixture.journal[0]);
        entry.previous_order_digest = super::super::FederationJournalDigest::from_bytes([94; 32]);
        mutations.push(entry);
        let mut entry = FederationCatchUpOperation::from(&fixture.journal[0]);
        entry.order_digest = super::super::FederationJournalDigest::from_bytes([95; 32]);
        mutations.push(entry);
        let mut entry = FederationCatchUpOperation::from(&fixture.journal[0]);
        entry.application = FederationApplicationState::Rejected {
            response: crate::error::ErrorResponse::from(crate::Error::TokenAlreadySpent),
        };
        mutations.push(entry);

        for mutation in mutations {
            let mut full_journal = fixture.journal.clone();
            full_journal[0].index = mutation.index;
            full_journal[0].operation_id = mutation.operation_id;
            full_journal[0].previous_order_digest = mutation.previous_order_digest;
            full_journal[0].order_digest = mutation.order_digest;
            full_journal[0].application = mutation.application.clone();
            let mut full = genesis_verifier(&fixture.config).expect("full verifier");
            let full_error = full
                .push_finalized_items(&fixture.finalized)
                .and_then(|()| full.push_journal_entries(&full_journal))
                .and_then(|()| {
                    full.finish_for_durable_frontier(&fixture.target)
                        .map(|_| ())
                })
                .expect_err("full journal mutation must fail");

            let mut compact_entries = fixture
                .journal
                .iter()
                .map(FederationCatchUpOperation::from)
                .collect::<Vec<_>>();
            compact_entries[0] = mutation;
            let mut compact = genesis_verifier(&fixture.config).expect("compact verifier");
            let compact_error = compact
                .push_finalized_items(&fixture.finalized)
                .and_then(|()| compact.push_catch_up_operations(&compact_entries))
                .and_then(|()| {
                    compact
                        .finish_for_durable_frontier(&fixture.target)
                        .map(|_| ())
                })
                .expect_err("compact journal mutation must fail");
            assert_eq!(
                std::mem::discriminant(&full_error),
                std::mem::discriminant(&compact_error)
            );
        }
    }

    #[test]
    fn terminal_application_after_a_gap_does_not_advance_the_digest() {
        let mut fixture = fixture();
        fixture.journal[0].application = FederationApplicationState::Accepted {};
        fixture.target.application_next_index = 0;
        fixture.target.application_digest = FederationApplicationDigest::genesis();
        let mut verifier = genesis_verifier(&fixture.config).expect("verifier");
        verifier
            .push_finalized_items(&fixture.finalized)
            .expect("finalized items");
        verifier
            .push_journal_entries(&fixture.journal)
            .expect("buffer journal claims");
        let verified = verifier
            .finish_for_durable_frontier(&fixture.target)
            .expect("suffix with a terminal result beyond the gap");
        assert_eq!(verified.frontier().application.next_index, 0);
    }

    #[test]
    fn invalid_authorization_and_duplicates_reproduce_live_finalization() {
        let (config, private) = test_config();
        let valid = operation(&config, &private, "duplicate", 11);
        let invalid =
            FederationOperationEnvelope::new(config.federation_id, valid.operation.clone());
        let payload = FederationAlephBftUnitPayload::new(
            config.federation_id,
            member_id(1),
            0,
            0,
            vec![
                FederationConsensusItem::operation(valid.clone()),
                FederationConsensusItem::operation(valid),
                FederationConsensusItem::operation(invalid),
            ],
        )
        .expect("payload");
        let mut live =
            FederationConsensusRuntime::authenticated_aleph_bft(&config, member_id(1), 0)
                .expect("live runtime");
        live.finalize_aleph_bft_unit(payload)
            .expect("live finalization");
        let finalized = live
            .aleph_bft_finalized_items()
            .expect("finalized items")
            .to_vec();
        let mut journal = live.journal().entries().to_vec();
        journal[0].application = FederationApplicationState::Applied {};
        let finalized_item_digest = finalized
            .iter()
            .try_fold(FederationFinalizedItemDigest::genesis(), |digest, item| {
                digest.append(item)
            })
            .expect("finalized digest");
        let application_digest = FederationApplicationDigest::genesis()
            .append_terminal(0, journal[0].operation_id, &journal[0].application)
            .expect("application digest");
        let target = FederationCheckpointFrontier {
            journal_next_index: 1,
            journal_order_digest: live.journal().latest_order_digest(),
            finalized_next_item_index: 3,
            finalized_item_digest,
            application_next_index: 1,
            application_digest,
            trusted_checkpoint_id: None,
            latest_audited_checkpoint_id: None,
            latest_materialized_state_digest: None,
            materialized_state_audit_mismatch: false,
        };
        let mut verifier = genesis_verifier(&config).expect("verifier");
        verifier
            .push_finalized_items(&finalized)
            .expect("finalized suffix");
        verifier
            .push_journal_entries(&journal)
            .expect("journal suffix");
        let verified = verifier
            .finish_for_durable_frontier(&target)
            .expect("verified suffix");

        assert_eq!(
            verified
                .finalized_items()
                .expect("verified finalized items")
                .iter()
                .map(|item| item.status)
                .collect::<Vec<_>>(),
            finalized.iter().map(|item| item.status).collect::<Vec<_>>()
        );
        assert_eq!(verified.journal_entries().len(), 1);
    }

    #[test]
    fn genesis_frontier_uses_the_expected_federation_id() {
        let (config, _) = test_config();
        assert_ne!(config.federation_id, FederationId::from_bytes([0; 32]));
    }
}
