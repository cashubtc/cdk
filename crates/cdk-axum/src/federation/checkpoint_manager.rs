//! Checkpoint capture, proof persistence, promotion, fetch, and operator audit.
//!
//! This manager may mutate checkpoint records only under the operation pipeline
//! lock. It never performs peer network requests and never orders operations.

#[cfg(test)]
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use cdk::federation::{
    FederationAlephBftFinalizedItem, FederationAlephBftFinalizedItemStatus, FederationCheckpoint,
    FederationCheckpointAuditResponse, FederationCheckpointBoundary,
    FederationCheckpointFetchResponse, FederationCheckpointId, FederationCheckpointKind,
    FederationCheckpointQuorum, FederationCheckpointQuorumResponse, FederationCheckpointSignature,
    FederationCheckpointSignatureResponse, FederationCheckpointSummary,
    FederationCheckpointSummaryResponse, FederationConsensusItem, FederationError,
    FederationRecoveryBarrier, FEDERATION_REST_API_VERSION,
};

use super::node_runtime::{
    checkpoint_matches_boundary, checkpoint_summary, materialized_state_digest_with_metrics,
    runtime_unavailable, stored_checkpoint_quorum_summary,
    verify_checkpoint_signature_with_metrics, FederationNodeRuntime,
};

const RECOVERY_BARRIER_PROPOSAL_LEASE: Duration = Duration::from_secs(30);
// Reuse a recent checkpoint instead of ordering another barrier while the
// uncovered live tail remains bounded by one maximum-sized AlephBFT unit.
const RECOVERY_CHECKPOINT_MIN_FINALIZED_ITEM_ADVANCE: u64 = 64;

impl FederationNodeRuntime {
    pub(super) async fn capture_scheduled_checkpoint_if_eligible_under_application_lock(
        &self,
    ) -> Result<(), cdk::Error> {
        let mint = self.active_mint()?;
        let runtime_config = mint
            .federation_config()
            .ok_or_else(|| runtime_unavailable("mint configuration"))?;
        let public_config = runtime_config.public_config();
        let db = mint.localstore();

        let latest_scheduled_quorum = db
            .get_latest_federation_checkpoint_quorum(FederationCheckpointKind::Scheduled)
            .await?;
        let latest_scheduled = match latest_scheduled_quorum.as_ref() {
            Some(quorum) => {
                super::verify_checkpoint_quorum_with_metrics(
                    quorum,
                    public_config,
                    runtime_config.member_id(),
                )?;
                Some(&quorum.checkpoint)
            }
            None => None,
        };
        let scheduled_sequence = latest_scheduled
            .map(|checkpoint| checkpoint.scheduled_checkpoint_sequence)
            .unwrap_or(0);
        if let Some(candidate) = db
            .get_latest_federation_checkpoint(FederationCheckpointKind::Scheduled)
            .await?
        {
            candidate.validate_against_config(public_config)?;
            if candidate.scheduled_checkpoint_sequence > scheduled_sequence {
                return Ok(());
            }
        }

        let Some(boundary) = self
            .current_checkpoint_boundary_under_application_lock()
            .await?
        else {
            return Ok(());
        };
        let batch_base_finalized_index = latest_scheduled
            .map(|checkpoint| checkpoint.finalized_next_item_index)
            .unwrap_or(0);
        let expected_batch_items = boundary
            .finalized_next_item_index
            .checked_sub(batch_base_finalized_index)
            .ok_or_else(|| FederationError::CheckpointInvalid {
                reason: "scheduled checkpoint finalized frontier regressed".to_string(),
            })?;
        if expected_batch_items > cdk::federation::FEDERATION_MAX_CHECKPOINT_BATCH_FINALIZED_ITEMS {
            return Err(FederationError::CheckpointInvalid {
                reason: "scheduled checkpoint batch exceeds the finalized-item maximum".to_string(),
            }
            .into());
        }
        let batch_item_limit = usize::try_from(expected_batch_items)
            .expect("bounded checkpoint finalized-item count fits in usize");
        let batch_finalized_items = db
            .get_federation_aleph_bft_finalized_items_range(
                batch_base_finalized_index,
                boundary.finalized_next_item_index,
                batch_item_limit,
            )
            .await?;
        if batch_finalized_items.len() != batch_item_limit {
            return Err(FederationError::CheckpointInvalid {
                reason: "scheduled checkpoint batch has missing finalized items".to_string(),
            }
            .into());
        }
        let batch_canonical_bytes =
            cdk::federation::federation_checkpoint_batch_canonical_bytes(&batch_finalized_items)?;
        if batch_canonical_bytes > cdk::federation::FEDERATION_MAX_CHECKPOINT_BATCH_CANONICAL_BYTES
        {
            return Err(FederationError::CheckpointInvalid {
                reason: "scheduled checkpoint batch exceeds the canonical byte maximum".to_string(),
            }
            .into());
        }
        if !boundary.is_scheduled_eligible(
            public_config.checkpoint_policy,
            latest_scheduled,
            batch_canonical_bytes,
        )? {
            return Ok(());
        }
        #[cfg(test)]
        if self
            .components
            .faults
            .suppress_next_scheduled_checkpoint_capture
            .swap(false, Ordering::AcqRel)
        {
            return Ok(());
        }

        #[cfg(feature = "prometheus")]
        let checkpoint_started_at = std::time::Instant::now();
        let materialized_state_digest =
            materialized_state_digest_with_metrics(mint, runtime_config.member_id()).await?;
        let checkpoint = boundary.scheduled_checkpoint(
            public_config,
            latest_scheduled,
            materialized_state_digest,
            batch_canonical_bytes,
        )?;
        let signature = FederationCheckpointSignature::new_signed(
            &checkpoint,
            runtime_config.private_config(),
        )?;
        let mut tx = db.begin_transaction().await?;
        tx.add_federation_checkpoint_candidate(&checkpoint).await?;
        tx.add_federation_checkpoint_signature(&signature).await?;
        tx.commit().await?;
        #[cfg(feature = "prometheus")]
        {
            let member = runtime_config.member_id().to_string();
            cdk_prometheus::METRICS.record_federation_checkpoint_work(
                &member,
                cdk_prometheus::FederationCheckpointWork::ScheduledCheckpointsCreated,
                1,
            );
            cdk_prometheus::METRICS.record_federation_checkpoint_duration(
                &member,
                cdk_prometheus::FederationCheckpointOperation::ScheduledCheckpointCreation,
                checkpoint_started_at.elapsed().as_secs_f64(),
            );
        }
        Ok(())
    }

