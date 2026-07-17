//! Checkpoint catch-up verification, durable staging, and application.
//!
//! Peer selection, page requests, and source failover live in `sync_loop`.
//! This module receives complete bounded pages only after network work has
//! finished, verifies each page against the trusted and target checkpoints,
//! stages it durably, applies deterministic effects, and promotes only the
//! exact target frontier. It never performs network I/O while holding the
//! operation-application or consensus mutation lock.

#[cfg(test)]
use std::sync::atomic::Ordering;

#[cfg(test)]
use cdk::federation::FederationCatchUpOperation;
use cdk::federation::{
    FederationCheckpointFrontier, FederationCheckpointQuorum, FederationConsensusRecoveryBase,
    FederationConsensusRuntime, FederationError, FederationJournalCatchUpResponse,
    FederationJournalEntry, FederationJournalFrontier, FederationSuffixVerifier,
    VerifiedFederationCheckpointQuorum,
};

use super::node_runtime::{runtime_unavailable, FederationNodeRuntime};

impl FederationNodeRuntime {
    #[cfg(test)]
    pub(crate) async fn apply_test_catch_up_response(
        &self,
        response: &FederationJournalCatchUpResponse,
    ) -> Result<Vec<FederationJournalEntry>, cdk::Error> {
        self.apply_verified_catch_up_response(response).await
    }

    #[cfg(test)]
    async fn apply_verified_catch_up_response(
        &self,
        response: &FederationJournalCatchUpResponse,
    ) -> Result<Vec<FederationJournalEntry>, cdk::Error> {
        let _application = self.components.operation.lock().await;
        self.apply_verified_catch_up_response_under_application_lock(response)
            .await
    }

    pub(crate) async fn apply_checkpoint_catch_up_response(
        &self,
        response: &FederationJournalCatchUpResponse,
        quorum: &FederationCheckpointQuorum,
    ) -> Result<Vec<FederationJournalEntry>, cdk::Error> {
        let _application = self.components.operation.lock().await;
        let mint = self.active_mint()?;
        let runtime_config = mint
            .federation_config()
            .cloned()
            .ok_or_else(|| runtime_unavailable("mint configuration"))?;
        #[cfg(feature = "prometheus")]
        let catch_up_started_at = std::time::Instant::now();
        let public_config = runtime_config.public_config();
        let verified_quorum = super::verify_checkpoint_quorum_with_metrics(
            quorum,
            public_config,
            runtime_config.member_id(),
            super::CheckpointQuorumVerificationContext::StartupCatchUp,
        )?;
        let checkpoint = verified_quorum.checkpoint();
        response.validate_checkpoint_binding(checkpoint)?;
        let db = mint.localstore();
        let frontier = db.get_federation_checkpoint_frontier().await?;
        let current = db.get_trusted_federation_checkpoint_quorum().await?;
        let current_verified = match current.as_ref() {
            Some(current) => Some(super::verify_checkpoint_quorum_with_metrics(
                current,
                public_config,
                runtime_config.member_id(),
                super::CheckpointQuorumVerificationContext::StartupCatchUp,
            )?),
            None => None,
        };
        checkpoint
            .validate_catch_up_batch_from(current.as_ref().map(|quorum| &quorum.checkpoint))?;
        if frontier.journal_next_index != response.from_index
            || frontier.application_next_index != response.from_index
            || frontier.finalized_next_item_index != response.aleph_bft_from_item_index
        {
            return Err(FederationError::CheckpointInvalid {
                reason: "catch-up response does not start at the durable local frontier"
                    .to_string(),
            }
            .into());
        }

        let applied = self
            .apply_verified_checkpoint_catch_up_response_under_application_lock(
                response,
                &frontier,
                current_verified.as_ref(),
                &verified_quorum,
                response.complete && response.aleph_bft_finalized_items_complete,
            )
            .await?;

        let frontier = db.get_federation_checkpoint_frontier().await?;
        let target_reached = response.complete && response.aleph_bft_finalized_items_complete;
        if !target_reached {
            checkpoint.committed_frontier().ensure_covers(
                &frontier.committed_frontier(
                    self.consensus
                        .as_ref()
                        .ok_or_else(|| runtime_unavailable("consensus"))?
                        .lock()
                        .await
                        .aleph_bft_recovery_cursor()?,
                )?,
            )?;
            #[cfg(feature = "prometheus")]
            {
                let member = runtime_config.member_id().to_string();
                let page_bytes = response.ensure_within_checkpoint_page_limit()? as u64;
                cdk_prometheus::METRICS.record_federation_checkpoint_work(
                    &member,
                    cdk_prometheus::FederationCheckpointWork::CatchUpPagesApplied,
                    1,
                );
                cdk_prometheus::METRICS.record_federation_checkpoint_work(
                    &member,
                    cdk_prometheus::FederationCheckpointWork::CatchUpAppliedBytes,
                    page_bytes,
                );
            }
            return Ok(applied);
        }
        if !frontier.exactly_matches_checkpoint(checkpoint) {
            return Err(FederationError::CheckpointInvalid {
                reason: "durable catch-up frontiers do not equal the target checkpoint".to_string(),
            }
            .into());
        }
        #[cfg(test)]
        if self
            .components
            .faults
            .fail_next_checkpoint_promotion
            .swap(false, Ordering::AcqRel)
        {
            return Err(FederationError::CheckpointInvalid {
                reason: "injected failure before checkpoint promotion".to_string(),
            }
            .into());
        }
        let checkpoint_runtime = FederationConsensusRuntime::recover_verified_suffix(
            FederationSuffixVerifier::new_for_validated_config(
                public_config,
                runtime_config.member_id(),
                checkpoint.aleph_bft_session,
                FederationConsensusRecoveryBase::Checkpoint(&verified_quorum),
            )?
            .finish_for_checkpoint(&verified_quorum)?,
        );
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
        let consensus = self
            .consensus
            .as_ref()
            .ok_or_else(|| runtime_unavailable("consensus"))?;
        *consensus.lock().await = checkpoint_runtime;
        #[cfg(feature = "prometheus")]
        {
            let member = runtime_config.member_id().to_string();
            cdk_prometheus::METRICS.record_federation_checkpoint_work(
                &member,
                cdk_prometheus::FederationCheckpointWork::CatchUpBatchesApplied,
                1,
            );
            cdk_prometheus::METRICS.record_federation_checkpoint_work(
                &member,
                cdk_prometheus::FederationCheckpointWork::CatchUpPagesApplied,
                1,
            );
            cdk_prometheus::METRICS.record_federation_checkpoint_work(
                &member,
                cdk_prometheus::FederationCheckpointWork::CatchUpAppliedBytes,
                response.ensure_within_checkpoint_page_limit()? as u64,
            );
            cdk_prometheus::METRICS.record_federation_checkpoint_duration(
                &member,
                cdk_prometheus::FederationCheckpointOperation::CatchUpBatch,
                catch_up_started_at.elapsed().as_secs_f64(),
            );
        }
        Ok(applied)
    }

