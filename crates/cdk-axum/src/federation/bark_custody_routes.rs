//! HTTP routes for bark custody.

use axum::extract::{Json, State};
use axum::response::Response;
use cdk::federation::{
    FederationBarkCustodyAbortRequest, FederationBarkThresholdDhRequest,
    FederationBarkThresholdDhResponse, FederationBarkVirtualMusigCommitmentRequest,
    FederationBarkVirtualMusigCommitmentResponse, FederationBarkVirtualMusigPartialRequest,
    FederationBarkVirtualMusigPartialResponse, FederationFrostSigningAck,
    FederationRestMessageKind, FederationSignedRequest,
};
use tracing::instrument;

use crate::MintState;

use super::frost_signing::signing_runtime;
use super::frost_signing_routes::{signing_error_response, signing_unavailable_response};
use super::{admit_private_request, federation_error_response, federation_runtime_config};

#[instrument(skip_all)]
pub async fn post_bark_virtual_musig_commitment(
    State(state): State<MintState>,
    Json(payload): Json<FederationSignedRequest<FederationBarkVirtualMusigCommitmentRequest>>,
) -> Result<Json<FederationSignedRequest<FederationBarkVirtualMusigCommitmentResponse>>, Response> {
    let kind = FederationRestMessageKind::ApplicationVirtualMusigCommitment;
    let verified = admit_private_request(&state, &payload, kind).await?;
    state
        .ensure_federation_ready_for_authoritative_read()
        .await
        .map_err(signing_error_response)?;
    let runtime_config = federation_runtime_config(&state)?;
    let (context, derivation, authorization) = payload
        .payload
        .decode(runtime_config.federation_id(), runtime_config.member_id())
        .map_err(federation_error_response)?;
    let commitment = signing_runtime(&state)
        .ok_or_else(signing_unavailable_response)?
        .commit_bark_virtual_musig(&state.mint, &context, &derivation, &authorization)
        .await
        .map_err(signing_error_response)?;
    state
        .record_federation_sequence(kind, verified.sender(), verified.sequence())
        .await;
    let response = FederationSignedRequest::new_signed(
        runtime_config.federation_id(),
        verified.sender(),
        verified.sequence(),
        kind,
        FederationBarkVirtualMusigCommitmentResponse::new(&commitment),
        runtime_config.private_config(),
    )
    .map_err(federation_error_response)?;
    Ok(Json(response))
}

#[instrument(skip_all)]
pub async fn post_bark_virtual_musig_partial(
    State(state): State<MintState>,
    Json(payload): Json<FederationSignedRequest<FederationBarkVirtualMusigPartialRequest>>,
) -> Result<Json<FederationSignedRequest<FederationBarkVirtualMusigPartialResponse>>, Response> {
    let kind = FederationRestMessageKind::ApplicationVirtualMusigPartial;
    let verified = admit_private_request(&state, &payload, kind).await?;
    state
        .ensure_federation_ready_for_authoritative_read()
        .await
        .map_err(signing_error_response)?;
    let runtime_config = federation_runtime_config(&state)?;
    let (context, nonce_id, authorization, commitments, transcript) = payload
        .payload
        .decode(runtime_config.federation_id(), runtime_config.member_id())
        .map_err(federation_error_response)?;
    let partial = signing_runtime(&state)
        .ok_or_else(signing_unavailable_response)?
        .sign_bark_virtual_musig(
            &state.mint,
            &context,
            nonce_id,
            &authorization,
            &commitments,
            &transcript,
        )
        .await
        .map_err(signing_error_response)?;
    state
        .record_federation_sequence(kind, verified.sender(), verified.sequence())
        .await;
    let response = FederationSignedRequest::new_signed(
        runtime_config.federation_id(),
        verified.sender(),
        verified.sequence(),
        kind,
        FederationBarkVirtualMusigPartialResponse::new(&partial),
        runtime_config.private_config(),
    )
    .map_err(federation_error_response)?;
    Ok(Json(response))
}

#[instrument(skip_all)]
pub async fn post_bark_threshold_dh(
    State(state): State<MintState>,
    Json(payload): Json<FederationSignedRequest<FederationBarkThresholdDhRequest>>,
) -> Result<Json<FederationSignedRequest<FederationBarkThresholdDhResponse>>, Response> {
    let kind = FederationRestMessageKind::ApplicationThresholdDh;
    let verified = admit_private_request(&state, &payload, kind).await?;
    state
        .ensure_federation_ready_for_authoritative_read()
        .await
        .map_err(signing_error_response)?;
    let runtime_config = federation_runtime_config(&state)?;
    let (context, derivation, authorization, request) = payload
        .payload
        .decode(runtime_config.federation_id(), runtime_config.member_id())
        .map_err(federation_error_response)?;
    let share = signing_runtime(&state)
        .ok_or_else(signing_unavailable_response)?
        .bark_threshold_dh(&state.mint, &context, &derivation, &authorization, &request)
        .await
        .map_err(signing_error_response)?;
    state
        .record_federation_sequence(kind, verified.sender(), verified.sequence())
        .await;
    let response = FederationSignedRequest::new_signed(
        runtime_config.federation_id(),
        verified.sender(),
        verified.sequence(),
        kind,
        FederationBarkThresholdDhResponse::new(&share),
        runtime_config.private_config(),
    )
    .map_err(federation_error_response)?;
    Ok(Json(response))
}

#[instrument(skip_all)]
pub async fn post_bark_custody_abort(
    State(state): State<MintState>,
    Json(payload): Json<FederationSignedRequest<FederationBarkCustodyAbortRequest>>,
) -> Result<Json<FederationSignedRequest<FederationFrostSigningAck>>, Response> {
    let kind = FederationRestMessageKind::ApplicationCustodyAbort;
    let verified = admit_private_request(&state, &payload, kind).await?;
    let runtime_config = federation_runtime_config(&state)?;
    let (context, authorization, reason) = payload
        .payload
        .decode(runtime_config.federation_id(), runtime_config.member_id())
        .map_err(federation_error_response)?;
    signing_runtime(&state)
        .ok_or_else(signing_unavailable_response)?
        .abort_bark_custody(&state.mint, &context, &authorization, reason)
        .await
        .map_err(signing_error_response)?;
    state
        .record_federation_sequence(kind, verified.sender(), verified.sequence())
        .await;
    let response = FederationSignedRequest::new_signed(
        runtime_config.federation_id(),
        verified.sender(),
        verified.sequence(),
        kind,
        FederationFrostSigningAck::new(context.session_id(), kind),
        runtime_config.private_config(),
    )
    .map_err(federation_error_response)?;
    Ok(Json(response))
}
