#[cfg(feature = "prometheus")]
use std::collections::BTreeMap;

use axum::extract::{Json, State};
use axum::response::Response;
use cdk::federation::{
    FederationAlephBftBackupStatus, FederationCheckpointKind, FederationCheckpointQuorumSummary,
    FederationCheckpointStatus, FederationCheckpointSummary, FederationConsensusRuntimeStatus,
    FederationInvalidOperationAlertSummary, FederationLocalStatusResponse,
    FederationMaterializedStateRowFingerprint, FederationPaymentObservationReportSummary,
    FederationPeerStatus, FederationPeerStatusRequest, FederationRestMessageKind,
    FederationSignedRequest, FederationWalletRecoveryStatus, FEDERATION_REST_API_VERSION,
};
use tracing::instrument;

use crate::MintState;

use super::{
    acquire_private_rest_admission, anyhow_response, cdk_error_response, local_peer_status,
    local_peer_status_with_materialized_rows, payment_observation_report_summaries,
    verify_federation_request,
};

#[instrument(skip_all)]
pub(super) async fn post_status(
    State(state): State<MintState>,
    Json(payload): Json<FederationSignedRequest<FederationPeerStatusRequest>>,
) -> Result<Json<FederationPeerStatus>, Response> {
    let _permit = acquire_private_rest_admission(&state, FederationRestMessageKind::Status)?;
    let verified =
        verify_federation_request(&state, &payload, FederationRestMessageKind::Status).await?;
    state
        .record_federation_sequence(
            FederationRestMessageKind::Status,
            verified.sender,
            verified.sequence,
        )
        .await;
    let peer_status = local_peer_status(&state).await.map_err(anyhow_response)?;
    Ok(Json(peer_status))
}

#[instrument(skip_all)]
pub(super) async fn post_local_status(
    State(state): State<MintState>,
    Json(payload): Json<FederationSignedRequest<FederationPeerStatusRequest>>,
) -> Result<Json<FederationLocalStatusResponse>, Response> {
    let _permit = acquire_private_rest_admission(&state, FederationRestMessageKind::Status)?;
    let verified =
        verify_federation_request(&state, &payload, FederationRestMessageKind::Status).await?;
    state
        .record_federation_sequence(
            FederationRestMessageKind::Status,
            verified.sender,
            verified.sequence,
        )
        .await;
    let (peer_status, materialized_state_rows) = if payload.payload.include_materialized_rows {
        local_peer_status_with_materialized_rows(&state)
            .await
            .map_err(anyhow_response)?
    } else {
        (
            local_peer_status(&state).await.map_err(anyhow_response)?,
            Vec::new(),
        )
    };
    let sync_status = state.federation_sync_status().await.unwrap_or_default();
    let checkpoint = federation_checkpoint_status(&state)
        .await
        .map_err(cdk_error_response)?;
    let payment_observation_reports = payment_observation_report_summaries(&state)
        .await
        .map_err(cdk_error_response)?;
    let pending_mempool_items = state.federation_pending_mempool_len().await;
    let notification_stats = state.federation_operation_notification_stats();
    let aleph_bft_status = state.federation_aleph_bft_status().await;
    let aleph_bft_live_session_active = state.federation_aleph_bft_live_session_active().await;
    let aleph_bft_network_inbound_active =
        state.federation_aleph_bft_network_inbound_active().await;
    let invalid_operation_alerts = state.federation_invalid_operation_alert_summaries().await;
    let onchain_wallets = state.federation_onchain_wallet_statuses().await;
    let wallet_recovery = federation_wallet_recovery_status(&state)
        .await
        .map_err(cdk_error_response)?;
    let wallet_liveness = state
        .mint
        .federation_config()
        .and_then(|runtime| runtime.public_config().frost_wallet.as_ref())
        .is_some()
        .then(|| state.federation.wallet_liveness.clone());
    let wallet_liveness = match wallet_liveness {
        Some(worker) => Some(worker.status().await),
        None => None,
    };
    let wallet_operator = federation_wallet_operator_status(&state, wallet_liveness.as_ref())
        .await
        .map_err(cdk_error_response)?;
    let response = federation_local_status_response(
        peer_status,
        sync_status,
        checkpoint,
        payment_observation_reports,
        materialized_state_rows,
        usize_to_u64_saturating(pending_mempool_items),
        usize_to_u64_saturating(notification_stats.pending_response_waiters),
        usize_to_u64_saturating(notification_stats.response_cache_items),
        notification_stats.response_cache_lookups,
        notification_stats.response_cache_hits,
        notification_stats.duplicate_submissions,
        notification_stats.duplicate_pending_waiter_joins,
        aleph_bft_status,
        aleph_bft_live_session_active,
        aleph_bft_network_inbound_active,
        invalid_operation_alerts,
        onchain_wallets,
        wallet_recovery,
        wallet_liveness,
        wallet_operator,
    );
    #[cfg(feature = "prometheus")]
    record_federation_local_status_metrics(&response);
    Ok(Json(response))
}