    async fn apply_verified_checkpoint_catch_up_response_under_application_lock(
        &self,
        response: &FederationJournalCatchUpResponse,
        durable_frontier: &FederationCheckpointFrontier,
        base_checkpoint: Option<&VerifiedFederationCheckpointQuorum>,
        target_checkpoint: &VerifiedFederationCheckpointQuorum,
        target_reached: bool,
    ) -> Result<Vec<FederationJournalEntry>, cdk::Error> {
        #[cfg(feature = "prometheus")]
        let runtime_config = self
            .active_mint()?
            .federation_config()
            .cloned()
            .ok_or_else(|| runtime_unavailable("mint configuration"))?;
        let consensus = self
            .consensus
            .as_ref()
            .ok_or_else(|| runtime_unavailable("consensus"))?;
        let mut consensus = consensus.lock().await;
        let base_journal_frontier = base_checkpoint
            .map_or_else(FederationJournalFrontier::genesis, |checkpoint| {
                checkpoint.checkpoint().committed_frontier().journal
            });
        let status = consensus.aleph_bft_status()?;
        if response.from_index != consensus.journal().next_index()
            || response.aleph_bft_from_item_index != status.next_item_index
        {
            return Err(FederationError::CheckpointInvalid {
                reason: "checkpoint catch-up response does not begin after the local suffix"
                    .to_string(),
            }
            .into());
        }

        let historical_entries = crate::historical_entries_referenced_by_finalized_items(
            self.active_mint()?,
            &response.aleph_bft_finalized_items,
            base_journal_frontier.next_index,
        )
        .await?;
        let target_session = target_checkpoint.checkpoint().aleph_bft_session;
        let local_entry_count = consensus.journal().entries().len();
        let committed_frontier =
            durable_frontier.committed_frontier(consensus.aleph_bft_recovery_cursor()?)?;
        let mut verifier = FederationSuffixVerifier::from_durable_runtime(
            consensus.clone(),
            target_session,
            base_journal_frontier,
            committed_frontier,
        )?;
        verifier.register_historical_entries(historical_entries)?;
        verifier.push_finalized_items(&response.aleph_bft_finalized_items)?;
        verifier.push_catch_up_operations(&response.entries)?;
        let verified_suffix = if target_reached {
            verifier.finish_for_checkpoint(target_checkpoint)?
        } else {
            verifier.finish_for_checkpoint_prefix(target_checkpoint)?
        };
        let recovered_entries = verified_suffix.journal_entries().to_vec();
        if recovered_entries.len() < local_entry_count {
            return Err(crate::aleph_bft_catch_up_recovery_mismatch(format!(
                "verified checkpoint suffix recovered {} entries but the local suffix has {}",
                recovered_entries.len(),
                local_entry_count
            )));
        }
        let applied = recovered_entries[local_entry_count..].to_vec();
        let next_consensus = FederationConsensusRuntime::recover_verified_suffix(verified_suffix);

        let entries_to_apply = crate::accepted_federation_journal_entries(&applied);
        self.persist_catch_up(
            target_session,
            &response.aleph_bft_finalized_items,
            &applied,
        )
        .await?;
        #[cfg(feature = "prometheus")]
        cdk_prometheus::METRICS.record_federation_checkpoint_work(
            &runtime_config.member_id().to_string(),
            cdk_prometheus::FederationCheckpointWork::CatchUpStagedBytes,
            response.ensure_within_checkpoint_page_limit()? as u64,
        );
        *consensus = next_consensus;
        drop(consensus);
        self.apply_journal_entries(&entries_to_apply).await?;
        self.verify_catch_up_rejections(&response.entries).await?;
        Ok(applied)
    }

