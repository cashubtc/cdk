//! Accepted Bark projection as the drivers see it: replayed into the runtime
//! and checked for readiness.

use std::sync::Arc;

use async_trait::async_trait;
use cdk::federation::{
    FederationBarkProjectionSnapshot, FederationBarkReceiveIntent, FederationJournalEntry,
    FederationOperationId,
};
use cdk::mint::{Mint, QuoteId};

use super::FederatedBarkNode;
use crate::{
    Error, FederatedBarkDegradedReason, FederatedBarkRecoveryDriver, FederatedBarkReplaySource,
    FederatedBarkRuntime, FederatedBarkRuntimeStatus,
};

/// Replay accepted journal entries into the Bark runtime.
pub async fn reconcile_federation_bark_runtime(
    mint: &Mint,
    runtime: &Arc<FederatedBarkRuntime>,
    entries: &[FederationJournalEntry],
) -> Result<(), cdk::Error> {
    FederatedBarkRecoveryDriver::new(runtime.clone())
        .reconcile(&MintReplaySource { mint }, entries)
        .await
        .map_err(|error| {
            error
                .into_federation_error("replay accepted state", None)
                .into()
        })
}

/// Accepted Bark projection after the runtime has replayed up to its frontier.
pub async fn federation_bark_ready_projection(
    mint: &Mint,
    runtime: &Arc<FederatedBarkRuntime>,
) -> Result<FederationBarkProjectionSnapshot, cdk::Error> {
    let mut projection = mint.federation_bark_projection_snapshot().await?;
    let required_frontier = projection.frontier().unwrap_or(0);
    if runtime.ensure_ready(required_frontier).await.is_err() {
        let entries = mint.localstore().get_federation_journal_entries().await?;
        Box::pin(reconcile_federation_bark_runtime(mint, runtime, &entries)).await?;
        projection = mint.federation_bark_projection_snapshot().await?;
    }
    runtime
        .ensure_ready(projection.frontier().unwrap_or(0))
        .await
        .map_err(|error| error.into_federation_error("verify projection readiness", None))?;
    Ok(projection)
}

/// Return the accepted projection for an ordinary ready runtime or an exact
/// operator/maintenance recovery flow.
///
/// Recovery remains fail-closed unless the member has replayed the complete
/// accepted frontier and its only degradation is a recovery condition.
pub async fn federation_bark_recovery_projection(
    state: &dyn FederatedBarkNode,
    runtime: &Arc<FederatedBarkRuntime>,
) -> Result<FederationBarkProjectionSnapshot, cdk::Error> {
    match federation_bark_ready_projection(state.mint(), runtime).await {
        Ok(projection) => Ok(projection),
        Err(error) => {
            let projection = state.mint().federation_bark_projection_snapshot().await?;
            let required_frontier = projection.frontier().unwrap_or(0);
            let status = runtime.status(required_frontier).await;
            if runtime_allows_recovery(status, required_frontier) {
                Ok(projection)
            } else {
                Err(projection_error(error))
            }
        }
    }
}

fn runtime_allows_recovery(status: FederatedBarkRuntimeStatus, required_frontier: u64) -> bool {
    status.local_frontier >= required_frontier
        && matches!(
            status.degraded_reason,
            Some(
                FederatedBarkDegradedReason::RecoveryRequired
                    | FederatedBarkDegradedReason::RecoveryFeeReserveInsufficient
            )
        )
}

/// Keep projection failures ambiguous while retaining a redacted local diagnostic.
pub(super) fn projection_error(error: impl std::fmt::Display) -> cdk::Error {
    cdk::federation::FederationError::BackendFailure {
        backend: "Bark",
        phase: "load ready projection",
        operation_id: None,
        reason: cdk::federation::redact_federation_operator_message(&error.to_string()),
    }
    .into()
}

struct MintReplaySource<'a> {
    mint: &'a Mint,
}

#[async_trait]
impl FederatedBarkReplaySource for MintReplaySource<'_> {
    async fn replay_projection(&self) -> Result<FederationBarkProjectionSnapshot, Error> {
        self.mint
            .federation_bark_projection_snapshot()
            .await
            .map_err(|_| Error::ReceiveProjectionFailed)
    }

    async fn replay_entry(
        &self,
        operation_id: FederationOperationId,
    ) -> Result<FederationJournalEntry, Error> {
        self.mint
            .localstore()
            .get_federation_journal_entry_by_operation_id(operation_id)
            .await
            .map_err(|_| Error::MaintenanceReconciliationFailed)?
            .ok_or(Error::MaintenanceReconciliationFailed)
    }

    async fn replay_receive_intent(
        &self,
        operation_id: FederationOperationId,
    ) -> Result<FederationBarkReceiveIntent, Error> {
        self.mint
            .federation_bark_receive_intent(operation_id)
            .await
            .map_err(|_| Error::ReceiveProjectionFailed)?
            .ok_or(Error::ReceiveProjectionFailed)
    }

    async fn replay_melt_quote(&self, quote: &str) -> Result<cdk_common::mint::MeltQuote, Error> {
        let quote = quote
            .parse::<QuoteId>()
            .map_err(|_| Error::SendReconciliationFailed)?;
        self.mint
            .localstore()
            .get_melt_quote(&quote)
            .await
            .map_err(|_| Error::SendReconciliationFailed)?
            .ok_or(Error::SendReconciliationFailed)
    }
}

#[cfg(test)]
mod tests {
    use super::runtime_allows_recovery;
    use crate::{FederatedBarkDegradedReason, FederatedBarkReadiness, FederatedBarkRuntimeStatus};

    #[test]
    fn recovery_projection_requires_caught_up_recovery_degradation() {
        let recovery = FederatedBarkRuntimeStatus {
            readiness: FederatedBarkReadiness::Degraded,
            local_frontier: 42,
            degraded_reason: Some(FederatedBarkDegradedReason::RecoveryRequired),
        };
        assert!(runtime_allows_recovery(recovery, 42));
        assert!(!runtime_allows_recovery(recovery, 43));
        for reason in [
            None,
            Some(FederatedBarkDegradedReason::WalletUnavailable),
            Some(FederatedBarkDegradedReason::ProjectionInvalid),
            Some(FederatedBarkDegradedReason::ParticipantsUnavailable),
        ] {
            assert!(!runtime_allows_recovery(
                FederatedBarkRuntimeStatus {
                    degraded_reason: reason,
                    ..recovery
                },
                42,
            ));
        }
        assert!(runtime_allows_recovery(
            FederatedBarkRuntimeStatus {
                degraded_reason: Some(FederatedBarkDegradedReason::RecoveryFeeReserveInsufficient,),
                ..recovery
            },
            42,
        ));
    }
}
