//! Federation node runtime ownership.

use std::sync::Arc;

use super::{FederationDkgAutoStartConfig, FederationPeerSyncConfig};
use crate::{
    FederationAlephBftLiveSessionState, FederationAlephBftNetworkInboundState,
    FederationAlephBftPendingSequenceHandle, FederationConsensusHandle,
    FederationDkgMemberAdmissionHandle, FederationInvalidOperationAlertHandle,
    FederationNotificationHandle, FederationOperationNotificationStats,
    FederationOperationWaitRegistration, FederationRestAdmissionHandle, FederationSequenceHandle,
    FederationSyncHandle, FederationSyncState, FederationSyncStatus, ALEPH_BFT_STARTUP_HALT_REASON,
};
use cdk::federation::{
    FederationAlephBftFinalizedItem, FederationAlephBftPendingOperationRecord,
    FederationAlephBftUnitPayload, FederationConfig, FederationConsensusItemAdmission,
    FederationConsensusRuntime, FederationConsensusRuntimeStatus, FederationDkgActivationRequest,
    FederationDkgCeremonyId, FederationDkgFinalizedMemberConfig, FederationDkgMemberKeysetShare,
    FederationDkgParticipantCommitment, FederationDkgParticipantCommitmentReveal,
    FederationDkgSecretShareRequest, FederationDkgSetupState, FederationDkgTranscriptHash,
    FederationDkgTranscriptSignatureRequest, FederationError, FederationJournalAppendOutcome,
    FederationJournalCatchUpResponse, FederationJournalEntry, FederationOperationEnvelope,
    FederationOperationId, FederationOperationStatus, FederationSessionOutcome,
    FederationSubmitOperationResponse, MemberId, FEDERATION_REST_API_VERSION,
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
    #[cfg(test)]
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

    #[cfg(feature = "prometheus")]
    pub(crate) fn metric_member_label(&self) -> Option<String> {
        self.mint
            .as_ref()
            .and_then(|mint| mint.federation_config())
            .map(|config| config.member_id().to_string())
    }

    #[cfg(feature = "prometheus")]
    fn record_accepted_operation_metric(&self, operation: &str) {
        if let Some(member) = self.metric_member_label() {
            cdk_prometheus::METRICS.record_federation_accepted_operation(&member, operation);
        }
    }

    #[cfg(feature = "prometheus")]
    fn record_rejected_operation_metric(&self, operation: &str, reason: &str) {
        if let Some(member) = self.metric_member_label() {
            cdk_prometheus::METRICS
                .record_federation_rejected_operation(&member, operation, reason);
        }
    }

    #[cfg(feature = "prometheus")]
    fn record_consensus_rejection_metric(
        &self,
        operation: &str,
        response: &FederationSubmitOperationResponse,
    ) {
        if response.outcome.entry().status == FederationOperationStatus::Rejected {
            self.record_rejected_operation_metric(operation, "consensus");
        }
    }

    #[cfg(feature = "prometheus")]
    async fn record_pending_mempool_metric(&self) {
        let Some(member) = self.metric_member_label() else {
            return;
        };
        let pending = self.pending_mempool_len().await;
        cdk_prometheus::METRICS.set_federation_pending_mempool_items(
            &member,
            i64::try_from(pending).unwrap_or(i64::MAX),
        );
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

    async fn update_sync_status<F>(&self, update: F)
    where
        F: FnOnce(&mut FederationSyncState),
    {
        let Some(sync) = self.sync.as_ref() else {
            return;
        };
        let status = sync.lock().await.update_status(update);
        if let Some(status) = status.as_ref() {
            self.persist_sync_status(status).await;
        }
    }

    async fn persist_sync_status(&self, status: &FederationSyncStatus) {
        let Ok(mint) = self.active_mint() else {
            return;
        };
        let Some(config) = mint.federation_config() else {
            return;
        };
        let snapshot = status
            .to_member_status_snapshot(config.member_id(), crate::federation_status_unix_time());
        let db = mint.localstore();
        let mut tx = match db.begin_transaction().await {
            Ok(tx) => tx,
            Err(err) => {
                tracing::warn!("Could not begin federation sync status transaction: {err}");
                return;
            }
        };

        if let Err(err) = tx.set_federation_member_status(&snapshot).await {
            tracing::warn!("Could not persist federation sync status: {err}");
            let _ = tx.rollback().await;
            return;
        }

        if let Err(err) = tx.commit().await {
            tracing::warn!("Could not commit federation sync status transaction: {err}");
        }
    }

    pub(crate) async fn restore_sync_status(&self) {
        let Some(sync) = &self.sync else {
            return;
        };
        let Ok(mint) = self.active_mint() else {
            return;
        };
        let Some(config) = mint.federation_config() else {
            return;
        };
        let current_status = sync.lock().await.status();
        let startup_halt = current_status.is_aleph_bft_startup_halt();
        if startup_halt && !self.live_session_available().await {
            self.persist_sync_status(&current_status).await;
            return;
        }

        match mint
            .localstore()
            .get_federation_member_status(config.member_id())
            .await
        {
            Ok(Some(snapshot)) => {
                match FederationSyncStatus::try_from_member_status_snapshot(snapshot) {
                    Ok(status) => sync.lock().await.status = status,
                    Err(err) => {
                        tracing::warn!("Could not restore persisted federation sync status: {err}");
                    }
                }
            }
            Ok(None) => {
                if startup_halt {
                    self.update_sync_status(FederationSyncState::mark_healthy)
                        .await;
                }
            }
            Err(err) => {
                tracing::warn!("Could not load persisted federation sync status: {err}");
            }
        }
    }

    pub(crate) async fn restore_startup_readiness(&self) {
        let Some(sync) = &self.sync else {
            return;
        };
        let Ok(mint) = self.active_mint() else {
            return;
        };
        let Some(config) = mint.federation_config() else {
            return;
        };
        match sync.lock().await.status() {
            FederationSyncStatus::Healthy
            | FederationSyncStatus::VerifyingSessionOutcome { .. } => {}
            FederationSyncStatus::Lagging { .. }
            | FederationSyncStatus::CatchingUp { .. }
            | FederationSyncStatus::MaterializedStateMismatch { .. }
            | FederationSyncStatus::Halted { .. } => return,
        }

        let entries = match mint.localstore().get_federation_journal_entries().await {
            Ok(entries) => entries,
            Err(err) => {
                self.mark_halted(format!(
                    "could not inspect federation startup journal for quorum evidence: {err}"
                ))
                .await;
                return;
            }
        };
        let operation_count = entries.len() as u64;
        if operation_count == 0 {
            return;
        }

        let verified_operation_count = match super::session_outcome_quorum_status(mint).await {
            Ok(Some(quorum)) => quorum.operation_count,
            Ok(None) => 0,
            Err(err) => {
                self.mark_halted(format!(
                    "could not inspect federation startup session outcome quorum: {err}"
                ))
                .await;
                return;
            }
        };
        if verified_operation_count >= operation_count {
            self.mark_healthy_if_session_outcome_verified(verified_operation_count)
                .await;
            return;
        }

        let peer = config
            .public_config()
            .members
            .values()
            .find(|member| member.member_id != config.member_id())
            .map(|member| member.member_id)
            .unwrap_or_else(|| config.member_id());
        self.mark_verifying_session_outcome(peer, operation_count)
            .await;
    }

    pub(crate) async fn mark_lagging(
        &self,
        peer: MemberId,
        local_operation_count: u64,
        peer_operation_count: u64,
    ) {
        self.update_sync_status(|sync| {
            sync.mark_lagging(peer, local_operation_count, peer_operation_count);
        })
        .await
    }

    pub(crate) async fn mark_catching_up(&self, peer: MemberId) {
        self.update_sync_status(|sync| sync.mark_catching_up(peer))
            .await
    }

    pub(crate) async fn mark_verifying_session_outcome(
        &self,
        peer: MemberId,
        operation_count: u64,
    ) {
        self.update_sync_status(|sync| {
            sync.mark_verifying_session_outcome(peer, operation_count);
        })
        .await
    }

    pub(crate) async fn mark_materialized_state_mismatch(
        &self,
        peer: MemberId,
        operation_count: u64,
    ) {
        #[cfg(feature = "prometheus")]
        if let Some(member) = self.metric_member_label() {
            cdk_prometheus::METRICS
                .record_federation_state_digest_mismatch(&member, &peer.to_string());
        }
        self.update_sync_status(|sync| {
            sync.mark_materialized_state_mismatch(peer, operation_count);
        })
        .await
    }

    pub(crate) async fn mark_healthy_if_session_outcome_verified(
        &self,
        verified_operation_count: u64,
    ) {
        self.update_sync_status(|sync| {
            sync.mark_healthy_if_session_outcome_verified(verified_operation_count);
        })
        .await
    }

    #[cfg(test)]
    pub(crate) async fn mark_healthy(&self) {
        self.update_sync_status(FederationSyncState::mark_healthy)
            .await
    }

    pub(crate) async fn mark_healthy_if_caught_up(&self, local_operation_count: u64) {
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

    pub(crate) async fn mark_halted(&self, reason: String) {
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

    pub(crate) async fn aleph_bft_status(&self) -> Option<FederationConsensusRuntimeStatus> {
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

    pub(crate) async fn submit_envelope_with_sequence(
        &self,
        envelope: FederationOperationEnvelope,
        sequence: Option<(MemberId, u64)>,
    ) -> Result<FederationSubmitOperationResponse, cdk::Error> {
        #[cfg(feature = "prometheus")]
        let operation_label = envelope.operation.kind().to_string();
        let operation_id = envelope.operation_id()?;
        let broadcast_activity = sequence.is_none();
        let sync_config = self.peer_sync_config.unwrap_or_default();
        let notifications = self.notifications.as_ref().ok_or_else(|| {
            cdk::Error::Custom(
                "federation operation notifications are unavailable for AlephBFT submit"
                    .to_string(),
            )
        })?;
        let consensus = self.consensus.as_ref().ok_or_else(|| {
            cdk::Error::Custom("federation mint is missing its consensus handle".to_string())
        })?;
        let mut consensus = consensus.lock().await;
        if let Some(entry) = consensus.journal().entry_by_operation_id(operation_id) {
            if matches!(
                entry.status,
                FederationOperationStatus::Accepted | FederationOperationStatus::Applying
            ) {
                notifications.record_duplicate_pending_waiter_join();
                let wait_registration = notifications
                    .register_waiter(operation_id, sync_config.response_waiter_limit)?;
                drop(consensus);
                self.mark_consensus_activity(operation_id, broadcast_activity)
                    .await;
                if let Some((sender, sequence)) = sequence {
                    self.remember_pending_sequence(operation_id, sender, sequence)
                        .await;
                }
                let response = Self::wait_for_operation_acceptance(
                    operation_id,
                    sync_config,
                    wait_registration,
                )
                .await?;
                #[cfg(feature = "prometheus")]
                self.record_consensus_rejection_metric(&operation_label, &response);
                return Ok(response);
            }

            let response = FederationSubmitOperationResponse::from_outcome(
                FederationJournalAppendOutcome::Duplicate(entry.clone()),
            );
            notifications.record_duplicate_submission();
            drop(consensus);
            if let Some(sequence) = sequence {
                self.persist_journal_entries(&[], &[], Some(sequence))
                    .await?;
                self.record_sequence(sequence.0, sequence.1).await;
            }
            return Ok(response);
        }

        let queued_before = consensus.pending_mempool_len();
        let mut next_consensus = consensus.clone();
        let admission = match next_consensus.submit_aleph_bft_envelope(envelope.clone()) {
            Ok(admission) => admission,
            Err(err) => {
                #[cfg(feature = "prometheus")]
                self.record_rejected_operation_metric(&operation_label, "consensus");
                return Err(cdk::Error::from(err));
            }
        };
        let accepted_admission = matches!(admission, FederationConsensusItemAdmission::Accepted);
        if accepted_admission {
            let limit = sync_config.pending_mempool_item_limit;
            if queued_before >= limit {
                let err = FederationError::PendingConsensusMempoolFull {
                    pending: queued_before,
                    max: limit,
                };
                #[cfg(feature = "prometheus")]
                self.record_rejected_operation_metric(&operation_label, "mempool_full");
                return Err(cdk::Error::from(err));
            }

            let session_id = next_consensus.aleph_bft_status()?.session;
            self.persist_pending_operation(FederationAlephBftPendingOperationRecord {
                operation_id,
                session_id,
                envelope,
            })
            .await?;
        }
        if matches!(
            admission,
            FederationConsensusItemAdmission::DuplicateOperation
        ) {
            notifications.record_duplicate_pending_waiter_join();
            tracing::debug!(
                operation_id = %operation_id,
                "Duplicate AlephBFT federation operation is already pending"
            );
        }
        let wait_registration =
            notifications.register_waiter(operation_id, sync_config.response_waiter_limit)?;

        *consensus = next_consensus;
        drop(consensus);
        self.mark_consensus_activity(operation_id, broadcast_activity)
            .await;
        if accepted_admission {
            #[cfg(feature = "prometheus")]
            self.record_accepted_operation_metric(&operation_label);
            super::aleph_bft_latency::record(
                super::aleph_bft_latency::CdkAlephBftLatencyStage::SubmitQueued,
                [operation_id],
            );
        }
        if let Some((sender, sequence)) = sequence {
            self.remember_pending_sequence(operation_id, sender, sequence)
                .await;
        }
        #[cfg(test)]
        self.finalize_unit_locally_for_test().await?;
        #[cfg(feature = "prometheus")]
        self.record_pending_mempool_metric().await;

        let response =
            Self::wait_for_operation_acceptance(operation_id, sync_config, wait_registration)
                .await?;
        #[cfg(feature = "prometheus")]
        self.record_consensus_rejection_metric(&operation_label, &response);
        Ok(response)
    }

    #[cfg(test)]
    async fn finalize_unit_locally_for_test(&self) -> Result<(), cdk::Error> {
        if !self.local_finality_test {
            return Ok(());
        }

        let unit = {
            let consensus = self.consensus.as_ref().ok_or_else(|| {
                cdk::Error::Custom("federation mint is missing its consensus handle".to_string())
            })?;
            consensus.lock().await.next_aleph_bft_unit_payload()?
        };
        if let Some(unit) = unit {
            self.apply_finalized_unit(unit, None).await?;
        }

        Ok(())
    }

    async fn wait_for_operation_acceptance(
        operation_id: FederationOperationId,
        sync_config: FederationPeerSyncConfig,
        wait_registration: FederationOperationWaitRegistration<'_>,
    ) -> Result<FederationSubmitOperationResponse, cdk::Error> {
        let wait_timeout = sync_config.operation_submission_timeout;
        let response = tokio::time::timeout(wait_timeout, wait_registration.wait())
            .await
            .map_err(|_| {
                cdk::Error::from(FederationError::OperationPendingFinalization {
                    operation_id,
                    timeout_ms: u64::try_from(wait_timeout.as_millis()).unwrap_or(u64::MAX),
                })
            })?;
        super::aleph_bft_latency::record(
            super::aleph_bft_latency::CdkAlephBftLatencyStage::WaiterReturned,
            [operation_id],
        );
        Ok(response)
    }

    #[cfg(test)]
    pub(crate) async fn apply_test_catch_up_response(
        &self,
        response: &FederationJournalCatchUpResponse,
    ) -> Result<Vec<FederationJournalEntry>, cdk::Error> {
        let runtime_config = self
            .active_mint()?
            .federation_config()
            .cloned()
            .ok_or_else(|| {
                cdk::Error::Custom("federation mint is missing its runtime config".to_string())
            })?;
        let consensus = self.consensus.as_ref().ok_or_else(|| {
            cdk::Error::Custom("federation mint is missing its consensus handle".to_string())
        })?;
        let consensus = consensus.lock().await;
        let local_entries = consensus.journal().entries().to_vec();
        let local_finalized_items = consensus.aleph_bft_finalized_items()?.to_vec();
        let response_to_apply =
            crate::catch_up_response_suffix_for_local_journal(&local_entries, response)?;
        let Some(response_to_apply) = response_to_apply else {
            return Ok(Vec::new());
        };
        if response_to_apply.entries.is_empty() {
            let local_order_digest = local_entries
                .last()
                .map_or_else(cdk::federation::FederationJournalDigest::genesis, |entry| {
                    entry.order_digest
                });
            if response.complete && response.latest_order_digest != local_order_digest {
                return Err(cdk::Error::Federation(
                    FederationError::CatchUpDigestMismatch {
                        index: local_entries.len() as u64,
                        expected: local_order_digest,
                        found: response.latest_order_digest,
                    },
                ));
            }
            return Ok(Vec::new());
        }
        let status = consensus.aleph_bft_status()?;
        drop(consensus);

        let public_config = runtime_config.public_config();
        let submitting_member = public_config
            .members
            .keys()
            .copied()
            .find(|member_id| *member_id != runtime_config.member_id())
            .unwrap_or(runtime_config.member_id());
        let target_session =
            crate::latest_aleph_bft_finalized_session(&local_finalized_items).max(status.session);
        let mut recovered_runtime =
            FederationConsensusRuntime::recover_for_members_from_finalized_items(
                public_config.federation_id,
                runtime_config.member_id(),
                target_session,
                public_config.members.keys().copied(),
                local_finalized_items.clone(),
            )?;
        let next_unit_index = local_finalized_items
            .iter()
            .filter(|item| {
                item.session == target_session && item.unit_submitting_member == submitting_member
            })
            .map(|item| item.unit_index)
            .max()
            .map_or(0, |index| index.saturating_add(1));
        let unit = FederationAlephBftUnitPayload::new(
            public_config.federation_id,
            submitting_member,
            target_session,
            next_unit_index,
            response_to_apply
                .entries
                .iter()
                .map(|entry| {
                    cdk::federation::FederationConsensusItem::operation(entry.envelope.clone())
                })
                .collect(),
        )?;
        recovered_runtime.finalize_unit(unit)?;
        let request =
            cdk::federation::FederationJournalCatchUpRequest::new(local_entries.len() as u64)
                .with_known_order_digest(
                    local_entries
                        .last()
                        .map_or_else(cdk::federation::FederationJournalDigest::genesis, |entry| {
                            entry.order_digest
                        }),
                )
                .with_aleph_bft_next_item_index(local_finalized_items.len() as u64);
        let mut aleph_response = FederationJournalCatchUpResponse::from_journal(
            public_config.federation_id,
            recovered_runtime.journal(),
            &request,
            u16::MAX,
            u16::MAX,
        )
        .with_aleph_bft_finalized_items(
            recovered_runtime.finalized_items()[local_finalized_items.len()..].to_vec(),
            true,
        );
        aleph_response.complete = response.complete;
        aleph_response.latest_order_digest = response.latest_order_digest;
        aleph_response.state_digest = response.state_digest;

        self.apply_verified_catch_up_response(&aleph_response, &[])
            .await
    }

    pub(crate) async fn apply_catch_up_response_with_session_outcome_quorum(
        &self,
        response: &FederationJournalCatchUpResponse,
        additional_outcomes: &[FederationSessionOutcome],
    ) -> Result<Vec<FederationJournalEntry>, cdk::Error> {
        let runtime_config = self
            .active_mint()?
            .federation_config()
            .cloned()
            .ok_or_else(|| {
                cdk::Error::Custom("federation mint is missing its runtime config".to_string())
            })?;
        let public_config = runtime_config.public_config().clone();
        if response.version != FEDERATION_REST_API_VERSION {
            return Err(cdk::Error::Federation(
                FederationError::RestVersionMismatch {
                    expected: FEDERATION_REST_API_VERSION,
                    found: response.version,
                },
            ));
        }
        if response.federation_id != public_config.federation_id {
            return Err(cdk::Error::Federation(
                FederationError::CatchUpFederationMismatch {
                    expected: public_config.federation_id,
                    found: response.federation_id,
                },
            ));
        }
        response.verify_session_outcome_quorum(&public_config, additional_outcomes)?;

        let mut session_outcomes = Vec::new();
        if let Some(session_outcome) = response.session_outcome.as_ref() {
            session_outcomes.push(session_outcome);
        }
        session_outcomes.extend(additional_outcomes.iter());
        for session_outcome in &session_outcomes {
            let member =
                public_config
                    .member(session_outcome.signer)
                    .ok_or(cdk::Error::Federation(FederationError::UnknownMember {
                        member_id: session_outcome.signer,
                    }))?;
            session_outcome.verify(
                member,
                response.federation_id,
                response.from_index,
                &response.entries,
                response.latest_order_digest,
                response.state_digest,
            )?;
        }

        self.apply_verified_catch_up_response(response, &session_outcomes)
            .await
    }

    async fn apply_verified_catch_up_response(
        &self,
        response: &FederationJournalCatchUpResponse,
        session_outcomes: &[&FederationSessionOutcome],
    ) -> Result<Vec<FederationJournalEntry>, cdk::Error> {
        let runtime_config = self
            .active_mint()?
            .federation_config()
            .cloned()
            .ok_or_else(|| {
                cdk::Error::Custom("federation mint is missing its runtime config".to_string())
            })?;
        let public_config = runtime_config.public_config().clone();
        let consensus = self.consensus.as_ref().ok_or_else(|| {
            cdk::Error::Custom("federation mint is missing its consensus handle".to_string())
        })?;
        let mut consensus = consensus.lock().await;
        let local_entries = consensus.journal().entries().to_vec();
        let local_finalized_items = consensus.aleph_bft_finalized_items()?.to_vec();
        let response_to_apply =
            crate::catch_up_response_suffix_for_local_journal(&local_entries, response)?;
        if let Some(response_to_apply) = response_to_apply.as_ref() {
            if response_to_apply.from_index != response.from_index
                || response_to_apply.entries.len() != response.entries.len()
            {
                return Err(cdk::Error::Federation(
                    FederationError::SessionOutcomeMismatch {
                        reason: "signed catch-up evidence does not cover the local suffix"
                            .to_string(),
                    },
                ));
            }
        }
        let new_finalized_items =
            crate::aleph_bft_finalized_item_suffix_for_local(&local_finalized_items, response)?;
        if new_finalized_items.is_empty()
            && response_to_apply
                .as_ref()
                .is_some_and(|response_to_apply| !response_to_apply.entries.is_empty())
        {
            return Err(crate::aleph_bft_catch_up_recovery_mismatch(
                "non-empty AlephBFT catch-up entries missing finalized-item evidence".to_string(),
            ));
        }

        let status = consensus.aleph_bft_status()?;
        let mut retained_items = local_finalized_items;
        retained_items.extend(new_finalized_items.iter().cloned());
        let target_session =
            crate::latest_aleph_bft_finalized_session(&retained_items).max(status.session);
        let member_ids = public_config.members.keys().copied();
        let mut next_consensus = FederationConsensusRuntime::aleph_bft_from_finalized_items(
            public_config.federation_id,
            runtime_config.member_id(),
            target_session,
            member_ids,
            public_config.threshold_params.consensus_threshold,
            retained_items,
        )?;
        let recovered_entries_before_status_overlay = next_consensus.journal().entries().to_vec();
        let entries_to_retry_from_local_overlap = crate::stale_local_aleph_bft_entries_to_retry(
            &local_entries,
            &recovered_entries_before_status_overlay,
        );
        for local_entry in &local_entries {
            if crate::should_overlay_local_status_after_aleph_bft_recovery(local_entry.status) {
                next_consensus.update_application_state(
                    local_entry.operation_id,
                    local_entry.status,
                    local_entry.rejection.clone(),
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
            crate::ensure_catch_up_overlap_matches(local_entry, recovered_entry)?;
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
        self.persist_catch_up(
            target_session,
            &new_finalized_items,
            &applied,
            session_outcomes,
        )
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

    pub(crate) async fn persist_sequence(
        &self,
        sender: MemberId,
        sequence: u64,
    ) -> Result<(), cdk::Error> {
        self.persist_journal_entries(&[], &[], Some((sender, sequence)))
            .await?;
        self.record_sequence(sender, sequence).await;
        Ok(())
    }

    pub(crate) async fn persist_dkg_commitment(
        &self,
        commitment: &FederationDkgParticipantCommitment,
        sender: MemberId,
        sequence: u64,
    ) -> Result<(), cdk::Error> {
        let public_config = self.public_config()?;
        let db = self.active_mint()?.localstore();
        let existing_commitments = db.get_federation_dkg_commitments().await?;
        if existing_commitments
            .iter()
            .any(|existing| existing == commitment)
        {
            commitment.validate_against_config(&public_config)?;
            return self.persist_sequence(sender, sequence).await;
        }
        let mut setup_state = self
            .load_dkg_setup_state(&public_config, commitment.ceremony_id)
            .await?;
        setup_state.insert_commitment(&public_config, *commitment)?;

        let mut tx = db.begin_transaction().await?;
        tx.add_federation_dkg_commitment(commitment).await?;
        tx.set_federation_member_sequence(sender, sequence).await?;
        tx.commit().await?;
        self.record_sequence(sender, sequence).await;
        Ok(())
    }

    pub(crate) async fn persist_dkg_reveal(
        &self,
        reveal: &FederationDkgParticipantCommitmentReveal,
        sender: MemberId,
        sequence: u64,
    ) -> Result<(), cdk::Error> {
        let public_config = self.public_config()?;
        let db = self.active_mint()?.localstore();
        let existing_reveals = db.get_federation_dkg_reveals().await?;
        if existing_reveals.iter().any(|existing| existing == reveal) {
            let commitments = db.get_federation_dkg_commitments().await?;
            let commitment = commitments
                .iter()
                .find(|commitment| {
                    commitment.ceremony_id == reveal.ceremony_id
                        && commitment.member_id == reveal.member_id
                })
                .ok_or_else(|| {
                    cdk::Error::Federation(FederationError::DkgCommitmentMismatch {
                        member_id: reveal.member_id,
                        reason: "missing prior commitment".to_string(),
                    })
                })?;
            commitment.validate_reveal(reveal, &public_config)?;
            return self.persist_sequence(sender, sequence).await;
        }
        let mut setup_state = self
            .load_dkg_setup_state(&public_config, reveal.ceremony_id)
            .await?;
        setup_state.insert_reveal(&public_config, reveal.clone())?;

        let mut tx = db.begin_transaction().await?;
        tx.add_federation_dkg_reveal(reveal).await?;
        tx.set_federation_member_sequence(sender, sequence).await?;
        tx.commit().await?;
        self.record_sequence(sender, sequence).await;
        Ok(())
    }

    pub(crate) async fn persist_dkg_secret_share_contribution(
        &self,
        contribution: &FederationDkgSecretShareRequest,
        sender: MemberId,
        sequence: u64,
    ) -> Result<(), cdk::Error> {
        let mint = self.active_mint()?;
        let runtime_config = mint.federation_config().ok_or_else(|| {
            cdk::Error::Custom("federation mint is missing its runtime config".to_string())
        })?;
        contribution.validate_sender_receiver(
            runtime_config.public_config(),
            sender,
            runtime_config.member_id(),
        )?;
        let public_config = runtime_config.public_config().clone();

        let db = mint.localstore();
        let existing_contributions = db.get_federation_dkg_secret_share_contributions().await?;
        if existing_contributions.iter().any(|existing| {
            existing.ceremony_id == contribution.ceremony_id
                && existing.sender == contribution.sender
                && existing.receiver == contribution.receiver
                && existing != contribution
        }) {
            return Err(cdk::Error::Federation(
                FederationError::DkgCommitmentMismatch {
                    member_id: contribution.sender,
                    reason: format!(
                        "conflicting secret-share contribution for receiver {}",
                        contribution.receiver
                    ),
                },
            ));
        }
        if existing_contributions
            .iter()
            .any(|existing| existing == contribution)
        {
            return self.persist_sequence(sender, sequence).await;
        }
        let setup_state = self
            .load_dkg_setup_state(&public_config, contribution.ceremony_id)
            .await?;
        let ceremony_id = setup_state.ceremony_id.ok_or_else(|| {
            cdk::Error::Federation(FederationError::DkgCommitmentMismatch {
                member_id: contribution.sender,
                reason: "missing commitment ceremony before secret contribution".to_string(),
            })
        })?;
        if contribution.ceremony_id != ceremony_id {
            return Err(cdk::Error::Federation(
                FederationError::DkgCommitmentMismatch {
                    member_id: contribution.sender,
                    reason: "secret contribution belongs to another ceremony".to_string(),
                },
            ));
        }
        let sender_reveal = setup_state
            .reveals
            .get(&contribution.sender)
            .cloned()
            .ok_or_else(|| {
                cdk::Error::Federation(FederationError::DkgCommitmentMismatch {
                    member_id: contribution.sender,
                    reason: "missing sender reveal before secret contribution".to_string(),
                })
            })?;
        let validation_config = public_config.clone();
        let validation_contribution = contribution.clone();
        crate::run_federation_dkg_cpu_task(
            "federation DKG secret-share verification task failed",
            move || {
                sender_reveal
                    .validate_secret_share_request(&validation_config, &validation_contribution)
            },
        )
        .await?;

        // Final keyset derivation belongs to the restartable driver after this durable write.
        let mut tx = db.begin_transaction().await?;
        tx.add_federation_dkg_secret_share_contribution(contribution)
            .await?;
        tx.set_federation_member_sequence(sender, sequence).await?;
        tx.commit().await?;
        self.record_sequence(sender, sequence).await;
        Ok(())
    }

    pub(crate) async fn persist_dkg_transcript_signature(
        &self,
        signature: &FederationDkgTranscriptSignatureRequest,
        sender: MemberId,
        sequence: u64,
    ) -> Result<(), cdk::Error> {
        let public_config = self.public_config()?;
        signature.validate_sender(&public_config, sender)?;
        let db = self.active_mint()?.localstore();
        let existing_signatures = db.get_federation_dkg_transcript_signatures().await?;
        if existing_signatures.iter().any(|existing| {
            existing.signer == signature.signer
                && existing.transcript_hash == signature.transcript_hash
                && existing != signature
        }) {
            return Err(cdk::Error::Federation(
                FederationError::DkgTranscriptMismatch {
                    reason: format!(
                        "conflicting transcript signature for member {} transcript {}",
                        signature.signer, signature.transcript_hash
                    ),
                },
            ));
        }
        if existing_signatures
            .iter()
            .any(|existing| existing == signature)
        {
            return self.persist_sequence(sender, sequence).await;
        }
        let expected_hash = self.expected_dkg_transcript_hash(&public_config).await?;
        if signature.transcript_hash != expected_hash {
            return Err(cdk::Error::Federation(
                FederationError::DkgTranscriptMismatch {
                    reason: format!(
                        "transcript signature hash {} does not match locally derived DKG transcript {}",
                        signature.transcript_hash,
                        expected_hash
                    ),
                },
            ));
        }

        let mut tx = db.begin_transaction().await?;
        tx.add_federation_dkg_transcript_signature(signature)
            .await?;
        tx.set_federation_member_sequence(sender, sequence).await?;
        tx.commit().await?;
        self.record_sequence(sender, sequence).await;
        Ok(())
    }

    pub(crate) async fn persist_dkg_activation(
        &self,
        activation: &FederationDkgActivationRequest,
        sender: MemberId,
        sequence: u64,
    ) -> Result<(), cdk::Error> {
        let public_config = self.public_config()?;
        activation.validate_signature_sender(&public_config, sender)?;
        let db = self.active_mint()?.localstore();
        let existing_activations = db.get_federation_dkg_activations().await?;
        if existing_activations.iter().any(|existing| {
            existing.ceremony_id == activation.ceremony_id
                && existing.signer == activation.signer
                && existing != activation
        }) {
            return Err(cdk::Error::Federation(
                FederationError::DkgTranscriptMismatch {
                    reason: format!(
                        "conflicting DKG activation for member {} ceremony {}",
                        activation.signer, activation.ceremony_id
                    ),
                },
            ));
        }
        if existing_activations
            .iter()
            .any(|existing| existing == activation)
        {
            return self.persist_sequence(sender, sequence).await;
        }

        let mut tx = db.begin_transaction().await?;
        tx.add_federation_dkg_activation(activation).await?;
        tx.set_federation_member_sequence(sender, sequence).await?;
        tx.commit().await?;
        self.record_sequence(sender, sequence).await;
        Ok(())
    }

    pub(crate) fn ready_dkg_finalized_member_config(
        &self,
    ) -> Option<Arc<FederationDkgFinalizedMemberConfig>> {
        self.dkg_member_admission
            .as_ref()?
            .finalized_config
            .get()
            .cloned()
    }

    pub(crate) async fn remember_dkg_finalized_member_config(
        &self,
        finalized: Arc<FederationDkgFinalizedMemberConfig>,
    ) -> Result<(), cdk::Error> {
        let Some(admission) = self.dkg_member_admission.as_ref() else {
            return Ok(());
        };
        let cached = admission
            .finalized_config
            .get_or_init(|| async { finalized.clone() })
            .await;
        if cached.as_ref() != finalized.as_ref() {
            return Err(cdk::Error::Federation(
                FederationError::DkgTranscriptMismatch {
                    reason: "cached finalized DKG config changed during one process".to_string(),
                },
            ));
        }
        Ok(())
    }

    async fn expected_dkg_transcript_hash(
        &self,
        setup_public_config: &FederationConfig,
    ) -> Result<FederationDkgTranscriptHash, cdk::Error> {
        let mint = self.active_mint()?;
        let runtime_config = mint.federation_config().ok_or_else(|| {
            cdk::Error::Custom("federation mint is missing its runtime config".to_string())
        })?;
        let local_member = runtime_config.member_id();
        let keyset_shares = mint
            .localstore()
            .get_federation_dkg_member_keyset_shares()
            .await?;
        for share in &keyset_shares {
            if share.member_id != local_member {
                return Err(cdk::Error::Federation(FederationError::MemberIdMismatch {
                    expected: local_member,
                    found: share.member_id,
                }));
            }
        }
        Ok(
            FederationDkgMemberKeysetShare::transcript_hash_for_locally_derived_records(
                setup_public_config,
                &keyset_shares,
            )?,
        )
    }

    fn public_config(&self) -> Result<FederationConfig, cdk::Error> {
        self.active_mint()?
            .federation_config()
            .map(|config| config.public_config().clone())
            .ok_or_else(|| {
                cdk::Error::Custom("federation mint is missing its runtime config".to_string())
            })
    }

    async fn load_dkg_setup_state(
        &self,
        public_config: &FederationConfig,
        ceremony_id: FederationDkgCeremonyId,
    ) -> Result<FederationDkgSetupState, cdk::Error> {
        let db = self.active_mint()?.localstore();
        let commitments = db.get_federation_dkg_commitments().await?;
        let reveals = db.get_federation_dkg_reveals().await?;
        let mut setup_state = FederationDkgSetupState::new(public_config);
        for commitment in commitments
            .into_iter()
            .filter(|commitment| commitment.ceremony_id == ceremony_id)
        {
            setup_state.insert_commitment(public_config, commitment)?;
        }
        for reveal in reveals
            .into_iter()
            .filter(|reveal| reveal.ceremony_id == ceremony_id)
        {
            setup_state.insert_reveal(public_config, reveal)?;
        }
        Ok(setup_state)
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
