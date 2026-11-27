//! HTTP routes for frost signing.

use axum::extract::{Json, State};
use axum::response::{IntoResponse, Response};
use cdk::federation::{
    FederationFrostAggregateResultNotification, FederationFrostBatchAggregateResultNotification,
    FederationFrostBatchCommitmentRequest, FederationFrostBatchCommitmentResponse,
    FederationFrostBatchSignatureShareResponse, FederationFrostBatchSigningAbortRequest,
    FederationFrostBatchSigningAck, FederationFrostBatchSigningPackageRequest,
    FederationFrostCommitmentRequest, FederationFrostCommitmentResponse,
    FederationFrostSignatureShareResponse, FederationFrostSigningAbortRequest,
    FederationFrostSigningAck, FederationFrostSigningPackageRequest, FederationRestMessageKind,
    FederationSignedRequest,
};
use cdk_federation::frost_signing::signing_runtime;
use tracing::instrument;

use crate::MintState;

use super::{admit_private_request, federation_error_response, federation_runtime_config};

pub(crate) fn signing_unavailable_response() -> Response {
    (
        axum::http::StatusCode::SERVICE_UNAVAILABLE,
        "federation FROST signing is unavailable",
    )
        .into_response()
}

pub(crate) fn signing_error_response(error: cdk::Error) -> Response {
    match error {
        cdk::Error::Federation(error) => federation_error_response(error),
        error => {
            tracing::error!(
                "Federation FROST signing failed: {}",
                cdk::federation::redact_federation_operator_message(&error.to_string())
            );
            (
                axum::http::StatusCode::SERVICE_UNAVAILABLE,
                "federation FROST signing failed closed",
            )
                .into_response()
        }
    }
}

#[instrument(skip_all)]
pub async fn post_frost_signing_commitment(
    State(state): State<MintState>,
    Json(payload): Json<FederationSignedRequest<FederationFrostCommitmentRequest>>,
) -> Result<Json<FederationSignedRequest<FederationFrostCommitmentResponse>>, Response> {
    let kind = FederationRestMessageKind::FrostSigningCommitment;
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
        .commit(&state.mint, &context, &derivation, &authorization)
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
        FederationFrostCommitmentResponse::new(&commitment),
        runtime_config.private_config(),
    )
    .map_err(federation_error_response)?;
    Ok(Json(response))
}

#[instrument(skip_all)]
pub async fn post_frost_signing_package(
    State(state): State<MintState>,
    Json(payload): Json<FederationSignedRequest<FederationFrostSigningPackageRequest>>,
) -> Result<Json<FederationSignedRequest<FederationFrostSignatureShareResponse>>, Response> {
    let kind = FederationRestMessageKind::FrostSigningPackage;
    let verified = admit_private_request(&state, &payload, kind).await?;
    state
        .ensure_federation_ready_for_authoritative_read()
        .await
        .map_err(signing_error_response)?;
    let runtime_config = federation_runtime_config(&state)?;
    let (context, nonce_id, authorization, commitments) = payload
        .payload
        .decode(runtime_config.federation_id(), runtime_config.member_id())
        .map_err(federation_error_response)?;
    let share = signing_runtime(&state)
        .ok_or_else(signing_unavailable_response)?
        .sign(
            &state.mint,
            &context,
            nonce_id,
            &authorization,
            &commitments,
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
        FederationFrostSignatureShareResponse::new(payload.payload.package_hash, &share),
        runtime_config.private_config(),
    )
    .map_err(federation_error_response)?;
    Ok(Json(response))
}

