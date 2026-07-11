use axum::extract::{Json, State};
use axum::response::Response;
use cdk::federation::{
    FederationDkgCommitmentRequest, FederationDkgReadinessRequest, FederationDkgRevealRequest,
    FederationDkgSecretShareRequest, FederationDkgSetupResponse,
    FederationDkgTranscriptSignatureRequest, FederationRestMessageKind, FederationSignedRequest,
};
use tracing::instrument;

use crate::MintState;

use super::{
    acquire_private_rest_admission, cdk_error_response, federation_error_response,
    federation_runtime_config, verify_federation_request,
};

#[instrument(skip_all)]
pub(super) async fn post_dkg_readiness(
    State(state): State<MintState>,
    Json(payload): Json<FederationSignedRequest<FederationDkgReadinessRequest>>,
) -> Result<Json<FederationDkgSetupResponse>, Response> {
    let _permit = acquire_private_rest_admission(&state, FederationRestMessageKind::DkgReadiness)?;
    let verified =
        verify_federation_request(&state, &payload, FederationRestMessageKind::DkgReadiness)
            .await?;
    let runtime_config = federation_runtime_config(&state)?;
    let dkg_config = state
        .federation_dkg_auto_start_config
        .as_ref()
        .ok_or_else(|| {
            cdk_error_response(cdk::Error::Custom(
                "federation member is not configured for live DKG".to_string(),
            ))
        })?;
    payload
        .payload
        .validate(
            runtime_config.public_config(),
            dkg_config.ceremony_id,
            &dkg_config.keyset_spec,
        )
        .map_err(federation_error_response)?;
    state
        .record_federation_sequence(verified.sender, verified.sequence)
        .await;

    Ok(Json(FederationDkgSetupResponse::new(
        runtime_config.federation_id(),
        runtime_config.member_id(),
        FederationRestMessageKind::DkgReadiness,
        verified.sender,
    )))
}

#[instrument(skip_all)]
pub(super) async fn post_dkg_commitment(
    State(state): State<MintState>,
    Json(payload): Json<FederationSignedRequest<FederationDkgCommitmentRequest>>,
) -> Result<Json<FederationDkgSetupResponse>, Response> {
    let _permit = acquire_private_rest_admission(&state, FederationRestMessageKind::DkgCommitment)?;
    let verified =
        verify_federation_request(&state, &payload, FederationRestMessageKind::DkgCommitment)
            .await?;
    let runtime_config = federation_runtime_config(&state)?;
    payload
        .payload
        .validate_sender(runtime_config.public_config(), verified.sender)
        .map_err(federation_error_response)?;
    let federation_id = runtime_config.federation_id();
    let local_member = runtime_config.member_id();
    state
        .persist_federation_dkg_commitment(
            &payload.payload.commitment,
            verified.sender,
            verified.sequence,
        )
        .await
        .map_err(cdk_error_response)?;

    Ok(Json(FederationDkgSetupResponse::new(
        federation_id,
        local_member,
        FederationRestMessageKind::DkgCommitment,
        payload.payload.commitment.member_id,
    )))
}

#[instrument(skip_all)]
pub(super) async fn post_dkg_reveal(
    State(state): State<MintState>,
    Json(payload): Json<FederationSignedRequest<FederationDkgRevealRequest>>,
) -> Result<Json<FederationDkgSetupResponse>, Response> {
    let _permit = acquire_private_rest_admission(&state, FederationRestMessageKind::DkgReveal)?;
    let verified =
        verify_federation_request(&state, &payload, FederationRestMessageKind::DkgReveal).await?;
    let runtime_config = federation_runtime_config(&state)?;
    payload
        .payload
        .validate_sender(runtime_config.public_config(), verified.sender)
        .map_err(federation_error_response)?;
    let federation_id = runtime_config.federation_id();
    let local_member = runtime_config.member_id();
    state
        .persist_federation_dkg_reveal(&payload.payload.reveal, verified.sender, verified.sequence)
        .await
        .map_err(cdk_error_response)?;

    Ok(Json(FederationDkgSetupResponse::new(
        federation_id,
        local_member,
        FederationRestMessageKind::DkgReveal,
        payload.payload.reveal.member_id,
    )))
}

#[instrument(skip_all)]
pub(super) async fn post_dkg_secret_share(
    State(state): State<MintState>,
    Json(payload): Json<FederationSignedRequest<FederationDkgSecretShareRequest>>,
) -> Result<Json<FederationDkgSetupResponse>, Response> {
    let _permit =
        acquire_private_rest_admission(&state, FederationRestMessageKind::DkgSecretShare)?;
    let verified =
        verify_federation_request(&state, &payload, FederationRestMessageKind::DkgSecretShare)
            .await?;
    let runtime_config = federation_runtime_config(&state)?;
    payload
        .payload
        .validate_sender_receiver(
            runtime_config.public_config(),
            verified.sender,
            runtime_config.member_id(),
        )
        .map_err(federation_error_response)?;
    let federation_id = runtime_config.federation_id();
    let local_member = runtime_config.member_id();
    state
        .persist_federation_dkg_secret_share_contribution(
            &payload.payload,
            verified.sender,
            verified.sequence,
        )
        .await
        .map_err(cdk_error_response)?;

    Ok(Json(FederationDkgSetupResponse::new(
        federation_id,
        local_member,
        FederationRestMessageKind::DkgSecretShare,
        payload.payload.sender,
    )))
}

#[instrument(skip_all)]
pub(super) async fn post_dkg_transcript_signature(
    State(state): State<MintState>,
    Json(payload): Json<FederationSignedRequest<FederationDkgTranscriptSignatureRequest>>,
) -> Result<Json<FederationDkgSetupResponse>, Response> {
    let _permit =
        acquire_private_rest_admission(&state, FederationRestMessageKind::DkgTranscriptSignature)?;
    let verified = verify_federation_request(
        &state,
        &payload,
        FederationRestMessageKind::DkgTranscriptSignature,
    )
    .await?;
    let runtime_config = federation_runtime_config(&state)?;
    payload
        .payload
        .validate_sender(runtime_config.public_config(), verified.sender)
        .map_err(federation_error_response)?;
    let federation_id = runtime_config.federation_id();
    let local_member = runtime_config.member_id();
    state
        .persist_federation_dkg_transcript_signature(
            &payload.payload,
            verified.sender,
            verified.sequence,
        )
        .await
        .map_err(cdk_error_response)?;

    Ok(Json(FederationDkgSetupResponse::new(
        federation_id,
        local_member,
        FederationRestMessageKind::DkgTranscriptSignature,
        payload.payload.signer,
    )))
}
