//! Federated Cashu mint node.
//!
//! Owns everything a federation member runs besides its HTTP surface: the
//! AlephBFT consensus session, peer synchronization and catch-up,
//! checkpoints, the federation peer client, DKG execution and the payment and
//! custody workers. `cdk-axum` serves this node's routes.
//!
//! Modules are public but hidden: they are this crate's working surface for
//! `cdk-axum`, not a stable API.

use cdk::federation::{
    FederationCheckpointQuorum, FederationConfig, FederationError,
    FederationMaterializedStateRowFingerprint, FederationMaterializedStateSnapshot,
    FederationOperationStatus, FederationPeerStatus, MemberId, VerifiedFederationCheckpointQuorum,
};

/// Member-local configuration, degraded states and errors of Bark custody.
#[cfg(feature = "federation-bark")]
pub use cdk_bark::{
    Error as FederatedBarkError, FederatedBarkDegradedReason, FederatedBarkLocalConfig,
};
/// Member-local configuration of the joint on-chain wallet.
#[cfg(feature = "federation-bdk")]
pub use cdk_bdk::FederatedBdkLocalConfig;
/// Random number generator used for DKG messages.
pub use cdk_frost::rand_core::OsRng;

#[doc(hidden)]
pub use self::bootstrap::*;
#[doc(hidden)]
pub use self::mint_state::*;
#[doc(hidden)]
pub use self::state::*;

#[doc(hidden)]
pub mod aleph_bft_latency;
#[doc(hidden)]
pub mod aleph_bft_network;
#[doc(hidden)]
pub mod aleph_bft_runtime;
#[doc(hidden)]
pub mod aleph_bft_session;
#[cfg(feature = "federation-bark")]
#[doc(hidden)]
pub mod bark_custody;
#[cfg(feature = "federation-bark")]
#[doc(hidden)]
pub mod bark_node;
#[doc(hidden)]
pub mod bootstrap;
#[cfg(feature = "federation-bdk")]
#[doc(hidden)]
pub mod broadcast;
#[doc(hidden)]
pub mod checkpoint_catch_up;
#[doc(hidden)]
pub mod checkpoint_manager;
#[doc(hidden)]
pub mod dkg_driver;
#[doc(hidden)]
pub mod dkg_persistence;
#[doc(hidden)]
pub mod frost_signing;
#[cfg_attr(
    not(any(feature = "federation-bdk", feature = "federation-bark")),
    allow(dead_code)
)] // FROST signing is used only by the custody backends.
mod signer_fan_out;
#[doc(hidden)]
pub use cdk::federation::latency;
#[doc(hidden)]
pub mod mint_state;
#[doc(hidden)]
pub mod node_runtime;
#[doc(hidden)]
pub mod operation_pipeline;
#[doc(hidden)]
pub mod payment_observation;
#[doc(hidden)]
pub mod payment_schedule;
#[doc(hidden)]
pub mod peer_client;
#[doc(hidden)]
pub mod runtime_components;
#[doc(hidden)]
pub mod services;
#[doc(hidden)]
pub mod state;
#[doc(hidden)]
pub mod state_transfer;
#[doc(hidden)]
pub mod sync_config;
#[doc(hidden)]
pub mod sync_loop;
#[doc(hidden)]
pub mod transport;
#[cfg(feature = "federation-bdk")]
#[doc(hidden)]
pub mod wallet_liveness;
#[doc(hidden)]
pub mod wallet_operation_publisher;