    #[cfg(test)]
    async fn apply_verified_catch_up_response_under_application_lock(
        &self,
        response: &FederationJournalCatchUpResponse,
    ) -> Result<Vec<FederationJournalEntry>, cdk::Error> {
        let runtime_config = self
            .active_mint()?
            .federation_config()
            .cloned()
            .ok_or_else(|| runtime_unavailable("mint configuration"))?;
        let public_config = runtime_config.public_config().clone();
        let consensus = self
            .consensus
            .as_ref()
            .ok_or_else(|| runtime_unavailable("consensus"))?;
        let mut consensus = consensus.lock().await;
        let local_entries = consensus
            .journal()
            .entries()
            .iter()
            .cloned()
            .collect::<Vec<_>>();
        let local_finalized_items = consensus.aleph_bft_finalized_items()?.to_vec();
        let status = consensus.aleph_bft_status()?;
        let response_to_apply = crate::catch_up_response_suffix_for_local_journal(
            &local_entries,
            consensus.journal().next_index(),
            response,
        )?;
        if let Some(response_to_apply) = response_to_apply.as_ref() {
            if response_to_apply.from_index != response.from_index
                || response_to_apply.entries.len() != response.entries.len()
            {
                return Err(crate::aleph_bft_catch_up_recovery_mismatch(
                    "checkpoint catch-up response does not cover the local suffix".to_string(),
                ));
            }
        }
        let new_finalized_items = crate::aleph_bft_finalized_item_suffix_for_local(
            &local_finalized_items,
            status.next_item_index,
            response,
        )?;
        if new_finalized_items.is_empty()
            && response_to_apply
                .as_ref()
                .is_some_and(|response_to_apply| !response_to_apply.entries.is_empty())
        {
            return Err(crate::aleph_bft_catch_up_recovery_mismatch(
                "non-empty AlephBFT catch-up entries missing finalized-item evidence".to_string(),
            ));
        }

        let mut retained_items = local_finalized_items;
        retained_items.extend(new_finalized_items.iter().cloned());
        let target_session =
            crate::latest_aleph_bft_finalized_session(&retained_items).max(status.session);
        let mut next_consensus = FederationConsensusRuntime::recover_finalized_items_for_test(
            &public_config,
            runtime_config.member_id(),
            target_session,
            retained_items,
        )?;
        let recovered_entries_before_status_overlay = next_consensus.journal().entries().to_vec();
        let entries_to_retry_from_local_overlap = crate::stale_local_aleph_bft_entries_to_retry(
            &local_entries,
            &recovered_entries_before_status_overlay,
        );
        for local_entry in &local_entries {
            if crate::should_overlay_local_status_after_aleph_bft_recovery(local_entry.status()) {
                next_consensus.update_application_state(
                    local_entry.operation_id,
                    local_entry.application.clone(),
                )?;
            }
        }
        let recovered_entries = next_consensus.journal().entries();
        if recovered_entries.len() < local_entries.len() {
            return Err(crate::aleph_bft_catch_up_recovery_mismatch(format!(
                "finalized-item catch-up recovered {} journal entries but local journal has {}",
                recovered_entries.len(),
                local_entries.len()
            )));
        }
        for (local_entry, recovered_entry) in local_entries.iter().zip(recovered_entries.iter()) {
            FederationCatchUpOperation::from(recovered_entry)
                .ensure_matches_journal_entry(local_entry)?;
        }
        let applied = recovered_entries[local_entries.len()..].to_vec();
        crate::ensure_catch_up_entries_match_recovered_journal(
            &applied,
            response_to_apply.as_ref(),
        )?;
        if response.complete
            && response.latest_order_digest != next_consensus.journal().latest_order_digest()
        {
            return Err(cdk::Error::Federation(
                FederationError::CatchUpDigestMismatch {
                    index: next_consensus.journal().next_index(),
                    expected: next_consensus.journal().latest_order_digest(),
                    found: response.latest_order_digest,
                },
            ));
        }

        let mut entries_to_apply = entries_to_retry_from_local_overlap;
        entries_to_apply.extend(crate::accepted_federation_journal_entries(&applied));
        self.persist_catch_up(target_session, &new_finalized_items, &applied)
            .await?;
        *consensus = next_consensus;
        drop(consensus);
        self.apply_journal_entries(&entries_to_apply).await?;
        if let Some(response_to_apply) = response_to_apply.as_ref() {
            self.verify_catch_up_rejections(&response_to_apply.entries)
                .await?;
        }
        Ok(applied)
    }
}