fn federation_local_status_response(
    peer_status: FederationPeerStatus,
    sync_status: crate::FederationSyncStatus,
    checkpoint: FederationCheckpointStatus,
    payment_observation_reports: Vec<FederationPaymentObservationReportSummary>,
    materialized_state_rows: Vec<FederationMaterializedStateRowFingerprint>,
    pending_mempool_items: u64,
    pending_response_waiters: u64,
    response_cache_items: u64,
    response_cache_lookups: u64,
    response_cache_hits: u64,
    duplicate_submissions: u64,
    duplicate_pending_waiter_joins: u64,
    aleph_bft_status: Option<FederationConsensusRuntimeStatus>,
    aleph_bft_live_session_active: bool,
    aleph_bft_network_inbound_active: bool,
    invalid_operation_alerts: Vec<FederationInvalidOperationAlertSummary>,
    onchain_wallets: Vec<cdk::federation::FederationOnchainWalletStatus>,
    wallet_recovery: Option<FederationWalletRecoveryStatus>,
    wallet_liveness: Option<cdk::federation::FederationWalletLivenessStatus>,
    wallet_operator: Option<cdk::federation::FederationWalletOperatorStatus>,
) -> FederationLocalStatusResponse {
    FederationLocalStatusResponse {
        version: FEDERATION_REST_API_VERSION,
        peer_status,
        sync_state: sync_status.state_name().to_string(),
        sync_ready: sync_status.is_ready(),
        sync_message: sync_status.not_ready_message(),
        sync_peer: sync_status.peer(),
        sync_local_operation_count: sync_status.local_operation_count(),
        sync_peer_operation_count: sync_status.peer_operation_count(),
        sync_operation_count: sync_status.operation_count(),
        pending_mempool_items,
        pending_response_waiters,
        response_cache_items,
        response_cache_lookups,
        response_cache_hits,
        response_cache_hit_rate_bps: response_cache_hit_rate_bps(
            response_cache_hits,
            response_cache_lookups,
        ),
        duplicate_submissions,
        duplicate_pending_waiter_joins,
        checkpoint,
        payment_observation_reports,
        aleph_bft_status,
        aleph_bft_live_session_active,
        aleph_bft_network_inbound_active,
        materialized_state_rows,
        invalid_operation_alerts,
        onchain_wallets,
        wallet_recovery,
        wallet_liveness,
        wallet_operator,
    }
}

