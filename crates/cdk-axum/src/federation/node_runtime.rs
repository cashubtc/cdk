//! Federation node runtime ownership.

use std::sync::atomic::AtomicU64;
#[cfg(test)]
use std::sync::atomic::Ordering;
use std::sync::Arc;

use super::runtime_components::FederationRuntimeComponents;
use super::{FederationPeerSyncConfig, FederationTransport};
#[cfg(test)]
use crate::FederationSyncHandle;
use crate::{
    FederationAlephBftLiveSessionState, FederationAlephBftNetworkInboundState,
    FederationConsensusHandle, FederationDkgMemberAdmissionHandle,
    FederationInvalidOperationAlertHandle, FederationNotificationHandle,
    FederationRestAdmissionHandle, FederationSequenceHandle, FederationSyncStatus,
};
use cdk::federation::{
    FederationAlephBftUnitPayload, FederationApplicationState, FederationCheckpoint,
    FederationCheckpointBoundary, FederationCheckpointQuorum, FederationCheckpointSignature,
    FederationCheckpointSummary, FederationConfig, FederationConsensusRuntimeStatus,
    FederationError, FederationJournalEntry, FederationMaterializedStateDigest,
    FederationOperationId, FederationOperationStatus, MemberConfigPublic, MemberId,
};
use cdk::mint::Mint;

/// Required handles shared by setup and active member operation.
#[derive(Clone)]
pub(crate) struct FederationMemberContext {
    pub(crate) mint: Arc<Mint>,
    pub(crate) consensus: FederationConsensusHandle,
    pub(crate) sequences: FederationSequenceHandle,
    pub(crate) notifications: FederationNotificationHandle,
    pub(crate) peer_sync_config: FederationPeerSyncConfig,
    pub(crate) private_rest_admission: FederationRestAdmissionHandle,
    pub(crate) invalid_operation_alerts: FederationInvalidOperationAlertHandle,
}

impl FederationMemberContext {
    pub(crate) fn new(
        mint: Arc<Mint>,
        consensus: FederationConsensusHandle,
        sequences: FederationSequenceHandle,
        peer_sync_config: FederationPeerSyncConfig,
    ) -> Self {
        Self {
            mint,
            consensus,
            sequences,
            peer_sync_config,
            notifications: Arc::new(crate::FederationOperationNotifications::default()),
            private_rest_admission: Arc::new(tokio::sync::Semaphore::new(
                peer_sync_config.private_rest_inbound_concurrency_limit,
            )),
            invalid_operation_alerts: Arc::new(tokio::sync::Mutex::new(
                crate::FederationInvalidOperationAlertTracker::default(),
            )),
        }
    }
}

/// Setup owns authenticated coordination handles but cannot start payment workers.
#[derive(Clone)]
pub(crate) enum FederationMembership {
    Standalone,
    Setup(FederationMemberContext),
    Active(FederationMemberContext),
}

impl FederationMembership {
    pub(crate) fn configured(member: FederationMemberContext) -> Self {
        if member
            .mint
            .federation_config()
            .is_some_and(|config| !config.public_config().keysets.is_empty())
        {
            Self::Active(member)
        } else {
            Self::Setup(member)
        }
    }

    fn context(&self) -> Option<&FederationMemberContext> {
        match self {
            Self::Standalone => None,
            Self::Setup(member) | Self::Active(member) => Some(member),
        }
    }

    pub(crate) fn is_active(&self) -> bool {
        matches!(self, Self::Active(_))
    }
}

/// Runtime-owned handles for one active federation member.
#[derive(Clone)]
#[allow(missing_debug_implementations)]
pub(crate) struct FederationNodeRuntime {
    pub(crate) transport: FederationTransport,
    pub(crate) membership: FederationMembership,
    pub(crate) tasks: super::services::FederationTaskGroup,
    pub(crate) payment_observer: super::FederationPaymentObserver,
    #[cfg(feature = "federation-bdk")]
    pub(crate) transaction_broadcaster: super::FederationTransactionBroadcaster,
    #[cfg(feature = "federation-bdk")]
    pub(crate) wallet_liveness: super::FederationWalletLivenessWorker,
    pub(crate) components: FederationRuntimeComponents,
    pub(crate) aleph_bft_network_inbound: FederationAlephBftNetworkInboundState,
    pub(crate) aleph_bft_live_session: FederationAlephBftLiveSessionState,
    pub(crate) aleph_bft_halt_epoch: Arc<AtomicU64>,
    #[cfg(test)]
    pub(crate) local_finality_test: bool,
    #[cfg(test)]
    pub(crate) manual_finality_test: bool,
}