    /// Recover the one scheduled candidate that a crash may have interrupted.
    ///
    /// Normal checkpoint production is driven by finalized-unit application. This startup-only
    /// hook closes the gap between committing one eligible unit and persisting its candidate
    /// without turning peer polling into a checkpoint-production loop.
    pub(crate) async fn recover_scheduled_checkpoint_candidate_at_startup(
        &self,
    ) -> Result<(), cdk::Error> {
        let _application = self.components.operation.lock().await;
        self.capture_scheduled_checkpoint_if_eligible_under_application_lock()
            .await
    }

    /// Recover a barrier checkpoint interrupted after durable unit finalization.
    pub(crate) async fn recover_recovery_checkpoint_candidate_at_startup(
        &self,
    ) -> Result<(), cdk::Error> {
        let _application = self.components.operation.lock().await;
        let consensus = self
            .consensus
            .as_ref()
            .ok_or_else(|| runtime_unavailable("consensus"))?;
        let consensus = consensus.lock().await;
        let status = consensus.aleph_bft_status()?;
        let Some(last_item) = consensus.aleph_bft_finalized_items()?.last() else {
            return Ok(());
        };
        if last_item.session != status.session
            || last_item.unit_finalization_index.saturating_add(1)
                != status.next_unit_finalization_index
        {
            return Ok(());
        }
        let finalized_items = consensus
            .aleph_bft_finalized_items()?
            .iter()
            .rev()
            .take_while(|item| {
                item.session == last_item.session
                    && item.unit_finalization_index == last_item.unit_finalization_index
            })
            .cloned()
            .collect::<Vec<_>>();
        drop(consensus);
        self.capture_recovery_checkpoint_for_finalized_items_under_application_lock(
            &finalized_items,
        )
        .await?;
        Ok(())
    }

    #[cfg(test)]
    pub(crate) fn suppress_next_scheduled_checkpoint_capture(&self) {
        self.components
            .faults
            .suppress_next_scheduled_checkpoint_capture
            .store(true, Ordering::Release);
    }

    #[cfg(test)]
    pub(crate) fn suppress_next_recovery_checkpoint_capture(&self) {
        self.components
            .faults
            .suppress_next_recovery_checkpoint_capture
            .store(true, Ordering::Release);
    }

