//! Federation node runtime ownership.

use std::collections::BTreeMap;
#[cfg(test)]
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;

use super::{FederationDkgAutoStartConfig, FederationPeerSyncConfig};
use crate::{
    FederationAlephBftLiveSessionState, FederationAlephBftNetworkInboundState,
    FederationConsensusHandle, FederationDkgMemberAdmissionHandle,
    FederationInvalidOperationAlertHandle, FederationNotificationHandle,
    FederationOperationNotificationStats, FederationOperationWaitRegistration,
    FederationRestAdmissionHandle, FederationSequenceHandle, FederationSyncHandle,
    FederationSyncState, FederationSyncStatus,
};
use cdk::federation::{
    FederationAlephBftFinalizedItem, FederationAlephBftFinalizedItemStatus,
    FederationAlephBftPendingOperationRecord, FederationAlephBftUnitPayload,
    FederationApplicationState, FederationCatchUpCertificate, FederationCatchUpOperation,
    FederationCheckpoint, FederationCheckpointBoundary, FederationCheckpointFetchResponse,
    FederationCheckpointId, FederationCheckpointKind, FederationCheckpointQuorum,
    FederationCheckpointQuorumResponse, FederationCheckpointSignature,
    FederationCheckpointSignatureResponse, FederationCheckpointSummary,
    FederationCheckpointSummaryResponse, FederationConfig, FederationConsensusItem,
    FederationConsensusItemAdmission, FederationConsensusRuntime, FederationConsensusRuntimeStatus,
    FederationDkgActivationRequest, FederationDkgCeremonyId, FederationDkgFinalizedMemberConfig,
    FederationDkgMemberKeysetShare, FederationDkgParticipantCommitment,
    FederationDkgParticipantCommitmentReveal, FederationDkgSecretShareRequest,
    FederationDkgSetupState, FederationDkgTranscriptHash, FederationDkgTranscriptSignatureRequest,
    FederationError, FederationFrostDkgConfirmationRequest, FederationFrostDkgLocalState,
    FederationFrostDkgRound1Request, FederationFrostDkgRound2Record, FederationFrostRootRecord,
    FederationJournalAppendOutcome, FederationJournalCatchUpResponse, FederationJournalDigest,
    FederationJournalEntry, FederationMaterializedStateDigest, FederationOperationEnvelope,
    FederationOperationId, FederationOperationStatus, FederationRestMessageKind,
    FederationSubmitOperationResponse, MemberId, FEDERATION_REST_API_VERSION,
};
use cdk::mint::Mint;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct MaterializedStateDigestCache {
    pub(super) operation_count: u64,
    pub(super) latest_order_digest: FederationJournalDigest,
    pub(super) latest_operation_status: Option<FederationOperationStatus>,
    pub(super) materialized_state_digest: FederationMaterializedStateDigest,
}

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
    pub(crate) payment_observer: super::FederationPaymentObserver,
    pub(crate) transaction_broadcaster: super::FederationTransactionBroadcaster,
    pub(crate) wallet_liveness: super::FederationWalletLivenessWorker,
    pub(crate) onchain_wallets: Vec<cdk_bdk::FederatedBdkWallet>,
    pub(crate) frost_signing: Option<super::frost_signing::FederationFrostSigningRuntime>,
    /// Serializes journal persistence and application across live finality and catch-up.
    pub(crate) operation_application: Arc<tokio::sync::Mutex<()>>,
    pub(crate) catch_up_certificate_quorum_cache:
        Arc<tokio::sync::Mutex<Option<Option<super::CatchUpCertificateQuorumStatus>>>>,
    pub(crate) materialized_state_digest_cache:
        Arc<tokio::sync::Mutex<Option<MaterializedStateDigestCache>>>,
    pub(crate) recovery_checkpoint_requested: Arc<AtomicBool>,
    pub(crate) recovery_checkpoint_observation:
        Arc<tokio::sync::Mutex<Option<FederationCheckpointBoundary>>>,
    #[cfg(test)]
    pub(crate) catch_up_certificate_quorum_validation_count: Arc<AtomicUsize>,
    #[cfg(test)]
    pub(crate) catch_up_certificate_journal_load_count: Arc<AtomicUsize>,
    #[cfg(test)]
    pub(crate) materialized_state_digest_computation_count: Arc<AtomicUsize>,
    #[cfg(test)]
    pub(crate) fail_next_pending_operation_persistence: Arc<AtomicBool>,
    #[cfg(test)]
    pub(crate) suppress_next_terminal_response_publication: Arc<AtomicBool>,
    pub(crate) aleph_bft_network_inbound: FederationAlephBftNetworkInboundState,
    pub(crate) aleph_bft_live_session: FederationAlephBftLiveSessionState,
    pub(crate) aleph_bft_halt_epoch: Arc<AtomicU64>,
    #[cfg(test)]
    pub(crate) local_finality_test: bool,
    #[cfg(test)]
    pub(crate) manual_finality_test: bool,
}

fn runtime_unavailable(component: &'static str) -> cdk::Error {
    FederationError::RuntimeUnavailable { component }.into()
}

fn checkpoint_summary(
    checkpoint: &FederationCheckpoint,
    config: &FederationConfig,
) -> Result<FederationCheckpointSummary, cdk::Error> {
    checkpoint.validate_against_config(config)?;
    Ok(FederationCheckpointSummary::from_checkpoint(checkpoint)?)
}

fn checkpoint_quorum_summary(
    quorum: &FederationCheckpointQuorum,
    config: &FederationConfig,
) -> Result<FederationCheckpointSummary, cdk::Error> {
    quorum.verify(config)?;
    checkpoint_summary(&quorum.checkpoint, config)
}

fn checkpoint_matches_boundary(
    checkpoint: &FederationCheckpoint,
    boundary: &FederationCheckpointBoundary,
) -> bool {
    checkpoint.journal_next_index == boundary.journal_next_index
        && checkpoint.journal_order_digest == boundary.journal_order_digest
        && checkpoint.finalized_next_item_index == boundary.finalized_next_item_index
        && checkpoint.finalized_item_digest == boundary.finalized_item_digest
        && checkpoint.application_digest == boundary.application_digest
        && checkpoint.aleph_bft_session == boundary.aleph_bft_session
        && checkpoint.next_unit_finalization_index == boundary.next_unit_finalization_index
        && checkpoint.next_unit_index_by_member == boundary.next_unit_index_by_member
}