pub use self::aleph_bft_network::CdkAlephBftNetworkInboundQueue;
#[cfg(any(test, feature = "test-utils"))]
#[doc(hidden)]
pub use self::aleph_bft_session::{
    federation_aleph_bft_live_session_startup_decision,
    halt_federation_aleph_bft_generation_for_test,
    rotate_federation_aleph_bft_live_session_to_next_empty_with_transport,
    start_federation_aleph_bft_live_session_with_config,
    start_federation_aleph_bft_live_session_with_options_and_transport,
    start_federation_aleph_bft_live_session_with_transport, FederationAlephBftLiveSessionOptions,
    FederationAlephBftLiveSessionStartupDecision,
};
pub use self::aleph_bft_session::{
    halt_current_federation_aleph_bft_session,
    start_federation_aleph_bft_live_session_with_replay_target_and_transport,
    stop_current_federation_aleph_bft_session_for_catch_up, AlephBftSessionSupervisor,
};
#[cfg(feature = "federation-bdk")]
pub use self::broadcast::FederationTransactionBroadcaster;
#[cfg(any(test, feature = "test-utils"))]
#[doc(hidden)]
pub use self::dkg_driver::{
    complete_federation_dkg_ceremony, federation_dkg_readiness_required,
    prepare_federation_dkg_participant_state, run_federation_dkg_driver_tick,
    run_federation_dkg_readiness_round, FederationDkgDeliveryTracker,
};
pub use self::dkg_driver::{frost_storage_key, spawn_federation_dkg_driver};
pub use self::dkg_driver::{
    FederationDkgAutoStartConfig, FederationDkgDriverRunConfig, FederationDkgDriverRunOutcome,
    FederationDkgDriverTickOutcome, FederationDkgSecretShareTransportPolicy,
};
pub use self::node_runtime::{
    apply_federation_journal_entry_for_replay, FederationMemberContext, FederationMembership,
    FederationNodeRuntime,
};
#[doc(hidden)]
pub use self::payment_observation::payment_observation_report_summaries;
pub use self::payment_observation::FederationPaymentObserver;
#[cfg(any(test, feature = "test-utils"))]
#[doc(hidden)]
pub use self::peer_client::FederationRestPeerClient;
#[cfg(any(test, feature = "test-utils"))]
#[doc(hidden)]
pub use self::sync_config::FederationPeerReconnectBackoff;
#[cfg(any(test, feature = "test-utils"))]
#[doc(hidden)]
pub use self::sync_config::{
    federation_peer_sync_disabled_value, federation_peer_sync_initial_delay,
    federation_peer_sync_interval, federation_reconnect_backoff_delay,
};
pub use self::sync_config::{FederationPeerSyncConfig, FederationPeerSyncConfigError};
#[cfg(any(test, feature = "test-utils"))]
#[doc(hidden)]
pub use self::sync_loop::exchange_checkpoint_round;
pub use self::sync_loop::spawn_federation_peer_sync_loop_with_config;
#[cfg(any(test, feature = "test-utils"))]
#[doc(hidden)]
pub use self::sync_loop::{
    run_federation_peer_sync_round, run_federation_peer_sync_round_with_backoff,
};

#[cfg(feature = "iroh")]
pub use self::transport::federation_transport_with_iroh_node;
pub use self::transport::{default_federation_transport, FederationTransport};
#[cfg(feature = "federation-bdk")]
pub use self::wallet_liveness::FederationWalletLivenessWorker;
/// Bark operation codecs and the Bark payment drivers this node runs.
#[doc(hidden)]
#[cfg(feature = "federation-bark")]
pub use cdk_bark::{
    advance_federated_bark_melt_quote, advance_federated_bark_melt_quote_from_evidence_delivery,
    advance_federated_bark_mint_quote, advance_federated_bark_mint_quote_from_evidence_delivery,
    advance_federated_bark_receive_intent, bark_application, bark_federation_operation,
    bark_operation, federated_bark_quote_id, federation_bark_ready_projection,
    federation_bark_recovery_projection, prepare_federated_bark_mint_quote,
    reconcile_federation_bark_runtime, retry_federated_bark_maintenance_action,
    submit_federated_bark_melt_with_plan,
};

const FEDERATION_DISABLE_PEER_SYNC_ENV_VAR: &str = "CDK_FEDERATION_DISABLE_PEER_SYNC";

#[doc(hidden)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CheckpointQuorumVerificationContext {
    Capture,
    Formation,
    Promotion,
    Publish,
    #[cfg(feature = "prometheus")]
    PeerFetch,
    StartupCatchUp,
    Audit,
}

#[doc(hidden)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CheckpointSignatureVerificationContext {
    Formation,
    Publish,
    #[cfg(feature = "prometheus")]
    PeerFetch,
}