    #[cfg(test)]
    pub(crate) fn fail_next_checkpoint_promotion(&self) {
        self.components
            .faults
            .fail_next_checkpoint_promotion
            .store(true, Ordering::Release);
    }

    /// Run the operator-requested full-state audit against the exact trusted checkpoint frontier.
    pub(crate) async fn audit_trusted_checkpoint_materialized_state(
        &self,
    ) -> Result<FederationCheckpointAuditResponse, cdk::Error> {
        let _application = self.components.operation.lock().await;
        let mint = self.active_mint()?;
        let runtime_config = mint
            .federation_config()
            .ok_or_else(|| runtime_unavailable("mint configuration"))?;
        let db = mint.localstore();
        let frontier = db.get_federation_checkpoint_frontier().await?;
        if frontier.materialized_state_audit_mismatch {
            return Err(FederationError::CheckpointInvalid {
                reason: "materialized-state audit mismatch is already latched".to_string(),
            }
            .into());
        }
        let quorum = db
            .get_trusted_federation_checkpoint_quorum()
            .await?
            .ok_or_else(|| FederationError::CheckpointInvalid {
                reason: "operator audit requires a trusted checkpoint".to_string(),
            })?;
        super::verify_checkpoint_quorum_with_metrics(
            &quorum,
            runtime_config.public_config(),
            runtime_config.member_id(),
        )?;
        let checkpoint = &quorum.checkpoint;
        let checkpoint_id = checkpoint.id()?;
        if frontier.trusted_checkpoint_id != Some(checkpoint_id)
            || !frontier.exactly_matches_checkpoint(checkpoint)
        {
            return Err(FederationError::CheckpointInvalid {
                reason: "operator audit requires the current durable frontier to equal the trusted checkpoint"
                    .to_string(),
            }
            .into());
        }

        let materialized_state_digest =
            materialized_state_digest_with_metrics(mint, runtime_config.member_id()).await?;
        if materialized_state_digest != checkpoint.materialized_state_digest {
            let mut tx = db.begin_transaction().await?;
            tx.latch_federation_materialized_state_audit_mismatch()
                .await?;
            tx.commit().await?;
            return Err(FederationError::MaterializedStateAuditMismatch {
                checkpoint_id,
                expected: checkpoint.materialized_state_digest,
                found: materialized_state_digest,
            }
            .into());
        }

        Ok(FederationCheckpointAuditResponse {
            version: FEDERATION_REST_API_VERSION,
            federation_id: runtime_config.federation_id(),
            member_id: runtime_config.member_id(),
            checkpoint: FederationCheckpointSummary::from_checkpoint(checkpoint)?,
            materialized_state_digest,
        })
    }

    async fn current_checkpoint_boundary_under_application_lock(
        &self,
    ) -> Result<Option<FederationCheckpointBoundary>, cdk::Error> {
        let mint = self.active_mint()?;
        let frontier = mint
            .localstore()
            .get_federation_checkpoint_frontier()
            .await?;
        let (session, next_unit_finalization_index, next_unit_index_by_member) = {
            let consensus = self
                .consensus
                .as_ref()
                .ok_or_else(|| runtime_unavailable("consensus"))?
                .lock()
                .await;
            (
                consensus.session(),
                consensus.status().next_unit_finalization_index,
                consensus.next_unit_index_by_member().clone(),
            )
        };
        if frontier.finalized_next_item_index == 0 || next_unit_finalization_index == 0 {
            return Ok(None);
        }
        Ok(Some(FederationCheckpointBoundary::from_complete_unit(
            &frontier,
            session,
            next_unit_finalization_index,
            next_unit_index_by_member,
        )?))
    }

