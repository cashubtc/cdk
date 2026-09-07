//! HTTP integration port for backend-owned Bark maintenance.

use std::collections::{BTreeMap, BTreeSet};
use std::str::FromStr;
use std::sync::Arc;

use async_trait::async_trait;
use cdk::federation::{
    FederationAddressAllocation, FederationAddressAllocationScope, FederationAddressKeychain,
    FederationBarkActionId, FederationBarkActionKind, FederationBarkEvidencePhase,
    FederationBarkObservationStatus, FederationBarkOperation, FederationBarkProjectionSnapshot,
    FederationBarkSemanticPhase, FederationChainObservationState, FederationJournalEntry,
    FederationOperation, FederationOperationId, FederationOperationStatus,
};
use cdk::mint::QuoteId;
use cdk::nuts::MeltQuoteState;

use crate::MintState;

const BARK_ROUTINE_MAINTENANCE_PAYMENT_QUIET_PERIOD: std::time::Duration =
    std::time::Duration::from_secs(30);
const BARK_RECENT_PAYMENT_JOURNAL_LIMIT: usize = 256;

pub(crate) fn spawn(state: &MintState) {
    let Some(runtime) = state.federation.bark_runtime().cloned() else {
        return;
    };
    let tasks = state.federation.tasks.clone();
    let stop = tasks.stop_token();
    let host = Arc::new(AxumBarkRecoveryHost {
        state: state.clone(),
    });
    tasks.spawn(
        "Bark recovery",
        true,
        cdk_bark::FederatedBarkRecoveryDriver::new(runtime).run_until(host, stop.cancelled_owned()),
    );
}

/// Retry at most one consensus-visible step for one exact accepted action.
pub(super) async fn retry_federated_bark_maintenance_action(
    state: MintState,
    action_id: FederationBarkActionId,
) -> Result<usize, cdk::Error> {
    advance_federated_bark_maintenance_inner(state, Some(action_id)).await
}

async fn advance_federated_bark_maintenance_inner(
    state: MintState,
    target_action_id: Option<FederationBarkActionId>,
) -> Result<usize, cdk::Error> {
    let runtime = match state.federation.bark_runtime() {
        Some(runtime) => runtime.clone(),
        None => return Ok(0),
    };
    cdk_bark::FederatedBarkRecoveryDriver::new(runtime)
        .advance(&AxumBarkRecoveryHost { state }, target_action_id)
        .await
        .map_err(|_| cdk::Error::Internal)
}

#[derive(Clone)]
struct AxumBarkRecoveryHost {
    state: MintState,
}

#[async_trait]
impl cdk_bark::FederatedBarkRecoveryHost for AxumBarkRecoveryHost {
    type Error = cdk::Error;

    async fn recovery_projection(
        &self,
        runtime: &Arc<cdk_bark::FederatedBarkRuntime>,
    ) -> Result<FederationBarkProjectionSnapshot, Self::Error> {
        recovery_projection(&self.state, runtime).await
    }

    async fn routine_payment_work_blocks(
        &self,
        projection: &FederationBarkProjectionSnapshot,
    ) -> Result<bool, Self::Error> {
        routine_payment_work_blocks_maintenance(&self.state, projection).await
    }

    async fn journal_entry(
        &self,
        operation_id: FederationOperationId,
    ) -> Result<Option<FederationJournalEntry>, Self::Error> {
        self.state
            .mint
            .localstore()
            .get_federation_journal_entry_by_operation_id(operation_id)
            .await
            .map_err(Into::into)
    }

    async fn operation_status(
        &self,
        operation: &FederationBarkOperation,
    ) -> Result<Option<FederationOperationStatus>, Self::Error> {
        let federation = self
            .state
            .mint
            .federation_config()
            .ok_or(cdk::Error::Internal)?;
        let envelope =
            federation.operation_envelope(super::bark_federation_operation(operation.clone())?);
        let operation_id = envelope.operation_id()?;
        Ok(self
            .state
            .mint
            .localstore()
            .get_federation_journal_entry_by_operation_id(operation_id)
            .await?
            .map(|entry| entry.status()))
    }