pub(super) fn runtime_unavailable(component: &'static str) -> cdk::Error {
    FederationError::RuntimeUnavailable { component }.into()
}

pub(super) async fn materialized_state_digest_with_metrics(
    mint: &Mint,
    member_id: MemberId,
) -> Result<FederationMaterializedStateDigest, cdk::Error> {
    #[cfg(feature = "prometheus")]
    let started_at = std::time::Instant::now();
    #[cfg(feature = "prometheus")]
    let load_started_at = std::time::Instant::now();
    let snapshot = mint.federation_materialized_state_snapshot().await;
    #[cfg(feature = "prometheus")]
    cdk_prometheus::METRICS.record_federation_performance_operation(
        cdk_prometheus::FederationPerformanceOperation::MaterializedStateLoad,
        cdk_prometheus::FederationPerformanceOutcome::from_success(snapshot.is_ok()),
        load_started_at.elapsed().as_secs_f64(),
    );
    let snapshot = snapshot?;
    #[cfg(feature = "prometheus")]
    cdk_prometheus::METRICS.record_federation_performance_work(
        cdk_prometheus::FederationPerformanceWork::MaterializedStateRowsLoaded,
        u64::try_from(snapshot.rows().len()).unwrap_or(u64::MAX),
    );
    #[cfg(feature = "prometheus")]
    let digest_started_at = std::time::Instant::now();
    let digest = snapshot.digest();
    #[cfg(feature = "prometheus")]
    cdk_prometheus::METRICS.record_federation_performance_operation(
        cdk_prometheus::FederationPerformanceOperation::MaterializedStateDigest,
        cdk_prometheus::FederationPerformanceOutcome::from_success(digest.is_ok()),
        digest_started_at.elapsed().as_secs_f64(),
    );
    let digest = digest?;
    #[cfg(feature = "prometheus")]
    cdk_prometheus::METRICS.record_federation_checkpoint_duration(
        &member_id.to_string(),
        cdk_prometheus::FederationCheckpointOperation::MaterializedStateDigest,
        started_at.elapsed().as_secs_f64(),
    );
    #[cfg(not(feature = "prometheus"))]
    let _ = member_id;
    Ok(digest)
}

pub(super) async fn persist_federation_operation_apply_failure(
    mint: &Mint,
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
    let db = mint.localstore();
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
    Ok(status)
}

pub(super) async fn mark_federation_operation_apply_failure(
    mint: &Mint,
    operation_id: FederationOperationId,
    error: cdk::Error,
) -> Result<(FederationOperationStatus, cdk::Error), cdk::Error> {
    if !is_definitive_federation_apply_rejection(&error) {
        let status = persist_federation_operation_apply_failure(mint, operation_id, None).await?;
        return Ok((status, error));
    }

    let rejection = cdk::error::ErrorResponse::from(error);
    let status =
        persist_federation_operation_apply_failure(mint, operation_id, Some(&rejection)).await?;
    Ok((status, cdk::Error::from(rejection)))
}

fn is_definitive_federation_apply_rejection(error: &cdk::Error) -> bool {
    // Wallet proof-release policy is deliberately conservative for spent or
    // reserved inputs. At ordered application, those states instead identify a
    // deterministic losing operation: its transaction rolled back, and replay
    // must record the rejection and continue beyond it.
    error.is_definitive_failure()
        || matches!(
            error,
            cdk::Error::PendingQuote | cdk::Error::TokenAlreadySpent | cdk::Error::TokenPending
        )
}