    pub(crate) async fn checkpoint_summary(
        &self,
        next_scheduled_quorum_after: Option<u64>,
    ) -> Result<FederationCheckpointSummaryResponse, cdk::Error> {
        let mint = self.active_mint()?;
        let runtime_config = mint
            .federation_config()
            .ok_or_else(|| runtime_unavailable("mint configuration"))?;
        let public_config = runtime_config.public_config();
        let db = mint.localstore();

        let latest_scheduled_candidate = db
            .get_latest_federation_checkpoint(FederationCheckpointKind::Scheduled)
            .await?
            .map(|checkpoint| checkpoint_summary(&checkpoint, public_config))
            .transpose()?;
        let latest_recovery_candidate = db
            .get_latest_federation_checkpoint(FederationCheckpointKind::Recovery)
            .await?
            .map(|checkpoint| checkpoint_summary(&checkpoint, public_config))
            .transpose()?;
        let latest_scheduled_quorum = db
            .get_latest_federation_checkpoint_quorum(FederationCheckpointKind::Scheduled)
            .await?
            .map(|quorum| stored_checkpoint_quorum_summary(&quorum, public_config))
            .transpose()?;
        let latest_recovery_quorum = db
            .get_latest_federation_checkpoint_quorum(FederationCheckpointKind::Recovery)
            .await?
            .map(|quorum| stored_checkpoint_quorum_summary(&quorum, public_config))
            .transpose()?;
        let next_scheduled_quorum = match next_scheduled_quorum_after {
            Some(sequence) => db
                .get_next_federation_scheduled_checkpoint_quorum(sequence)
                .await?
                .map(|quorum| stored_checkpoint_quorum_summary(&quorum, public_config))
                .transpose()?,
            None => None,
        };

        Ok(FederationCheckpointSummaryResponse {
            version: FEDERATION_REST_API_VERSION,
            federation_id: runtime_config.federation_id(),
            member_id: runtime_config.member_id(),
            latest_scheduled_candidate,
            latest_recovery_candidate,
            latest_scheduled_quorum,
            latest_recovery_quorum,
            next_scheduled_quorum,
        })
    }

    pub(crate) async fn request_recovery_checkpoint(&self) {
        self.components.checkpoint.request_recovery().await;
    }

    pub(crate) async fn recovery_checkpoint_is_requested(&self) -> bool {
        self.components.checkpoint.recovery_is_requested().await
    }

    async fn clear_recovery_checkpoint_request(&self) {
        self.components.checkpoint.clear_recovery().await;
    }

    pub(crate) async fn propose_recovery_barrier_if_requested(&self) -> Result<bool, cdk::Error> {
        if !self.recovery_checkpoint_is_requested().await
            || !matches!(
                self.sync_status().await,
                None | Some(crate::FederationSyncStatus::Healthy)
            )
            || !self.live_session_available().await
        {
            return Ok(false);
        }
        if !self
            .components
            .checkpoint
            .reserve_recovery_proposal(Instant::now(), RECOVERY_BARRIER_PROPOSAL_LEASE)
            .await
        {
            return Ok(false);
        }

        let _application = self.components.operation.lock().await;
        if !self.recovery_checkpoint_is_requested().await {
            return Ok(false);
        }
        let mint = self.active_mint()?;
        let runtime_config = mint
            .federation_config()
            .ok_or_else(|| runtime_unavailable("mint configuration"))?;
        let public_config = runtime_config.public_config();
        let db = mint.localstore();
        let latest_scheduled_quorum = db
            .get_latest_federation_checkpoint_quorum(FederationCheckpointKind::Scheduled)
            .await?;
        let latest_scheduled = match latest_scheduled_quorum.as_ref() {
            Some(quorum) => Some(
                super::verify_checkpoint_quorum_with_metrics(
                    quorum,
                    public_config,
                    runtime_config.member_id(),
                )?
                .checkpoint()
                .clone(),
            ),
            None => None,
        };
        let consensus = self
            .consensus
            .as_ref()
            .ok_or_else(|| runtime_unavailable("consensus"))?;
        let mut consensus = consensus.lock().await;
        let requested_after = consensus.next_item_index();
        if consensus.has_pending_aleph_bft_recovery_barrier() {
            return Ok(false);
        }

        if self
            .recent_checkpoint_candidate_frontier(public_config, requested_after)
            .await?
        {
            self.clear_recovery_checkpoint_request().await;
            return Ok(false);
        }

        let (scheduled_sequence, scheduled_anchor_id) = match latest_scheduled.as_ref() {
            Some(checkpoint) => (
                checkpoint.scheduled_checkpoint_sequence,
                Some(checkpoint.id()?),
            ),
            None => (0, None),
        };
        let barrier = FederationRecoveryBarrier::new(
            runtime_config.federation_id(),
            scheduled_sequence,
            scheduled_anchor_id,
            requested_after,
        )?;
        self.clear_recovery_checkpoint_request().await;
        match consensus.submit_aleph_bft_recovery_barrier(barrier) {
            Ok(true) => {}
            Ok(false) => {
                self.request_recovery_checkpoint().await;
                return Ok(false);
            }
            Err(err) => {
                self.request_recovery_checkpoint().await;
                return Err(err.into());
            }
        }
        drop(consensus);
        drop(_application);
        self.mark_local_consensus_activity().await;
        #[cfg(test)]
        self.finalize_unit_locally_for_test().await?;
        Ok(true)
    }

