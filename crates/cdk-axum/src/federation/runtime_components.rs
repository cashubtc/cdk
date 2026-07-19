//! Focused state owners composed by the federation node runtime.
//!
//! Authority is deliberately split by workflow:
//!
//! - [`FederationOperationPipeline`] owns the application serialization lock;
//! - [`FederationCheckpointManager`] owns recovery-checkpoint demand and the
//!   local proposal lease;
//! - [`FederationReadinessCoordinator`] owns mutable readiness state;
//! - [`FederationDkgCoordinator`] owns DKG ceremony configuration and admission;
//! - [`FederationWalletReconciler`] owns on-chain/FROST reconciliation handles.
//!
//! None of these components owns networking or may make peer requests. Network
//! work completes before runtime mutation begins. The mutation lock order is:
//! short readiness/recovery-proposal locks independently, then operation
//! application, then consensus, then a database transaction. A component must
//! not acquire an earlier lock while holding a later one. Database reads used
//! to build a deterministic plan may occur under the consensus lock; no peer
//! request may.

#[cfg(test)]
use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use std::time::{Duration, Instant};

use cdk::federation::MemberId;
use cdk::mint::Mint;

use super::frost_signing::FederationFrostSigningRuntime;
use super::FederationDkgAutoStartConfig;
use crate::{
    FederationDkgMemberAdmissionHandle, FederationSyncHandle, FederationSyncState,
    FederationSyncStatus,
};

/// Serializes deterministic journal persistence and business application.
#[derive(Debug, Clone, Default)]
pub(super) struct FederationOperationPipeline {
    pub(super) application: Arc<tokio::sync::Mutex<()>>,
}

/// Held outer operation-application lock with bounded wait/hold instrumentation.
#[derive(Debug)]
pub(super) struct FederationOperationPipelineGuard<'a> {
    _guard: tokio::sync::MutexGuard<'a, ()>,
    #[cfg(feature = "prometheus")]
    acquired_at: Instant,
}

#[cfg(feature = "prometheus")]
impl Drop for FederationOperationPipelineGuard<'_> {
    fn drop(&mut self) {
        cdk_prometheus::METRICS.record_federation_performance_operation(
            cdk_prometheus::FederationPerformanceOperation::OperationApplicationLockHold,
            cdk_prometheus::FederationPerformanceOutcome::Success,
            self.acquired_at.elapsed().as_secs_f64(),
        );
    }
}

