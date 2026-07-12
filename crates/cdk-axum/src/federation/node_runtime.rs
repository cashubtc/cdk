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
    FederationAlephBftAdapterStatus, FederationAlephBftUnitPayload, FederationConsensusRuntime,
    FederationError, FederationJournalAppendOutcome, FederationJournalEntry, FederationOperationId,
    FederationSubmitOperationResponse, MemberId,
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
}