    async fn recent_checkpoint_candidate_frontier(
        &self,
        public_config: &cdk::federation::FederationConfig,
        requested_after: u64,
    ) -> Result<bool, cdk::Error> {
        let db = self.active_mint()?.localstore();
        let mut newest_finalized_frontier = None;
        for kind in [
            FederationCheckpointKind::Scheduled,
            FederationCheckpointKind::Recovery,
        ] {
            if let Some(checkpoint) = db.get_latest_federation_checkpoint(kind).await? {
                checkpoint.validate_against_config(public_config)?;
                newest_finalized_frontier = Some(
                    newest_finalized_frontier
                        .unwrap_or(0)
                        .max(checkpoint.finalized_next_item_index),
                );
            }
            if let Some(quorum) = db.get_latest_federation_checkpoint_quorum(kind).await? {
                let verified = quorum.verify(public_config)?;
                newest_finalized_frontier = Some(
                    newest_finalized_frontier
                        .unwrap_or(0)
                        .max(verified.checkpoint().finalized_next_item_index),
                );
            }
        }
        Ok(newest_finalized_frontier.is_some_and(|frontier| {
            frontier >= requested_after
                || requested_after.saturating_sub(frontier)
                    < RECOVERY_CHECKPOINT_MIN_FINALIZED_ITEM_ADVANCE
        }))
    }

