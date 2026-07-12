#[cfg(feature = "prometheus")]
use std::collections::BTreeMap;

use axum::extract::{Json, State};
use axum::response::Response;
use cdk::federation::{
    FederationAlephBftAdapterStatus, FederationMaterializedStateRowFingerprint,
    FederationPaymentObservationReportSummary, FederationPeerStatus, FederationPeerStatusRequest,
    FederationRestMessageKind, FederationSignedRequest, MemberId, FEDERATION_REST_API_VERSION,
};
use serde::{Deserialize, Serialize};
use tracing::instrument;

use crate::MintState;

use super::{
    acquire_private_rest_admission, anyhow_response, cdk_error_response, local_peer_order_status,
    local_peer_status, local_peer_status_with_materialized_rows,
    payment_observation_report_summaries, session_outcome_quorum_status, verify_federation_request,
    SessionOutcomeQuorumStatus,
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
        .record_federation_sequence(verified.sender, verified.sequence)
        .await;
    let peer_status = local_peer_order_status(&state)
        .await
        .map_err(anyhow_response)?;
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
        .record_federation_sequence(verified.sender, verified.sequence)
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
    let session_outcome_quorum = session_outcome_quorum_status(&state)
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
    let response = FederationLocalStatusResponse::new(
        peer_status,
        sync_status,
        session_outcome_quorum,
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
    );
    #[cfg(feature = "prometheus")]
    record_federation_local_status_metrics(&response);
    Ok(Json(response))
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct FederationLocalStatusResponse {
    pub(crate) version: u16,
    pub(crate) peer_status: FederationPeerStatus,
    pub(crate) sync_state: String,
    pub(crate) sync_ready: bool,
    pub(crate) sync_message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) sync_peer: Option<MemberId>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) sync_local_operation_count: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) sync_peer_operation_count: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) sync_operation_count: Option<u64>,
    pub(crate) pending_mempool_items: u64,
    pub(crate) pending_response_waiters: u64,
    pub(crate) response_cache_items: u64,
    pub(crate) response_cache_lookups: u64,
    pub(crate) response_cache_hits: u64,
    pub(crate) response_cache_hit_rate_bps: u64,
    pub(crate) duplicate_submissions: u64,
    pub(crate) duplicate_pending_waiter_joins: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) session_outcome_quorum_operation_count: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) session_outcome_quorum_from_index: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) session_outcome_quorum_latest_index: Option<u64>,
    pub(crate) session_outcome_quorum_signers: Vec<cdk::federation::MemberId>,
    pub(crate) payment_observation_reports: Vec<FederationPaymentObservationReportSummary>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) aleph_bft_status: Option<FederationAlephBftAdapterStatus>,
    pub(crate) aleph_bft_live_session_active: bool,
    pub(crate) aleph_bft_network_inbound_active: bool,
    pub(crate) materialized_state_rows: Vec<FederationMaterializedStateRowFingerprint>,
    pub(crate) invalid_operation_alerts: Vec<crate::FederationInvalidOperationAlertSummary>,
}

impl FederationLocalStatusResponse {
    fn new(
        peer_status: FederationPeerStatus,
        sync_status: crate::FederationSyncStatus,
        session_outcome_quorum: Option<SessionOutcomeQuorumStatus>,
        payment_observation_reports: Vec<FederationPaymentObservationReportSummary>,
        materialized_state_rows: Vec<FederationMaterializedStateRowFingerprint>,
        pending_mempool_items: u64,
        pending_response_waiters: u64,
        response_cache_items: u64,
        response_cache_lookups: u64,
        response_cache_hits: u64,
        duplicate_submissions: u64,
        duplicate_pending_waiter_joins: u64,
        aleph_bft_status: Option<FederationAlephBftAdapterStatus>,
        aleph_bft_live_session_active: bool,
        aleph_bft_network_inbound_active: bool,
        invalid_operation_alerts: Vec<crate::FederationInvalidOperationAlertSummary>,
    ) -> Self {
        let (
            session_outcome_quorum_operation_count,
            session_outcome_quorum_from_index,
            session_outcome_quorum_latest_index,
            session_outcome_quorum_signers,
        ) = match session_outcome_quorum {
            Some(quorum) => (
                Some(quorum.operation_count),
                Some(quorum.from_index),
                Some(quorum.latest_index),
                quorum.signers.into_iter().collect(),
            ),
            None => (None, None, None, Vec::new()),
        };
        Self {
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
            session_outcome_quorum_operation_count,
            session_outcome_quorum_from_index,
            session_outcome_quorum_latest_index,
            session_outcome_quorum_signers,
            payment_observation_reports,
            aleph_bft_status,
            aleph_bft_live_session_active,
            aleph_bft_network_inbound_active,
            materialized_state_rows,
            invalid_operation_alerts,
        }
    }
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
        "lagging",
        "catching_up",
        "verifying_session_outcome",
        "materialized_state_mismatch",
        "halted",
    ];
    const PAYMENT_REPORT_STATUSES: &[&str] =
        &["empty", "awaiting_quorum", "quorum_reached", "conflicting"];
    const PAYMENT_REPORT_ACTIONS: &[&str] = &["none", "inspect", "quarantine"];

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
    cdk_prometheus::METRICS.set_federation_session_outcome_quorum_operation_count(
        &member,
        response
            .session_outcome_quorum_operation_count
            .map(super::u64_to_i64_saturating)
            .unwrap_or(0),
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
}

fn usize_to_u64_saturating(value: usize) -> u64 {
    match u64::try_from(value) {
        Ok(value) => value,
        Err(_) => u64::MAX,
    }
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