fn checkpoint_covers(current: &FederationCheckpoint, candidate: &FederationCheckpoint) -> bool {
    current.scheduled_checkpoint_sequence >= candidate.scheduled_checkpoint_sequence
        && current.journal_next_index >= candidate.journal_next_index
        && current.finalized_next_item_index >= candidate.finalized_next_item_index
}

fn finalized_unit_recovery_mismatch(
    payload: &FederationAlephBftUnitPayload,
    reason: &str,
) -> cdk::Error {
    FederationError::AlephBftFinalizedItemRecoveryMismatch {
        reason: format!(
            "{reason}: member={} session={} unit={}",
            payload.submitting_member, payload.session, payload.unit_index
        ),
    }
    .into()
}

fn federation_not_ready_error(status: FederationSyncStatus) -> cdk::Error {
    match status {
        FederationSyncStatus::Healthy => runtime_unavailable("readiness gate"),
        FederationSyncStatus::AwaitingQuorum { operation_count } => {
            FederationError::NodeAwaitingQuorum { operation_count }.into()
        }
        FederationSyncStatus::Lagging {
            peer,
            local_operation_count,
            peer_operation_count,
        } => FederationError::NodeLagging {
            peer,
            local_operation_count,
            peer_operation_count,
        }
        .into(),
        FederationSyncStatus::CatchingUp { peer } => {
            FederationError::NodeCatchingUp { peer }.into()
        }
        FederationSyncStatus::VerifyingCatchUpCertificate {
            peer,
            operation_count,
        } => FederationError::NodeVerifyingCatchUpCertificate {
            peer,
            operation_count,
        }
        .into(),
        FederationSyncStatus::MaterializedStateMismatch {
            peer,
            operation_count,
        } => FederationError::NodeMaterializedStateMismatch {
            peer,
            operation_count,
        }
        .into(),
        FederationSyncStatus::Halted { reason } => FederationError::NodeHalted {
            reason: cdk::federation::redact_federation_operator_message(&reason),
        }
        .into(),
    }
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
            payment_observer: super::FederationPaymentObserver::default(),
            transaction_broadcaster: super::FederationTransactionBroadcaster::default(),
            wallet_liveness: super::FederationWalletLivenessWorker::default(),
            onchain_wallets: Vec::new(),
            frost_signing: None,
            operation_application: Arc::new(tokio::sync::Mutex::new(())),
            catch_up_certificate_quorum_cache: Arc::new(tokio::sync::Mutex::new(None)),
            materialized_state_digest_cache: Arc::new(tokio::sync::Mutex::new(None)),
            recovery_checkpoint_requested: Arc::new(AtomicBool::new(false)),
            recovery_checkpoint_observation: Arc::new(tokio::sync::Mutex::new(None)),
            catch_up_certificate_quorum_validation_count: Arc::new(AtomicUsize::new(0)),
            catch_up_certificate_journal_load_count: Arc::new(AtomicUsize::new(0)),
            materialized_state_digest_computation_count: Arc::new(AtomicUsize::new(0)),
            fail_next_pending_operation_persistence: Arc::new(AtomicBool::new(false)),
            suppress_next_terminal_response_publication: Arc::new(AtomicBool::new(false)),
            aleph_bft_network_inbound: Arc::new(tokio::sync::RwLock::new(None)),
            aleph_bft_live_session: Arc::new(tokio::sync::RwLock::new(None)),
            aleph_bft_halt_epoch: Arc::new(AtomicU64::new(0)),
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
        envelope.verify_authorization(config.public_config())?;
        config.validate_federated_operation_submission(&envelope.operation)?;
        if let cdk::federation::FederationOperation::TransactionSigned(signed) = &envelope.operation
        {
            mint.validate_federation_signed_transaction_submission(signed)
                .await?;
        }
        match &envelope.operation {
            cdk::federation::FederationOperation::TransactionBroadcastIntent(intent) => {
                mint.validate_federation_transaction_broadcast_intent_submission(intent)
                    .await?;
            }
            cdk::federation::FederationOperation::TransactionBroadcastObservation(observation) => {
                mint.validate_federation_transaction_broadcast_observation_submission(observation)
                    .await?;
            }
            _ => {}
        }
        Ok(())
    }

    fn active_mint(&self) -> Result<&Arc<Mint>, cdk::Error> {
        self.mint
            .as_ref()
            .ok_or_else(|| runtime_unavailable("mint"))
    }

    pub(super) async fn catch_up_certificate_quorum_status(
        &self,
        mint: &Mint,
    ) -> Result<Option<super::CatchUpCertificateQuorumStatus>, cdk::Error> {
        let mut cache = self.catch_up_certificate_quorum_cache.lock().await;
        if let Some(status) = cache.as_ref() {
            return Ok(status.clone());
        }
        #[cfg(test)]
        self.catch_up_certificate_quorum_validation_count
            .fetch_add(1, Ordering::SeqCst);
        let status = super::catch_up_certificate_quorum_status(mint).await?;
        *cache = Some(status.clone());
        Ok(status)
    }

    async fn invalidate_catch_up_certificate_quorum_status(&self) {
        *self.catch_up_certificate_quorum_cache.lock().await = None;
    }

    async fn invalidate_materialized_state_digest(&self) {
        *self.materialized_state_digest_cache.lock().await = None;
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
        if response.outcome.entry().status() == FederationOperationStatus::Rejected {
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

    #[cfg(test)]
    pub(crate) async fn wait_for_pending_response_waiter_count(&self, expected: usize) {
        match self.notifications.as_ref() {
            Some(notifications) => notifications.wait_for_pending_waiter_count(expected).await,
            None => assert_eq!(
                expected, 0,
                "inert runtime cannot register response waiters"
            ),
        }
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

    async fn publish_terminal_operation_responses(
        &self,
        responses: &[FederationSubmitOperationResponse],
    ) -> Result<Vec<FederationSubmitOperationResponse>, cdk::Error> {
        let responses = self.responses_with_current_status(responses).await;
        if let Some(response) = responses.iter().find(|response| {
            !matches!(
                response.outcome.entry().status(),
                FederationOperationStatus::Applied | FederationOperationStatus::Rejected
            )
        }) {
            return Err(cdk::Error::Custom(format!(
                "federation operation {} was not terminal after finalized application",
                response.operation_id
            )));
        }
        super::aleph_bft_latency::record(
            super::aleph_bft_latency::CdkAlephBftLatencyStage::ApplyCompleted,
            responses.iter().map(|response| response.operation_id),
        );
        #[cfg(test)]
        if self
            .suppress_next_terminal_response_publication
            .swap(false, Ordering::SeqCst)
        {
            return Ok(responses);
        }
        for response in &responses {
            self.notify_operation(response.clone());
        }
        Ok(responses)
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

        if finalized_items
            .iter()
            .any(|item| item.unit_body_hash != payload.body_hash)
        {
            // AlephBFT may order both branches emitted by an equivocating creator at the
            // same round. Its common ordering makes every honest member select the same
            // first branch, so the later branch is a live duplicate, not recovery
            // corruption. Retained finalized-item recovery remains strict.
            tracing::warn!(
                member = %payload.submitting_member,
                session = payload.session,
                unit = payload.unit_index,
                "Ignoring conflicting branch for an already finalized AlephBFT unit"
            );
            return Ok(Some(Vec::new()));
        }

        if finalized_items.len() != payload.items.len() {
            return Err(finalized_unit_recovery_mismatch(
                payload,
                "replay changed item count",
            ));
        }

        let mut responses = Vec::new();
        for (expected_index, finalized_item) in finalized_items.iter().enumerate() {
            let expected_index_u64 =
                u64::try_from(expected_index).expect("unit item index fits in u64");
            if finalized_item.item_index_in_unit != expected_index_u64 {
                return Err(finalized_unit_recovery_mismatch(
                    payload,
                    "replay changed item order",
                ));
            }
            let Some(payload_item) = payload.items.get(expected_index) else {
                return Err(finalized_unit_recovery_mismatch(
                    payload,
                    "replay is missing an item",
                ));
            };
            if finalized_item.item.canonical_bytes()? != payload_item.canonical_bytes()? {
                return Err(finalized_unit_recovery_mismatch(
                    payload,
                    "replay changed item body",
                ));
            }
            let payload_operation_id = payload_item.operation_id()?;
            if finalized_item.operation_id != payload_operation_id {
                return Err(finalized_unit_recovery_mismatch(
                    payload,
                    "replay changed operation ID",
                ));
            }

            if finalized_item.status
                == FederationAlephBftFinalizedItemStatus::RejectedInvalidOperation
            {
                continue;
            }

            if let Some(operation_id) = finalized_item.operation_id {
                let entry = consensus
                    .journal()
                    .entry_by_operation_id(operation_id)
                    .ok_or_else(|| {
                        finalized_unit_recovery_mismatch(
                            payload,
                            &format!("operation {operation_id} is missing from the local journal"),
                        )
                    })?;
                responses.push(FederationSubmitOperationResponse::from_outcome(
                    FederationJournalAppendOutcome::Duplicate(entry.clone()),
                ));
            }
        }

        Ok(Some(responses))
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
        sync.lock().await.update_status(update);
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
        let initial_status = sync.lock().await.status();
        match &initial_status {
            FederationSyncStatus::Healthy
            | FederationSyncStatus::AwaitingQuorum { .. }
            | FederationSyncStatus::VerifyingCatchUpCertificate { .. } => {}
            FederationSyncStatus::Lagging { .. }
            | FederationSyncStatus::CatchingUp { .. }
            | FederationSyncStatus::MaterializedStateMismatch { .. }
            | FederationSyncStatus::Halted { .. } => return,
        }

        let peer = config
            .public_config()
            .members
            .values()
            .find(|member| member.member_id != config.member_id())
            .map(|member| member.member_id)
            .unwrap_or_else(|| config.member_id());

        let entries = match mint.localstore().get_federation_journal_entries().await {
            Ok(entries) => entries,
            Err(err) => {
                self.finish_startup_revalidation(
                    &initial_status,
                    FederationSyncStatus::Halted {
                        reason: format!(
                            "could not inspect federation startup journal for quorum evidence: {err}"
                        ),
                    },
                )
                .await;
                return;
            }
        };
        let operation_count = entries.len() as u64;
        if operation_count == 0 {
            self.finish_startup_revalidation(
                &initial_status,
                FederationSyncStatus::AwaitingQuorum { operation_count },
            )
            .await;
            return;
        }

        let quorum = match self.catch_up_certificate_quorum_status(mint).await {
            Ok(quorum) => quorum,
            Err(err) => {
                self.finish_startup_revalidation(
                    &initial_status,
                    FederationSyncStatus::Halted {
                        reason: format!(
                            "could not inspect federation startup catch-up certificate quorum: {err}"
                        ),
                    },
                )
                .await;
                return;
            }
        };

        let Some(quorum) = quorum else {
            self.finish_startup_revalidation(
                &initial_status,
                FederationSyncStatus::VerifyingCatchUpCertificate {
                    peer,
                    operation_count,
                },
            )
            .await;
            return;
        };

        if quorum.operation_count != operation_count {
            self.finish_startup_revalidation(
                &initial_status,
                FederationSyncStatus::VerifyingCatchUpCertificate {
                    peer,
                    operation_count,
                },
            )
            .await;
            return;
        }

        let local_digest = match mint.federation_materialized_state_snapshot().await {
            Ok(snapshot) => match snapshot.digest() {
                Ok(digest) => digest,
                Err(err) => {
                    self.finish_startup_revalidation(
                        &initial_status,
                        FederationSyncStatus::Halted {
                            reason: format!(
                                "could not compute federation startup materialized-state digest: {err}"
                            ),
                        },
                    )
                    .await;
                    return;
                }
            },
            Err(err) => {
                self.finish_startup_revalidation(
                    &initial_status,
                    FederationSyncStatus::Halted {
                        reason: format!(
                            "could not project federation startup materialized state: {err}"
                        ),
                    },
                )
                .await;
                return;
            }
        };
        let next_status = if local_digest == quorum.materialized_state_digest {
            FederationSyncStatus::AwaitingQuorum { operation_count }
        } else {
            FederationSyncStatus::MaterializedStateMismatch {
                peer,
                operation_count,
            }
        };
        self.finish_startup_revalidation(&initial_status, next_status)
            .await;
    }

    async fn finish_startup_revalidation(
        &self,
        expected: &FederationSyncStatus,
        next: FederationSyncStatus,
    ) {
        self.update_sync_status(|sync| sync.finish_startup_revalidation(expected, next))
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

    pub(crate) async fn mark_awaiting_quorum(&self, operation_count: u64) {
        self.update_sync_status(|sync| sync.mark_awaiting_quorum(operation_count))
            .await
    }

    pub(crate) async fn mark_catching_up(&self, peer: MemberId) {
        self.update_sync_status(|sync| sync.mark_catching_up(peer))
            .await
    }

    pub(crate) async fn mark_verifying_catch_up_certificate(
        &self,
        peer: MemberId,
        operation_count: u64,
    ) {
        self.update_sync_status(|sync| {
            sync.mark_verifying_catch_up_certificate(peer, operation_count);
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

    pub(crate) async fn mark_healthy(&self) {
        self.update_sync_status(FederationSyncState::mark_healthy)
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
            return Err(federation_not_ready_error(status));
        }
        if self.live_session_available().await {
            return Ok(());
        }
        Err(runtime_unavailable("AlephBFT live session"))
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
        let status = self
            .aleph_bft_status()
            .await
            .ok_or_else(|| runtime_unavailable("AlephBFT consensus"))?;
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
            .ok_or_else(|| runtime_unavailable("AlephBFT activity receiver"))?;
        handle.mark_peer_activity(operation_id);
        Ok(())
    }

    pub(crate) async fn submit_envelope(
        &self,
        envelope: FederationOperationEnvelope,
    ) -> Result<FederationSubmitOperationResponse, cdk::Error> {
        #[cfg(feature = "prometheus")]
        let operation_label = envelope.operation.kind().to_string();
        let operation_id = envelope.operation_id()?;
        let sync_config = self.peer_sync_config.unwrap_or_default();
        let notifications = self
            .notifications
            .as_ref()
            .ok_or_else(|| runtime_unavailable("operation notifications"))?;
        let consensus = self
            .consensus
            .as_ref()
            .ok_or_else(|| runtime_unavailable("consensus"))?;
        let mut consensus = consensus.lock().await;
        if let Some(entry) = consensus.journal().entry_by_operation_id(operation_id) {
            if matches!(
                entry.status(),
                FederationOperationStatus::Accepted | FederationOperationStatus::Applying
            ) {
                notifications.record_duplicate_pending_waiter_join();
                let wait_registration = notifications
                    .register_waiter(operation_id, sync_config.response_waiter_limit)?;
                drop(consensus);
                self.mark_consensus_activity(operation_id, true).await;
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
        if accepted_admission {
            let session_id = next_consensus.aleph_bft_status()?.session;
            self.persist_pending_operation(FederationAlephBftPendingOperationRecord {
                operation_id,
                session_id,
                envelope,
            })
            .await?;
        }

        *consensus = next_consensus;
        drop(consensus);
        self.mark_consensus_activity(operation_id, true).await;
        if accepted_admission {
            #[cfg(feature = "prometheus")]
            self.record_accepted_operation_metric(&operation_label);
            super::aleph_bft_latency::record(
                super::aleph_bft_latency::CdkAlephBftLatencyStage::SubmitQueued,
                [operation_id],
            );
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
            let consensus = self
                .consensus
                .as_ref()
                .ok_or_else(|| runtime_unavailable("consensus"))?;
            consensus.lock().await.next_aleph_bft_unit_payload()?
        };
        if let Some(unit) = unit {
            self.apply_finalized_unit(unit).await?;
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
        self.apply_verified_catch_up_response(response, &[]).await
    }

    pub(crate) async fn apply_catch_up_response_with_catch_up_certificate_quorum(
        &self,
        response: &FederationJournalCatchUpResponse,
        additional_certificates: &[FederationCatchUpCertificate],
    ) -> Result<Vec<FederationJournalEntry>, cdk::Error> {
        let runtime_config = self
            .active_mint()?
            .federation_config()
            .cloned()
            .ok_or_else(|| runtime_unavailable("mint configuration"))?;
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
        response.verify_catch_up_certificate_quorum(&public_config, additional_certificates)?;

        let mut catch_up_certificates = Vec::new();
        if let Some(catch_up_certificate) = response.catch_up_certificate.as_ref() {
            catch_up_certificates.push(catch_up_certificate);
        }
        catch_up_certificates.extend(additional_certificates.iter());
        for catch_up_certificate in &catch_up_certificates {
            let member =
                public_config
                    .member(catch_up_certificate.signer)
                    .ok_or(cdk::Error::Federation(FederationError::UnknownMember {
                        member_id: catch_up_certificate.signer,
                    }))?;
            catch_up_certificate.verify(
                member,
                response.federation_id,
                response.from_index,
                &response.entries,
                response.latest_order_digest,
                response.materialized_state_digest,
            )?;
        }

        self.apply_verified_catch_up_response(response, &catch_up_certificates)
            .await
    }

    async fn apply_verified_catch_up_response(
        &self,
        response: &FederationJournalCatchUpResponse,
        catch_up_certificates: &[&FederationCatchUpCertificate],
    ) -> Result<Vec<FederationJournalEntry>, cdk::Error> {
        let _application = self.operation_application.lock().await;
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
        let local_entries = consensus.journal().entries().to_vec();
        let local_finalized_items = consensus.aleph_bft_finalized_items()?.to_vec();
        let response_to_apply =
            crate::catch_up_response_suffix_for_local_journal(&local_entries, response)?;
        if let Some(response_to_apply) = response_to_apply.as_ref() {
            if response_to_apply.from_index != response.from_index
                || response_to_apply.entries.len() != response.entries.len()
            {
                return Err(cdk::Error::Federation(
                    FederationError::CatchUpCertificateMismatch {
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
        let mut next_consensus =
            FederationConsensusRuntime::authenticated_aleph_bft_from_finalized_items(
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
            crate::ensure_catch_up_overlap_matches(
                local_entry,
                &FederationCatchUpOperation::from(recovered_entry),
            )?;
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
            catch_up_certificates,
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

    pub(crate) async fn persist_dkg_sequence(
        &self,
        sender: MemberId,
        sequence: u64,
    ) -> Result<(), cdk::Error> {
        let db = self.active_mint()?.localstore();
        let mut tx = db.begin_transaction().await?;
        tx.set_federation_dkg_member_sequence(sender, sequence)
            .await?;
        tx.commit().await?;
        self.record_sequence(FederationRestMessageKind::DkgCommitment, sender, sequence)
            .await;
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
            return self.persist_dkg_sequence(sender, sequence).await;
        }
        let mut setup_state = self
            .load_dkg_setup_state(&public_config, commitment.ceremony_id)
            .await?;
        setup_state.insert_commitment(&public_config, *commitment)?;

        let mut tx = db.begin_transaction().await?;
        tx.add_federation_dkg_commitment(commitment).await?;
        tx.set_federation_dkg_member_sequence(sender, sequence)
            .await?;
        tx.commit().await?;
        self.record_sequence(FederationRestMessageKind::DkgCommitment, sender, sequence)
            .await;
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
            return self.persist_dkg_sequence(sender, sequence).await;
        }
        let mut setup_state = self
            .load_dkg_setup_state(&public_config, reveal.ceremony_id)
            .await?;
        setup_state.insert_reveal(&public_config, reveal.clone())?;

        let mut tx = db.begin_transaction().await?;
        tx.add_federation_dkg_reveal(reveal).await?;
        tx.set_federation_dkg_member_sequence(sender, sequence)
            .await?;
        tx.commit().await?;
        self.record_sequence(FederationRestMessageKind::DkgReveal, sender, sequence)
            .await;
        Ok(())
    }

    pub(crate) async fn persist_frost_dkg_round1(
        &self,
        request: &FederationFrostDkgRound1Request,
        sender: MemberId,
        sequence: u64,
    ) -> Result<(), cdk::Error> {
        let db = self.active_mint()?.localstore();
        let mut tx = db.begin_transaction().await?;
        tx.add_federation_frost_dkg_round1(request).await?;
        tx.set_federation_dkg_member_sequence(sender, sequence)
            .await?;
        tx.commit().await?;
        self.record_sequence(FederationRestMessageKind::FrostDkgRound1, sender, sequence)
            .await;
        Ok(())
    }

    pub(crate) async fn persist_frost_dkg_round2(
        &self,
        record: &FederationFrostDkgRound2Record,
        sender: MemberId,
        sequence: u64,
    ) -> Result<(), cdk::Error> {
        let db = self.active_mint()?.localstore();
        let mut tx = db.begin_transaction().await?;
        tx.add_federation_frost_dkg_round2(record).await?;
        tx.set_federation_dkg_member_sequence(sender, sequence)
            .await?;
        tx.commit().await?;
        self.record_sequence(FederationRestMessageKind::FrostDkgRound2, sender, sequence)
            .await;
        Ok(())
    }

    pub(crate) async fn persist_frost_dkg_local_state(
        &self,
        state: &FederationFrostDkgLocalState,
    ) -> Result<(), cdk::Error> {
        let db = self.active_mint()?.localstore();
        let mut tx = db.begin_transaction().await?;
        tx.set_federation_frost_dkg_local_state(state).await?;
        tx.commit().await?;
        Ok(())
    }

    pub(crate) async fn persist_frost_root_record(
        &self,
        record: &FederationFrostRootRecord,
    ) -> Result<(), cdk::Error> {
        let db = self.active_mint()?.localstore();
        let mut tx = db.begin_transaction().await?;
        tx.add_federation_frost_root_record(record).await?;
        tx.commit().await?;
        Ok(())
    }

    pub(crate) async fn persist_frost_dkg_confirmation(
        &self,
        request: &FederationFrostDkgConfirmationRequest,
        sender: MemberId,
        sequence: u64,
    ) -> Result<(), cdk::Error> {
        let db = self.active_mint()?.localstore();
        let mut tx = db.begin_transaction().await?;
        tx.add_federation_frost_dkg_confirmation(request).await?;
        tx.set_federation_dkg_member_sequence(sender, sequence)
            .await?;
        tx.commit().await?;
        self.record_sequence(
            FederationRestMessageKind::FrostDkgConfirmation,
            sender,
            sequence,
        )
        .await;
        Ok(())
    }

    pub(crate) async fn persist_dkg_secret_share_contribution(
        &self,
        contribution: &FederationDkgSecretShareRequest,
        sender: MemberId,
        sequence: u64,
    ) -> Result<(), cdk::Error> {
        let mint = self.active_mint()?;
        let runtime_config = mint
            .federation_config()
            .ok_or_else(|| runtime_unavailable("mint configuration"))?;
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
            return self.persist_dkg_sequence(sender, sequence).await;
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
        tx.set_federation_dkg_member_sequence(sender, sequence)
            .await?;
        tx.commit().await?;
        self.record_sequence(FederationRestMessageKind::DkgSecretShare, sender, sequence)
            .await;
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
            return self.persist_dkg_sequence(sender, sequence).await;
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
        tx.set_federation_dkg_member_sequence(sender, sequence)
            .await?;
        tx.commit().await?;
        self.record_sequence(
            FederationRestMessageKind::DkgTranscriptSignature,
            sender,
            sequence,
        )
        .await;
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
            return self.persist_dkg_sequence(sender, sequence).await;
        }

        let mut tx = db.begin_transaction().await?;
        tx.add_federation_dkg_activation(activation).await?;
        tx.set_federation_dkg_member_sequence(sender, sequence)
            .await?;
        tx.commit().await?;
        self.record_sequence(FederationRestMessageKind::DkgActivation, sender, sequence)
            .await;
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
        let runtime_config = mint
            .federation_config()
            .ok_or_else(|| runtime_unavailable("mint configuration"))?;
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
            .ok_or_else(|| runtime_unavailable("mint configuration"))
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
        catch_up_certificates: &[&FederationCatchUpCertificate],
    ) -> Result<(), cdk::Error> {
        let db = self.active_mint()?.localstore();
        let mut tx = db.begin_transaction().await?;
        for entry in entries {
            tx.add_federation_journal_entry(entry).await?;
        }
        for catch_up_certificate in catch_up_certificates {
            tx.add_federation_catch_up_certificate(catch_up_certificate)
                .await?;
        }
        tx.commit().await?;
        if !entries.is_empty() || !catch_up_certificates.is_empty() {
            self.invalidate_catch_up_certificate_quorum_status().await;
        }
        if !entries.is_empty() {
            self.invalidate_materialized_state_digest().await;
        }
        Ok(())
    }

    pub(crate) async fn persist_pending_operation(
        &self,
        operation: FederationAlephBftPendingOperationRecord,
    ) -> Result<(), cdk::Error> {
        #[cfg(test)]
        if self
            .fail_next_pending_operation_persistence
            .swap(false, Ordering::SeqCst)
        {
            return Err(cdk::Error::Custom(
                "injected pending-operation persistence failure".to_string(),
            ));
        }
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
        tx.commit().await?;
        if !entries.is_empty() {
            self.invalidate_catch_up_certificate_quorum_status().await;
            self.invalidate_materialized_state_digest().await;
        }
        Ok(())
    }

    pub(crate) async fn persist_catch_up(
        &self,
        active_session: u64,
        finalized_items: &[FederationAlephBftFinalizedItem],
        entries: &[FederationJournalEntry],
        catch_up_certificates: &[&FederationCatchUpCertificate],
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
        for catch_up_certificate in catch_up_certificates {
            tx.add_federation_catch_up_certificate(catch_up_certificate)
                .await?;
        }
        tx.commit().await?;
        if !entries.is_empty() || !catch_up_certificates.is_empty() {
            self.invalidate_catch_up_certificate_quorum_status().await;
        }
        if !entries.is_empty() {
            self.invalidate_materialized_state_digest().await;
        }
        Ok(())
    }

    pub(crate) async fn record_sequence(
        &self,
        kind: FederationRestMessageKind,
        sender: MemberId,
        sequence: u64,
    ) {
        if let Some(sequences) = &self.sequences {
            sequences
                .lock()
                .await
                .record_sequence(kind, sender, sequence);
        }
    }

    pub(crate) async fn apply_finalized_unit(
        &self,
        payload: FederationAlephBftUnitPayload,
    ) -> Result<Vec<FederationSubmitOperationResponse>, cdk::Error> {
        let _application = self.operation_application.lock().await;
        let db = self.active_mint()?.localstore();
        let mut durable_entries = BTreeMap::new();
        for item in &payload.items {
            let FederationConsensusItem::Operation(envelope) = item else {
                continue;
            };
            let operation_id = envelope.operation_id()?;
            if let Some(entry) = db
                .get_federation_journal_entry_by_operation_id(operation_id)
                .await?
            {
                durable_entries.insert(entry.index, entry);
            }
            for conflict_key in envelope.operation.conflict_keys() {
                if let Some(entry) = db
                    .get_federation_journal_entry_by_conflict_key(&conflict_key)
                    .await?
                {
                    durable_entries.insert(entry.index, entry);
                }
            }
        }
        let consensus = self
            .consensus
            .as_ref()
            .ok_or_else(|| runtime_unavailable("consensus"))?;
        let mut consensus = consensus.lock().await;
        if let Some(responses) = Self::duplicate_finalized_unit_responses(&consensus, &payload)? {
            drop(consensus);
            let entries = responses
                .iter()
                .map(|response| response.outcome.entry().clone())
                .collect::<Vec<_>>();
            let entries_to_apply = crate::accepted_federation_journal_entries(&entries);
            self.apply_journal_entries(&entries_to_apply).await?;
            self.capture_scheduled_checkpoint_if_eligible_under_application_lock()
                .await?;
            return self.publish_terminal_operation_responses(&responses).await;
        }

        let mut next_consensus = consensus.clone();
        for entry in durable_entries.into_values() {
            next_consensus.register_historical_journal_entry(entry)?;
        }
        let finalized_item_start = next_consensus.aleph_bft_finalized_items()?.len();
        let responses = next_consensus.finalize_aleph_bft_unit(payload)?;
        let finalized_items =
            next_consensus.aleph_bft_finalized_items()?[finalized_item_start..].to_vec();
        let entries = responses
            .iter()
            .filter_map(|response| match &response.outcome {
                FederationJournalAppendOutcome::Appended(entry) => Some(entry.clone()),
                FederationJournalAppendOutcome::Duplicate(_) => None,
            })
            .collect::<Vec<_>>();
        let entries_to_apply = crate::accepted_federation_journal_entries(&entries);
        self.persist_finalization(&finalized_items, &entries)
            .await?;
        *consensus = next_consensus;
        drop(consensus);

        self.apply_journal_entries(&entries_to_apply).await?;
        self.capture_scheduled_checkpoint_if_eligible_under_application_lock()
            .await?;
        self.publish_terminal_operation_responses(&responses).await
    }

    async fn capture_scheduled_checkpoint_if_eligible_under_application_lock(
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
                quorum.verify(public_config)?;
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
        if !boundary.is_scheduled_eligible(public_config.checkpoint_policy, latest_scheduled)? {
            return Ok(());
        }

        let materialized_state_digest = mint
            .federation_materialized_state_snapshot()
            .await?
            .digest()?;
        let checkpoint = boundary.scheduled_checkpoint(
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
        Ok(())
    }

    pub(crate) async fn ensure_scheduled_checkpoint_candidate(&self) -> Result<(), cdk::Error> {
        let _application = self.operation_application.lock().await;
        self.capture_scheduled_checkpoint_if_eligible_under_application_lock()
            .await
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

    pub(crate) async fn stable_checkpoint_boundary(
        &self,
    ) -> Result<Option<FederationCheckpointBoundary>, cdk::Error> {
        let _application = self.operation_application.lock().await;
        self.current_checkpoint_boundary_under_application_lock()
            .await
    }

    pub(crate) async fn checkpoint_summary(
        &self,
    ) -> Result<FederationCheckpointSummaryResponse, cdk::Error> {
        self.ensure_scheduled_checkpoint_candidate().await?;
        let stable_tip = self.stable_checkpoint_boundary().await?;
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
            .map(|quorum| checkpoint_quorum_summary(&quorum, public_config))
            .transpose()?;
        let latest_recovery_quorum = db
            .get_latest_federation_checkpoint_quorum(FederationCheckpointKind::Recovery)
            .await?
            .map(|quorum| checkpoint_quorum_summary(&quorum, public_config))
            .transpose()?;

        Ok(FederationCheckpointSummaryResponse {
            version: FEDERATION_REST_API_VERSION,
            federation_id: runtime_config.federation_id(),
            member_id: runtime_config.member_id(),
            stable_tip,
            latest_scheduled_candidate,
            latest_recovery_candidate,
            latest_scheduled_quorum,
            latest_recovery_quorum,
        })
    }

    pub(crate) fn request_recovery_checkpoint(&self) {
        self.recovery_checkpoint_requested
            .store(true, Ordering::Release);
    }

    pub(crate) fn recovery_checkpoint_is_requested(&self) -> bool {
        self.recovery_checkpoint_requested.load(Ordering::Acquire)
    }

    pub(crate) async fn observe_recovery_checkpoint_boundary(
        &self,
        boundary: Option<&FederationCheckpointBoundary>,
    ) -> bool {
        let mut observation = self.recovery_checkpoint_observation.lock().await;
        let Some(boundary) = boundary else {
            *observation = None;
            return false;
        };
        if observation.as_ref() == Some(boundary) {
            true
        } else {
            *observation = Some(boundary.clone());
            false
        }
    }

    async fn clear_recovery_checkpoint_request(&self) {
        self.recovery_checkpoint_requested
            .store(false, Ordering::Release);
        *self.recovery_checkpoint_observation.lock().await = None;
    }

    async fn clear_recovery_checkpoint_request_if_current(
        &self,
        checkpoint: &FederationCheckpoint,
    ) -> Result<(), cdk::Error> {
        let current_boundary = self
            .current_checkpoint_boundary_under_application_lock()
            .await?;
        if current_boundary
            .as_ref()
            .is_some_and(|boundary| checkpoint_matches_boundary(checkpoint, boundary))
        {
            self.clear_recovery_checkpoint_request().await;
        }
        Ok(())
    }

    pub(crate) async fn capture_recovery_checkpoint(
        &self,
        expected_boundary: &FederationCheckpointBoundary,
    ) -> Result<Option<FederationCheckpoint>, cdk::Error> {
        let _application = self.operation_application.lock().await;
        let Some(boundary) = self
            .current_checkpoint_boundary_under_application_lock()
            .await?
        else {
            return Ok(None);
        };
        if &boundary != expected_boundary {
            return Ok(None);
        }

        let mint = self.active_mint()?;
        let runtime_config = mint
            .federation_config()
            .ok_or_else(|| runtime_unavailable("mint configuration"))?;
        let public_config = runtime_config.public_config();
        boundary.validate_against_config(public_config)?;
        let db = mint.localstore();
        let latest_scheduled_quorum = db
            .get_latest_federation_checkpoint_quorum(FederationCheckpointKind::Scheduled)
            .await?;
        let latest_scheduled = match latest_scheduled_quorum.as_ref() {
            Some(quorum) => {
                quorum.verify(public_config)?;
                Some(&quorum.checkpoint)
            }
            None => None,
        };
        if latest_scheduled.is_some_and(|checkpoint| {
            checkpoint.journal_next_index > boundary.journal_next_index
                || checkpoint.finalized_next_item_index > boundary.finalized_next_item_index
        }) {
            return Ok(None);
        }
        if latest_scheduled
            .is_some_and(|checkpoint| checkpoint_matches_boundary(checkpoint, &boundary))
        {
            self.clear_recovery_checkpoint_request().await;
            return Ok(None);
        }
        let (scheduled_sequence, scheduled_anchor_id) = match latest_scheduled {
            Some(checkpoint) => (
                checkpoint.scheduled_checkpoint_sequence,
                Some(checkpoint.id()?),
            ),
            None => (0, None),
        };
        if let Some(quorum) = db
            .get_latest_federation_checkpoint_quorum(FederationCheckpointKind::Recovery)
            .await?
        {
            quorum.verify(public_config)?;
            if quorum.checkpoint.journal_next_index > boundary.journal_next_index
                || quorum.checkpoint.finalized_next_item_index > boundary.finalized_next_item_index
            {
                return Ok(None);
            }
            if quorum.checkpoint.scheduled_checkpoint_sequence == scheduled_sequence
                && quorum.checkpoint.scheduled_anchor_id == scheduled_anchor_id
                && checkpoint_matches_boundary(&quorum.checkpoint, &boundary)
            {
                self.clear_recovery_checkpoint_request().await;
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

        let materialized_state_digest = mint
            .federation_materialized_state_snapshot()
            .await?
            .digest()?;
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
        let quorum = db.get_federation_checkpoint_quorum(checkpoint_id).await?;
        if let Some(quorum) = quorum.as_ref() {
            quorum.verify(public_config)?;
        }
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
        self.ensure_scheduled_checkpoint_candidate().await?;
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
            signature.verify(checkpoint, member)?;
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
        let _application = self.operation_application.lock().await;
        let mint = self.active_mint()?;
        let runtime_config = mint
            .federation_config()
            .ok_or_else(|| runtime_unavailable("mint configuration"))?;
        quorum.verify(runtime_config.public_config())?;
        let checkpoint_id = quorum.checkpoint.id()?;
        let db = mint.localstore();
        let mut tx = db.begin_transaction().await?;
        tx.add_federation_checkpoint_quorum(quorum).await?;
        tx.commit().await?;
        let frontier = db.get_federation_checkpoint_frontier().await?;
        let locally_reached = !frontier.materialized_state_audit_mismatch
            && quorum.checkpoint.journal_next_index <= frontier.journal_next_index
            && quorum.checkpoint.journal_next_index <= frontier.application_next_index
            && quorum.checkpoint.finalized_next_item_index <= frontier.finalized_next_item_index;
        let should_promote = if locally_reached {
            match db.get_trusted_federation_checkpoint_quorum().await? {
                Some(current) => {
                    current.verify(runtime_config.public_config())?;
                    if checkpoint_covers(&current.checkpoint, &quorum.checkpoint) {
                        false
                    } else {
                        quorum
                            .checkpoint
                            .validate_promotion_from(Some(&current.checkpoint))?;
                        true
                    }
                }
                None => true,
            }
        } else {
            false
        };
        if should_promote {
            let mut tx = db.begin_transaction().await?;
            tx.promote_verified_federation_checkpoint(quorum).await?;
            tx.commit().await?;
        }
        if should_promote && quorum.checkpoint.kind == FederationCheckpointKind::Recovery {
            self.clear_recovery_checkpoint_request_if_current(&quorum.checkpoint)
                .await?;
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
        let application = self.operation_application.lock().await;
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
        signature.verify(&checkpoint, member)?;
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
        quorum.verify(public_config)?;
        drop(application);
        self.persist_checkpoint_quorum(&quorum).await?;
        Ok(Some(quorum))
    }

    pub(crate) async fn apply_journal_entries(
        &self,
        entries: &[FederationJournalEntry],
    ) -> Result<Vec<FederationOperationId>, cdk::Error> {
        if !entries.is_empty() {
            self.invalidate_materialized_state_digest().await;
        }
        let db = self.active_mint()?.localstore();
        let mut applied = Vec::new();
        let mut terminal_applications = Vec::with_capacity(entries.len());
        for entry in entries {
            let persisted_before_apply = db
                .get_federation_journal_entry_by_operation_id(entry.operation_id)
                .await?;
            let persisted = match persisted_before_apply {
                Some(persisted)
                    if matches!(
                        persisted.status(),
                        FederationOperationStatus::Applied | FederationOperationStatus::Rejected
                    ) =>
                {
                    persisted
                }
                _ => {
                    if self.apply_journal_entry_for_replay(entry).await?.is_some() {
                        applied.push(entry.operation_id);
                    }
                    db.get_federation_journal_entry_by_operation_id(entry.operation_id)
                        .await?
                        .ok_or_else(|| {
                            cdk::Error::Federation(FederationError::UnknownOperation {
                                operation_id: entry.operation_id.to_string(),
                            })
                        })?
                }
            };
            if !matches!(
                persisted.status(),
                FederationOperationStatus::Applied | FederationOperationStatus::Rejected
            ) {
                return Err(cdk::Error::Custom(format!(
                    "federation operation {} remained non-terminal after journal application",
                    entry.operation_id
                )));
            }
            terminal_applications.push((entry.operation_id, persisted.application));
        }
        self.mark_operation_applications(&terminal_applications)
            .await?;
        if !entries.is_empty() {
            self.invalidate_materialized_state_digest().await;
        }
        let mint = self.active_mint()?;
        crate::reconcile_federation_onchain_wallets(mint, &self.onchain_wallets)
            .await
            .map_err(|_| {
                cdk::Error::Custom(
                    "accepted federation on-chain state could not be reconciled locally"
                        .to_string(),
                )
            })?;
        Ok(applied)
    }

    pub(crate) async fn verify_catch_up_rejections(
        &self,
        peer_entries: &[FederationCatchUpOperation],
    ) -> Result<(), cdk::Error> {
        for peer_entry in peer_entries
            .iter()
            .filter(|entry| entry.application.status() == FederationOperationStatus::Rejected)
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
            if local_entry.application != peer_entry.application {
                return Err(cdk::Error::Federation(
                    FederationError::CatchUpCertificateMismatch {
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

    async fn mark_operation_applications(
        &self,
        operations: &[(FederationOperationId, FederationApplicationState)],
    ) -> Result<(), cdk::Error> {
        let Some(consensus) = &self.consensus else {
            return Ok(());
        };
        let mut consensus = consensus.lock().await;
        for (operation_id, application) in operations {
            consensus.update_application_state(*operation_id, application.clone())?;
        }
        Ok(())
    }

    async fn persist_operation_apply_failure(
        &self,
        operation_id: FederationOperationId,
        rejection: Option<&cdk::error::ErrorResponse>,
    ) -> Result<FederationOperationStatus, cdk::Error> {
        let application = match rejection {
            Some(rejection) => FederationApplicationState::Rejected {
                response: rejection.clone(),
            },
            None => FederationApplicationState::Failed {},
        };
        let status = application.status();
        let db = self.active_mint()?.localstore();
        let mut tx = db.begin_transaction().await?;
        let outcome = tx
            .set_federation_operation_application_state(operation_id, &application)
            .await;
        match outcome {
            Ok(()) => tx.commit().await?,
            Err(error) => {
                tx.rollback().await?;
                return Err(error.into());
            }
        }
        self.invalidate_materialized_state_digest().await;

        if let Some(consensus) = &self.consensus {
            consensus
                .lock()
                .await
                .update_application_state(operation_id, application)?;
        }
        Ok(status)
    }

    async fn mark_operation_apply_failure(
        &self,
        operation_id: FederationOperationId,
        error: cdk::Error,
    ) -> Result<(FederationOperationStatus, cdk::Error), cdk::Error> {
        if !error.is_definitive_failure() {
            let status = self
                .persist_operation_apply_failure(operation_id, None)
                .await?;
            return Ok((status, error));
        }

        let rejection = cdk::error::ErrorResponse::from(error);
        let status = self
            .persist_operation_apply_failure(operation_id, Some(&rejection))
            .await?;

        Ok((status, cdk::Error::from(rejection)))
    }
}