    pub(super) async fn capture_recovery_checkpoint_for_finalized_items_under_application_lock(
        &self,
        finalized_items: &[FederationAlephBftFinalizedItem],
    ) -> Result<Option<FederationCheckpoint>, cdk::Error> {
        let Some(barrier) = finalized_items
            .iter()
            .filter(|item| item.status == FederationAlephBftFinalizedItemStatus::Accepted)
            .filter_map(|item| match &item.item {
                FederationConsensusItem::RecoveryBarrier(barrier) => Some(barrier),
                FederationConsensusItem::Operation(_)
                | FederationConsensusItem::ModuleFact(_)
                | FederationConsensusItem::Default(_) => None,
            })
            .max_by_key(|barrier| barrier.requested_after_finalized_item_index)
        else {
            return Ok(None);
        };
        let Some(boundary) = self
            .current_checkpoint_boundary_under_application_lock()
            .await?
        else {
            return Ok(None);
        };
        if boundary.finalized_next_item_index <= barrier.requested_after_finalized_item_index {
            return Ok(None);
        }

        let mint = self.active_mint()?;
        let runtime_config = mint
            .federation_config()
            .ok_or_else(|| runtime_unavailable("mint configuration"))?;
        let public_config = runtime_config.public_config();
        boundary.validate_against_config(public_config)?;
        let db = mint.localstore();
        if self
            .recent_checkpoint_candidate_frontier(
                public_config,
                barrier.requested_after_finalized_item_index,
            )
            .await?
        {
            return Ok(None);
        }
        let latest_scheduled_quorum = db
            .get_latest_federation_checkpoint_quorum(FederationCheckpointKind::Scheduled)
            .await?;
        let latest_scheduled = match latest_scheduled_quorum.as_ref() {
            Some(quorum) => {
                super::verify_checkpoint_quorum_with_metrics(
                    quorum,
                    public_config,
                    runtime_config.member_id(),
                )?;
                Some(&quorum.checkpoint)
            }
            None => None,
        };
        if latest_scheduled.is_some_and(|checkpoint| {
            boundary
                .committed_frontier()
                .ensure_covers(&checkpoint.committed_frontier())
                .is_err()
        }) {
            return Ok(None);
        }
        if latest_scheduled
            .is_some_and(|checkpoint| checkpoint_matches_boundary(checkpoint, &boundary))
        {
            return Ok(None);
        }
        let (scheduled_sequence, scheduled_anchor_id) = match latest_scheduled {
            Some(checkpoint) => (
                checkpoint.scheduled_checkpoint_sequence,
                Some(checkpoint.id()?),
            ),
            None => (0, None),
        };
        if barrier.scheduled_checkpoint_sequence != scheduled_sequence
            || barrier.scheduled_anchor_id != scheduled_anchor_id
        {
            let has_current_anchor_barrier = finalized_items
                .iter()
                .filter(|item| item.status == FederationAlephBftFinalizedItemStatus::Accepted)
                .filter_map(|item| match &item.item {
                    FederationConsensusItem::RecoveryBarrier(barrier)
                        if barrier.scheduled_checkpoint_sequence == scheduled_sequence
                            && barrier.scheduled_anchor_id == scheduled_anchor_id =>
                    {
                        Some(barrier)
                    }
                    FederationConsensusItem::Operation(_)
                    | FederationConsensusItem::RecoveryBarrier(_)
                    | FederationConsensusItem::ModuleFact(_)
                    | FederationConsensusItem::Default(_) => None,
                })
                .max_by_key(|barrier| barrier.requested_after_finalized_item_index)
                .is_some();
            if !has_current_anchor_barrier {
                self.request_recovery_checkpoint().await;
                return Ok(None);
            }
        }
        if let Some(quorum) = db
            .get_latest_federation_checkpoint_quorum(FederationCheckpointKind::Recovery)
            .await?
        {
            super::verify_checkpoint_quorum_with_metrics(
                &quorum,
                public_config,
                runtime_config.member_id(),
            )?;
            if boundary
                .committed_frontier()
                .ensure_covers(&quorum.checkpoint.committed_frontier())
                .is_err()
            {
                return Ok(None);
            }
            if quorum.checkpoint.scheduled_checkpoint_sequence == scheduled_sequence
                && quorum.checkpoint.scheduled_anchor_id == scheduled_anchor_id
                && checkpoint_matches_boundary(&quorum.checkpoint, &boundary)
            {
                return Ok(None);
            }
        }
        if let Some(candidate) = db
            .get_latest_federation_checkpoint(FederationCheckpointKind::Recovery)
            .await?
        {
            candidate.validate_against_config(public_config)?;
            if candidate.scheduled_checkpoint_sequence == scheduled_sequence
                && candidate.scheduled_anchor_id == scheduled_anchor_id
                && checkpoint_matches_boundary(&candidate, &boundary)
            {
                return Ok(Some(candidate));
            }
        }

        let batch_base_finalized_index = latest_scheduled
            .map(|checkpoint| checkpoint.finalized_next_item_index)
            .unwrap_or(0);
        let expected_batch_items = boundary
            .finalized_next_item_index
            .checked_sub(batch_base_finalized_index)
            .ok_or_else(|| FederationError::CheckpointInvalid {
                reason: "recovery checkpoint finalized frontier regressed".to_string(),
            })?;
        if expected_batch_items > cdk::federation::FEDERATION_MAX_CHECKPOINT_BATCH_FINALIZED_ITEMS {
            return Err(FederationError::CheckpointInvalid {
                reason: "recovery checkpoint batch exceeds the finalized-item maximum".to_string(),
            }
            .into());
        }
        let batch_item_limit = usize::try_from(expected_batch_items)
            .expect("bounded checkpoint finalized-item count fits in usize");
        let batch_finalized_items = db
            .get_federation_aleph_bft_finalized_items_range(
                batch_base_finalized_index,
                boundary.finalized_next_item_index,
                batch_item_limit,
            )
            .await?;
        if batch_finalized_items.len() != batch_item_limit {
            return Err(FederationError::CheckpointInvalid {
                reason: "recovery checkpoint batch has missing finalized items".to_string(),
            }
            .into());
        }
        let batch_canonical_bytes =
            cdk::federation::federation_checkpoint_batch_canonical_bytes(&batch_finalized_items)?;
        if batch_canonical_bytes > cdk::federation::FEDERATION_MAX_CHECKPOINT_BATCH_CANONICAL_BYTES
        {
            return Err(FederationError::CheckpointInvalid {
                reason: "recovery checkpoint batch exceeds the canonical byte maximum".to_string(),
            }
            .into());
        }

        #[cfg(test)]
        if self
            .components
            .faults
            .suppress_next_recovery_checkpoint_capture
            .swap(false, Ordering::AcqRel)
        {
            return Ok(None);
        }

        #[cfg(feature = "prometheus")]
        let checkpoint_started_at = std::time::Instant::now();
        let materialized_state_digest =
            materialized_state_digest_with_metrics(mint, runtime_config.member_id()).await?;
        let checkpoint = boundary.recovery_checkpoint(
            public_config,
            latest_scheduled,
            materialized_state_digest,
        )?;
        let signature = FederationCheckpointSignature::new_signed(
            &checkpoint,
            runtime_config.private_config(),
        )?;
        let mut tx = db.begin_transaction().await?;
        tx.add_federation_checkpoint_candidate(&checkpoint).await?;
        tx.add_federation_checkpoint_signature(&signature).await?;
        tx.commit().await?;
        #[cfg(feature = "prometheus")]
        {
            let member = runtime_config.member_id().to_string();
            cdk_prometheus::METRICS.record_federation_checkpoint_work(
                &member,
                cdk_prometheus::FederationCheckpointWork::RecoveryCheckpointsCreated,
                1,
            );
            cdk_prometheus::METRICS.record_federation_checkpoint_duration(
                &member,
                cdk_prometheus::FederationCheckpointOperation::RecoveryCheckpointCreation,
                checkpoint_started_at.elapsed().as_secs_f64(),
            );
        }
        Ok(Some(checkpoint))
    }

