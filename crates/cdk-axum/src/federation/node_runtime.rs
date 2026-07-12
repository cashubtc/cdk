//! Federation node runtime ownership.

use std::sync::Arc;

use super::{FederationDkgAutoStartConfig, FederationPeerSyncConfig};
use crate::{
    FederationAlephBftLiveSessionState, FederationAlephBftNetworkInboundState,
    FederationAlephBftPendingSequenceHandle, FederationConsensusHandle,
    FederationDkgMemberAdmissionHandle, FederationInvalidOperationAlertHandle,
    FederationNotificationHandle, FederationOperationNotificationStats,
    FederationRestAdmissionHandle, FederationSequenceHandle, FederationSyncHandle,
    FederationSyncState, FederationSyncStatus, ALEPH_BFT_STARTUP_HALT_REASON,
};
use cdk::federation::{
    FederationAlephBftAdapterStatus, FederationAlephBftFinalizedItem,
    FederationAlephBftPendingOperationRecord, FederationAlephBftUnitPayload,
    FederationConsensusRuntime, FederationError, FederationJournalAppendOutcome,
    FederationJournalEntry, FederationOperationId, FederationOperationStatus,
    FederationSessionOutcome, FederationSubmitOperationResponse, MemberId,
};
use cdk::mint::Mint;

/// Runtime-owned handles for one active federation member.
#[derive(Clone)]
#[allow(missing_debug_implementations)]
pub(crate) struct FederationNodeRuntime {
    pub(crate) mint: Option<Arc<Mint>>,
    pub(crate) consensus: Option<FederationConsensusHandle>,
    pub(crate) peer_sync_config: Option<FederationPeerSyncConfig>,
    pub(crate) dkg_auto_start_config: Option<Arc<FederationDkgAutoStartConfig>>,
    pub(crate) sequences: Option<FederationSequenceHandle>,
    pub(crate) sync: Option<FederationSyncHandle>,
    pub(crate) notifications: Option<FederationNotificationHandle>,
    pub(crate) private_rest_admission: Option<FederationRestAdmissionHandle>,
    pub(crate) dkg_member_admission: Option<FederationDkgMemberAdmissionHandle>,
    pub(crate) invalid_operation_alerts: Option<FederationInvalidOperationAlertHandle>,
    pub(crate) aleph_bft_pending_sequences: Option<FederationAlephBftPendingSequenceHandle>,
    pub(crate) aleph_bft_network_inbound: FederationAlephBftNetworkInboundState,
    pub(crate) aleph_bft_live_session: FederationAlephBftLiveSessionState,
    #[cfg(test)]
    pub(crate) local_finality_test: bool,
    #[cfg(test)]
    pub(crate) manual_finality_test: bool,
}

impl FederationNodeRuntime {
    /// Build the inert runtime stored by a standalone mint in a federation-enabled binary.
    pub(crate) fn disabled() -> Self {
        Self {
            mint: None,
            consensus: None,
            peer_sync_config: None,
            dkg_auto_start_config: None,
            sequences: None,
            sync: None,
            notifications: None,
            private_rest_admission: None,
            dkg_member_admission: None,
            invalid_operation_alerts: None,
            aleph_bft_pending_sequences: None,
            aleph_bft_network_inbound: Arc::new(tokio::sync::RwLock::new(None)),
            aleph_bft_live_session: Arc::new(tokio::sync::RwLock::new(None)),
            #[cfg(test)]
            local_finality_test: false,
            #[cfg(test)]
            manual_finality_test: false,
        }
    }

    pub(crate) async fn validate_envelope_admission(
        &self,
        envelope: &cdk::federation::FederationOperationEnvelope,
    ) -> Result<(), cdk::Error> {
        let Some(mint) = &self.mint else {
            return Ok(());
        };
        let Some(config) = mint.effective_federation_config().await? else {
            return Ok(());
        };
        config.validate_federated_operation_submission(&envelope.operation)
    }