#[cfg(feature = "prometheus")]
impl CheckpointQuorumVerificationContext {
    const fn performance_labels(
        self,
    ) -> (
        cdk_prometheus::FederationPerformanceOperation,
        cdk_prometheus::FederationPerformanceWork,
        cdk_prometheus::FederationPerformanceWork,
    ) {
        use cdk_prometheus::{FederationPerformanceOperation, FederationPerformanceWork};

        match self {
            Self::Capture => (
                FederationPerformanceOperation::QuorumVerificationCapture,
                FederationPerformanceWork::QuorumVerificationCaptureAttempts,
                FederationPerformanceWork::QuorumVerificationCaptureFailures,
            ),
            Self::Formation => (
                FederationPerformanceOperation::QuorumVerificationFormation,
                FederationPerformanceWork::QuorumVerificationFormationAttempts,
                FederationPerformanceWork::QuorumVerificationFormationFailures,
            ),
            Self::Promotion => (
                FederationPerformanceOperation::QuorumVerificationPromotion,
                FederationPerformanceWork::QuorumVerificationPromotionAttempts,
                FederationPerformanceWork::QuorumVerificationPromotionFailures,
            ),
            Self::Publish => (
                FederationPerformanceOperation::QuorumVerificationPublish,
                FederationPerformanceWork::QuorumVerificationPublishAttempts,
                FederationPerformanceWork::QuorumVerificationPublishFailures,
            ),
            Self::PeerFetch => (
                FederationPerformanceOperation::QuorumVerificationPeerFetch,
                FederationPerformanceWork::QuorumVerificationPeerFetchAttempts,
                FederationPerformanceWork::QuorumVerificationPeerFetchFailures,
            ),
            Self::StartupCatchUp => (
                FederationPerformanceOperation::QuorumVerificationStartupCatchUp,
                FederationPerformanceWork::QuorumVerificationStartupCatchUpAttempts,
                FederationPerformanceWork::QuorumVerificationStartupCatchUpFailures,
            ),
            Self::Audit => (
                FederationPerformanceOperation::QuorumVerificationAudit,
                FederationPerformanceWork::QuorumVerificationAuditAttempts,
                FederationPerformanceWork::QuorumVerificationAuditFailures,
            ),
        }
    }
}

#[cfg(feature = "prometheus")]
impl CheckpointSignatureVerificationContext {
    const fn performance_operation(self) -> cdk_prometheus::FederationPerformanceOperation {
        match self {
            Self::Formation => {
                cdk_prometheus::FederationPerformanceOperation::CheckpointSignatureVerificationFormation
            }
            Self::Publish => {
                cdk_prometheus::FederationPerformanceOperation::CheckpointSignatureVerificationPublish
            }
            Self::PeerFetch => {
                cdk_prometheus::FederationPerformanceOperation::CheckpointSignatureVerificationPeerFetch
            }
        }
    }
}

#[doc(hidden)]
#[cfg(feature = "prometheus")]
pub fn record_checkpoint_quorum_verification_performance(
    context: CheckpointQuorumVerificationContext,
    success: bool,
    duration_seconds: f64,
) {
    let (operation, attempts_work, failures_work) = context.performance_labels();
    cdk_prometheus::METRICS.record_federation_performance_work(attempts_work, 1);
    if !success {
        cdk_prometheus::METRICS.record_federation_performance_work(failures_work, 1);
    }
    cdk_prometheus::METRICS.record_federation_performance_operation(
        operation,
        cdk_prometheus::FederationPerformanceOutcome::from_success(success),
        duration_seconds,
    );
}

#[doc(hidden)]
#[cfg(feature = "prometheus")]
pub fn record_checkpoint_signature_verification_performance(
    context: CheckpointSignatureVerificationContext,
    success: bool,
    duration_seconds: f64,
) {
    cdk_prometheus::METRICS.record_federation_performance_operation(
        context.performance_operation(),
        cdk_prometheus::FederationPerformanceOutcome::from_success(success),
        duration_seconds,
    );
}