#[instrument(skip_all)]
pub async fn post_frost_signing_abort(
    State(state): State<MintState>,
    Json(payload): Json<FederationSignedRequest<FederationFrostSigningAbortRequest>>,
) -> Result<Json<FederationSignedRequest<FederationFrostSigningAck>>, Response> {
    let kind = FederationRestMessageKind::FrostSigningAbort;
    let verified = admit_private_request(&state, &payload, kind).await?;
    let runtime_config = federation_runtime_config(&state)?;
    let (context, authorization, reason) = payload
        .payload
        .decode(runtime_config.federation_id(), runtime_config.member_id())
        .map_err(federation_error_response)?;
    signing_runtime(&state)
        .ok_or_else(signing_unavailable_response)?
        .abort(&state.mint, &context, &authorization, reason)
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

#[instrument(skip_all)]
pub async fn post_frost_aggregate_result(
    State(state): State<MintState>,
    Json(payload): Json<FederationSignedRequest<FederationFrostAggregateResultNotification>>,
) -> Result<Json<FederationSignedRequest<FederationFrostSigningAck>>, Response> {
    let kind = FederationRestMessageKind::FrostAggregateResult;
    let verified = admit_private_request(&state, &payload, kind).await?;
    let runtime_config = federation_runtime_config(&state)?;
    let (context, authorization, package_hash, signature) = payload
        .payload
        .decode()
        .map_err(federation_error_response)?;
    signing_runtime(&state)
        .ok_or_else(signing_unavailable_response)?
        .record_aggregate(
            &state.mint,
            &context,
            &authorization,
            package_hash,
            signature,
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
        FederationFrostSigningAck::new(context.session_id(), kind),
        runtime_config.private_config(),
    )
    .map_err(federation_error_response)?;
    Ok(Json(response))
}

#[instrument(skip_all)]
pub async fn post_frost_signing_batch_commitment(
    State(state): State<MintState>,
    Json(payload): Json<FederationSignedRequest<FederationFrostBatchCommitmentRequest>>,
) -> Result<Json<FederationSignedRequest<FederationFrostBatchCommitmentResponse>>, Response> {
    let kind = FederationRestMessageKind::FrostSigningBatchCommitment;
    let verified = admit_private_request(&state, &payload, kind).await?;
    state
        .ensure_federation_ready_for_authoritative_read()
        .await
        .map_err(signing_error_response)?;
    let runtime_config = federation_runtime_config(&state)?;
    let items = payload
        .payload
        .decode(runtime_config.federation_id(), runtime_config.member_id())
        .map_err(federation_error_response)?;
    let commitments = signing_runtime(&state)
        .ok_or_else(signing_unavailable_response)?
        .commit_batch(&state.mint, &items)
        .await
        .map_err(signing_error_response)?;
    state
        .record_federation_sequence(kind, verified.sender(), verified.sequence())
        .await;
    let indexed = items
        .iter()
        .zip(commitments)
        .map(|(item, commitment)| (item.input_index, commitment))
        .collect::<Vec<_>>();
    let response = FederationSignedRequest::new_signed(
        runtime_config.federation_id(),
        verified.sender(),
        verified.sequence(),
        kind,
        FederationFrostBatchCommitmentResponse::new(&indexed),
        runtime_config.private_config(),
    )
    .map_err(federation_error_response)?;
    Ok(Json(response))
}

#[instrument(skip_all)]
pub async fn post_frost_signing_batch_package(
    State(state): State<MintState>,
    Json(payload): Json<FederationSignedRequest<FederationFrostBatchSigningPackageRequest>>,
) -> Result<Json<FederationSignedRequest<FederationFrostBatchSignatureShareResponse>>, Response> {
    let kind = FederationRestMessageKind::FrostSigningBatchPackage;
    let verified = admit_private_request(&state, &payload, kind).await?;
    state
        .ensure_federation_ready_for_authoritative_read()
        .await
        .map_err(signing_error_response)?;
    let runtime_config = federation_runtime_config(&state)?;
    let items = payload
        .payload
        .decode(runtime_config.federation_id(), runtime_config.member_id())
        .map_err(federation_error_response)?;
    let shares = signing_runtime(&state)
        .ok_or_else(signing_unavailable_response)?
        .sign_batch(&state.mint, &items)
        .await
        .map_err(signing_error_response)?;
    state
        .record_federation_sequence(kind, verified.sender(), verified.sequence())
        .await;
    let indexed = items
        .iter()
        .zip(shares)
        .map(|(item, share)| (item.input_index, item.package_hash, share))
        .collect::<Vec<_>>();
    let response = FederationSignedRequest::new_signed(
        runtime_config.federation_id(),
        verified.sender(),
        verified.sequence(),
        kind,
        FederationFrostBatchSignatureShareResponse::new(&indexed),
        runtime_config.private_config(),
    )
    .map_err(federation_error_response)?;
    Ok(Json(response))
}

#[instrument(skip_all)]
pub async fn post_frost_signing_batch_abort(
    State(state): State<MintState>,
    Json(payload): Json<FederationSignedRequest<FederationFrostBatchSigningAbortRequest>>,
) -> Result<Json<FederationSignedRequest<FederationFrostBatchSigningAck>>, Response> {
    let kind = FederationRestMessageKind::FrostSigningBatchAbort;
    let verified = admit_private_request(&state, &payload, kind).await?;
    let runtime_config = federation_runtime_config(&state)?;
    let (reason, items) = payload
        .payload
        .decode(runtime_config.federation_id(), runtime_config.member_id())
        .map_err(federation_error_response)?;
    signing_runtime(&state)
        .ok_or_else(signing_unavailable_response)?
        .abort_batch(&state.mint, &items, reason)
        .await
        .map_err(signing_error_response)?;
    state
        .record_federation_sequence(kind, verified.sender(), verified.sequence())
        .await;
    let session_ids = items
        .iter()
        .map(|item| item.context.session_id())
        .collect::<Vec<_>>();
    let response = FederationSignedRequest::new_signed(
        runtime_config.federation_id(),
        verified.sender(),
        verified.sequence(),
        kind,
        FederationFrostBatchSigningAck::new(&session_ids, kind),
        runtime_config.private_config(),
    )
    .map_err(federation_error_response)?;
    Ok(Json(response))
}

#[instrument(skip_all)]
pub async fn post_frost_aggregate_batch_result(
    State(state): State<MintState>,
    Json(payload): Json<FederationSignedRequest<FederationFrostBatchAggregateResultNotification>>,
) -> Result<Json<FederationSignedRequest<FederationFrostBatchSigningAck>>, Response> {
    let kind = FederationRestMessageKind::FrostAggregateBatchResult;
    let verified = admit_private_request(&state, &payload, kind).await?;
    let runtime_config = federation_runtime_config(&state)?;
    let items = payload
        .payload
        .decode()
        .map_err(federation_error_response)?;
    signing_runtime(&state)
        .ok_or_else(signing_unavailable_response)?
        .record_aggregate_batch(&state.mint, &items)
        .await
        .map_err(signing_error_response)?;
    state
        .record_federation_sequence(kind, verified.sender(), verified.sequence())
        .await;
    let session_ids = items
        .iter()
        .map(|item| item.context.session_id())
        .collect::<Vec<_>>();
    let response = FederationSignedRequest::new_signed(
        runtime_config.federation_id(),
        verified.sender(),
        verified.sequence(),
        kind,
        FederationFrostBatchSigningAck::new(&session_ids, kind),
        runtime_config.private_config(),
    )
    .map_err(federation_error_response)?;
    Ok(Json(response))
}