pub(crate) async fn apply_federation_journal_entry_for_replay(
    mint: &Mint,
    entry: &FederationJournalEntry,
) -> Result<Option<FederationOperationId>, cdk::Error> {
    if entry.application.is_terminal() {
        return Ok(None);
    }
    let mut replay_entry = entry.clone();
    if replay_entry.status() == FederationOperationStatus::Failed {
        replay_entry.application = FederationApplicationState::Accepted {};
    }
    let mut attempts = 0usize;
    let mut retry_delay = crate::FEDERATION_APPLY_RETRY_INITIAL_DELAY;
    let started_at = tokio::time::Instant::now();

    loop {
        match mint
            .apply_accepted_federation_operation(&replay_entry)
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
                let elapsed = started_at.elapsed();
                if elapsed >= crate::federation_apply_retry_budget() {
                    tracing::error!(
                        operation_id = %entry.operation_id,
                        retry_attempts = attempts,
                        elapsed_ms = elapsed.as_millis(),
                        "Federation journal replay apply retry budget exhausted; transitioning to bounded operator-repairable state"
                    );
                    return Err(cdk::Error::Custom(format!(
                        "federation journal replay apply retry budget exhausted for operation {} after {} attempts over {:.1}s: {}",
                        entry.operation_id,
                        attempts,
                        elapsed.as_secs_f64(),
                        err
                    )));
                }
                if attempts == 1 || attempts.is_multiple_of(10) {
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
                let (status, err) =
                    mark_federation_operation_apply_failure(mint, entry.operation_id, err).await?;
                if status == FederationOperationStatus::Rejected {
                    return Ok(None);
                }
                return Err(err);
            }
        }
    }
}

pub(super) fn verify_checkpoint_signature_with_metrics(
    signature: &FederationCheckpointSignature,
    checkpoint: &FederationCheckpoint,
    signer: &MemberConfigPublic,
    local_member: MemberId,
    context: super::CheckpointSignatureVerificationContext,
) -> Result<(), cdk::Error> {
    #[cfg(feature = "prometheus")]
    let started_at = std::time::Instant::now();
    let result = signature
        .verify(checkpoint, signer)
        .map_err(cdk::Error::from);
    #[cfg(feature = "prometheus")]
    {
        let duration_seconds = started_at.elapsed().as_secs_f64();
        let member = local_member.to_string();
        cdk_prometheus::METRICS.record_federation_checkpoint_work(
            &member,
            cdk_prometheus::FederationCheckpointWork::SignaturesVerified,
            1,
        );
        cdk_prometheus::METRICS.record_federation_checkpoint_duration(
            &member,
            cdk_prometheus::FederationCheckpointOperation::SignatureVerification,
            duration_seconds,
        );
        super::record_checkpoint_signature_verification_performance(
            context,
            result.is_ok(),
            duration_seconds,
        );
    }
    #[cfg(not(feature = "prometheus"))]
    let _ = (local_member, context);
    result
}

pub(super) fn checkpoint_summary(
    checkpoint: &FederationCheckpoint,
    config: &FederationConfig,
) -> Result<FederationCheckpointSummary, cdk::Error> {
    checkpoint.validate_against_validated_config(config)?;
    Ok(FederationCheckpointSummary::from_checkpoint(checkpoint)?)
}

pub(super) fn stored_checkpoint_quorum_summary(
    quorum: &FederationCheckpointQuorum,
    config: &FederationConfig,
) -> Result<FederationCheckpointSummary, cdk::Error> {
    // This is presentation-only metadata. Stored proofs are verified at startup or ingress, and
    // a peer must verify the full proof after fetching it before making any safety decision.
    checkpoint_summary(&quorum.checkpoint, config)
}

pub(super) fn checkpoint_matches_boundary(
    checkpoint: &FederationCheckpoint,
    boundary: &FederationCheckpointBoundary,
) -> bool {
    checkpoint
        .committed_frontier()
        .exactly_matches(&boundary.committed_frontier())
}

pub(super) fn finalized_unit_recovery_mismatch(
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
        FederationSyncStatus::Halted { reason } => FederationError::NodeHalted {
            reason: cdk::federation::redact_federation_operator_message(&reason),
        }
        .into(),
    }
}