    fn active_mint(&self) -> Result<&Arc<Mint>, cdk::Error> {
        self.mint.as_ref().ok_or_else(|| {
            cdk::Error::Custom("federation runtime is not active for this mint".to_string())
        })
    }

    #[cfg(test)]
    pub(crate) async fn wait_for_operation(
        &self,
        operation_id: FederationOperationId,
        waiter_limit: usize,
    ) -> Result<Option<FederationSubmitOperationResponse>, cdk::Error> {
        let Some(notifications) = self.notifications.as_ref() else {
            return Ok(None);
        };
        Ok(Some(
            notifications.wait_for(operation_id, waiter_limit).await?,
        ))
    }

    pub(crate) fn pending_response_waiter_count(&self) -> usize {
        self.notifications
            .as_ref()
            .map(|notifications| notifications.pending_waiter_count())
            .unwrap_or_default()
    }

    pub(crate) fn response_cache_entry_count(&self) -> usize {
        self.notifications
            .as_ref()
            .map(|notifications| notifications.response_cache_count())
            .unwrap_or_default()
    }

    pub(crate) fn operation_notification_stats(&self) -> FederationOperationNotificationStats {
        self.notifications
            .as_ref()
            .map(|notifications| notifications.stats())
            .unwrap_or_default()
    }

    pub(crate) fn notify_operation(&self, response: FederationSubmitOperationResponse) {
        let Some(notifications) = &self.notifications else {
            return;
        };
        super::aleph_bft_latency::record(
            super::aleph_bft_latency::CdkAlephBftLatencyStage::OperationNotified,
            [response.operation_id],
        );
        let response_cache_limit = self
            .peer_sync_config
            .unwrap_or_default()
            .response_cache_limit;
        notifications.notify(response, response_cache_limit);
    }

    pub(crate) async fn responses_with_current_status(
        &self,
        responses: &[FederationSubmitOperationResponse],
    ) -> Vec<FederationSubmitOperationResponse> {
        let Some(consensus) = &self.consensus else {
            return responses.to_vec();
        };
        let consensus = consensus.lock().await;
        responses
            .iter()
            .map(|response| {
                let entry = consensus
                    .journal()
                    .entry_by_operation_id(response.operation_id)
                    .cloned()
                    .unwrap_or_else(|| response.outcome.entry().clone());
                crate::federation_submit_response_with_entry(response, entry)
            })
            .collect()
    }

