//! Federation operation admission, ordering, persistence, application, and publication.
//!
//! This pipeline is the sole owner of deterministic operation mutation. It
//! serializes live finality and catch-up application, persists ordering evidence
//! before business effects, reconciles local wallet/FROST state only after
//! deterministic application, and publishes terminal responses last. It does
//! not select peers or perform network requests.

#[cfg(any(test, feature = "test-utils"))]
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex, OnceLock, Weak};

#[cfg(feature = "federation-bark")]
use cdk::federation::FederationAlephBftOrderedOperationGroup;
use cdk::federation::{
    FederationAlephBftError, FederationAlephBftFinalizedItem,
    FederationAlephBftFinalizedItemStatus, FederationAlephBftPendingOperationRecord,
    FederationAlephBftUnitPayload, FederationApplicationState, FederationCatchUpOperation,
    FederationConsensusItem, FederationConsensusItemAdmission, FederationConsensusRuntime,
    FederationError, FederationJournalAppendOutcome, FederationJournalEntry,
    FederationOperationEnvelope, FederationOperationId, FederationOperationStatus,
    FederationRestMessageKind, FederationSubmitOperationResponse, MemberId,
};
use cdk::mint::Mint;
use cdk_common::database::{self, DynMintDatabase};
use futures::future::BoxFuture;

use super::node_runtime::{
    finalized_unit_recovery_mismatch, runtime_unavailable, FederationNodeRuntime,
};
use super::FederationPeerSyncConfig;
use crate::{FederationOperationNotificationStats, FederationOperationWaitRegistration};

struct PendingOperationBatchRequest {
    operations: Vec<FederationAlephBftPendingOperationRecord>,
    completion: tokio::sync::oneshot::Sender<Result<(), cdk::Error>>,
}

#[derive(Default)]
struct PendingOperationGroupCommitter {
    queue: Mutex<Vec<PendingOperationBatchRequest>>,
    commit_lock: Arc<tokio::sync::Mutex<()>>,
}

type PendingCommitterRegistryEntry = (
    Weak<dyn database::MintDatabase<database::Error> + Send + Sync>,
    Arc<PendingOperationGroupCommitter>,
);

fn pending_operation_committer_for_db(db: &DynMintDatabase) -> Arc<PendingOperationGroupCommitter> {
    static REGISTRY: OnceLock<Mutex<Vec<PendingCommitterRegistryEntry>>> = OnceLock::new();
    let registry = REGISTRY.get_or_init(|| Mutex::new(Vec::new()));
    let mut entries = match registry.lock() {
        Ok(entries) => entries,
        Err(poisoned) => poisoned.into_inner(),
    };
    entries.retain(|(weak, _)| weak.strong_count() > 0);
    if let Some((_, committer)) = entries
        .iter()
        .find(|(weak, _)| weak.upgrade().is_some_and(|live| Arc::ptr_eq(&live, db)))
    {
        return Arc::clone(committer);
    }
    let committer = Arc::new(PendingOperationGroupCommitter::default());
    entries.push((Arc::downgrade(db), Arc::clone(&committer)));
    committer
}

async fn write_pending_operations_tx(
    db: &DynMintDatabase,
    operations: &[FederationAlephBftPendingOperationRecord],
) -> Result<(), cdk::Error> {
    let mut tx = db.begin_transaction().await?;
    for operation in operations {
        if let Err(err) = tx
            .add_federation_aleph_bft_pending_operation(operation)
            .await
        {
            let _ = tx.rollback().await;
            return Err(err.into());
        }
    }
    tx.commit().await?;
    Ok(())
}

impl PendingOperationGroupCommitter {
    async fn persist(
        &self,
        db: &DynMintDatabase,
        operations: Vec<FederationAlephBftPendingOperationRecord>,
    ) -> Result<(), cdk::Error> {
        let (tx, rx) = tokio::sync::oneshot::channel();
        {
            let mut queue = match self.queue.lock() {
                Ok(queue) => queue,
                Err(poisoned) => poisoned.into_inner(),
            };
            queue.push(PendingOperationBatchRequest {
                operations,
                completion: tx,
            });
        }

        let commit_guard = Arc::clone(&self.commit_lock).lock_owned().await;
        let drained = {
            let mut queue = match self.queue.lock() {
                Ok(queue) => queue,
                Err(poisoned) => poisoned.into_inner(),
            };
            std::mem::take(&mut *queue)
                .into_iter()
                .filter(|req| !req.completion.is_closed())
                .collect::<Vec<_>>()
        };

        if !drained.is_empty() {
            // Spawn the commit task holding `commit_guard` so cancelling the
            // leader caller's future never drops other batched callers'
            // oneshot senders mid-transaction.
            let db = Arc::clone(db);
            tokio::spawn(async move {
                let _commit_guard = commit_guard;
                if drained.len() == 1 {
                    if let Some(request) = drained.into_iter().next() {
                        if !request.completion.is_closed() {
                            let result =
                                write_pending_operations_tx(&db, &request.operations).await;
                            let _ = request.completion.send(result);
                        }
                    }
                } else {
                    let combined_len = drained.iter().map(|req| req.operations.len()).sum();
                    let mut combined = Vec::with_capacity(combined_len);
                    for request in &drained {
                        combined.extend(request.operations.iter().cloned());
                    }
                    match write_pending_operations_tx(&db, &combined).await {
                        Ok(()) => {
                            for request in drained {
                                let _ = request.completion.send(Ok(()));
                            }
                        }
                        Err(_) => {
                            // Isolate failures when a coalesced batch hits an error
                            // (e.g. one caller had a stale session) so unrelated
                            // callers still commit cleanly.
                            for request in drained {
                                if !request.completion.is_closed() {
                                    let result =
                                        write_pending_operations_tx(&db, &request.operations).await;
                                    let _ = request.completion.send(result);
                                }
                            }
                        }
                    }
                }
            });
        } else {
            drop(commit_guard);
        }

        rx.await.map_err(|_| {
            cdk::Error::Custom("pending operation group commit channel closed".to_string())
        })?
    }
}

#[cfg(any(test, feature = "federation-bdk"))]
pub(crate) fn operation_kind_requires_bdk_reconciliation(
    kind: cdk::federation::FederationOperationKind,
) -> bool {
    matches!(
        kind,
        cdk::federation::FederationOperationKind::AddressAllocation
            | cdk::federation::FederationOperationKind::ChainObservation
            | cdk::federation::FederationOperationKind::TransactionProposal
            | cdk::federation::FederationOperationKind::TransactionSigned
            | cdk::federation::FederationOperationKind::TransactionBroadcastIntent
            | cdk::federation::FederationOperationKind::TransactionBroadcastObservation
    )
}

#[cfg(any(test, feature = "federation-bdk"))]
pub(crate) fn entries_require_bdk_reconciliation(entries: &[FederationJournalEntry]) -> bool {
    entries
        .iter()
        .any(|entry| operation_kind_requires_bdk_reconciliation(entry.envelope.operation.kind()))
}