impl FederationNodeRuntime {
    pub(super) fn configured_member(&self) -> Result<&FederationMemberContext, cdk::Error> {
        self.membership
            .context()
            .ok_or_else(|| runtime_unavailable("member context"))
    }

    pub(crate) fn mint(&self) -> Option<&Arc<Mint>> {
        self.membership.context().map(|member| &member.mint)
    }

    pub(crate) fn consensus(&self) -> Option<&FederationConsensusHandle> {
        self.membership.context().map(|member| &member.consensus)
    }

    pub(crate) fn sequences(&self) -> Option<&FederationSequenceHandle> {
        self.membership.context().map(|member| &member.sequences)
    }

    pub(crate) fn notifications(&self) -> Option<&FederationNotificationHandle> {
        self.membership
            .context()
            .map(|member| &member.notifications)
    }

    pub(crate) fn peer_sync_config(&self) -> Option<FederationPeerSyncConfig> {
        self.membership
            .context()
            .map(|member| member.peer_sync_config)
    }

    pub(crate) fn private_rest_admission(&self) -> Option<&FederationRestAdmissionHandle> {
        self.membership
            .context()
            .map(|member| &member.private_rest_admission)
    }

    pub(crate) fn invalid_operation_alerts(
        &self,
    ) -> Option<&FederationInvalidOperationAlertHandle> {
        self.membership
            .context()
            .map(|member| &member.invalid_operation_alerts)
    }

    #[cfg(test)]
    pub(crate) fn member_mut_for_test(&mut self) -> &mut FederationMemberContext {
        match &mut self.membership {
            FederationMembership::Standalone => panic!("test requires a configured member"),
            FederationMembership::Setup(member) | FederationMembership::Active(member) => member,
        }
    }

    /// Build the inert runtime stored by a standalone mint in a federation-enabled binary.
    #[cfg(test)]
    pub(crate) fn disabled() -> Self {
        Self {
            transport: super::default_federation_transport(),
            membership: FederationMembership::Standalone,
            tasks: crate::federation::services::FederationTaskGroup::default(),
            payment_observer: super::FederationPaymentObserver::default(),
            #[cfg(feature = "federation-bdk")]
            transaction_broadcaster: super::FederationTransactionBroadcaster::default(),
            #[cfg(feature = "federation-bdk")]
            wallet_liveness: super::FederationWalletLivenessWorker::default(),
            components: FederationRuntimeComponents::default(),
            aleph_bft_network_inbound: Arc::new(tokio::sync::RwLock::new(None)),
            aleph_bft_live_session: Arc::new(tokio::sync::RwLock::new(None)),
            aleph_bft_halt_epoch: Arc::new(AtomicU64::new(0)),
            #[cfg(test)]
            local_finality_test: false,
            #[cfg(test)]
            manual_finality_test: false,
        }
    }

    #[cfg(test)]
    pub(crate) fn sync_handle(&self) -> Option<&FederationSyncHandle> {
        self.components.readiness.sync.as_ref()
    }

    #[cfg(test)]
    pub(crate) fn set_sync_handle(&mut self, sync: Option<FederationSyncHandle>) {
        self.components.readiness.sync = sync;
    }

    pub(crate) fn dkg_auto_start_config(
        &self,
    ) -> Option<&Arc<super::FederationDkgAutoStartConfig>> {
        self.components.dkg.auto_start_config.as_ref()
    }

    pub(crate) fn set_dkg_components(
        &mut self,
        auto_start_config: Option<Arc<super::FederationDkgAutoStartConfig>>,
        member_admission: Option<FederationDkgMemberAdmissionHandle>,
    ) {
        self.components.dkg.auto_start_config = auto_start_config;
        self.components.dkg.member_admission = member_admission;
    }

    pub(crate) fn dkg_member_admission(&self) -> Option<&FederationDkgMemberAdmissionHandle> {
        self.components.dkg.member_admission.as_ref()
    }

    #[cfg(test)]
    pub(crate) fn set_dkg_auto_start_config_for_test(
        &mut self,
        config: Option<Arc<super::FederationDkgAutoStartConfig>>,
    ) {
        self.components.dkg.auto_start_config = config;
    }