    pub(crate) fn duplicate_finalized_unit_responses(
        consensus: &FederationConsensusRuntime,
        payload: &FederationAlephBftUnitPayload,
    ) -> Result<Option<Vec<FederationSubmitOperationResponse>>, cdk::Error> {
        let mut finalized_items = consensus
            .aleph_bft_finalized_items()?
            .iter()
            .filter(|item| {
                item.session == payload.session
                    && item.unit_submitting_member == payload.submitting_member
                    && item.unit_index == payload.unit_index
            })
            .collect::<Vec<_>>();
        if finalized_items.is_empty() {
            return Ok(None);
        }
        finalized_items.sort_by_key(|item| item.item_index_in_unit);

        if finalized_items.len() != payload.items.len() {
            return Err(cdk::Error::Custom(format!(
                "AlephBFT finalized unit replay changed item count for member {} session {} unit {}",
                payload.submitting_member, payload.session, payload.unit_index
            )));
        }

        let mut responses = Vec::new();
        for (expected_index, finalized_item) in finalized_items.iter().enumerate() {
            let expected_index_u64 =
                u64::try_from(expected_index).expect("unit item index fits in u64");
            if finalized_item.item_index_in_unit != expected_index_u64 {
                return Err(cdk::Error::Custom(format!(
                    "AlephBFT finalized unit replay changed item order for member {} session {} unit {}",
                    payload.submitting_member, payload.session, payload.unit_index
                )));
            }
            if finalized_item.unit_body_hash != payload.body_hash {
                return Err(cdk::Error::Custom(format!(
                    "AlephBFT finalized unit replay changed body hash for member {} session {} unit {}",
                    payload.submitting_member, payload.session, payload.unit_index
                )));
            }

            let Some(payload_item) = payload.items.get(expected_index) else {
                return Err(cdk::Error::Custom(format!(
                    "AlephBFT finalized unit replay is missing item {} for member {} session {} unit {}",
                    expected_index, payload.submitting_member, payload.session, payload.unit_index
                )));
            };
            if finalized_item.item.canonical_bytes()? != payload_item.canonical_bytes()? {
                return Err(cdk::Error::Custom(format!(
                    "AlephBFT finalized unit replay changed item body for member {} session {} unit {}",
                    payload.submitting_member, payload.session, payload.unit_index
                )));
            }
            let payload_operation_id = payload_item.operation_id()?;
            if finalized_item.operation_id != payload_operation_id {
                return Err(cdk::Error::Custom(format!(
                    "AlephBFT finalized unit replay changed operation ID for member {} session {} unit {}",
                    payload.submitting_member, payload.session, payload.unit_index
                )));
            }

            if let Some(operation_id) = finalized_item.operation_id {
                let entry = consensus
                    .journal()
                    .entry_by_operation_id(operation_id)
                    .ok_or_else(|| {
                        cdk::Error::Custom(format!(
                            "AlephBFT finalized unit replay operation {} is missing from the local journal",
                            operation_id
                        ))
                    })?;
                responses.push(FederationSubmitOperationResponse::from_outcome(
                    FederationJournalAppendOutcome::Duplicate(entry.clone()),
                ));
            }
        }

        Ok(Some(responses))
    }

    pub(crate) async fn remember_pending_sequence(
        &self,
        operation_id: FederationOperationId,
        sender: MemberId,
        sequence: u64,
    ) {
        let Some(pending) = &self.aleph_bft_pending_sequences else {
            return;
        };
        pending
            .lock()
            .await
            .entry(operation_id)
            .or_default()
            .insert(sender, sequence);
    }

    pub(crate) async fn pending_sequences_for_entries(
        &self,
        entries: &[FederationJournalEntry],
    ) -> Vec<(MemberId, u64)> {
        let Some(pending) = &self.aleph_bft_pending_sequences else {
            return Vec::new();
        };
        let pending = pending.lock().await;
        let mut sequences = Vec::new();
        for entry in entries {
            if let Some(entry_sequences) = pending.get(&entry.operation_id) {
                sequences.extend(
                    entry_sequences
                        .iter()
                        .map(|(member, sequence)| (*member, *sequence)),
                );
            }
        }
        sequences
    }

    pub(crate) async fn clear_pending_sequences_for_entries(
        &self,
        entries: &[FederationJournalEntry],
    ) {
        let Some(pending) = &self.aleph_bft_pending_sequences else {
            return;
        };
        let mut pending = pending.lock().await;
        for entry in entries {
            pending.remove(&entry.operation_id);
        }
    }

    pub(crate) async fn sync_status(&self) -> Option<FederationSyncStatus> {
        let sync = self.sync.as_ref()?;
        Some(sync.lock().await.status())
    }

    async fn update_sync_status<F>(&self, update: F) -> Option<FederationSyncStatus>
    where
        F: FnOnce(&mut FederationSyncState),
    {
        let sync = self.sync.as_ref()?;
        sync.lock().await.update_status(update)
    }

    pub(crate) async fn mark_lagging(
        &self,
        peer: MemberId,
        local_operation_count: u64,
        peer_operation_count: u64,
    ) -> Option<FederationSyncStatus> {
        self.update_sync_status(|sync| {
            sync.mark_lagging(peer, local_operation_count, peer_operation_count);
        })
        .await
    }

    pub(crate) async fn mark_catching_up(&self, peer: MemberId) -> Option<FederationSyncStatus> {
        self.update_sync_status(|sync| sync.mark_catching_up(peer))
            .await
    }

