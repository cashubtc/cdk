use axum::extract::{Json, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::response::Response;
use cdk::federation::{
    FederationAlephBftActivityRequest, FederationAlephBftActivityResponse,
    FederationAlephBftNetworkMessageRequest, FederationAlephBftNetworkMessageResponse,
    FederationRestMessageKind, FederationSignedRequest,
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
    let inbound = state
        .federation
        .aleph_bft_network_inbound
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
