use axum::extract::{Json, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::response::Response;
use cdk::federation::{
    FederationAlephBftActivityRequest, FederationAlephBftActivityResponse,
    FederationAlephBftFinalizedUnitRequest, FederationAlephBftFinalizedUnitResponse,
    FederationAlephBftNetworkMessageRequest, FederationAlephBftNetworkMessageResponse,
    FederationConsensusBackendKind, FederationError, FederationRestMessageKind,
    FederationSignedRequest,
};
use tracing::instrument;

use crate::MintState;

use super::{
    acquire_private_rest_admission, cdk_error_response, federation_error_response,
    federation_runtime_config, verify_federation_request,
};

#[instrument(skip_all)]
pub(super) async fn post_aleph_bft_activity(
    State(state): State<MintState>,
    Json(payload): Json<FederationSignedRequest<FederationAlephBftActivityRequest>>,
) -> Result<Json<FederationAlephBftActivityResponse>, Response> {
    let _permit =
        acquire_private_rest_admission(&state, FederationRestMessageKind::AlephBftActivity)?;
    let verified = verify_federation_request(
        &state,
        &payload,
        FederationRestMessageKind::AlephBftActivity,
    )
    .await?;
    let runtime_config = federation_runtime_config(&state)?;
    if runtime_config.consensus_backend() != FederationConsensusBackendKind::AlephBft {
        return Err((
            StatusCode::BAD_REQUEST,
            format!(
                "AlephBFT activity is not accepted for {} consensus backend",
                runtime_config.consensus_backend()
            ),
        )
            .into_response());
    }
    state
        .mark_aleph_bft_peer_activity(payload.payload.session_id, payload.payload.operation_id)
        .await
        .map_err(cdk_error_response)?;
    state
        .record_federation_sequence(verified.sender, verified.sequence)
        .await;

    Ok(Json(FederationAlephBftActivityResponse::new(
        runtime_config.federation_id(),
        payload.payload.session_id,
        payload.payload.operation_id,
    )))
}

#[instrument(skip_all)]
pub(super) async fn post_aleph_bft_finalized_unit(
    State(state): State<MintState>,
    Json(payload): Json<FederationSignedRequest<FederationAlephBftFinalizedUnitRequest>>,
) -> Result<Json<FederationAlephBftFinalizedUnitResponse>, Response> {
    let _permit =
        acquire_private_rest_admission(&state, FederationRestMessageKind::AlephBftFinalizedUnit)?;
    let verified = verify_federation_request(
        &state,
        &payload,
        FederationRestMessageKind::AlephBftFinalizedUnit,
    )
    .await?;
    if payload.payload.unit.submitting_member != verified.sender {
        return Err(federation_error_response(
            FederationError::MemberIdMismatch {
                expected: verified.sender,
                found: payload.payload.unit.submitting_member,
            },
        ));
    }
    payload
        .payload
        .ensure_admissible_size()
        .map_err(federation_error_response)?;
    let responses = state
        .apply_federation_aleph_bft_finalized_unit_with_sequence(
            payload.payload.unit.clone(),
            Some((verified.sender, verified.sequence)),
        )
        .await
        .map_err(cdk_error_response)?;
    let runtime_config = federation_runtime_config(&state)?;
    let response = FederationAlephBftFinalizedUnitResponse::new(
        runtime_config.federation_id(),
        &payload.payload.unit,
        responses,
    );

    Ok(Json(response))
}

#[instrument(skip_all)]
pub(super) async fn post_aleph_bft_network_message(
    State(state): State<MintState>,
    Json(payload): Json<FederationSignedRequest<FederationAlephBftNetworkMessageRequest>>,
) -> Result<Json<FederationAlephBftNetworkMessageResponse>, Response> {
    let _permit =
        acquire_private_rest_admission(&state, FederationRestMessageKind::AlephBftNetworkMessage)?;
    let verified = verify_federation_request(
        &state,
        &payload,
        FederationRestMessageKind::AlephBftNetworkMessage,
    )
    .await?;
    payload
        .payload
        .ensure_admissible_size()
        .map_err(federation_error_response)?;
    let runtime_config = federation_runtime_config(&state)?;
    let federation_id = runtime_config.federation_id();
    let consensus_backend = runtime_config.consensus_backend();
    if consensus_backend != FederationConsensusBackendKind::AlephBft {
        return Err((
            StatusCode::BAD_REQUEST,
            format!(
                "AlephBFT network messages are not accepted for {} consensus backend",
                consensus_backend
            ),
        )
            .into_response());
    }

    let inbound = state
        .federation_aleph_bft_network_inbound
        .read()
        .await
        .clone();
    let Some(inbound) = inbound else {
        return Err((
            StatusCode::SERVICE_UNAVAILABLE,
            "AlephBFT network message receiver is not available",
        )
            .into_response());
    };
    inbound
        .queue_request(&payload.payload)
        .map_err(cdk_error_response)?;

    state
        .record_federation_sequence(verified.sender, verified.sequence)
        .await;

    Ok(Json(FederationAlephBftNetworkMessageResponse::new(
        federation_id,
        payload.payload.session_id,
    )))
}