    pub(crate) async fn fetch_checkpoint(
        &self,
        checkpoint_id: FederationCheckpointId,
    ) -> Result<FederationCheckpointFetchResponse, cdk::Error> {
        let mint = self.active_mint()?;
        let runtime_config = mint
            .federation_config()
            .ok_or_else(|| runtime_unavailable("mint configuration"))?;
        let public_config = runtime_config.public_config();
        let db = mint.localstore();
        let checkpoint = db.get_federation_checkpoint(checkpoint_id).await?;
        if let Some(checkpoint) = checkpoint.as_ref() {
            checkpoint.validate_against_config(public_config)?;
            if checkpoint.id()? != checkpoint_id {
                return Err(FederationError::CheckpointInvalid {
                    reason: "stored checkpoint does not match its lookup id".to_string(),
                }
                .into());
            }
        }
        // Serving a stored proof is not a trust transition. The fetching peer verifies the full
        // quorum before persistence, catch-up, or promotion.
        let quorum = db.get_federation_checkpoint_quorum(checkpoint_id).await?;
        Ok(FederationCheckpointFetchResponse {
            version: FEDERATION_REST_API_VERSION,
            federation_id: runtime_config.federation_id(),
            member_id: runtime_config.member_id(),
            checkpoint,
            quorum,
        })
    }

    pub(crate) async fn local_checkpoint_signature(
        &self,
        checkpoint_id: FederationCheckpointId,
    ) -> Result<FederationCheckpointSignatureResponse, cdk::Error> {
        let mint = self.active_mint()?;
        let runtime_config = mint
            .federation_config()
            .ok_or_else(|| runtime_unavailable("mint configuration"))?;
        let public_config = runtime_config.public_config();
        let db = mint.localstore();
        let checkpoint = db.get_federation_checkpoint(checkpoint_id).await?;
        let signature = match checkpoint.as_ref() {
            Some(checkpoint) => {
                checkpoint.validate_against_config(public_config)?;
                db.get_federation_checkpoint_signatures(checkpoint_id)
                    .await?
                    .into_iter()
                    .find(|signature| signature.signer == runtime_config.member_id())
            }
            None => None,
        };
        if let (Some(checkpoint), Some(signature)) = (checkpoint.as_ref(), signature.as_ref()) {
            let member = public_config.member(runtime_config.member_id()).ok_or(
                FederationError::UnknownMember {
                    member_id: runtime_config.member_id(),
                },
            )?;
            verify_checkpoint_signature_with_metrics(
                signature,
                checkpoint,
                member,
                runtime_config.member_id(),
            )?;
        }
        Ok(FederationCheckpointSignatureResponse {
            version: FEDERATION_REST_API_VERSION,
            federation_id: runtime_config.federation_id(),
            member_id: runtime_config.member_id(),
            signature,
        })
    }