#[doc(hidden)]
pub fn verify_checkpoint_quorum_with_metrics(
    quorum: &FederationCheckpointQuorum,
    config: &FederationConfig,
    local_member: MemberId,
    context: CheckpointQuorumVerificationContext,
) -> Result<VerifiedFederationCheckpointQuorum, cdk::Error> {
    #[cfg(feature = "prometheus")]
    let started_at = std::time::Instant::now();
    let result = quorum
        .verify_against_validated_config(config)
        .map_err(cdk::Error::from);
    #[cfg(feature = "prometheus")]
    {
        let duration_seconds = started_at.elapsed().as_secs_f64();
        let member = local_member.to_string();
        cdk_prometheus::METRICS.record_federation_checkpoint_work(
            &member,
            cdk_prometheus::FederationCheckpointWork::SignaturesVerified,
            u64::try_from(quorum.signatures.len()).unwrap_or(u64::MAX),
        );
        cdk_prometheus::METRICS.record_federation_checkpoint_duration(
            &member,
            cdk_prometheus::FederationCheckpointOperation::SignatureVerification,
            duration_seconds,
        );
        record_checkpoint_quorum_verification_performance(
            context,
            result.is_ok(),
            duration_seconds,
        );
    }
    #[cfg(not(feature = "prometheus"))]
    let _ = (local_member, context);
    result
}

#[doc(hidden)]
pub async fn local_peer_status(state: &FederationNode) -> anyhow::Result<FederationPeerStatus> {
    let mut status = local_peer_order_status(state).await?;
    let frontier = state
        .mint
        .localstore()
        .get_federation_checkpoint_frontier()
        .await?;
    status.application_digest = Some(frontier.application_digest);
    status.latest_audited_checkpoint_id = frontier.latest_audited_checkpoint_id;
    status.materialized_state_digest = frontier.latest_materialized_state_digest;
    Ok(status)
}

#[doc(hidden)]
pub async fn local_peer_order_status(
    state: &FederationNode,
) -> anyhow::Result<FederationPeerStatus> {
    Ok(local_peer_order_status_with_latest_operation_status(state)
        .await?
        .0)
}

#[doc(hidden)]
pub async fn local_peer_order_status_with_latest_operation_status(
    state: &FederationNode,
) -> anyhow::Result<(FederationPeerStatus, Option<FederationOperationStatus>)> {
    let runtime_config =
        state
            .mint
            .federation_config()
            .ok_or(FederationError::RuntimeUnavailable {
                component: "mint configuration",
            })?;
    let tip = state.mint.localstore().get_federation_journal_tip().await?;
    let (mut status, latest_operation_status) = match tip {
        Some(tip) => {
            let operation_count = tip
                .index
                .checked_add(1)
                .ok_or_else(|| anyhow::anyhow!("federation journal index overflow"))?;
            (
                FederationPeerStatus::new(
                    runtime_config.federation_id(),
                    runtime_config.member_id(),
                    operation_count,
                    Some(tip.index),
                    tip.order_digest,
                ),
                Some(tip.status),
            )
        }
        None => (
            FederationPeerStatus::new(
                runtime_config.federation_id(),
                runtime_config.member_id(),
                0,
                None,
                cdk::federation::FederationJournalDigest::genesis(),
            ),
            None,
        ),
    };
    let next_item_index = local_finalized_item_cursor(state).await?;
    let recovery_cursor = state
        .federation
        .consensus()
        .ok_or(FederationError::RuntimeUnavailable {
            component: "AlephBFT consensus",
        })?
        .lock()
        .await
        .aleph_bft_recovery_cursor()?;
    status = status
        .with_aleph_bft_next_item_index(next_item_index)
        .with_aleph_bft_next_unit_index_by_member(recovery_cursor.next_unit_index_by_member)
        .with_aleph_bft_session(recovery_cursor.session);
    Ok((status, latest_operation_status))
}

#[doc(hidden)]
pub async fn local_peer_status_with_materialized_rows(
    state: &FederationNode,
) -> anyhow::Result<(
    FederationPeerStatus,
    Vec<FederationMaterializedStateRowFingerprint>,
)> {
    let mut status = local_peer_status(state).await?;
    #[cfg(feature = "prometheus")]
    let digest_started_at = std::time::Instant::now();
    let snapshot = materialized_state_snapshot(state).await?;
    let digest = snapshot.digest()?;
    #[cfg(feature = "prometheus")]
    cdk_prometheus::METRICS.record_federation_checkpoint_duration(
        &status.member_id.to_string(),
        cdk_prometheus::FederationCheckpointOperation::MaterializedStateDigest,
        digest_started_at.elapsed().as_secs_f64(),
    );
    // This digest describes the current full-row snapshot, not the older checkpoint
    // audit cached in `local_peer_status`. Exact cross-member comparison is therefore
    // only meaningful while writes are quiesced: the snapshot and frontier reads are
    // deliberately bounded but are not one atomic database observation.
    status.latest_audited_checkpoint_id = None;
    status.materialized_state_digest = Some(digest);
    let materialized_state_rows = snapshot.row_fingerprints()?;
    Ok((status, materialized_state_rows))
}