    async fn submit_operation(
        &self,
        operation: FederationBarkOperation,
    ) -> Result<usize, Self::Error> {
        let federation = self
            .state
            .mint
            .federation_config()
            .ok_or(cdk::Error::Internal)?;
        let envelope = federation.operation_envelope(super::bark_federation_operation(operation)?);
        self.state
            .clone()
            .submit_federation_envelope(envelope)
            .await?
            .ensure_accepted()?;
        Ok(1)
    }

    async fn address_allocation(
        &self,
        scope: FederationAddressAllocationScope,
        quote: &str,
    ) -> Result<Option<FederationAddressAllocation>, Self::Error> {
        self.state
            .mint
            .federation_address_allocation(scope, quote)
            .await
    }

    async fn submit_address_allocation(
        &self,
        scope: FederationAddressAllocationScope,
        quote: String,
        keychain: FederationAddressKeychain,
    ) -> Result<usize, Self::Error> {
        let federation = self
            .state
            .mint
            .federation_config()
            .ok_or(cdk::Error::Internal)?;
        let envelope = federation.address_allocation_operation(
            &quote,
            scope.application_id,
            scope.application_key_epoch,
            keychain,
        )?;
        self.state
            .clone()
            .submit_federation_envelope(envelope)
            .await?
            .ensure_accepted()?;
        Ok(1)
    }

    async fn chain_observation_state(
        &self,
    ) -> Result<FederationChainObservationState, Self::Error> {
        self.state.mint.federation_chain_observation_state().await
    }
}

/// Return the accepted projection for an ordinary ready runtime or an exact
/// operator/maintenance recovery flow.
///
/// Recovery remains fail-closed unless the member has replayed the complete
/// accepted frontier and its only degradation is a recovery condition.
pub(super) async fn recovery_projection(
    state: &MintState,
    runtime: &Arc<cdk_bark::FederatedBarkRuntime>,
) -> Result<FederationBarkProjectionSnapshot, cdk::Error> {
    match crate::federation_bark_ready_projection(&state.mint, runtime).await {
        Ok(projection) => Ok(projection),
        Err(_) => {
            let projection = state.mint.federation_bark_projection_snapshot().await?;
            let required_frontier = projection.frontier().unwrap_or(0);
            let status = runtime.status(required_frontier).await;
            if runtime_allows_recovery(status, required_frontier) {
                Ok(projection)
            } else {
                Err(cdk::Error::Internal)
            }
        }
    }
}

fn runtime_allows_recovery(
    status: cdk_bark::FederatedBarkRuntimeStatus,
    required_frontier: u64,
) -> bool {
    status.local_frontier >= required_frontier
        && matches!(
            status.degraded_reason,
            Some(
                cdk_bark::FederatedBarkDegradedReason::RecoveryRequired
                    | cdk_bark::FederatedBarkDegradedReason::RecoveryFeeReserveInsufficient
            )
        )
}

async fn routine_payment_work_blocks_maintenance(
    state: &MintState,
    projection: &FederationBarkProjectionSnapshot,
) -> Result<bool, cdk::Error> {
    if projection
        .actions
        .iter()
        .any(|action| payment_action_blocks_routine_maintenance(projection, action))
    {
        return Ok(true);
    }

    let now = current_unix_time_seconds();
    if recent_receive_accounting_blocks_routine_maintenance(state, projection, now).await? {
        return Ok(true);
    }

    recent_bolt11_melt_blocks_routine_maintenance(state, now).await
}

async fn recent_receive_accounting_blocks_routine_maintenance(
    state: &MintState,
    projection: &FederationBarkProjectionSnapshot,
    now: u64,
) -> Result<bool, cdk::Error> {
    let store = state.mint.localstore();
    let Some(tip) = store.get_federation_journal_tip().await? else {
        return Ok(false);
    };
    let journal_floor = recent_payment_journal_floor(tip.index);
    let quote_ids = projection
        .actions
        .iter()
        .filter(|action| {
            action.action_kind == Some(FederationBarkActionKind::IncomingClaim)
                && action.latest_operation_index >= journal_floor
        })
        .filter_map(|action| action.quote.as_deref())
        .map(QuoteId::from_str)
        .collect::<Result<BTreeSet<_>, _>>()
        .map_err(|_| cdk::Error::Internal)?
        .into_iter()
        .collect::<Vec<_>>();
    if quote_ids.is_empty() {
        return Ok(false);
    }

    let quotes = store.get_mint_quotes_by_ids(&quote_ids).await?;
    Ok(quotes
        .into_iter()
        .flatten()
        .any(|quote| payment_timestamp_is_quiet(quote.updated_at(), now)))
}

