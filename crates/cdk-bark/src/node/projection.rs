//! Accepted Bark projection as the drivers see it: replayed into the runtime
//! and checked for readiness.

use std::sync::Arc;

use async_trait::async_trait;
use cdk::federation::{
    FederationBarkProjectionSnapshot, FederationBarkReceiveIntent, FederationJournalEntry,
    FederationOperationId, FederationOperationStatus,
};
use cdk::mint::{Mint, QuoteId};
use cdk_common::database::mint::{FederationIndexRange, FederationJournalEntryKey};

use super::FederatedBarkNode;
use crate::{
    Error, FederatedBarkDegradedReason, FederatedBarkRecoveryDriver, FederatedBarkReplaySource,
    FederatedBarkRuntime, FederatedBarkRuntimeStatus,
};

const REPLAY_PRIMARY_NAMESPACE: &str = "cdk_bark";
const REPLAY_SECONDARY_NAMESPACE: &str = "replay_next_index";

/// Replay the accepted journal entries this member has not yet replayed into
/// its Bark runtime.
///
/// A durable marker per Bark scope records the first journal index not yet
/// replayed, so a call reads only the entries after it rather than the whole
/// journal. The marker moves past every finished entry once replay succeeds,
/// and stops at the first entry still awaiting application. A failed replay
/// leaves it unchanged, so the same entries are replayed again; replay is
/// idempotent against the member's durable Bark wallet.
pub async fn reconcile_federation_bark_runtime(
    mint: &Mint,
    runtime: &Arc<FederatedBarkRuntime>,
) -> Result<(), cdk::Error> {
    let marker_key = runtime.scope().storage_key();
    let next_index = read_replay_marker(mint, &marker_key).await?;
    let entries = mint
        .localstore()
        .get_federation_journal_entries(FederationIndexRange::starting_at(next_index))
        .await?;
    FederatedBarkRecoveryDriver::new(runtime.clone())
        .reconcile(&MintReplaySource { mint }, &entries)
        .await
        .map_err(|error| -> cdk::Error {
            error
                .into_federation_error("replay accepted state", None)
                .into()
        })?;
    let replayed_until = entries
        .iter()
        .find(|entry| entry.status() == FederationOperationStatus::Accepted)
        .map(|entry| entry.index)
        .or_else(|| entries.last().map(|entry| entry.index.saturating_add(1)))
        .unwrap_or(next_index);
    advance_replay_marker(mint, &marker_key, replayed_until).await
}

async fn read_replay_marker(mint: &Mint, marker_key: &str) -> Result<u64, cdk::Error> {
    mint.localstore()
        .kv_read(
            REPLAY_PRIMARY_NAMESPACE,
            REPLAY_SECONDARY_NAMESPACE,
            marker_key,
        )
        .await?
        .map(|bytes| decode_replay_marker(&bytes))
        .transpose()
        .map(Option::unwrap_or_default)
}

/// Move the marker forward, never back, so concurrent replays cannot lose
/// each other's progress.
async fn advance_replay_marker(
    mint: &Mint,
    marker_key: &str,
    replayed_until: u64,
) -> Result<(), cdk::Error> {
    let mut transaction = mint.localstore().begin_transaction().await?;
    let current = transaction
        .kv_read(
            REPLAY_PRIMARY_NAMESPACE,
            REPLAY_SECONDARY_NAMESPACE,
            marker_key,
        )
        .await?
        .map(|bytes| decode_replay_marker(&bytes))
        .transpose()?
        .unwrap_or_default();
    if replayed_until > current {
        transaction
            .kv_write(
                REPLAY_PRIMARY_NAMESPACE,
                REPLAY_SECONDARY_NAMESPACE,
                marker_key,
                &replayed_until.to_be_bytes(),
            )
            .await?;
    }
    transaction.commit().await?;
    Ok(())
}

fn decode_replay_marker(bytes: &[u8]) -> Result<u64, cdk::Error> {
    let bytes: [u8; 8] = bytes.try_into().map_err(|_| -> cdk::Error {
        cdk::federation::FederationError::StateRowsInvalid {
            reason: "Bark replay marker is not an eight-byte index",
        }
        .into()
    })?;
    Ok(u64::from_be_bytes(bytes))
}

/// Accepted Bark projection after the runtime has replayed up to its frontier.
pub async fn federation_bark_ready_projection(
    mint: &Mint,
    runtime: &Arc<FederatedBarkRuntime>,
) -> Result<FederationBarkProjectionSnapshot, cdk::Error> {
    let mut projection = mint.federation_bark_projection_snapshot().await?;
    let required_frontier = projection.frontier().unwrap_or(0);
    if runtime.ensure_ready(required_frontier).await.is_err() {
        Box::pin(reconcile_federation_bark_runtime(mint, runtime)).await?;
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
            .get_federation_journal_entry(FederationJournalEntryKey::Operation(operation_id), None)
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