    #[cfg(test)]
    pub(crate) fn set_dkg_member_admission_for_test(
        &mut self,
        admission: Option<FederationDkgMemberAdmissionHandle>,
    ) {
        self.components.dkg.member_admission = admission;
    }

    #[cfg(feature = "federation-bdk")]
    pub(crate) fn onchain_wallets(&self) -> &[cdk_bdk::FederatedBdkWallet] {
        &self.components.wallet.onchain_wallets
    }

    pub(crate) fn frost_signing(
        &self,
    ) -> Option<&super::frost_signing::FederationFrostSigningRuntime> {
        self.components.wallet.frost_signing.as_ref()
    }

    #[cfg(feature = "federation-bark")]
    pub(crate) fn bark_receive_secrets(
        &self,
    ) -> Option<Arc<cdk_bark::FederatedBarkReceiveSecretStore>> {
        self.components
            .wallet
            .bark
            .as_ref()
            .map(|runtime| runtime.receive_secret_store())
    }

    #[cfg(feature = "federation-bark")]
    pub(crate) fn bark_runtime(&self) -> Option<&Arc<cdk_bark::FederatedBarkRuntime>> {
        self.components.wallet.bark.as_ref()
    }

    #[cfg(feature = "federation-bark")]
    pub(crate) fn bark_incoming_lock(&self) -> &Arc<tokio::sync::Mutex<()>> {
        &self.components.wallet.bark_incoming
    }

    #[cfg(test)]
    pub(crate) fn set_frost_signing_for_test(
        &mut self,
        frost_signing: Option<super::frost_signing::FederationFrostSigningRuntime>,
    ) {
        self.components.wallet.frost_signing = frost_signing;
    }

    #[cfg(test)]
    pub(crate) fn operation_application_lock_for_test(&self) -> Arc<tokio::sync::Mutex<()>> {
        Arc::clone(&self.components.operation.application)
    }

    #[cfg(test)]
    pub(crate) fn suppress_next_terminal_response_publication_for_test(&self) {
        self.components
            .faults
            .suppress_next_terminal_response_publication
            .store(true, Ordering::Release);
    }

    #[cfg(test)]
    pub(crate) fn fail_next_pending_operation_persistence_for_test(&self) {
        self.components
            .faults
            .fail_next_pending_operation_persistence
            .store(true, Ordering::Release);
    }

    #[cfg(test)]
    pub(crate) fn fail_next_finalization_persistence_for_test(&self) {
        self.components
            .faults
            .fail_next_finalization_persistence
            .store(true, Ordering::Release);
    }

    #[cfg(all(test, feature = "federation-bdk"))]
    pub(crate) fn fail_next_onchain_reconciliation_for_test(&self) {
        self.components
            .faults
            .fail_next_onchain_reconciliation
            .store(true, Ordering::Release);
    }

    #[cfg(test)]
    pub(crate) fn permanent_federation_apply_contention_for_test(&self) {
        self.components
            .faults
            .permanent_federation_apply_contention
            .store(true, Ordering::Release);
    }

    #[cfg(all(test, feature = "federation-bdk"))]
    pub(crate) async fn bdk_degraded_reason_for_test(&self) -> Option<String> {
        self.components.wallet.bdk_degraded_reason().await
    }

    pub(crate) async fn sync_status(&self) -> Option<FederationSyncStatus> {
        self.components.readiness.status().await
    }

