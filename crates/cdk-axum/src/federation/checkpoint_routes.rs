use axum::extract::{Json, State};
use axum::response::Response;
use cdk::federation::{
    FederationCheckpointAuditRequest, FederationCheckpointAuditResponse,
    FederationCheckpointFetchRequest, FederationCheckpointFetchResponse,
    FederationCheckpointQuorumRequest, FederationCheckpointQuorumResponse,
    FederationCheckpointSignatureRequest, FederationCheckpointSignatureResponse,
    FederationCheckpointSummaryRequest, FederationCheckpointSummaryResponse,
    FederationRestMessageKind, FederationSignedRequest,
};
use tracing::instrument;

use crate::MintState;

use super::{
    acquire_private_rest_admission, cdk_error_response, federation_error_response,
    federation_runtime_config, verify_federation_request,
};

#[instrument(skip_all)]
pub(super) async fn post_checkpoint_summary(
    State(state): State<MintState>,
    Json(payload): Json<FederationSignedRequest<FederationCheckpointSummaryRequest>>,
) -> Result<Json<FederationSignedRequest<FederationCheckpointSummaryResponse>>, Response> {
    let kind = FederationRestMessageKind::CheckpointSummary;
    let _permit = acquire_private_rest_admission(&state, kind)?;
    let verified = verify_federation_request(&state, &payload, kind).await?;
    payload
        .payload
        .validate()
        .map_err(federation_error_response)?;
    state
        .record_federation_sequence(kind, verified.sender, verified.sequence)
        .await;
    if payload.payload.request_recovery_checkpoint {
        state.federation.request_recovery_checkpoint();
    }
    let response = state
        .federation
        .checkpoint_summary(payload.payload.next_scheduled_quorum_after)
        .await
        .map_err(cdk_error_response)?;
    let runtime_config = federation_runtime_config(&state)?;
    let response = FederationSignedRequest::new_signed(
        runtime_config.federation_id(),
        verified.sender,
        verified.sequence,
        kind,
        response,
        runtime_config.private_config(),
    )
    .map_err(federation_error_response)?;
    Ok(Json(response))
}

#[instrument(skip_all)]
pub(super) async fn post_checkpoint_fetch(
    State(state): State<MintState>,
    Json(payload): Json<FederationSignedRequest<FederationCheckpointFetchRequest>>,
) -> Result<Json<FederationCheckpointFetchResponse>, Response> {
    let kind = FederationRestMessageKind::CheckpointFetch;
    let _permit = acquire_private_rest_admission(&state, kind)?;
    let verified = verify_federation_request(&state, &payload, kind).await?;
    payload
        .payload
        .validate()
        .map_err(federation_error_response)?;
    state
        .record_federation_sequence(kind, verified.sender, verified.sequence)
        .await;
    state
        .federation
        .fetch_checkpoint(payload.payload.checkpoint_id)
        .await
        .map(Json)
        .map_err(cdk_error_response)
}

#[instrument(skip_all)]
pub(super) async fn post_checkpoint_signature(
    State(state): State<MintState>,
    Json(payload): Json<FederationSignedRequest<FederationCheckpointSignatureRequest>>,
) -> Result<Json<FederationCheckpointSignatureResponse>, Response> {
    let kind = FederationRestMessageKind::CheckpointSignature;
    let _permit = acquire_private_rest_admission(&state, kind)?;
    let verified = verify_federation_request(&state, &payload, kind).await?;
    payload
        .payload
        .validate()
        .map_err(federation_error_response)?;
    state
        .record_federation_sequence(kind, verified.sender, verified.sequence)
        .await;
    state
        .federation
        .local_checkpoint_signature(payload.payload.checkpoint_id)
        .await
        .map(Json)
        .map_err(cdk_error_response)
}

#[instrument(skip_all)]
pub(super) async fn post_checkpoint_quorum(
    State(state): State<MintState>,
    Json(payload): Json<FederationSignedRequest<FederationCheckpointQuorumRequest>>,
) -> Result<Json<FederationCheckpointQuorumResponse>, Response> {
    let kind = FederationRestMessageKind::CheckpointQuorum;
    let _permit = acquire_private_rest_admission(&state, kind)?;
    let verified = verify_federation_request(&state, &payload, kind).await?;
    let runtime_config = state.mint.federation_config().ok_or_else(|| {
        federation_error_response(cdk::federation::FederationError::RuntimeUnavailable {
            component: "mint configuration",
        })
    })?;
    payload
        .payload
        .validate(runtime_config.public_config())
        .map_err(federation_error_response)?;
    state
        .record_federation_sequence(kind, verified.sender, verified.sequence)
        .await;
    state
        .federation
        .persist_checkpoint_quorum(&payload.payload.quorum)
        .await
        .map(Json)
        .map_err(cdk_error_response)
}

#[instrument(skip_all)]
pub(super) async fn post_checkpoint_audit(
    State(state): State<MintState>,
    Json(payload): Json<FederationSignedRequest<FederationCheckpointAuditRequest>>,
) -> Result<Json<FederationCheckpointAuditResponse>, Response> {
    let kind = FederationRestMessageKind::CheckpointAudit;
    let _permit = acquire_private_rest_admission(&state, kind)?;
    let verified = verify_federation_request(&state, &payload, kind).await?;
    payload
        .payload
        .validate()
        .map_err(federation_error_response)?;
    state
        .record_federation_sequence(kind, verified.sender, verified.sequence)
        .await;
    let audit = state
        .federation
        .audit_trusted_checkpoint_materialized_state()
        .await;
    if matches!(
        &audit,
        Err(cdk::Error::Federation(
            cdk::federation::FederationError::MaterializedStateAuditMismatch { .. }
        ))
    ) {
        super::halt_current_federation_aleph_bft_session(
            &state,
            "failed an operator-requested trusted-checkpoint materialized-state audit",
        )
        .await;
    }
    audit.map(Json).map_err(cdk_error_response)
}
