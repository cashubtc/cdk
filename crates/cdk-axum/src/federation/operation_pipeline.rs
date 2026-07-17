//! Federation operation admission, ordering, persistence, application, and publication.
//!
//! This pipeline is the sole owner of deterministic operation mutation. It
//! serializes live finality and catch-up application, persists ordering evidence
//! before business effects, reconciles local wallet/FROST state only after
//! deterministic application, and publishes terminal responses last. It does
//! not select peers or perform network requests.

use std::collections::BTreeMap;
#[cfg(test)]
use std::sync::atomic::Ordering;
use std::sync::Arc;

use cdk::federation::{
    FederationAlephBftFinalizedItem, FederationAlephBftFinalizedItemStatus,
    FederationAlephBftPendingOperationRecord, FederationAlephBftUnitPayload,
    FederationApplicationState, FederationCatchUpOperation, FederationConsensusItem,
    FederationConsensusItemAdmission, FederationConsensusRuntime, FederationError,
    FederationJournalAppendOutcome, FederationJournalEntry, FederationOperationEnvelope,
    FederationOperationId, FederationOperationStatus, FederationRestMessageKind,
    FederationSubmitOperationResponse, MemberId,
};
use cdk::mint::Mint;

use super::node_runtime::{
    apply_federation_journal_entry_for_replay, finalized_unit_recovery_mismatch,
    runtime_unavailable, FederationNodeRuntime,
};
use super::FederationPeerSyncConfig;
use crate::{FederationOperationNotificationStats, FederationOperationWaitRegistration};

impl FederationNodeRuntime {
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

    pub(super) fn active_mint(&self) -> Result<&Arc<Mint>, cdk::Error> {
        self.mint
            .as_ref()
            .ok_or_else(|| runtime_unavailable("mint"))
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
            .components
            .faults
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
        let prepared = match consensus.prepare_aleph_bft_envelope(envelope.clone()) {
            Ok(prepared) => prepared,
            Err(err) => {
                #[cfg(feature = "prometheus")]
                self.record_rejected_operation_metric(&operation_label, "consensus");
                return Err(cdk::Error::from(err));
            }
        };
        let admission = prepared.admission();
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
            let session_id = consensus.aleph_bft_status()?.session;
            self.persist_pending_operation(FederationAlephBftPendingOperationRecord {
                operation_id,
                session_id,
                envelope,
            })
            .await?;
        }

        consensus.commit_prepared_aleph_bft_envelope(prepared);
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
    pub(super) async fn finalize_unit_locally_for_test(&self) -> Result<(), cdk::Error> {
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

    pub(crate) async fn persist_pending_operation(
        &self,
        operation: FederationAlephBftPendingOperationRecord,
    ) -> Result<(), cdk::Error> {
        #[cfg(test)]
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
        #[cfg(test)]
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
        for operation_id in finalized_items.iter().filter_map(|item| item.operation_id) {
            tx.remove_federation_aleph_bft_pending_operation(operation_id)
                .await?;
        }
        for entry in entries {
            tx.add_federation_journal_entry(entry).await?;
        }
        tx.commit().await?;
        Ok(())
    }

    pub(crate) async fn persist_catch_up(
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
        for operation_id in finalized_items.iter().filter_map(|item| item.operation_id) {
            tx.remove_federation_aleph_bft_pending_operation(operation_id)
                .await?;
        }
        for entry in entries {
            tx.add_federation_journal_entry(entry).await?;
        }
        tx.commit().await?;
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
        let _application = self.components.operation.lock().await;
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
        for entry in durable_entries.into_values() {
            if consensus
                .journal()
                .entry_by_operation_id(entry.operation_id)
                .is_none()
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

    pub(crate) async fn apply_journal_entries(
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
        let mint = self.active_mint()?;
        self.components.wallet.reconcile(mint).await.map_err(|_| {
            cdk::Error::Custom(
                "accepted federation on-chain state could not be reconciled locally".to_string(),
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
                return Err(crate::aleph_bft_catch_up_recovery_mismatch(format!(
                    "catch-up rejection mismatch at operation {}",
                    peer_entry.operation_id
                )));
            }
        }

        Ok(())
    }

    async fn apply_journal_entry_for_replay(
        &self,
        entry: &FederationJournalEntry,
    ) -> Result<Option<FederationOperationId>, cdk::Error> {
        let mint = self.active_mint()?;
        let outcome = apply_federation_journal_entry_for_replay(mint, entry).await;
        if outcome.is_err() {
            let persisted = mint
                .localstore()
                .get_federation_journal_entry_by_operation_id(entry.operation_id)
                .await?
                .ok_or_else(|| {
                    cdk::Error::Federation(FederationError::UnknownOperation {
                        operation_id: entry.operation_id.to_string(),
                    })
                })?;
            self.mark_operation_applications(&[(entry.operation_id, persisted.application)])
                .await?;
        }
        outcome
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
}