async fn federation_checkpoint_status(
    state: &MintState,
) -> Result<FederationCheckpointStatus, cdk::Error> {
    let db = state.mint.localstore();
    let frontier = db.get_federation_checkpoint_frontier().await?;
    let latest_scheduled_checkpoint = db
        .get_latest_federation_checkpoint(FederationCheckpointKind::Scheduled)
        .await?
        .as_ref()
        .map(FederationCheckpointSummary::from_checkpoint)
        .transpose()?;
    let latest_scheduled_quorum = db
        .get_latest_federation_checkpoint_quorum(FederationCheckpointKind::Scheduled)
        .await?
        .as_ref()
        .map(FederationCheckpointQuorumSummary::from_stored_quorum)
        .transpose()?;
    let latest_recovery_quorum = db
        .get_latest_federation_checkpoint_quorum(FederationCheckpointKind::Recovery)
        .await?
        .as_ref()
        .map(FederationCheckpointQuorumSummary::from_stored_quorum)
        .transpose()?;
    let trusted_checkpoint = db
        .get_trusted_federation_checkpoint_quorum()
        .await?
        .as_ref()
        .map(FederationCheckpointQuorumSummary::from_stored_quorum)
        .transpose()?;
    let trusted_journal_next_index = trusted_checkpoint
        .as_ref()
        .map_or(0, |trusted| trusted.checkpoint.frontier.journal.next_index);
    let active_session_backup = match db.get_federation_aleph_bft_active_session().await? {
        Some(session) => {
            let metadata = db.get_federation_aleph_bft_backup_metadata(session).await?;
            Some(FederationAlephBftBackupStatus {
                session,
                chunk_count: metadata.chunk_count,
                total_bytes: metadata.total_bytes,
            })
        }
        None => None,
    };

    Ok(FederationCheckpointStatus {
        lag_from_trusted_checkpoint: frontier
            .journal_next_index
            .saturating_sub(trusted_journal_next_index),
        frontier,
        latest_scheduled_checkpoint,
        latest_scheduled_quorum,
        latest_recovery_quorum,
        trusted_checkpoint,
        retained_recovery_floor: Default::default(),
        active_session_backup,
    })
}

async fn federation_wallet_operator_status(
    state: &MintState,
    liveness: Option<&cdk::federation::FederationWalletLivenessStatus>,
) -> Result<Option<cdk::federation::FederationWalletOperatorStatus>, cdk::Error> {
    let Some(runtime) = state.mint.effective_federation_config().await? else {
        return Ok(None);
    };
    let Some(wallet) = runtime.public_config().frost_wallet.as_ref() else {
        return Ok(None);
    };
    let addresses = state.mint.federation_address_allocation_state().await?;
    let chain = state.mint.federation_chain_observation_state().await?;
    let transactions = state.mint.federation_transaction_state().await?;
    let default_liveness = cdk::federation::FederationWalletLivenessStatus::default();
    Ok(Some(
        cdk::federation::FederationWalletOperatorStatus::from_states(
            wallet,
            &addresses,
            &chain,
            &transactions,
            liveness.unwrap_or(&default_liveness),
        )?,
    ))
}

async fn federation_wallet_recovery_status(
    state: &MintState,
) -> Result<Option<FederationWalletRecoveryStatus>, cdk::Error> {
    let Some(runtime) = state.mint.federation_config() else {
        return Ok(None);
    };
    if runtime.public_config().frost_wallet.is_none() {
        return Ok(None);
    }
    let addresses = state.mint.federation_address_allocation_state().await?;
    let chain = state.mint.federation_chain_observation_state().await?;
    let transactions = state.mint.federation_transaction_state().await?;
    Ok(Some(FederationWalletRecoveryStatus::from_states(
        &addresses,
        &chain,
        &transactions,
    )?))
}

fn response_cache_hit_rate_bps(hits: u64, lookups: u64) -> u64 {
    if lookups == 0 {
        return 0;
    }

    let bounded_hits = hits.min(lookups);
    let rate = u128::from(bounded_hits) * 10_000 / u128::from(lookups);
    match u64::try_from(rate) {
        Ok(rate) => rate,
        Err(_) => 10_000,
    }
}