    pub(crate) async fn restore_startup_readiness(&self) {
        let Some(sync) = &self.components.readiness.sync else {
            return;
        };
        let Ok(mint) = self.active_mint() else {
            return;
        };
        let initial_status = sync.lock().await.status();
        match &initial_status {
            FederationSyncStatus::Healthy | FederationSyncStatus::AwaitingQuorum { .. } => {}
            FederationSyncStatus::Lagging { .. }
            | FederationSyncStatus::CatchingUp { .. }
            | FederationSyncStatus::Halted { .. } => return,
        }

        let frontier = match mint.localstore().get_federation_checkpoint_frontier().await {
            Ok(frontier) => frontier,
            Err(err) => {
                self.finish_startup_revalidation(
                    &initial_status,
                    FederationSyncStatus::Halted {
                        reason: format!(
                            "could not inspect federation startup checkpoint frontier: {err}"
                        ),
                    },
                )
                .await;
                return;
            }
        };
        let next_status = if frontier.materialized_state_audit_mismatch {
            FederationSyncStatus::Halted {
                reason: "materialized-state audit mismatch is latched".to_string(),
            }
        } else {
            FederationSyncStatus::AwaitingQuorum {
                operation_count: frontier.journal_next_index,
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
        self.components
            .readiness
            .finish_startup_revalidation(expected, next)
            .await;
    }

    pub(crate) async fn mark_lagging(
        &self,
        peer: MemberId,
        local_operation_count: u64,
        peer_operation_count: u64,
    ) {
        self.components
            .readiness
            .mark_lagging(peer, local_operation_count, peer_operation_count)
            .await;
        #[cfg(feature = "prometheus")]
        self.record_sync_transition(cdk_prometheus::FederationSyncTransition::Lagging);
    }

    pub(crate) async fn mark_awaiting_quorum(&self, operation_count: u64) {
        self.components
            .readiness
            .mark_awaiting_quorum(operation_count)
            .await
    }

    pub(crate) async fn mark_catching_up(&self, peer: MemberId) {
        self.components.readiness.mark_catching_up(peer).await;
        #[cfg(feature = "prometheus")]
        self.record_sync_transition(cdk_prometheus::FederationSyncTransition::CatchingUp);
    }

    #[cfg(feature = "prometheus")]
    fn record_sync_transition(&self, transition: cdk_prometheus::FederationSyncTransition) {
        let Some(runtime_config) = self.mint().and_then(|mint| mint.federation_config()) else {
            return;
        };
        cdk_prometheus::METRICS
            .record_federation_sync_transition(&runtime_config.member_id().to_string(), transition);
    }

    pub(crate) async fn mark_healthy(&self) {
        self.components.readiness.mark_healthy().await
    }

    pub(crate) async fn mark_healthy_after_replay(&self) {
        self.components.readiness.mark_healthy_after_replay().await
    }

    pub(crate) async fn mark_halted(&self, reason: String) {
        self.components.readiness.mark_halted(reason).await
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

    pub(crate) async fn live_session_replay_ready(&self) -> bool {
        self.aleph_bft_live_session
            .read()
            .await
            .as_ref()
            .is_some_and(|handle| handle.replay_ready())
    }

    pub(crate) async fn live_session_available(&self) -> bool {
        #[cfg(test)]
        if self.local_finality_test || self.manual_finality_test {
            return true;
        }
        self.live_session_active().await && self.network_inbound_active().await
    }

    pub(crate) async fn pending_mempool_len(&self) -> usize {
        match &self.consensus() {
            Some(consensus) => consensus.lock().await.pending_mempool_len(),
            None => 0,
        }
    }

    pub(crate) async fn aleph_bft_status(&self) -> Option<FederationConsensusRuntimeStatus> {
        let consensus = self.consensus()?;
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

    pub(crate) async fn mark_local_consensus_activity(&self) {
        let Some(handle) = self.aleph_bft_live_session.read().await.clone() else {
            return;
        };
        handle.mark_local_activity();
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
}

#[cfg(test)]
mod tests {
    use super::is_definitive_federation_apply_rejection;

    #[test]
    fn ordered_conflicts_are_rejections_without_releasing_wallet_proofs() {
        for error in [
            cdk::Error::PendingQuote,
            cdk::Error::TokenAlreadySpent,
            cdk::Error::TokenPending,
        ] {
            assert!(!error.is_definitive_failure());
            assert!(is_definitive_federation_apply_rejection(&error));
        }
        assert!(is_definitive_federation_apply_rejection(
            &cdk::Error::RequestAlreadyPaid
        ));
        assert!(!is_definitive_federation_apply_rejection(
            &cdk::Error::Internal
        ));
    }
}