#[doc(hidden)]
pub async fn local_finalized_item_cursor(state: &FederationNode) -> anyhow::Result<u64> {
    if let Some(status) = state.federation_aleph_bft_status().await {
        return Ok(status.next_item_index);
    }

    #[cfg(any(test, feature = "test-utils"))]
    return Ok(0);

    #[cfg(not(any(test, feature = "test-utils")))]
    return Err(FederationError::RuntimeUnavailable {
        component: "AlephBFT consensus status",
    }
    .into());
}

#[cfg(any(test, feature = "test-utils"))]
#[doc(hidden)]
pub async fn materialized_state_digest(
    state: &FederationNode,
    _entries: &[cdk::federation::FederationJournalEntry],
) -> anyhow::Result<cdk::federation::FederationMaterializedStateDigest> {
    Ok(materialized_state_snapshot(state).await?.digest()?)
}

#[doc(hidden)]
pub async fn materialized_state_snapshot(
    state: &FederationNode,
) -> anyhow::Result<FederationMaterializedStateSnapshot> {
    Ok(state.mint.federation_materialized_state_snapshot().await?)
}

#[cfg(any(test, feature = "test-utils"))]
#[doc(hidden)]
pub fn federation_completed_operation_uuid(
    operation_id: cdk::federation::FederationOperationId,
) -> uuid::Uuid {
    let bytes = operation_id.to_bytes();
    let mut uuid = [0_u8; 16];
    uuid.copy_from_slice(&bytes[..16]);
    uuid::Uuid::from_bytes(uuid)
}

/// Converts a count to `u64`, saturating on platforms where `usize` is wider.
pub fn usize_to_u64_saturating(value: usize) -> u64 {
    u64::try_from(value).unwrap_or(u64::MAX)
}

/// Converts a `u64` to `i64`, saturating at `i64::MAX`.
#[cfg(feature = "prometheus")]
pub fn u64_to_i64_saturating(value: u64) -> i64 {
    i64::try_from(value).unwrap_or(i64::MAX)
}

#[cfg(all(test, feature = "prometheus"))]
mod performance_instrumentation_tests {
    use std::collections::HashSet;

    use super::{CheckpointQuorumVerificationContext, CheckpointSignatureVerificationContext};

    #[test]
    fn quorum_verification_contexts_have_distinct_fixed_metric_labels() {
        let contexts = [
            CheckpointQuorumVerificationContext::Capture,
            CheckpointQuorumVerificationContext::Formation,
            CheckpointQuorumVerificationContext::Promotion,
            CheckpointQuorumVerificationContext::Publish,
            CheckpointQuorumVerificationContext::PeerFetch,
            CheckpointQuorumVerificationContext::StartupCatchUp,
            CheckpointQuorumVerificationContext::Audit,
        ];
        let mut operations = HashSet::new();
        let mut attempts = HashSet::new();
        let mut failures = HashSet::new();

        for context in contexts {
            let (operation, attempt, failure) = context.performance_labels();
            assert!(operations.insert(operation));
            assert!(attempts.insert(attempt));
            assert!(failures.insert(failure));
            assert_ne!(attempt, failure);
        }

        assert_eq!(operations.len(), contexts.len());
        assert_eq!(attempts.len(), contexts.len());
        assert_eq!(failures.len(), contexts.len());
    }

    #[test]
    fn checkpoint_signature_contexts_have_distinct_fixed_metric_labels() {
        let contexts = [
            CheckpointSignatureVerificationContext::Formation,
            CheckpointSignatureVerificationContext::Publish,
            CheckpointSignatureVerificationContext::PeerFetch,
        ];
        let operations = contexts
            .into_iter()
            .map(CheckpointSignatureVerificationContext::performance_operation)
            .collect::<HashSet<_>>();

        assert_eq!(operations.len(), contexts.len());
    }
}