async fn recent_bolt11_melt_blocks_routine_maintenance(
    state: &MintState,
    now: u64,
) -> Result<bool, cdk::Error> {
    let store = state.mint.localstore();
    let Some(tip) = store.get_federation_journal_tip().await? else {
        return Ok(false);
    };
    let next_index = recent_payment_journal_floor(tip.index);
    let entries = store
        .get_federation_journal_entries_range(
            next_index,
            tip.index.saturating_add(1),
            BARK_RECENT_PAYMENT_JOURNAL_LIMIT,
        )
        .await?;
    let recent_quotes = entries
        .iter()
        .filter_map(|entry| match &entry.envelope.operation {
            FederationOperation::MeltQuote(operation)
                if matches!(operation.request, cdk::MeltQuoteRequest::Bolt11(_)) =>
            {
                Some((operation.quote.as_str(), operation.metadata.created_time))
            }
            _ => None,
        })
        .map(|(quote, created_time)| {
            QuoteId::from_str(quote)
                .map(|quote| (quote, created_time))
                .map_err(|_| cdk::Error::Internal)
        })
        .collect::<Result<BTreeMap<_, _>, _>>()?;
    if recent_quotes
        .values()
        .any(|created_time| payment_timestamp_is_quiet(*created_time, now))
    {
        return Ok(true);
    }
    if recent_quotes.is_empty() {
        return Ok(false);
    }

    let quote_ids = recent_quotes.keys().cloned().collect::<Vec<_>>();
    let quotes = store.get_federation_melt_quotes(&quote_ids).await?;
    Ok(quotes
        .iter()
        .any(|quote| melt_quote_blocks_routine_maintenance(quote.state, quote.paid_time, now)))
}

fn melt_quote_blocks_routine_maintenance(
    state: MeltQuoteState,
    paid_time: Option<u64>,
    now: u64,
) -> bool {
    state == MeltQuoteState::Pending
        || (state == MeltQuoteState::Paid
            && paid_time.is_some_and(|paid_time| payment_timestamp_is_quiet(paid_time, now)))
}

fn payment_action_blocks_routine_maintenance(
    projection: &FederationBarkProjectionSnapshot,
    action: &cdk::federation::FederationBarkActionRecord,
) -> bool {
    if action.quote.is_none() {
        return false;
    }
    let expires_at = match action.phase {
        FederationBarkSemanticPhase::ReceiveIntent
        | FederationBarkSemanticPhase::ReceivePrepared => match &action.operation {
            FederationBarkOperation::ReceiveIntent(intent) => Some(intent.expires_at),
            FederationBarkOperation::ReceivePrepared(prepared) => Some(prepared.expires_at),
            _ => None,
        },
        _ => None,
    };
    let accepted_evidence = projection.observations.iter().any(|record| {
        matches!(
            &record.operation,
            FederationBarkOperation::MemberObservation(observation)
                if observation.action_id == action.action_id
                    && observation.status == FederationBarkObservationStatus::Accepted
        )
    });
    let claimed_result_certified = projection.observations.iter().any(|record| {
        matches!(
            &record.operation,
            FederationBarkOperation::EvidenceCertificate(certificate)
                if certificate.binding.action_id == action.action_id
                    && certificate.binding.phase == FederationBarkEvidencePhase::ReceiveClaimed
        )
    });
    payment_state_blocks_routine_maintenance(
        action.phase,
        expires_at,
        accepted_evidence,
        claimed_result_certified,
        current_unix_time_seconds(),
    )
}