#[cfg(any(test, feature = "federation-bark"))]
pub(crate) fn operation_kind_requires_bark_reconciliation(
    kind: cdk::federation::FederationOperationKind,
) -> bool {
    matches!(
        kind,
        cdk::federation::FederationOperationKind::Application
            | cdk::federation::FederationOperationKind::MintQuotePayment
            | cdk::federation::FederationOperationKind::MeltQuotePayment
    )
}

#[cfg(any(test, feature = "federation-bark"))]
pub(crate) fn entries_require_bark_reconciliation(entries: &[FederationJournalEntry]) -> bool {
    entries
        .iter()
        .any(|entry| operation_kind_requires_bark_reconciliation(entry.envelope.operation.kind()))
}

#[cfg(feature = "federation-bark")]
fn validate_ordered_group_responses(
    responses: &[FederationSubmitOperationResponse],
) -> Result<(), cdk::Error> {
    let Some(first) = responses.first() else {
        return Err(
            FederationError::AlephBft(FederationAlephBftError::OrderedGroupInvalid {
                reason: "ordered group produced no responses",
            })
            .into(),
        );
    };
    let first_evidence = first.outcome.entry().consensus_evidence.ok_or({
        FederationError::AlephBft(FederationAlephBftError::OrderedGroupInvalid {
            reason: "ordered group response is missing consensus evidence",
        })
    })?;
    for (offset, response) in responses.iter().enumerate() {
        let evidence = response.outcome.entry().consensus_evidence.ok_or({
            FederationError::AlephBft(FederationAlephBftError::OrderedGroupInvalid {
                reason: "ordered group response is missing consensus evidence",
            })
        })?;
        let expected_item_index = first_evidence
            .item_index
            .saturating_add(u64::try_from(offset).expect("group offset fits in u64"));
        if evidence.session != first_evidence.session
            || evidence.unit_finalization_index != first_evidence.unit_finalization_index
            || evidence.unit_submitting_member != first_evidence.unit_submitting_member
            || evidence.unit_index != first_evidence.unit_index
            || evidence.item_index != expected_item_index
        {
            return Err(
                FederationError::AlephBft(FederationAlephBftError::OrderedGroupInvalid {
                    reason: "ordered group did not finalize consecutively in one unit",
                })
                .into(),
            );
        }
    }
    Ok(())
}

impl FederationNodeRuntime {
    pub async fn validate_envelope_admission(
        &self,
        envelope: &cdk::federation::FederationOperationEnvelope,
    ) -> Result<(), cdk::Error> {
        self.validate_envelope_static_admission(envelope).await?;
        let Some(mint) = &self.mint() else {
            return Ok(());
        };
        if let cdk::federation::FederationOperation::TransactionSigned(signed) = &envelope.operation
        {
            mint.validate_federation_signed_transaction_submission(signed)
                .await?;
        }
        match &envelope.operation {
            #[cfg(feature = "federation-bark")]
            cdk::federation::FederationOperation::Application(_) => {
                let operation = super::bark_operation(&envelope.operation)?;
                mint.validate_federation_bark_operation_submission(&operation, None)
                    .await?;
            }
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

    pub async fn validate_envelope_static_admission(
        &self,
        envelope: &cdk::federation::FederationOperationEnvelope,
    ) -> Result<(), cdk::Error> {
        let Some(mint) = &self.mint() else {
            return Ok(());
        };
        let Some(config) = mint.effective_federation_config().await? else {
            return Ok(());
        };
        envelope.verify_authorization(config.public_config())?;
        config.validate_federated_operation_submission(&envelope.operation)?;
        Ok(())
    }

    /// Stamp a locally authored envelope with the active session before it
    /// enters the mempool, so it gets the full validity window.
    fn stamp_for_session(
        &self,
        envelope: FederationOperationEnvelope,
        session: u64,
    ) -> Result<FederationOperationEnvelope, cdk::Error> {
        match self.mint().and_then(|mint| mint.federation_config()) {
            Some(config) => config.stamp_operation_envelope(envelope, session),
            None => Ok(envelope),
        }
    }

    pub fn active_mint(&self) -> Result<&Arc<Mint>, cdk::Error> {
        self.mint().ok_or_else(|| runtime_unavailable("mint"))
    }

    #[cfg(feature = "prometheus")]
    pub fn metric_member_label(&self) -> Option<String> {
        self.mint()
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

    pub async fn wait_for_operation_result(
        &self,
        operation_id: FederationOperationId,
    ) -> Result<Option<FederationSubmitOperationResponse>, cdk::Error> {
        self.wait_for_operation_result_with_limit(
            operation_id,
            self.peer_sync_config()
                .unwrap_or_default()
                .response_waiter_limit,
        )
        .await
    }

    pub async fn wait_for_operation_result_with_limit(
        &self,
        operation_id: FederationOperationId,
        waiter_limit: usize,
    ) -> Result<Option<FederationSubmitOperationResponse>, cdk::Error> {
        let Some(notifications) = self.notifications() else {
            return Ok(None);
        };
        Ok(Some(
            notifications.wait_for(operation_id, waiter_limit).await?,
        ))
    }

    #[cfg(any(test, feature = "test-utils"))]
    pub fn pending_response_waiter_count(&self) -> usize {
        self.notifications()
            .map(|notifications| notifications.pending_waiter_count())
            .unwrap_or_default()
    }

    #[cfg(any(test, feature = "test-utils"))]
    pub async fn wait_for_pending_response_waiter_count(&self, expected: usize) {
        match self.notifications() {
            Some(notifications) => notifications.wait_for_pending_waiter_count(expected).await,
            None => assert_eq!(
                expected, 0,
                "inert runtime cannot register response waiters"
            ),
        }
    }

    #[cfg(any(test, feature = "test-utils"))]
    pub fn response_cache_entry_count(&self) -> usize {
        self.notifications()
            .map(|notifications| notifications.response_cache_count())
            .unwrap_or_default()
    }

    pub fn operation_notification_stats(&self) -> FederationOperationNotificationStats {
        self.notifications()
            .map(|notifications| notifications.stats())
            .unwrap_or_default()
    }

    pub fn notify_operation(&self, response: FederationSubmitOperationResponse) {
        let Some(notifications) = &self.notifications() else {
            return;
        };
        super::aleph_bft_latency::record(
            super::aleph_bft_latency::CdkAlephBftLatencyStage::OperationNotified,
            [response.operation_id],
        );
        let response_cache_limit = self
            .peer_sync_config()
            .unwrap_or_default()
            .response_cache_limit;
        notifications.notify(response, response_cache_limit);
    }

    pub async fn responses_with_current_status(
        &self,
        responses: &[FederationSubmitOperationResponse],
    ) -> Vec<FederationSubmitOperationResponse> {
        let Some(consensus) = &self.consensus() else {
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
        #[cfg(any(test, feature = "test-utils"))]
        if self
            .components
            .faults
            .suppress_next_terminal_response_publication
            .swap(false, Ordering::SeqCst)
        {
            return Ok(responses);
        }
        for response in &responses {
            #[cfg(feature = "federation-bdk")]
            self.transaction_broadcaster
                .notify_accepted_operation(response.outcome.entry());
            self.payment_observer
                .notify_accepted_operation(response.outcome.entry());
            self.notify_operation(response.clone());
        }
        Ok(responses)
    }

    pub fn duplicate_finalized_unit_responses(
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
                        cdk::Error::from(FederationAlephBftError::RecoveryOperationMismatch {
                            reason: "finalized operation is missing from the local journal",
                            operation_id,
                        })
                    })?;
                responses.push(FederationSubmitOperationResponse::from_outcome(
                    FederationJournalAppendOutcome::Duplicate(entry.clone()),
                ));
            }
        }

        Ok(Some(responses))
    }