impl FederationOperationPipeline {
    /// Acquire the outer mutation lock shared by live finality and catch-up.
    pub(super) async fn lock(&self) -> FederationOperationPipelineGuard<'_> {
        #[cfg(feature = "prometheus")]
        let wait_started_at = Instant::now();
        let guard = self.application.lock().await;
        #[cfg(feature = "prometheus")]
        cdk_prometheus::METRICS.record_federation_performance_operation(
            cdk_prometheus::FederationPerformanceOperation::OperationApplicationLockWait,
            cdk_prometheus::FederationPerformanceOutcome::Success,
            wait_started_at.elapsed().as_secs_f64(),
        );
        FederationOperationPipelineGuard {
            _guard: guard,
            #[cfg(feature = "prometheus")]
            acquired_at: Instant::now(),
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
enum FederationRecoveryRequestState {
    #[default]
    Idle,
    Requested {
        next_proposal_at: Option<Instant>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct ScheduledCheckpointCanonicalByteBase {
    pub(super) batch_base_finalized_index: u64,
    pub(super) retained_from_finalized_index: u64,
    pub(super) canonical_bytes: u64,
}

/// Owns checkpoint demand and the bounded local barrier-proposal lease.
#[derive(Debug, Clone, Default)]
pub(super) struct FederationCheckpointManager {
    recovery: Arc<tokio::sync::Mutex<FederationRecoveryRequestState>>,
    pub(super) scheduled_canonical_byte_base:
        Arc<tokio::sync::Mutex<Option<ScheduledCheckpointCanonicalByteBase>>>,
}

impl FederationCheckpointManager {
    pub(super) async fn request_recovery(&self) {
        let mut state = self.recovery.lock().await;
        if *state == FederationRecoveryRequestState::Idle {
            *state = FederationRecoveryRequestState::Requested {
                next_proposal_at: None,
            };
        }
    }

    pub(super) async fn recovery_is_requested(&self) -> bool {
        matches!(
            *self.recovery.lock().await,
            FederationRecoveryRequestState::Requested { .. }
        )
    }

    pub(super) async fn reserve_recovery_proposal(&self, now: Instant, lease: Duration) -> bool {
        let mut state = self.recovery.lock().await;
        match *state {
            FederationRecoveryRequestState::Requested {
                next_proposal_at: Some(deadline),
            } if deadline > now => false,
            FederationRecoveryRequestState::Requested { .. } => {
                *state = FederationRecoveryRequestState::Requested {
                    next_proposal_at: Some(now + lease),
                };
                true
            }
            FederationRecoveryRequestState::Idle => false,
        }
    }

    pub(super) async fn clear_recovery(&self) {
        *self.recovery.lock().await = FederationRecoveryRequestState::Idle;
    }
}

/// Owns readiness transitions and the local peer-view state they mutate.
#[derive(Debug, Clone, Default)]
pub(super) struct FederationReadinessCoordinator {
    pub(super) sync: Option<FederationSyncHandle>,
}

impl FederationReadinessCoordinator {
    pub(super) async fn status(&self) -> Option<FederationSyncStatus> {
        let sync = self.sync.as_ref()?;
        Some(sync.lock().await.status())
    }

    pub(super) async fn update<F>(&self, update: F)
    where
        F: FnOnce(&mut FederationSyncState),
    {
        let Some(sync) = self.sync.as_ref() else {
            return;
        };
        sync.lock().await.update_status(update);
    }

    pub(super) async fn finish_startup_revalidation(
        &self,
        expected: &FederationSyncStatus,
        next: FederationSyncStatus,
    ) {
        self.update(|sync| sync.finish_startup_revalidation(expected, next))
            .await;
    }

    pub(super) async fn mark_lagging(
        &self,
        peer: MemberId,
        local_operation_count: u64,
        peer_operation_count: u64,
    ) {
        self.update(|sync| {
            sync.mark_lagging(peer, local_operation_count, peer_operation_count);
        })
        .await;
    }

    pub(super) async fn mark_awaiting_quorum(&self, operation_count: u64) {
        self.update(|sync| sync.mark_awaiting_quorum(operation_count))
            .await;
    }

    pub(super) async fn mark_catching_up(&self, peer: MemberId) {
        self.update(|sync| sync.mark_catching_up(peer)).await;
    }

    pub(super) async fn mark_healthy(&self) {
        self.update(FederationSyncState::mark_healthy).await;
    }

    pub(super) async fn mark_healthy_after_replay(&self) {
        self.update(FederationSyncState::mark_healthy_after_replay)
            .await;
    }

    pub(super) async fn mark_halted(&self, reason: String) {
        self.update(|sync| sync.mark_halted(reason)).await;
    }
}

/// Owns DKG ceremony configuration and per-member admission state.
#[derive(Debug, Clone, Default)]
pub(super) struct FederationDkgCoordinator {
    pub(super) auto_start_config: Option<Arc<FederationDkgAutoStartConfig>>,
    pub(super) member_admission: Option<FederationDkgMemberAdmissionHandle>,
}

/// Owns wallet/FROST handles used only after deterministic application.
#[derive(Clone, Default)]
#[allow(missing_debug_implementations)]
pub(super) struct FederationWalletReconciler {
    pub(super) onchain_wallets: Vec<cdk_bdk::FederatedBdkWallet>,
    pub(super) frost_signing: Option<FederationFrostSigningRuntime>,
    #[cfg(feature = "federation-bark")]
    pub(super) bark_receive_secrets: Option<Arc<cdk_bark::FederatedBarkReceiveSecretStore>>,
    bdk_degraded_reason: std::sync::Arc<tokio::sync::RwLock<Option<String>>>,
}

impl FederationWalletReconciler {
    /// Reconcile the accepted on-chain projection into member-local BDK
    /// state. A failure marks this member BDK-degraded with the typed reason
    /// and is returned so the caller can log it; a success clears the
    /// degraded state. The degraded state never participates in the
    /// deterministic consensus application decision.
    pub(super) async fn reconcile(&self, mint: &Mint) -> anyhow::Result<()> {
        match crate::reconcile_federation_onchain_wallets(mint, &self.onchain_wallets).await {
            Ok(()) => {
                if self.bdk_degraded_reason.write().await.take().is_some() {
                    tracing::info!("federation local BDK projection recovered from degraded state");
                }
                Ok(())
            }
            Err(err) => {
                let reason = format!("{err:#}");
                tracing::error!(
                    reason = %reason,
                    "federation local BDK reconciliation failed; member is BDK-degraded and local signing is blocked"
                );
                self.set_bdk_degraded_reason(Some(reason)).await;
                Err(err)
            }
        }
    }

    /// Typed reason this member is BDK-degraded, when reconciliation last
    /// failed. While set, local FROST signing must stay blocked; it never
    /// affects the deterministic consensus application decision.
    pub(super) async fn bdk_degraded_reason(&self) -> Option<String> {
        self.bdk_degraded_reason.read().await.clone()
    }

    /// Record or clear the BDK-degraded reason after a reconciliation
    /// failure or recovery.
    pub(super) async fn set_bdk_degraded_reason(&self, reason: Option<String>) {
        *self.bdk_degraded_reason.write().await = reason;
    }
}

/// State components composed by [`super::FederationNodeRuntime`].
#[derive(Clone, Default)]
#[allow(missing_debug_implementations)]
pub(crate) struct FederationRuntimeComponents {
    pub(super) operation: FederationOperationPipeline,
    pub(super) checkpoint: FederationCheckpointManager,
    pub(super) readiness: FederationReadinessCoordinator,
    pub(super) dkg: FederationDkgCoordinator,
    pub(super) wallet: FederationWalletReconciler,
    #[cfg(test)]
    pub(super) faults: FederationRuntimeFaultHarness,
}

impl FederationRuntimeComponents {
    /// Compose runtime state without exposing component-private fields.
    pub(crate) fn new(
        sync: Option<FederationSyncHandle>,
        dkg_auto_start_config: Option<Arc<FederationDkgAutoStartConfig>>,
        dkg_member_admission: Option<FederationDkgMemberAdmissionHandle>,
        onchain_wallets: Vec<cdk_bdk::FederatedBdkWallet>,
        frost_signing: Option<FederationFrostSigningRuntime>,
    ) -> Self {
        Self {
            readiness: FederationReadinessCoordinator { sync },
            dkg: FederationDkgCoordinator {
                auto_start_config: dkg_auto_start_config,
                member_admission: dkg_member_admission,
            },
            wallet: FederationWalletReconciler {
                onchain_wallets,
                frost_signing,
                #[cfg(feature = "federation-bark")]
                bark_receive_secrets: None,
                bdk_degraded_reason: std::sync::Arc::new(tokio::sync::RwLock::new(None)),
            },
            ..Default::default()
        }
    }

    /// Seed the startup BDK-degraded reason after a failed startup
    /// reconciliation; the member starts degraded with local signing blocked
    /// instead of failing startup.
    pub(crate) async fn set_startup_bdk_degraded_reason(&self, reason: Option<String>) {
        self.wallet.set_bdk_degraded_reason(reason).await;
    }
}

/// Fault controls injected only into the test runtime composition.
#[cfg(test)]
#[derive(Debug, Clone, Default)]
pub(super) struct FederationRuntimeFaultHarness {
    pub(super) fail_next_pending_operation_persistence: Arc<AtomicBool>,
    pub(super) fail_next_finalization_persistence: Arc<AtomicBool>,
    pub(super) fail_next_onchain_reconciliation: Arc<AtomicBool>,
    pub(super) permanent_federation_apply_contention: Arc<AtomicBool>,
    pub(super) suppress_next_terminal_response_publication: Arc<AtomicBool>,
    pub(super) suppress_next_scheduled_checkpoint_capture: Arc<AtomicBool>,
    pub(super) suppress_next_recovery_checkpoint_capture: Arc<AtomicBool>,
    pub(super) fail_next_checkpoint_promotion: Arc<AtomicBool>,
}