fn payment_state_blocks_routine_maintenance(
    phase: FederationBarkSemanticPhase,
    expires_at: Option<u64>,
    accepted_evidence_present: bool,
    claimed_result_certified: bool,
    now: u64,
) -> bool {
    if phase == FederationBarkSemanticPhase::ReceiveClaimed {
        return !claimed_result_certified;
    }
    if payment_phase_is_terminal(phase) {
        return false;
    }
    match phase {
        FederationBarkSemanticPhase::ReceiveIntent
        | FederationBarkSemanticPhase::ReceivePrepared => expires_at.is_none_or(|expires_at| {
            !super::bark_incoming::receive_is_automatically_terminal(
                expires_at,
                accepted_evidence_present,
                now,
            )
        }),
        _ => true,
    }
}

fn payment_phase_is_terminal(phase: FederationBarkSemanticPhase) -> bool {
    matches!(
        phase,
        FederationBarkSemanticPhase::PaymentSucceeded
            | FederationBarkSemanticPhase::RevocationCompleted
    )
}

fn payment_timestamp_is_quiet(timestamp: u64, now: u64) -> bool {
    timestamp <= now
        && now.saturating_sub(timestamp) < BARK_ROUTINE_MAINTENANCE_PAYMENT_QUIET_PERIOD.as_secs()
}

fn recent_payment_journal_floor(tip_index: u64) -> u64 {
    let journal_limit = u64::try_from(BARK_RECENT_PAYMENT_JOURNAL_LIMIT).unwrap_or(u64::MAX);
    tip_index.saturating_sub(journal_limit.saturating_sub(1))
}

fn current_unix_time_seconds() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use cdk::federation::FederationBarkSemanticPhase;
    use cdk::nuts::MeltQuoteState;
    use cdk_bark::{
        FederatedBarkDegradedReason, FederatedBarkReadiness, FederatedBarkRuntimeStatus,
    };

    use super::{
        melt_quote_blocks_routine_maintenance, payment_phase_is_terminal,
        payment_state_blocks_routine_maintenance, payment_timestamp_is_quiet,
        recent_payment_journal_floor, runtime_allows_recovery, BARK_RECENT_PAYMENT_JOURNAL_LIMIT,
        BARK_ROUTINE_MAINTENANCE_PAYMENT_QUIET_PERIOD,
    };

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

    #[test]
    fn payment_phases_gate_routine_maintenance() {
        assert!(payment_phase_is_terminal(
            FederationBarkSemanticPhase::PaymentSucceeded
        ));
        assert!(!payment_phase_is_terminal(
            FederationBarkSemanticPhase::PaymentFailed
        ));
        assert!(payment_state_blocks_routine_maintenance(
            FederationBarkSemanticPhase::ReceiveClaimed,
            None,
            true,
            false,
            1_000,
        ));
        assert!(!payment_state_blocks_routine_maintenance(
            FederationBarkSemanticPhase::ReceiveClaimed,
            None,
            true,
            true,
            1_000,
        ));
    }

    #[test]
    fn payment_quiet_period_and_melt_state_are_bounded() {
        let now = 1_000;
        let quiet = BARK_ROUTINE_MAINTENANCE_PAYMENT_QUIET_PERIOD.as_secs();
        assert!(payment_timestamp_is_quiet(now, now));
        assert!(!payment_timestamp_is_quiet(now - quiet, now));
        assert!(!payment_timestamp_is_quiet(now + 1, now));
        assert!(melt_quote_blocks_routine_maintenance(
            MeltQuoteState::Pending,
            None,
            now,
        ));
        assert!(melt_quote_blocks_routine_maintenance(
            MeltQuoteState::Paid,
            Some(now),
            now,
        ));
        assert!(!melt_quote_blocks_routine_maintenance(
            MeltQuoteState::Unpaid,
            Some(now),
            now,
        ));
    }

    #[test]
    fn recent_payment_scan_is_bounded_by_the_journal_tip() {
        let limit = u64::try_from(BARK_RECENT_PAYMENT_JOURNAL_LIMIT).expect("journal limit");
        assert_eq!(recent_payment_journal_floor(0), 0);
        assert_eq!(recent_payment_journal_floor(limit), 1);
        assert_eq!(recent_payment_journal_floor(u64::MAX), u64::MAX - limit + 1);
    }
}