    pub(crate) async fn mark_verifying_session_outcome(
        &self,
        peer: MemberId,
        operation_count: u64,
    ) -> Option<FederationSyncStatus> {
        self.update_sync_status(|sync| {
            sync.mark_verifying_session_outcome(peer, operation_count);
        })
        .await
    }

    pub(crate) async fn mark_materialized_state_mismatch(
        &self,
        peer: MemberId,
        operation_count: u64,
    ) -> Option<FederationSyncStatus> {
        self.update_sync_status(|sync| {
            sync.mark_materialized_state_mismatch(peer, operation_count);
        })
        .await
    }

    pub(crate) async fn mark_healthy_if_session_outcome_verified(
        &self,
        verified_operation_count: u64,
    ) -> Option<FederationSyncStatus> {
        self.update_sync_status(|sync| {
            sync.mark_healthy_if_session_outcome_verified(verified_operation_count);
        })
        .await
    }

    #[cfg(test)]
    pub(crate) async fn mark_healthy(&self) -> Option<FederationSyncStatus> {
        self.update_sync_status(FederationSyncState::mark_healthy)
            .await
    }

    pub(crate) async fn mark_healthy_if_caught_up(
        &self,
        local_operation_count: u64,
    ) -> Option<FederationSyncStatus> {
        self.update_sync_status(|sync| match sync.status() {
            FederationSyncStatus::Lagging {
                peer_operation_count,
                ..
            } if peer_operation_count > local_operation_count => {}
            FederationSyncStatus::VerifyingSessionOutcome { .. }
            | FederationSyncStatus::Halted { .. } => {}
            _ => sync.mark_healthy(),
        })
        .await
    }

    pub(crate) async fn mark_halted(&self, reason: String) -> Option<FederationSyncStatus> {
        self.update_sync_status(|sync| sync.mark_halted(reason))
            .await
    }

    pub(crate) async fn ensure_ready_for_authoritative_read(&self) -> Result<(), cdk::Error> {
        let Some(status) = self.sync_status().await else {
            return Ok(());
        };
        if !status.is_ready() {
            return Err(cdk::Error::Custom(status.not_ready_message()));
        }
        if self.live_session_available().await {
            return Ok(());
        }
        Err(cdk::Error::Custom(
            ALEPH_BFT_STARTUP_HALT_REASON.to_string(),
        ))
    }

    pub(crate) async fn network_inbound_active(&self) -> bool {
        self.aleph_bft_network_inbound.read().await.is_some()
    }

    pub(crate) async fn live_session_active(&self) -> bool {
        self.aleph_bft_live_session.read().await.is_some()
    }

    pub(crate) async fn live_session_available(&self) -> bool {
        #[cfg(test)]
        if self.local_finality_test || self.manual_finality_test {
            return true;
        }
        self.live_session_active().await && self.network_inbound_active().await
    }

    pub(crate) async fn pending_mempool_len(&self) -> usize {
        match &self.consensus {
            Some(consensus) => consensus.lock().await.pending_mempool_len(),
            None => 0,
        }
    }

    pub(crate) async fn aleph_bft_status(&self) -> Option<FederationAlephBftAdapterStatus> {
        let consensus = self.consensus.as_ref()?;
        consensus.lock().await.aleph_bft_status().ok()
    }

    pub(crate) async fn mark_consensus_activity(
        &self,
        operation_id: FederationOperationId,
        broadcast: bool,
    ) {
        let Some(handle) = self.aleph_bft_live_session.read().await.clone() else {
            return;
        };
        handle.mark_activity(operation_id, broadcast);
    }