    pub fn submit_envelope(
        &self,
        envelope: FederationOperationEnvelope,
    ) -> BoxFuture<'_, Result<FederationSubmitOperationResponse, cdk::Error>> {
        Box::pin(self.submit_envelope_timed(envelope))
    }

    async fn submit_envelope_timed(
        &self,
        envelope: FederationOperationEnvelope,
    ) -> Result<FederationSubmitOperationResponse, cdk::Error> {
        let operation_id = envelope.operation_id()?;
        let operation_kind = envelope.operation.kind().to_string();
        tracing::trace!(
            target: "cdk_axum::federation::transaction_latency",
            event = "classification",
            correlation = "operation",
            correlation_id = %operation_id,
            operation_kind = %operation_kind,
            "Federated consensus operation classified"
        );
        let timer = super::latency::FederationLatencyTimer::start(
            super::latency::FederationLatencyCorrelation::Operation,
            operation_id.to_string(),
            super::latency::FederationLatencyBackend::Consensus,
            super::latency::FederationLatencyDirection::Internal,
            super::latency::FederationLatencyPhase::ConsensusSubmission,
        );
        let result = self.submit_envelope_inner(envelope, operation_id).await;
        let outcome = match &result {
            Ok(response)
                if response.outcome.entry().status() == FederationOperationStatus::Rejected =>
            {
                super::latency::FederationLatencyOutcome::Error
            }
            Ok(_) => super::latency::FederationLatencyOutcome::Success,
            Err(_) => super::latency::FederationLatencyOutcome::Error,
        };
        timer.finish(outcome);
        result
    }

    #[cfg(feature = "federation-bark")]
    pub async fn submit_envelope_group(
        &self,
        envelopes: Vec<FederationOperationEnvelope>,
    ) -> Result<Vec<FederationSubmitOperationResponse>, cdk::Error> {
        if envelopes.is_empty() {
            return Err(
                FederationError::AlephBft(FederationAlephBftError::OrderedGroupInvalid {
                    reason: "ordered group cannot be empty",
                })
                .into(),
            );
        }
        let operation_ids = envelopes
            .iter()
            .map(FederationOperationEnvelope::operation_id)
            .collect::<Result<Vec<_>, _>>()?;
        let member = self.configured_member()?;
        let sync_config = member.peer_sync_config;
        let notifications = &member.notifications;

        let mut persisted_pending = false;
        let mut wait_registrations: Option<Vec<FederationOperationWaitRegistration<'_>>> = None;

        let (wait_registrations, accepted_admission) = loop {
            let (stamped_envelopes, session_id) = {
                let consensus = member.consensus.lock().await;
                let existing = operation_ids
                    .iter()
                    .map(|operation_id| {
                        consensus
                            .journal()
                            .entry_by_operation_id(*operation_id)
                            .cloned()
                    })
                    .collect::<Vec<_>>();
                if existing.iter().any(Option::is_some) {
                    if !existing.iter().all(Option::is_some) {
                        drop(consensus);
                        if persisted_pending {
                            self.cleanup_uncommitted_pending_operations(&operation_ids)
                                .await;
                        }
                        return Err(FederationError::AlephBft(
                            FederationAlephBftError::OrderedGroupInvalid {
                                reason: "ordered group is only partially present in the journal",
                            },
                        )
                        .into());
                    }
                    let waits = match wait_registrations.take() {
                        Some(existing_waits) => existing_waits
                            .into_iter()
                            .zip(&existing)
                            .map(|(wait, entry)| {
                                let entry = entry
                                    .as_ref()
                                    .expect("all ordered group entries were checked as present");
                                if matches!(entry.status(), FederationOperationStatus::Accepted) {
                                    Some(wait)
                                } else {
                                    None
                                }
                            })
                            .collect::<Vec<_>>(),
                        None => {
                            let mut waits = Vec::with_capacity(existing.len());
                            for (operation_id, entry) in
                                operation_ids.iter().copied().zip(&existing)
                            {
                                let entry = entry
                                    .as_ref()
                                    .expect("all ordered group entries were checked as present");
                                if matches!(entry.status(), FederationOperationStatus::Accepted) {
                                    match notifications.register_waiter(
                                        operation_id,
                                        sync_config.response_waiter_limit,
                                    ) {
                                        Ok(wait) => waits.push(Some(wait)),
                                        Err(err) => {
                                            drop(consensus);
                                            if persisted_pending {
                                                self.cleanup_uncommitted_pending_operations(
                                                    &operation_ids,
                                                )
                                                .await;
                                            }
                                            return Err(err);
                                        }
                                    }
                                } else {
                                    waits.push(None);
                                }
                            }
                            waits
                        }
                    };
                    drop(consensus);
                    if persisted_pending {
                        self.cleanup_uncommitted_pending_operations(&operation_ids)
                            .await;
                    }
                    let mut responses = Vec::with_capacity(operation_ids.len());
                    for ((operation_id, wait), entry) in
                        operation_ids.iter().copied().zip(waits).zip(existing)
                    {
                        let entry =
                            entry.expect("all ordered group entries were checked as present");
                        let response = match wait {
                            Some(wait) => {
                                Self::wait_for_operation_acceptance(operation_id, sync_config, wait)
                                    .await?
                            }
                            None => FederationSubmitOperationResponse::from_outcome(
                                FederationJournalAppendOutcome::Duplicate(entry),
                            ),
                        };
                        responses.push(response);
                    }
                    validate_ordered_group_responses(&responses)?;
                    return Ok(responses);
                }

                let session_id = match consensus.aleph_bft_status() {
                    Ok(status) => status.session,
                    Err(err) => {
                        drop(consensus);
                        if persisted_pending {
                            self.cleanup_uncommitted_pending_operations(&operation_ids)
                                .await;
                        }
                        return Err(err.into());
                    }
                };
                let stamped_envelopes = match envelopes
                    .iter()
                    .cloned()
                    .map(|envelope| self.stamp_for_session(envelope, session_id))
                    .collect::<Result<Vec<_>, _>>()
                {
                    Ok(stamped) => stamped,
                    Err(err) => {
                        drop(consensus);
                        if persisted_pending {
                            self.cleanup_uncommitted_pending_operations(&operation_ids)
                                .await;
                        }
                        return Err(err);
                    }
                };
                let queued_before = consensus.pending_mempool_len();
                let prepared =
                    match consensus.prepare_aleph_bft_envelope_group(stamped_envelopes.clone()) {
                        Ok(prepared) => prepared,
                        Err(err) => {
                            drop(consensus);
                            if persisted_pending {
                                self.cleanup_uncommitted_pending_operations(&operation_ids)
                                    .await;
                            }
                            return Err(err.into());
                        }
                    };
                let admission = prepared.admission();
                if admission == FederationConsensusItemAdmission::DuplicateOperation {
                    let waits = match wait_registrations.take() {
                        Some(waits) => waits,
                        None => operation_ids
                            .iter()
                            .map(|operation_id| {
                                notifications.register_waiter(
                                    *operation_id,
                                    sync_config.response_waiter_limit,
                                )
                            })
                            .collect::<Result<Vec<_>, _>>()?,
                    };
                    drop(consensus);
                    break (waits, false);
                }
                if queued_before.saturating_add(stamped_envelopes.len())
                    > sync_config.pending_mempool_item_limit
                {
                    drop(consensus);
                    if persisted_pending {
                        self.cleanup_uncommitted_pending_operations(&operation_ids)
                            .await;
                    }
                    return Err(FederationError::PendingConsensusMempoolFull {
                        pending: queued_before,
                        max: sync_config.pending_mempool_item_limit,
                    }
                    .into());
                }
                // Register response waiters before persisting pending rows so
                // waiter-limit rejections never leave orphan DB rows.
                if wait_registrations.is_none() {
                    match operation_ids
                        .iter()
                        .map(|operation_id| {
                            notifications
                                .register_waiter(*operation_id, sync_config.response_waiter_limit)
                        })
                        .collect::<Result<Vec<_>, _>>()
                    {
                        Ok(waits) => wait_registrations = Some(waits),
                        Err(err) => {
                            drop(consensus);
                            if persisted_pending {
                                self.cleanup_uncommitted_pending_operations(&operation_ids)
                                    .await;
                            }
                            return Err(err);
                        }
                    }
                }
                (stamped_envelopes, session_id)
            };

            let ordered_group = FederationAlephBftOrderedOperationGroup {
                operation_ids: operation_ids.clone(),
            };
            let pending = operation_ids
                .iter()
                .copied()
                .zip(stamped_envelopes.iter().cloned())
                .map(
                    |(operation_id, envelope)| FederationAlephBftPendingOperationRecord {
                        operation_id,
                        session_id,
                        envelope,
                        ordered_group: Some(ordered_group.clone()),
                    },
                )
                .collect::<Vec<_>>();
            if let Err(err) = self.persist_pending_operations(&pending).await {
                let current_session = member
                    .consensus
                    .lock()
                    .await
                    .aleph_bft_status()
                    .map(|status| status.session)
                    .ok();
                if current_session.is_some_and(|current| current > session_id) {
                    continue;
                }
                if persisted_pending {
                    self.cleanup_uncommitted_pending_operations(&operation_ids)
                        .await;
                }
                return Err(err);
            }
            persisted_pending = true;

            let mut consensus = member.consensus.lock().await;
            let existing = operation_ids
                .iter()
                .map(|operation_id| {
                    consensus
                        .journal()
                        .entry_by_operation_id(*operation_id)
                        .cloned()
                })
                .collect::<Vec<_>>();
            if existing.iter().any(Option::is_some) {
                drop(consensus);
                self.cleanup_uncommitted_pending_operations(&operation_ids)
                    .await;
                if !existing.iter().all(Option::is_some) {
                    return Err(FederationError::AlephBft(
                        FederationAlephBftError::OrderedGroupInvalid {
                            reason: "ordered group is only partially present in the journal",
                        },
                    )
                    .into());
                }
                let waits = wait_registrations
                    .take()
                    .expect("ordered group waiters were registered before persistence");
                let mut responses = Vec::with_capacity(operation_ids.len());
                for ((operation_id, wait), entry) in
                    operation_ids.iter().copied().zip(waits).zip(existing)
                {
                    let entry = entry.expect("all ordered group entries were checked as present");
                    let response = if matches!(entry.status(), FederationOperationStatus::Accepted)
                    {
                        Self::wait_for_operation_acceptance(operation_id, sync_config, wait).await?
                    } else {
                        FederationSubmitOperationResponse::from_outcome(
                            FederationJournalAppendOutcome::Duplicate(entry),
                        )
                    };
                    responses.push(response);
                }
                validate_ordered_group_responses(&responses)?;
                return Ok(responses);
            }
            match consensus.aleph_bft_status() {
                Ok(status) if status.session != session_id => continue,
                Ok(_) => {}
                Err(err) => {
                    drop(consensus);
                    self.cleanup_uncommitted_pending_operations(&operation_ids)
                        .await;
                    return Err(err.into());
                }
            }
            let queued_before = consensus.pending_mempool_len();
            let prepared = match consensus.prepare_aleph_bft_envelope_group(stamped_envelopes) {
                Ok(prepared) => prepared,
                Err(err) => {
                    drop(consensus);
                    self.cleanup_uncommitted_pending_operations(&operation_ids)
                        .await;
                    return Err(err.into());
                }
            };
            let admission = prepared.admission();
            if admission == FederationConsensusItemAdmission::DuplicateOperation {
                let waits = wait_registrations
                    .take()
                    .expect("ordered group waiters were registered before persistence");
                drop(consensus);
                break (waits, false);
            }
            if queued_before.saturating_add(operation_ids.len())
                > sync_config.pending_mempool_item_limit
            {
                drop(consensus);
                self.cleanup_uncommitted_pending_operations(&operation_ids)
                    .await;
                return Err(FederationError::PendingConsensusMempoolFull {
                    pending: queued_before,
                    max: sync_config.pending_mempool_item_limit,
                }
                .into());
            }
            let waits = wait_registrations
                .take()
                .expect("ordered group waiters were registered before persistence");
            consensus.commit_prepared_aleph_bft_envelope_group(prepared);
            drop(consensus);
            break (waits, true);
        };
        for operation_id in &operation_ids {
            self.mark_consensus_activity(*operation_id, true).await;
            if accepted_admission {
                super::aleph_bft_latency::record(
                    super::aleph_bft_latency::CdkAlephBftLatencyStage::SubmitQueued,
                    [*operation_id],
                );
            }
        }
        #[cfg(any(test, feature = "test-utils"))]
        self.finalize_unit_locally_for_test().await?;
        let mut responses = Vec::with_capacity(operation_ids.len());
        for (operation_id, wait_registration) in operation_ids.into_iter().zip(wait_registrations) {
            responses.push(
                Self::wait_for_operation_acceptance(operation_id, sync_config, wait_registration)
                    .await?,
            );
        }
        validate_ordered_group_responses(&responses)?;
        Ok(responses)
    }

    async fn submit_envelope_inner(
        &self,
        envelope: FederationOperationEnvelope,
        operation_id: FederationOperationId,
    ) -> Result<FederationSubmitOperationResponse, cdk::Error> {
        #[cfg(feature = "prometheus")]
        let operation_label = envelope.operation.kind().to_string();
        let member = self.configured_member()?;
        let sync_config = member.peer_sync_config;
        let notifications = &member.notifications;

        let mut persisted_pending = false;
        let mut wait_registration: Option<FederationOperationWaitRegistration<'_>> = None;

        let (wait_registration, accepted_admission) = loop {
            let (stamped_envelope, session_id) = {
                let consensus = member.consensus.lock().await;
                if let Some(entry) = consensus.journal().entry_by_operation_id(operation_id) {
                    if matches!(entry.status(), FederationOperationStatus::Accepted) {
                        notifications.record_duplicate_pending_waiter_join();
                        let wait = match wait_registration.take() {
                            Some(wait) => wait,
                            None => match notifications
                                .register_waiter(operation_id, sync_config.response_waiter_limit)
                            {
                                Ok(wait) => wait,
                                Err(err) => {
                                    drop(consensus);
                                    if persisted_pending {
                                        self.cleanup_uncommitted_pending_operations(&[
                                            operation_id,
                                        ])
                                        .await;
                                    }
                                    return Err(err);
                                }
                            },
                        };
                        drop(consensus);
                        if persisted_pending {
                            self.cleanup_uncommitted_pending_operations(&[operation_id])
                                .await;
                        }
                        self.mark_consensus_activity(operation_id, true).await;
                        let response =
                            Self::wait_for_operation_acceptance(operation_id, sync_config, wait)
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
                    if persisted_pending {
                        self.cleanup_uncommitted_pending_operations(&[operation_id])
                            .await;
                    }
                    return Ok(response);
                }

                let session_id = match consensus.aleph_bft_status() {
                    Ok(status) => status.session,
                    Err(err) => {
                        drop(consensus);
                        if persisted_pending {
                            self.cleanup_uncommitted_pending_operations(&[operation_id])
                                .await;
                        }
                        return Err(err.into());
                    }
                };
                let stamped_envelope = match self.stamp_for_session(envelope.clone(), session_id) {
                    Ok(stamped) => stamped,
                    Err(err) => {
                        drop(consensus);
                        if persisted_pending {
                            self.cleanup_uncommitted_pending_operations(&[operation_id])
                                .await;
                        }
                        return Err(err);
                    }
                };
                let queued_before = consensus.pending_mempool_len();
                let prepared = match consensus.prepare_aleph_bft_envelope(stamped_envelope.clone())
                {
                    Ok(prepared) => prepared,
                    Err(err) => {
                        drop(consensus);
                        if persisted_pending {
                            self.cleanup_uncommitted_pending_operations(&[operation_id])
                                .await;
                        }
                        #[cfg(feature = "prometheus")]
                        self.record_rejected_operation_metric(&operation_label, "consensus");
                        return Err(cdk::Error::from(err));
                    }
                };
                if matches!(
                    prepared.admission(),
                    FederationConsensusItemAdmission::DuplicateOperation
                ) {
                    notifications.record_duplicate_pending_waiter_join();
                    tracing::debug!(
                        operation_id = %operation_id,
                        "Duplicate AlephBFT federation operation is already pending"
                    );
                    let wait = match wait_registration.take() {
                        Some(wait) => wait,
                        None => notifications
                            .register_waiter(operation_id, sync_config.response_waiter_limit)?,
                    };
                    drop(consensus);
                    break (wait, false);
                }
                let limit = sync_config.pending_mempool_item_limit;
                if queued_before >= limit {
                    drop(consensus);
                    if persisted_pending {
                        self.cleanup_uncommitted_pending_operations(&[operation_id])
                            .await;
                    }
                    let err = FederationError::PendingConsensusMempoolFull {
                        pending: queued_before,
                        max: limit,
                    };
                    #[cfg(feature = "prometheus")]
                    self.record_rejected_operation_metric(&operation_label, "mempool_full");
                    return Err(cdk::Error::from(err));
                }
                // Register response waiter before persisting the pending row so
                // waiter-limit rejections never write an orphan pending row.
                if wait_registration.is_none() {
                    match notifications
                        .register_waiter(operation_id, sync_config.response_waiter_limit)
                    {
                        Ok(wait) => wait_registration = Some(wait),
                        Err(err) => {
                            drop(consensus);
                            if persisted_pending {
                                self.cleanup_uncommitted_pending_operations(&[operation_id])
                                    .await;
                            }
                            return Err(err);
                        }
                    }
                }
                (stamped_envelope, session_id)
            };

            if let Err(err) = self
                .persist_pending_operation(FederationAlephBftPendingOperationRecord {
                    operation_id,
                    session_id,
                    envelope: stamped_envelope.clone(),
                    ordered_group: None,
                })
                .await
            {
                let current_session = member
                    .consensus
                    .lock()
                    .await
                    .aleph_bft_status()
                    .map(|status| status.session)
                    .ok();
                if current_session.is_some_and(|current| current > session_id) {
                    continue;
                }
                if persisted_pending {
                    self.cleanup_uncommitted_pending_operations(&[operation_id])
                        .await;
                }
                return Err(err);
            }
            persisted_pending = true;

            let mut consensus = member.consensus.lock().await;
            if let Some(entry) = consensus.journal().entry_by_operation_id(operation_id) {
                let is_accepted = matches!(entry.status(), FederationOperationStatus::Accepted);
                let cloned_entry = (!is_accepted).then(|| entry.clone());
                if is_accepted {
                    notifications.record_duplicate_pending_waiter_join();
                } else {
                    notifications.record_duplicate_submission();
                }
                drop(consensus);
                self.cleanup_uncommitted_pending_operations(&[operation_id])
                    .await;
                if is_accepted {
                    let wait = wait_registration
                        .take()
                        .expect("operation waiter was registered before persistence");
                    self.mark_consensus_activity(operation_id, true).await;
                    let response =
                        Self::wait_for_operation_acceptance(operation_id, sync_config, wait)
                            .await?;
                    #[cfg(feature = "prometheus")]
                    self.record_consensus_rejection_metric(&operation_label, &response);
                    return Ok(response);
                }

                let response = FederationSubmitOperationResponse::from_outcome(
                    FederationJournalAppendOutcome::Duplicate(
                        cloned_entry.expect("entry cloned for terminal duplicate"),
                    ),
                );
                return Ok(response);
            }
            match consensus.aleph_bft_status() {
                Ok(status) if status.session != session_id => continue,
                Ok(_) => {}
                Err(err) => {
                    drop(consensus);
                    self.cleanup_uncommitted_pending_operations(&[operation_id])
                        .await;
                    return Err(err.into());
                }
            }
            let queued_before = consensus.pending_mempool_len();
            let prepared = match consensus.prepare_aleph_bft_envelope(stamped_envelope) {
                Ok(prepared) => prepared,
                Err(err) => {
                    drop(consensus);
                    self.cleanup_uncommitted_pending_operations(&[operation_id])
                        .await;
                    #[cfg(feature = "prometheus")]
                    self.record_rejected_operation_metric(&operation_label, "consensus");
                    return Err(cdk::Error::from(err));
                }
            };
            if matches!(
                prepared.admission(),
                FederationConsensusItemAdmission::DuplicateOperation
            ) {
                notifications.record_duplicate_pending_waiter_join();
                tracing::debug!(
                    operation_id = %operation_id,
                    "Duplicate AlephBFT federation operation is already pending"
                );
                let wait = wait_registration
                    .take()
                    .expect("operation waiter was registered before persistence");
                drop(consensus);
                break (wait, false);
            }
            let limit = sync_config.pending_mempool_item_limit;
            if queued_before >= limit {
                drop(consensus);
                self.cleanup_uncommitted_pending_operations(&[operation_id])
                    .await;
                let err = FederationError::PendingConsensusMempoolFull {
                    pending: queued_before,
                    max: limit,
                };
                #[cfg(feature = "prometheus")]
                self.record_rejected_operation_metric(&operation_label, "mempool_full");
                return Err(cdk::Error::from(err));
            }
            let wait = wait_registration
                .take()
                .expect("operation waiter was registered before persistence");
            consensus.commit_prepared_aleph_bft_envelope(prepared);
            drop(consensus);
            break (wait, true);
        };
        self.mark_consensus_activity(operation_id, true).await;
        if accepted_admission {
            #[cfg(feature = "prometheus")]
            self.record_accepted_operation_metric(&operation_label);
            super::aleph_bft_latency::record(
                super::aleph_bft_latency::CdkAlephBftLatencyStage::SubmitQueued,
                [operation_id],
            );
        }
        #[cfg(any(test, feature = "test-utils"))]
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

    #[cfg(any(test, feature = "test-utils"))]
    pub async fn finalize_unit_locally_for_test(&self) -> Result<(), cdk::Error> {
        if self.finality != crate::FederationFinalityMode::Local {
            return Ok(());
        }

        let unit = {
            let consensus = self
                .consensus()
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

    pub async fn persist_pending_operation(
        &self,
        operation: FederationAlephBftPendingOperationRecord,
    ) -> Result<(), cdk::Error> {
        self.persist_pending_operations(&[operation]).await
    }

    async fn cleanup_uncommitted_pending_operations(
        &self,
        operation_ids: &[FederationOperationId],
    ) {
        if operation_ids.is_empty() {
            return;
        }
        let Ok(mint) = self.active_mint() else {
            return;
        };
        let db = mint.localstore();
        let Ok(mut tx) = db.begin_transaction().await else {
            return;
        };
        if tx
            .remove_federation_aleph_bft_pending_operations(operation_ids)
            .await
            .is_ok()
        {
            let _ = tx.commit().await;
        } else {
            let _ = tx.rollback().await;
        }
    }

    pub async fn persist_pending_operations(
        &self,
        operations: &[FederationAlephBftPendingOperationRecord],
    ) -> Result<(), cdk::Error> {
        #[cfg(any(test, feature = "test-utils"))]
        if self
            .components
            .faults
            .fail_next_pending_operation_persistence
            .swap(false, Ordering::SeqCst)
        {
            return Err(cdk::Error::Custom(
                "injected pending-operation persistence failure".to_string(),
            ));
        }
        if operations.is_empty() {
            return Ok(());
        }
        let db = self.active_mint()?.localstore();
        let committer = pending_operation_committer_for_db(&db);
        committer.persist(&db, operations.to_vec()).await
    }

    pub async fn persist_finalization(
        &self,
        finalized_items: &[FederationAlephBftFinalizedItem],
        entries: &[FederationJournalEntry],
    ) -> Result<(), cdk::Error> {
        #[cfg(any(test, feature = "test-utils"))]
        if self
            .components
            .faults
            .fail_next_finalization_persistence
            .swap(false, Ordering::SeqCst)
        {
            return Err(cdk::Error::Custom(
                "injected finalization persistence failure".to_string(),
            ));
        }
        let db = self.active_mint()?.localstore();
        let mut tx = db.begin_transaction().await?;
        tx.add_federation_aleph_bft_finalized_items(finalized_items)
            .await?;
        let pending_operation_ids = finalized_items
            .iter()
            .filter_map(|item| item.operation_id)
            .collect::<Vec<_>>();
        tx.remove_federation_aleph_bft_pending_operations(&pending_operation_ids)
            .await?;
        tx.add_federation_journal_entries(entries).await?;
        tx.commit().await?;
        Ok(())
    }

    pub async fn persist_catch_up(
        &self,
        active_session: u64,
        finalized_items: &[FederationAlephBftFinalizedItem],
        entries: &[FederationJournalEntry],
    ) -> Result<(), cdk::Error> {
        let db = self.active_mint()?.localstore();
        let mut tx = db.begin_transaction().await?;
        tx.set_federation_aleph_bft_active_session(active_session)
            .await?;
        tx.add_federation_aleph_bft_finalized_items(finalized_items)
            .await?;
        let pending_operation_ids = finalized_items
            .iter()
            .filter_map(|item| item.operation_id)
            .collect::<Vec<_>>();
        tx.remove_federation_aleph_bft_pending_operations(&pending_operation_ids)
            .await?;
        tx.add_federation_journal_entries(entries).await?;
        tx.commit().await?;
        Ok(())
    }

    pub async fn record_sequence(
        &self,
        kind: FederationRestMessageKind,
        sender: MemberId,
        sequence: u64,
    ) {
        if let Some(sequences) = &self.sequences() {
            sequences
                .lock()
                .await
                .record_sequence(kind, sender, sequence);
        }
    }

    pub fn apply_finalized_unit(
        &self,
        payload: FederationAlephBftUnitPayload,
    ) -> BoxFuture<'_, Result<Vec<FederationSubmitOperationResponse>, cdk::Error>> {
        Box::pin(self.apply_finalized_unit_inner(payload))
    }

    async fn apply_finalized_unit_inner(
        &self,
        payload: FederationAlephBftUnitPayload,
    ) -> Result<Vec<FederationSubmitOperationResponse>, cdk::Error> {
        #[cfg(any(test, feature = "test-utils"))]
        if self
            .components
            .faults
            .permanent_federation_apply_contention
            .load(Ordering::Acquire)
        {
            return Err(cdk::Error::ConcurrentUpdate);
        }
        let _application = self.components.operation.lock().await;
        let db = self.active_mint()?.localstore();
        // Duplicate and conflict decisions read only entries ordered inside
        // the unit's validity window, so every member decides alike whatever
        // older history it still holds.
        let mut precheck_operation_ids = Vec::new();
        let mut precheck_conflict_keys = Vec::new();
        for item in &payload.items {
            let FederationConsensusItem::Operation(envelope) = item else {
                continue;
            };
            precheck_operation_ids.push(envelope.operation_id()?);
            precheck_conflict_keys.extend(envelope.operation.conflict_keys());
        }
        let durable_entries = db
            .get_federation_journal_entries_by_keys(
                &precheck_operation_ids,
                &precheck_conflict_keys,
                None,
            )
            .await?
            .into_iter()
            .filter(|entry| entry.guards_session(payload.session))
            .collect::<Vec<_>>();
        let consensus = self
            .consensus()
            .ok_or_else(|| runtime_unavailable("consensus"))?;
        let mut consensus = consensus.lock().await;
        for entry in durable_entries {
            if consensus
                .journal()
                .entry_by_operation_id(entry.operation_id)
                .is_none_or(|known| known.index < entry.index)
            {
                consensus.register_historical_journal_entry(entry)?;
            }
        }
        if let Some(responses) = Self::duplicate_finalized_unit_responses(&consensus, &payload)? {
            let finalized_items = consensus
                .aleph_bft_finalized_items()?
                .iter()
                .filter(|item| {
                    item.session == payload.session
                        && item.unit_submitting_member == payload.submitting_member
                        && item.unit_index == payload.unit_index
                        && item.unit_body_hash == payload.body_hash
                })
                .cloned()
                .collect::<Vec<_>>();
            drop(consensus);
            let entries = responses
                .iter()
                .map(|response| response.outcome.entry().clone())
                .collect::<Vec<_>>();
            let entries_to_apply = crate::accepted_federation_journal_entries(&entries);
            self.apply_journal_entries(&entries_to_apply).await?;
            self.capture_scheduled_checkpoint_if_eligible_under_application_lock()
                .await?;
            self.capture_recovery_checkpoint_for_finalized_items_under_application_lock(
                &finalized_items,
            )
            .await?;
            return self.publish_terminal_operation_responses(&responses).await;
        }

        let prepared = consensus.prepare_aleph_bft_finalization(payload)?;
        let finalized_items = prepared.finalized_items().to_vec();
        let entries = prepared.appended_entries().to_vec();
        let entries_to_apply = crate::accepted_federation_journal_entries(&entries);
        self.persist_finalization(&finalized_items, &entries)
            .await?;
        let responses = consensus.commit_prepared_aleph_bft_finalization(prepared);
        drop(consensus);

        self.apply_journal_entries(&entries_to_apply).await?;
        self.capture_scheduled_checkpoint_if_eligible_under_application_lock()
            .await?;
        self.capture_recovery_checkpoint_for_finalized_items_under_application_lock(
            &finalized_items,
        )
        .await?;
        self.publish_terminal_operation_responses(&responses).await
    }

    pub fn apply_journal_entries<'a>(
        &'a self,
        entries: &'a [FederationJournalEntry],
    ) -> BoxFuture<'a, Result<Vec<FederationOperationId>, cdk::Error>> {
        Box::pin(self.apply_journal_entries_timed(entries))
    }

    async fn apply_journal_entries_timed(
        &self,
        entries: &[FederationJournalEntry],
    ) -> Result<Vec<FederationOperationId>, cdk::Error> {
        #[cfg(feature = "prometheus")]
        let started_at = std::time::Instant::now();
        #[cfg(feature = "prometheus")]
        cdk_prometheus::METRICS.record_federation_performance_work(
            cdk_prometheus::FederationPerformanceWork::JournalEntriesProcessed,
            u64::try_from(entries.len()).unwrap_or(u64::MAX),
        );
        let result = self.apply_journal_entries_inner(entries).await;
        #[cfg(feature = "prometheus")]
        cdk_prometheus::METRICS.record_federation_performance_operation(
            cdk_prometheus::FederationPerformanceOperation::JournalApplication,
            cdk_prometheus::FederationPerformanceOutcome::from_success(result.is_ok()),
            started_at.elapsed().as_secs_f64(),
        );
        result
    }

    async fn apply_journal_entries_inner(
        &self,
        entries: &[FederationJournalEntry],
    ) -> Result<Vec<FederationOperationId>, cdk::Error> {
        let mint = self.active_mint()?;
        if !entries.is_empty() {
            if let Some(config) = mint.effective_federation_config().await? {
                config.preverify_journal_entries(entries).await?;
            }
        }
        let label = format!(
            "Federation journal batch apply of {} operations",
            entries.len()
        );
        let outcomes = match crate::retry_transient_storage(
            &label,
            crate::is_retryable_federation_apply_error,
            || mint.apply_federation_journal_entries_batch(entries, true),
        )
        .await
        {
            Ok(outcomes) => outcomes,
            Err(crate::TransientRetryFailure::Exhausted {
                error,
                attempts,
                elapsed,
            }) => {
                self.sync_persisted_application_states_for_entries(entries)
                    .await;
                return Err(cdk::Error::Custom(format!(
                    "federation journal batch apply retry budget exhausted after {} attempts over {:.1}s: {}",
                    attempts,
                    elapsed.as_secs_f64(),
                    error
                )));
            }
            Err(crate::TransientRetryFailure::Permanent(err)) => {
                self.sync_persisted_application_states_for_entries(entries)
                    .await;
                return Err(err);
            }
        };

        let mut applied = Vec::new();
        let mut terminal_applications = Vec::with_capacity(outcomes.len());
        for (operation_id, newly_applied, application) in outcomes {
            if newly_applied {
                applied.push(operation_id);
            }
            if !matches!(
                application.status(),
                FederationOperationStatus::Applied | FederationOperationStatus::Rejected
            ) {
                return Err(cdk::Error::Custom(format!(
                    "federation operation {operation_id} remained non-terminal after journal application"
                )));
            }
            terminal_applications.push((operation_id, application));
        }
        self.mark_operation_applications(&terminal_applications)
            .await?;
        #[cfg(feature = "federation-bdk")]
        {
            #[cfg(any(test, feature = "test-utils"))]
            let injected_failure = self
                .components
                .faults
                .fail_next_onchain_reconciliation
                .swap(false, Ordering::SeqCst);
            #[cfg(not(any(test, feature = "test-utils")))]
            let injected_failure = false;

            if injected_failure {
                self.components
                    .wallet
                    .set_bdk_degraded_reason(Some(
                        "injected on-chain reconciliation failure".to_string(),
                    ))
                    .await;
                tracing::error!(
                    reason = "injected on-chain reconciliation failure",
                    "federation local BDK reconciliation failed after deterministic application; member continues BDK-degraded"
                );
            } else {
                let should_reconcile_bdk =
                    self.components.wallet.bdk_degraded_reason().await.is_some()
                        || (!self.components.wallet.onchain_wallets.is_empty()
                            && entries_require_bdk_reconciliation(entries));
                // Local BDK projection health is separate from the consensus
                // state-machine decision: a reconciliation failure marks this
                // member BDK-degraded (blocking local signing) and is retried
                // on later applications, but it never aborts deterministic
                // application or halts the session.
                if should_reconcile_bdk {
                    if let Err(err) = self.components.wallet.reconcile(mint).await {
                        tracing::error!(
                            reason = %format!("{err:#}"),
                            "federation local BDK reconciliation failed after deterministic application; member continues BDK-degraded"
                        );
                    }
                }
            }
        }
        #[cfg(feature = "federation-bark")]
        if let Some(runtime) = self.components.wallet.bark.as_ref() {
            let should_reconcile_bark = entries_require_bark_reconciliation(entries)
                || runtime.degraded_reason().await.is_some();
            if should_reconcile_bark {
                if let Err(err) =
                    Box::pin(crate::reconcile_federation_bark_runtime(mint, runtime)).await
                {
                    tracing::error!(
                        reason = %format!("{err:#}"),
                        "federation local Bark reconciliation failed after deterministic application; member continues Bark-degraded"
                    );
                }
            }
        }
        Ok(applied)
    }

    async fn sync_persisted_application_states_for_entries(
        &self,
        entries: &[FederationJournalEntry],
    ) {
        if entries.is_empty() {
            return;
        }
        let Ok(mint) = self.active_mint() else {
            return;
        };
        let operation_ids = entries
            .iter()
            .map(|entry| entry.operation_id)
            .collect::<Vec<_>>();
        let Ok(states) = mint
            .localstore()
            .get_federation_journal_application_states(&operation_ids)
            .await
        else {
            return;
        };
        let updates = entries
            .iter()
            .filter_map(|entry| {
                states
                    .get(&entry.operation_id)
                    .map(|(_, state)| (entry.operation_id, state.clone()))
            })
            .collect::<Vec<_>>();
        if !updates.is_empty() {
            let _ = self.mark_operation_applications(&updates).await;
        }
    }

    pub async fn verify_catch_up_rejections(
        &self,
        peer_entries: &[FederationCatchUpOperation],
    ) -> Result<(), cdk::Error> {
        let rejected_peer_entries = peer_entries
            .iter()
            .filter(|entry| entry.application.status() == FederationOperationStatus::Rejected)
            .collect::<Vec<_>>();
        if rejected_peer_entries.is_empty() {
            return Ok(());
        }
        let operation_ids = rejected_peer_entries
            .iter()
            .map(|entry| entry.operation_id)
            .collect::<Vec<_>>();
        let local_states = self
            .active_mint()?
            .localstore()
            .get_federation_journal_application_states(&operation_ids)
            .await?;
        for peer_entry in rejected_peer_entries {
            let (_, local_application) =
                local_states.get(&peer_entry.operation_id).ok_or_else(|| {
                    cdk::Error::Federation(FederationError::UnknownOperation {
                        operation_id: peer_entry.operation_id.to_string(),
                    })
                })?;
            if *local_application != peer_entry.application {
                return Err(cdk::Error::from(
                    FederationAlephBftError::RecoveryOperationMismatch {
                        reason: "catch-up rejection mismatch",
                        operation_id: peer_entry.operation_id,
                    },
                ));
            }
        }

        Ok(())
    }

    async fn mark_operation_applications(
        &self,
        operations: &[(FederationOperationId, FederationApplicationState)],
    ) -> Result<(), cdk::Error> {
        let Some(consensus) = &self.consensus() else {
            return Ok(());
        };
        let mut consensus = consensus.lock().await;
        for (operation_id, application) in operations {
            consensus.update_application_state(*operation_id, application.clone())?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use cdk::federation::{
        FederationAlephBftPendingOperationRecord, FederationId, FederationOperation,
        FederationOperationEnvelope, FederationOperationKind, FederationSwapOperation,
        InMemoryFederationJournal,
    };
    use cdk_common::database::DynMintDatabase;
    use cdk_common::SwapRequest;

    use super::{
        entries_require_bark_reconciliation, entries_require_bdk_reconciliation,
        operation_kind_requires_bark_reconciliation, operation_kind_requires_bdk_reconciliation,
        pending_operation_committer_for_db,
    };

    #[test]
    fn gated_reconciliation_skips_pure_ecash_and_triggers_for_relevant_ops() {
        let pure_ecash = [
            FederationOperationKind::Swap,
            FederationOperationKind::Mint,
            FederationOperationKind::MintQuote,
            FederationOperationKind::MeltQuote,
            FederationOperationKind::Melt,
            FederationOperationKind::KeysetRotation,
        ];
        for kind in pure_ecash {
            assert!(
                !operation_kind_requires_bdk_reconciliation(kind),
                "pure e-cash op {kind} must not trigger BDK reconciliation"
            );
            assert!(
                !operation_kind_requires_bark_reconciliation(kind),
                "pure e-cash op {kind} must not trigger Bark reconciliation"
            );
        }

        for kind in [
            FederationOperationKind::AddressAllocation,
            FederationOperationKind::ChainObservation,
            FederationOperationKind::TransactionProposal,
            FederationOperationKind::TransactionSigned,
            FederationOperationKind::TransactionBroadcastIntent,
            FederationOperationKind::TransactionBroadcastObservation,
        ] {
            assert!(
                operation_kind_requires_bdk_reconciliation(kind),
                "on-chain op {kind} must trigger BDK reconciliation"
            );
            assert!(
                !operation_kind_requires_bark_reconciliation(kind),
                "on-chain op {kind} must not trigger Bark reconciliation"
            );
        }

        for kind in [
            FederationOperationKind::Application,
            FederationOperationKind::MintQuotePayment,
            FederationOperationKind::MeltQuotePayment,
        ] {
            assert!(
                operation_kind_requires_bark_reconciliation(kind),
                "Bark/payment op {kind} must trigger Bark reconciliation"
            );
            assert!(
                !operation_kind_requires_bdk_reconciliation(kind),
                "non-onchain op {kind} must not trigger BDK reconciliation"
            );
        }

        let mut journal = InMemoryFederationJournal::new();
        let swap_entry = journal
            .append(FederationOperationEnvelope::new(
                FederationId::from_bytes([9; 32]),
                FederationOperation::Swap(FederationSwapOperation {
                    request: SwapRequest::new(Vec::new(), Vec::new()),
                }),
            ))
            .expect("append swap")
            .entry()
            .clone();
        assert!(!entries_require_bdk_reconciliation(std::slice::from_ref(
            &swap_entry
        )));
        assert!(!entries_require_bark_reconciliation(std::slice::from_ref(
            &swap_entry
        )));
    }

    #[tokio::test]
    async fn pending_operation_group_committer_coalesces_concurrent_writes() {
        let db: DynMintDatabase =
            Arc::new(cdk_sqlite::mint::memory::empty().await.expect("mint db"));
        let committer = pending_operation_committer_for_db(&db);
        let mut tasks = Vec::new();
        let mut expected_ids = Vec::new();

        for idx in 0..8_u8 {
            let envelope = FederationOperationEnvelope::new(
                FederationId::from_bytes([idx + 1; 32]),
                FederationOperation::Swap(FederationSwapOperation {
                    request: SwapRequest::new(Vec::new(), Vec::new()),
                }),
            );
            let operation_id = envelope.operation_id().expect("operation id");
            expected_ids.push(operation_id);
            let record = FederationAlephBftPendingOperationRecord {
                operation_id,
                session_id: 0,
                envelope,
                ordered_group: None,
            };
            let db = Arc::clone(&db);
            let committer = Arc::clone(&committer);
            tasks.push(tokio::spawn(async move {
                committer.persist(&db, vec![record]).await
            }));
        }

        for task in tasks {
            task.await
                .expect("join pending commit task")
                .expect("group commit succeeded");
        }

        let persisted = db
            .get_federation_aleph_bft_pending_operations(0)
            .await
            .expect("persisted pending operations");
        assert_eq!(persisted.len(), expected_ids.len());
        for expected_id in expected_ids {
            assert!(persisted
                .iter()
                .any(|record| record.operation_id == expected_id));
        }
    }
}
