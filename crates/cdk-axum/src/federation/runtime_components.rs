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

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

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

impl FederationOperationPipeline {
    /// Acquire the outer mutation lock shared by live finality and catch-up.
    pub(super) async fn lock(&self) -> tokio::sync::MutexGuard<'_, ()> {
        self.application.lock().await
    }
}

/// Owns checkpoint demand and the bounded local barrier-proposal lease.
#[derive(Debug, Clone, Default)]
pub(super) struct FederationCheckpointManager {
    pub(super) recovery_requested: Arc<AtomicBool>,
    pub(super) next_recovery_proposal_at: Arc<tokio::sync::Mutex<Option<std::time::Instant>>>,
}

impl FederationCheckpointManager {
    pub(super) fn request_recovery(&self) {
        self.recovery_requested.store(true, Ordering::Release);
    }

    pub(super) fn recovery_is_requested(&self) -> bool {
        self.recovery_requested.load(Ordering::Acquire)
    }

    pub(super) async fn reserve_recovery_proposal(
        &self,
        now: std::time::Instant,
        lease: std::time::Duration,
    ) -> bool {
        let mut next_proposal_at = self.next_recovery_proposal_at.lock().await;
        if next_proposal_at.is_some_and(|deadline| deadline > now) {
            return false;
        }
        *next_proposal_at = Some(now + lease);
        true
    }

    pub(super) fn clear_recovery(&self) {
        self.recovery_requested.store(false, Ordering::Release);
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
}

impl FederationWalletReconciler {
    pub(super) async fn reconcile(&self, mint: &Mint) -> anyhow::Result<()> {
        crate::reconcile_federation_onchain_wallets(mint, &self.onchain_wallets).await
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
            },
            ..Default::default()
        }
    }
}

/// Fault controls injected only into the test runtime composition.
#[cfg(test)]
#[derive(Debug, Clone, Default)]
pub(super) struct FederationRuntimeFaultHarness {
    pub(super) fail_next_pending_operation_persistence: Arc<AtomicBool>,
    pub(super) suppress_next_terminal_response_publication: Arc<AtomicBool>,
    pub(super) suppress_next_scheduled_checkpoint_capture: Arc<AtomicBool>,
    pub(super) suppress_next_recovery_checkpoint_capture: Arc<AtomicBool>,
    pub(super) fail_next_checkpoint_promotion: Arc<AtomicBool>,
}
