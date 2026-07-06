use axum::extract::{Json, State};
use axum::response::Response;
use cdk::federation::{
    FederationAlephBftFinalizedUnitRequest, FederationAlephBftFinalizedUnitResponse,
    FederationError, FederationRestMessageKind, FederationSignedRequest,
};
use tracing::instrument;

use crate::MintState;

use super::{
    acquire_private_rest_admission, cdk_error_response, federation_error_response,
    federation_runtime_config, verify_federation_request,
};

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