    pub(crate) async fn mark_peer_activity(
        &self,
        session_id: u64,
        operation_id: FederationOperationId,
    ) -> Result<(), cdk::Error> {
        let status = self.aleph_bft_status().await.ok_or_else(|| {
            cdk::Error::Custom("AlephBFT activity requires an active consensus runtime".to_string())
        })?;
        if status.session != session_id {
            return Err(cdk::Error::from(
                FederationError::AlephBftUnitSessionMismatch {
                    expected: status.session,
                    found: session_id,
                },
            ));
        }
        let handle = self
            .aleph_bft_live_session
            .read()
            .await
            .clone()
            .ok_or_else(|| {
                cdk::Error::Custom("AlephBFT activity receiver is not available".to_string())
            })?;
        handle.mark_peer_activity(operation_id);
        Ok(())
    }

    pub(crate) async fn persist_journal_entries(
        &self,
        entries: &[FederationJournalEntry],
        session_outcomes: &[&FederationSessionOutcome],
        sequence: Option<(MemberId, u64)>,
    ) -> Result<(), cdk::Error> {
        let db = self.active_mint()?.localstore();
        let mut tx = db.begin_transaction().await?;
        for entry in entries {
            tx.add_federation_journal_entry(entry).await?;
        }
        for session_outcome in session_outcomes {
            tx.add_federation_session_outcome(session_outcome).await?;
        }
        if let Some((sender, sequence)) = sequence {
            tx.set_federation_member_sequence(sender, sequence).await?;
        }
        tx.commit().await?;
        Ok(())
    }

    pub(crate) async fn persist_pending_operation(
        &self,
        operation: FederationAlephBftPendingOperationRecord,
    ) -> Result<(), cdk::Error> {
        let db = self.active_mint()?.localstore();
        let mut tx = db.begin_transaction().await?;
        tx.add_federation_aleph_bft_pending_operation(&operation)
            .await?;
        tx.commit().await?;
        Ok(())
    }

    pub(crate) async fn persist_finalization(
        &self,
        finalized_items: &[FederationAlephBftFinalizedItem],
        entries: &[FederationJournalEntry],
        sequences: &[(MemberId, u64)],
    ) -> Result<(), cdk::Error> {
        let db = self.active_mint()?.localstore();
        let mut tx = db.begin_transaction().await?;
        tx.add_federation_aleph_bft_finalized_items(finalized_items)
            .await?;
        for operation_id in finalized_items.iter().filter_map(|item| item.operation_id) {
            tx.remove_federation_aleph_bft_pending_operation(operation_id)
                .await?;
        }
        for entry in entries {
            tx.add_federation_journal_entry(entry).await?;
        }
        for (sender, sequence) in sequences {
            tx.set_federation_member_sequence(*sender, *sequence)
                .await?;
        }
        tx.commit().await?;
        Ok(())
    }

    pub(crate) async fn persist_catch_up(
        &self,
        active_session: u64,
        finalized_items: &[FederationAlephBftFinalizedItem],
        entries: &[FederationJournalEntry],
        session_outcomes: &[&FederationSessionOutcome],
    ) -> Result<(), cdk::Error> {
        let db = self.active_mint()?.localstore();
        let mut tx = db.begin_transaction().await?;
        tx.set_federation_aleph_bft_active_session(active_session)
            .await?;
        tx.add_federation_aleph_bft_finalized_items(finalized_items)
            .await?;
        for operation_id in finalized_items.iter().filter_map(|item| item.operation_id) {
            tx.remove_federation_aleph_bft_pending_operation(operation_id)
                .await?;
        }
        for entry in entries {
            tx.add_federation_journal_entry(entry).await?;
        }
        for session_outcome in session_outcomes {
            tx.add_federation_session_outcome(session_outcome).await?;
        }
        tx.commit().await?;
        Ok(())
    }

    pub(crate) async fn record_sequence(&self, sender: MemberId, sequence: u64) {
        if let Some(sequences) = &self.sequences {
            sequences.lock().await.record_sequence(sender, sequence);
        }
    }