    pub(crate) async fn persist_checkpoint_quorum(
        &self,
        quorum: &FederationCheckpointQuorum,
    ) -> Result<FederationCheckpointQuorumResponse, cdk::Error> {
        let _application = self.components.operation.lock().await;
        let mint = self.active_mint()?;
        let runtime_config = mint
            .federation_config()
            .ok_or_else(|| runtime_unavailable("mint configuration"))?;
        let verified_quorum = super::verify_checkpoint_quorum_with_metrics(
            quorum,
            runtime_config.public_config(),
            runtime_config.member_id(),
        )?;
        let checkpoint = verified_quorum.checkpoint();
        let checkpoint_id = checkpoint.id()?;
        let db = mint.localstore();
        let mut tx = db.begin_transaction().await?;
        tx.add_federation_checkpoint_quorum(quorum).await?;
        tx.commit().await?;
        let frontier = db.get_federation_checkpoint_frontier().await?;
        let locally_reached = !frontier.materialized_state_audit_mismatch
            && frontier.ensure_covers_checkpoint(checkpoint).is_ok();
        let should_promote = if locally_reached {
            match db.get_trusted_federation_checkpoint_quorum().await? {
                Some(current) => {
                    let verified_current = super::verify_checkpoint_quorum_with_metrics(
                        &current,
                        runtime_config.public_config(),
                        runtime_config.member_id(),
                    )?;
                    if verified_current.checkpoint().covers(checkpoint) {
                        false
                    } else {
                        checkpoint.validate_promotion_from(Some(verified_current.checkpoint()))?;
                        true
                    }
                }
                None => true,
            }
        } else {
            false
        };
        if should_promote {
            #[cfg(feature = "prometheus")]
            let promotion_started_at = std::time::Instant::now();
            let mut tx = db.begin_transaction().await?;
            tx.promote_verified_federation_checkpoint(&verified_quorum)
                .await?;
            tx.commit().await?;
            #[cfg(feature = "prometheus")]
            {
                let member = runtime_config.member_id().to_string();
                cdk_prometheus::METRICS.record_federation_checkpoint_work(
                    &member,
                    cdk_prometheus::FederationCheckpointWork::CheckpointsPromoted,
                    1,
                );
                cdk_prometheus::METRICS.record_federation_checkpoint_duration(
                    &member,
                    cdk_prometheus::FederationCheckpointOperation::CheckpointPromotion,
                    promotion_started_at.elapsed().as_secs_f64(),
                );
            }
        }
        if should_promote && checkpoint.kind == FederationCheckpointKind::Recovery {
            self.clear_recovery_checkpoint_request().await;
        }
        Ok(FederationCheckpointQuorumResponse {
            version: FEDERATION_REST_API_VERSION,
            federation_id: runtime_config.federation_id(),
            member_id: runtime_config.member_id(),
            checkpoint_id,
        })
    }

    pub(crate) async fn persist_checkpoint_signature(
        &self,
        checkpoint_id: FederationCheckpointId,
        signature: &FederationCheckpointSignature,
    ) -> Result<Option<FederationCheckpointQuorum>, cdk::Error> {
        let application = self.components.operation.lock().await;
        let mint = self.active_mint()?;
        let runtime_config = mint
            .federation_config()
            .ok_or_else(|| runtime_unavailable("mint configuration"))?;
        let public_config = runtime_config.public_config();
        let db = mint.localstore();
        let checkpoint = db
            .get_federation_checkpoint(checkpoint_id)
            .await?
            .ok_or_else(|| FederationError::CheckpointInvalid {
                reason: "cannot store a signature for an unknown checkpoint".to_string(),
            })?;
        checkpoint.validate_against_config(public_config)?;
        if checkpoint.id()? != checkpoint_id || signature.checkpoint_id != checkpoint_id {
            return Err(FederationError::CheckpointInvalid {
                reason: "checkpoint signature lookup binding mismatch".to_string(),
            }
            .into());
        }
        let member =
            public_config
                .member(signature.signer)
                .ok_or(FederationError::UnknownMember {
                    member_id: signature.signer,
                })?;
        verify_checkpoint_signature_with_metrics(
            signature,
            &checkpoint,
            member,
            runtime_config.member_id(),
        )?;
        let mut tx = db.begin_transaction().await?;
        tx.add_federation_checkpoint_signature(signature).await?;
        tx.commit().await?;

        let signatures = db
            .get_federation_checkpoint_signatures(checkpoint_id)
            .await?;
        if signatures.len() < usize::from(public_config.threshold_params.consensus_threshold) {
            return Ok(None);
        }
        let quorum = FederationCheckpointQuorum::new(checkpoint, signatures);
        super::verify_checkpoint_quorum_with_metrics(
            &quorum,
            public_config,
            runtime_config.member_id(),
        )?;
        drop(application);
        self.persist_checkpoint_quorum(&quorum).await?;
        Ok(Some(quorum))
    }
}
