#[cfg(feature = "prometheus")]
use std::collections::BTreeMap;

use axum::extract::{Json, State};
use axum::response::Response;
use cdk::federation::{
    FederationMaterializedStateRowFingerprint, FederationPaymentObservationReportSummary,
    FederationPeerStatus, FederationPeerStatusRequest, FederationRestMessageKind,
    FederationSignedRequest, FEDERATION_REST_API_VERSION,
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
        .persist_federation_sequence(verified.sender, verified.sequence)
        .await
        .map_err(cdk_error_response)?;
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
        .persist_federation_sequence(verified.sender, verified.sequence)
        .await
        .map_err(cdk_error_response)?;
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
    let invalid_operation_alerts = state.federation_invalid_operation_alert_summaries().await;
    let response = FederationLocalStatusResponse::new(
        peer_status,
        sync_status,
        session_outcome_quorum,
        payment_observation_reports,
        materialized_state_rows,
        usize_to_u64_saturating(pending_mempool_items),
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
    #[serde(default)]
    pub(crate) pending_mempool_items: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) session_outcome_quorum_operation_count: Option<u64>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) session_outcome_quorum_signers: Vec<cdk::federation::MemberId>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) payment_observation_reports: Vec<FederationPaymentObservationReportSummary>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) materialized_state_rows: Vec<FederationMaterializedStateRowFingerprint>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
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
        invalid_operation_alerts: Vec<crate::FederationInvalidOperationAlertSummary>,
    ) -> Self {
        let session_outcome_quorum_operation_count = session_outcome_quorum
            .as_ref()
            .map(|quorum| quorum.operation_count);
        let session_outcome_quorum_signers = session_outcome_quorum
            .map(|quorum| quorum.signers.into_iter().collect())
            .unwrap_or_default();
        Self {
            version: FEDERATION_REST_API_VERSION,
            peer_status,
            sync_state: sync_status.state_name().to_string(),
            sync_ready: sync_status.is_ready(),
            sync_message: sync_status.not_ready_message(),
            pending_mempool_items,
            session_outcome_quorum_operation_count,
            session_outcome_quorum_signers,
            payment_observation_reports,
            materialized_state_rows,
            invalid_operation_alerts,
        }
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