    pub(crate) async fn apply_finalized_unit(
        &self,
        payload: FederationAlephBftUnitPayload,
        sequence: Option<(MemberId, u64)>,
    ) -> Result<Vec<FederationSubmitOperationResponse>, cdk::Error> {
        let consensus = self.consensus.as_ref().ok_or_else(|| {
            cdk::Error::Custom("federation mint is missing its consensus handle".to_string())
        })?;
        let mut consensus = consensus.lock().await;
        if let Some(responses) = Self::duplicate_finalized_unit_responses(&consensus, &payload)? {
            drop(consensus);
            if let Some(sequence) = sequence {
                self.persist_journal_entries(&[], &[], Some(sequence))
                    .await?;
                self.record_sequence(sequence.0, sequence.1).await;
            }
            let entries = responses
                .iter()
                .map(|response| response.outcome.entry().clone())
                .collect::<Vec<_>>();
            let entries_to_apply = crate::accepted_federation_journal_entries(&entries);
            self.apply_journal_entries(&entries_to_apply).await?;
            return Ok(self.responses_with_current_status(&responses).await);
        }

        let mut next_consensus = consensus.clone();
        let finalized_item_start = next_consensus.aleph_bft_finalized_items()?.len();
        let responses = next_consensus.finalize_aleph_bft_unit(payload)?;
        let finalized_items =
            next_consensus.aleph_bft_finalized_items()?[finalized_item_start..].to_vec();
        let outcome_entries = responses
            .iter()
            .map(|response| response.outcome.entry().clone())
            .collect::<Vec<_>>();
        let entries = responses
            .iter()
            .filter_map(|response| match &response.outcome {
                FederationJournalAppendOutcome::Appended(entry) => Some(entry.clone()),
                FederationJournalAppendOutcome::Duplicate(_) => None,
            })
            .collect::<Vec<_>>();
        let entries_to_apply = crate::accepted_federation_journal_entries(&entries);
        let mut sequences = self.pending_sequences_for_entries(&outcome_entries).await;
        if let Some(sequence) = sequence {
            sequences.push(sequence);
        }

        self.persist_finalization(&finalized_items, &entries, &sequences)
            .await?;
        *consensus = next_consensus;
        drop(consensus);
        self.clear_pending_sequences_for_entries(&outcome_entries)
            .await;

        self.apply_journal_entries(&entries_to_apply).await?;
        let responses = self.responses_with_current_status(&responses).await;
        for (sender, sequence) in &sequences {
            self.record_sequence(*sender, *sequence).await;
        }
        super::aleph_bft_latency::record(
            super::aleph_bft_latency::CdkAlephBftLatencyStage::ApplyCompleted,
            responses.iter().map(|response| response.operation_id),
        );
        for response in &responses {
            self.notify_operation(response.clone());
        }

        Ok(responses)
    }

    pub(crate) async fn apply_journal_entries(
        &self,
        entries: &[FederationJournalEntry],
    ) -> Result<Vec<FederationOperationId>, cdk::Error> {
        let mut applied = Vec::new();
        for entry in entries {
            if let Some(operation_id) = self.apply_journal_entry_for_replay(entry).await? {
                applied.push(operation_id);
            }
        }
        self.mark_operations_applied(&applied).await?;
        Ok(applied)
    }

    pub(crate) async fn verify_catch_up_rejections(
        &self,
        peer_entries: &[FederationJournalEntry],
    ) -> Result<(), cdk::Error> {
        for peer_entry in peer_entries
            .iter()
            .filter(|entry| entry.status == FederationOperationStatus::Rejected)
        {
            let local_entry = self
                .active_mint()?
                .localstore()
                .get_federation_journal_entry_by_operation_id(peer_entry.operation_id)
                .await?
                .ok_or_else(|| {
                    cdk::Error::Federation(FederationError::UnknownOperation {
                        operation_id: peer_entry.operation_id.to_string(),
                    })
                })?;
            if local_entry.status != FederationOperationStatus::Rejected
                || local_entry.rejection != peer_entry.rejection
            {
                return Err(cdk::Error::Federation(
                    FederationError::SessionOutcomeMismatch {
                        reason: format!(
                            "catch-up rejection mismatch at operation {}",
                            peer_entry.operation_id
                        ),
                    },
                ));
            }
        }

        Ok(())
    }