#[cfg(feature = "prometheus")]
fn record_federation_local_status_metrics(response: &FederationLocalStatusResponse) {
    const SYNC_STATES: &[&str] = &[
        "healthy",
        "awaiting_quorum",
        "lagging",
        "catching_up",
        "halted",
    ];
    const PAYMENT_REPORT_STATUSES: &[&str] =
        &["empty", "awaiting_quorum", "quorum_reached", "conflicting"];
    const PAYMENT_REPORT_ACTIONS: &[&str] = &["none", "inspect"];
    const BROADCAST_HEALTH: &[&str] = &[
        "idle",
        "awaiting_authorization",
        "broadcasting",
        "confirming",
        "degraded",
    ];

    let member = response.peer_status.member_id.to_string();
    cdk_prometheus::METRICS.set_federation_consensus_operation_count(
        &member,
        super::u64_to_i64_saturating(response.peer_status.operation_count),
    );
    cdk_prometheus::METRICS.set_federation_pending_mempool_items(
        &member,
        super::u64_to_i64_saturating(response.pending_mempool_items),
    );
    cdk_prometheus::METRICS.set_federation_sync_ready(&member, response.sync_ready);
    for state in SYNC_STATES {
        cdk_prometheus::METRICS.set_federation_sync_state(
            &member,
            state,
            *state == response.sync_state,
        );
    }
    cdk_prometheus::METRICS.set_federation_checkpoint_lag_operations(
        &member,
        super::u64_to_i64_saturating(response.checkpoint.lag_from_trusted_checkpoint),
    );
    for status in PAYMENT_REPORT_STATUSES {
        for action in PAYMENT_REPORT_ACTIONS {
            cdk_prometheus::METRICS
                .set_federation_payment_observation_report_count(&member, status, action, 0);
        }
    }

    let mut report_counts = BTreeMap::<(String, String), i64>::new();
    for report in &response.payment_observation_reports {
        let status = report.status.to_string();
        let action = report.recommended_action.to_string();
        match report_counts.get_mut(&(status.clone(), action.clone())) {
            Some(count) => *count += 1,
            None => {
                report_counts.insert((status, action), 1);
            }
        }
    }
    for ((status, action), count) in report_counts {
        cdk_prometheus::METRICS
            .set_federation_payment_observation_report_count(&member, &status, &action, count);
    }

    if let Some(wallet) = &response.wallet_operator {
        for utxo in &wallet.utxos {
            cdk_prometheus::METRICS.set_federation_wallet_utxos(
                &member,
                &utxo.state.to_string(),
                super::u64_to_i64_saturating(utxo.count),
                super::u64_to_i64_saturating(utxo.value_sat),
            );
        }
        for phase in &wallet.operation_phases {
            cdk_prometheus::METRICS.set_federation_wallet_operation_phase(
                &member,
                &phase.phase.to_string(),
                super::u64_to_i64_saturating(phase.count),
            );
        }
        cdk_prometheus::METRICS.set_federation_wallet_revealed_count(
            &member,
            "external",
            super::u64_to_i64_saturating(wallet.external_revealed_count),
        );
        cdk_prometheus::METRICS.set_federation_wallet_revealed_count(
            &member,
            "internal",
            super::u64_to_i64_saturating(wallet.internal_revealed_count),
        );
        cdk_prometheus::METRICS.set_federation_wallet_liveness(
            &member,
            super::u64_to_i64_saturating(wallet.pending_batches),
            super::u64_to_i64_saturating(wallet.signing_attempts),
            super::u64_to_i64_saturating(wallet.signing_failures),
        );
        for health in BROADCAST_HEALTH {
            cdk_prometheus::METRICS.set_federation_wallet_broadcast_health(
                &member,
                health,
                *health == wallet.broadcast_health.to_string(),
            );
        }
        cdk_prometheus::METRICS.set_federation_wallet_broadcast_failures(
            &member,
            super::u64_to_i64_saturating(wallet.broadcast_failures),
        );
    }
}

fn usize_to_u64_saturating(value: usize) -> u64 {
    u64::try_from(value).unwrap_or(u64::MAX)
}

#[cfg(test)]
mod tests {
    use super::response_cache_hit_rate_bps;

    #[test]
    fn response_cache_hit_rate_handles_zero_and_fractional_lookups() {
        assert_eq!(response_cache_hit_rate_bps(0, 0), 0);
        assert_eq!(response_cache_hit_rate_bps(0, 7), 0);
        assert_eq!(response_cache_hit_rate_bps(3, 7), 4285);
        assert_eq!(response_cache_hit_rate_bps(7, 7), 10_000);
        assert_eq!(response_cache_hit_rate_bps(9, 7), 10_000);
    }
}
