use axum::extract::{Json, State};
use axum::response::{IntoResponse, Response};
use cdk::federation::{
    redact_federation_operator_message, FederationDkgActivationRequest,
    FederationDkgCommitmentRequest, FederationDkgReadinessRequest, FederationDkgRevealRequest,
    FederationDkgSecretShareRequest, FederationDkgSetupResponse,
    FederationDkgTranscriptSignatureRequest, FederationRestMessageKind, FederationSignedRequest,
};
use tracing::instrument;

use crate::MintState;

use super::{
    acquire_private_dkg_member_admission, acquire_private_rest_admission,
    federation_error_response, federation_runtime_config, verify_federation_request,
};

fn dkg_error_response(err: cdk::Error) -> Response {
    match err {
        cdk::Error::Federation(err) => federation_error_response(err),
        cdk::Error::Database(err) => {
            tracing::error!(
                "Federation DKG storage mutation failed: {}",
                redact_federation_operator_message(&err.to_string())
            );
            (
                axum::http::StatusCode::SERVICE_UNAVAILABLE,
                "federation DKG storage unavailable",
            )
                .into_response()
        }
        err => {
            tracing::error!(
                "Federation DKG processing failed: {}",
                redact_federation_operator_message(&err.to_string())
            );
            (
                axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                "federation DKG processing failed",
            )
                .into_response()
        }
    }
}

fn active_dkg_config(
    state: &MintState,
) -> Result<std::sync::Arc<super::FederationDkgAutoStartConfig>, Response> {
    state
        .federation_dkg_auto_start_config
        .clone()
        .ok_or_else(|| {
            (
                axum::http::StatusCode::CONFLICT,
                "federation DKG ceremony is not active",
            )
                .into_response()
        })
}

#[instrument(skip_all)]
pub(super) async fn post_dkg_readiness(
    State(state): State<MintState>,
    Json(payload): Json<FederationSignedRequest<FederationDkgReadinessRequest>>,
) -> Result<Json<FederationDkgSetupResponse>, Response> {
    let _permit = acquire_private_rest_admission(&state, FederationRestMessageKind::DkgReadiness)?;
    let verified =
        verify_federation_request(&state, &payload, FederationRestMessageKind::DkgReadiness)
            .await?;
    let _member_permit = acquire_private_dkg_member_admission(
        &state,
        verified.sender,
        FederationRestMessageKind::DkgReadiness,
    )?;
    let dkg_config = active_dkg_config(&state)?;
    let runtime_config = federation_runtime_config(&state)?;
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
    let _member_permit = acquire_private_dkg_member_admission(
        &state,
        verified.sender,
        FederationRestMessageKind::DkgCommitment,
    )?;
    let dkg_config = active_dkg_config(&state)?;
    let runtime_config = federation_runtime_config(&state)?;
    payload
        .payload
        .validate_sender_for_ceremony(
            runtime_config.public_config(),
            verified.sender,
            dkg_config.ceremony_id,
        )
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
        .map_err(dkg_error_response)?;

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
    let _member_permit = acquire_private_dkg_member_admission(
        &state,
        verified.sender,
        FederationRestMessageKind::DkgReveal,
    )?;
    let dkg_config = active_dkg_config(&state)?;
    let runtime_config = federation_runtime_config(&state)?;
    payload
        .payload
        .validate_sender_for_ceremony(
            runtime_config.public_config(),
            verified.sender,
            dkg_config.ceremony_id,
            &dkg_config.keyset_spec,
        )
        .map_err(federation_error_response)?;
    let federation_id = runtime_config.federation_id();
    let local_member = runtime_config.member_id();
    state
        .persist_federation_dkg_reveal(&payload.payload.reveal, verified.sender, verified.sequence)
        .await
        .map_err(dkg_error_response)?;

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
    let _member_permit = acquire_private_dkg_member_admission(
        &state,
        verified.sender,
        FederationRestMessageKind::DkgSecretShare,
    )?;
    let dkg_config = active_dkg_config(&state)?;
    let runtime_config = federation_runtime_config(&state)?;
    payload
        .payload
        .validate_sender_receiver_for_ceremony(
            runtime_config.public_config(),
            verified.sender,
            runtime_config.member_id(),
            dkg_config.ceremony_id,
            &dkg_config.keyset_spec,
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
        .map_err(dkg_error_response)?;

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
    let _member_permit = acquire_private_dkg_member_admission(
        &state,
        verified.sender,
        FederationRestMessageKind::DkgTranscriptSignature,
    )?;
    let _dkg_config = active_dkg_config(&state)?;
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
        .map_err(dkg_error_response)?;

    Ok(Json(FederationDkgSetupResponse::new(
        federation_id,
        local_member,
        FederationRestMessageKind::DkgTranscriptSignature,
        payload.payload.signer,
    )))
}

#[instrument(skip_all)]
pub(super) async fn post_dkg_activation(
    State(state): State<MintState>,
    Json(payload): Json<FederationSignedRequest<FederationDkgActivationRequest>>,
) -> Result<Json<FederationDkgSetupResponse>, Response> {
    let _permit = acquire_private_rest_admission(&state, FederationRestMessageKind::DkgActivation)?;
    let verified =
        verify_federation_request(&state, &payload, FederationRestMessageKind::DkgActivation)
            .await?;
    let _member_permit = acquire_private_dkg_member_admission(
        &state,
        verified.sender,
        FederationRestMessageKind::DkgActivation,
    )?;
    let dkg_config = active_dkg_config(&state)?;
    if payload.payload.ceremony_id != dkg_config.ceremony_id {
        return Err(federation_error_response(
            cdk::federation::FederationError::DkgTranscriptMismatch {
                reason: "DKG activation belongs to another ceremony".to_string(),
            },
        ));
    }
    let runtime_config = federation_runtime_config(&state)?;
    let finalized = state
        .cached_federation_dkg_finalized_member_config()
        .await
        .map_err(dkg_error_response)?;
    payload
        .payload
        .validate_sender_against_validated_configs(
            runtime_config.public_config(),
            &finalized.public_config,
            verified.sender,
        )
        .map_err(federation_error_response)?;
    let federation_id = runtime_config.federation_id();
    let local_member = runtime_config.member_id();
    state
        .persist_federation_dkg_activation(&payload.payload, verified.sender, verified.sequence)
        .await
        .map_err(dkg_error_response)?;

    Ok(Json(FederationDkgSetupResponse::new(
        federation_id,
        local_member,
        FederationRestMessageKind::DkgActivation,
        payload.payload.signer,
    )))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dkg_error_response_distinguishes_storage_from_internal_failure() {
        let storage = dkg_error_response(cdk::Error::Database(cdk::cdk_database::Error::Duplicate));
        assert_eq!(
            storage.status(),
            axum::http::StatusCode::SERVICE_UNAVAILABLE
        );

        let internal = dkg_error_response(cdk::Error::Custom(
            "private internal response detail".to_string(),
        ));
        assert_eq!(
            internal.status(),
            axum::http::StatusCode::INTERNAL_SERVER_ERROR
        );
    }
}