    async fn apply_journal_entry_for_replay(
        &self,
        entry: &FederationJournalEntry,
    ) -> Result<Option<FederationOperationId>, cdk::Error> {
        let mut attempts = 0usize;
        let mut retry_delay = crate::FEDERATION_APPLY_RETRY_INITIAL_DELAY;

        loop {
            match self
                .active_mint()?
                .apply_accepted_federation_operation(entry)
                .await
            {
                Ok(true) => {
                    if attempts > 0 {
                        tracing::info!(
                            operation_id = %entry.operation_id,
                            retry_attempts = attempts,
                            "Federation journal replay apply recovered after transient storage contention"
                        );
                    }
                    return Ok(Some(entry.operation_id));
                }
                Ok(false) => return Ok(None),
                Err(err) if crate::is_retryable_federation_apply_error(&err) => {
                    attempts = attempts.saturating_add(1);
                    if attempts == 1 || attempts % 10 == 0 {
                        tracing::warn!(
                            operation_id = %entry.operation_id,
                            retry_attempts = attempts,
                            retry_delay_ms = retry_delay.as_millis(),
                            "Federation journal replay apply hit transient storage contention; retrying"
                        );
                    }
                    tokio::time::sleep(retry_delay).await;
                    retry_delay = retry_delay
                        .saturating_mul(2)
                        .min(crate::FEDERATION_APPLY_RETRY_MAX_DELAY);
                }
                Err(err) => {
                    let (status, err) = self
                        .mark_operation_apply_failure(entry.operation_id, err)
                        .await?;
                    if status == FederationOperationStatus::Rejected {
                        return Ok(None);
                    }
                    return Err(err);
                }
            }
        }
    }

    async fn mark_operations_applied(
        &self,
        operation_ids: &[FederationOperationId],
    ) -> Result<(), cdk::Error> {
        let Some(consensus) = &self.consensus else {
            return Ok(());
        };
        let mut consensus = consensus.lock().await;
        for operation_id in operation_ids {
            consensus.update_status(*operation_id, FederationOperationStatus::Applied)?;
        }
        Ok(())
    }

    async fn mark_operation_status(
        &self,
        operation_id: FederationOperationId,
        status: FederationOperationStatus,
    ) -> Result<(), cdk::Error> {
        let db = self.active_mint()?.localstore();
        let mut tx = db.begin_transaction().await?;
        tx.update_federation_operation_status(operation_id, status)
            .await?;
        tx.commit().await?;

        if let Some(consensus) = &self.consensus {
            consensus.lock().await.update_status(operation_id, status)?;
        }
        Ok(())
    }

    async fn mark_operation_apply_failure(
        &self,
        operation_id: FederationOperationId,
        error: cdk::Error,
    ) -> Result<(FederationOperationStatus, cdk::Error), cdk::Error> {
        if !error.is_definitive_failure() {
            self.mark_operation_status(operation_id, FederationOperationStatus::Failed)
                .await?;
            return Ok((FederationOperationStatus::Failed, error));
        }

        let rejection = cdk::error::ErrorResponse::from(error);
        let db = self.active_mint()?.localstore();
        let mut tx = db.begin_transaction().await?;
        tx.reject_federation_operation(operation_id, &rejection)
            .await?;
        tx.commit().await?;

        if let Some(consensus) = &self.consensus {
            consensus.lock().await.update_application_state(
                operation_id,
                FederationOperationStatus::Rejected,
                Some(rejection.clone()),
            )?;
        }

        Ok((
            FederationOperationStatus::Rejected,
            cdk::Error::from(rejection),
        ))
    }
}
