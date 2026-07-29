use axum::extract::{Json, State};
use axum::response::{IntoResponse, Response};
use cdk::federation::{
    redact_federation_operator_message, FederationDkgActivationRequest,
    FederationDkgCommitmentRequest, FederationDkgReadinessRequest, FederationDkgRevealRequest,
    FederationDkgSecretShareRequest, FederationDkgSetupResponse,
    FederationDkgTranscriptSignatureRequest, FederationFrostDkgConfirmationRequest,
    FederationFrostDkgRound1Request, FederationFrostDkgRound2Record,
    FederationFrostDkgRound2Request, FederationRestMessageKind, FederationSignedRequest,
};
use cdk_frost::rand_core::OsRng;
use serde::Serialize;
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
        .federation
        .dkg_auto_start_config()
        .cloned()
        .ok_or_else(|| {
            (
                axum::http::StatusCode::CONFLICT,
                "federation DKG ceremony is not active",
            )
                .into_response()
        })
}

fn dkg_finalized_config_not_ready_response() -> Response {
    (
        axum::http::StatusCode::TOO_EARLY,
        "federation DKG finalized config is not ready",
    )
        .into_response()
}

struct DkgRequestAdmission {
    _rest_permit: Option<tokio::sync::OwnedSemaphorePermit>,
    _member_permit: Option<tokio::sync::OwnedSemaphorePermit>,
    verified: super::VerifiedFederationRequest,
}

async fn admit_dkg_request<T>(
    state: &MintState,
    payload: &FederationSignedRequest<T>,
    kind: FederationRestMessageKind,
) -> Result<DkgRequestAdmission, Response>
where
    T: Serialize,
{
    let rest_permit = acquire_private_rest_admission(state, kind)?;
    let verified = verify_federation_request(state, payload, kind).await?;
    let member_permit = acquire_private_dkg_member_admission(state, verified.sender, kind)?;
    Ok(DkgRequestAdmission {
        _rest_permit: rest_permit,
        _member_permit: member_permit,
        verified,
    })
}

#[instrument(skip_all)]
pub(super) async fn post_dkg_readiness(
    State(state): State<MintState>,
    Json(payload): Json<FederationSignedRequest<FederationDkgReadinessRequest>>,
) -> Result<Json<FederationDkgSetupResponse>, Response> {
    let admission =
        admit_dkg_request(&state, &payload, FederationRestMessageKind::DkgReadiness).await?;
    let verified = admission.verified;
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
        .record_federation_sequence(
            FederationRestMessageKind::DkgReadiness,
            verified.sender,
            verified.sequence,
        )
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
    let admission =
        admit_dkg_request(&state, &payload, FederationRestMessageKind::DkgCommitment).await?;
    let verified = admission.verified;
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
    let admission =
        admit_dkg_request(&state, &payload, FederationRestMessageKind::DkgReveal).await?;
    let verified = admission.verified;
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
    let admission =
        admit_dkg_request(&state, &payload, FederationRestMessageKind::DkgSecretShare).await?;
    let verified = admission.verified;
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
pub(super) async fn post_frost_dkg_round1(
    State(state): State<MintState>,
    Json(payload): Json<FederationSignedRequest<FederationFrostDkgRound1Request>>,
) -> Result<Json<FederationDkgSetupResponse>, Response> {
    let kind = FederationRestMessageKind::FrostDkgRound1;
    let admission = admit_dkg_request(&state, &payload, kind).await?;
    let verified = admission.verified;
    let dkg_config = active_dkg_config(&state)?;
    let runtime_config = federation_runtime_config(&state)?;
    if payload.payload.context.ceremony_id != dkg_config.ceremony_id
        || payload.payload.context.root_key_epoch != dkg_config.root_key_epoch
    {
        return Err(federation_error_response(
            cdk::federation::FederationError::DkgTranscriptMismatch {
                reason: "FROST round one belongs to another ceremony or root epoch".to_string(),
            },
        ));
    }
    payload
        .payload
        .decode_package(runtime_config.public_config(), verified.sender)
        .map_err(federation_error_response)?;
    state
        .persist_federation_frost_dkg_round1(&payload.payload, verified.sender, verified.sequence)
        .await
        .map_err(dkg_error_response)?;
    Ok(Json(FederationDkgSetupResponse::new(
        runtime_config.federation_id(),
        runtime_config.member_id(),
        kind,
        verified.sender,
    )))
}

#[instrument(skip_all)]
pub(super) async fn post_frost_dkg_round2(
    State(state): State<MintState>,
    Json(payload): Json<FederationSignedRequest<FederationFrostDkgRound2Request>>,
) -> Result<Json<FederationDkgSetupResponse>, Response> {
    let kind = FederationRestMessageKind::FrostDkgRound2;
    let admission = admit_dkg_request(&state, &payload, kind).await?;
    let verified = admission.verified;
    let dkg_config = active_dkg_config(&state)?;
    let runtime_config = federation_runtime_config(&state)?;
    if payload.payload.context.ceremony_id != dkg_config.ceremony_id
        || payload.payload.context.root_key_epoch != dkg_config.root_key_epoch
    {
        return Err(federation_error_response(
            cdk::federation::FederationError::DkgTranscriptMismatch {
                reason: "FROST round two belongs to another ceremony or root epoch".to_string(),
            },
        ));
    }
    let package = payload
        .payload
        .decode_package(
            runtime_config.public_config(),
            verified.sender,
            runtime_config.member_id(),
        )
        .map_err(federation_error_response)?;
    let storage_key = super::dkg_driver::frost_storage_key(
        runtime_config.private_config(),
        &payload.payload.context,
    )
    .map_err(dkg_error_response)?;
    let records = state
        .mint
        .localstore()
        .get_federation_frost_dkg_round2()
        .await
        .map_err(|error| dkg_error_response(error.into()))?;
    let existing = records.into_iter().find(|record| {
        record.context == payload.payload.context
            && record.sender == verified.sender
            && record.receiver == runtime_config.member_id()
    });
    let record = match existing {
        Some(record) => {
            let stored = record
                .open(runtime_config.public_config(), &storage_key)
                .map_err(federation_error_response)?;
            if stored != package {
                return Err(federation_error_response(
                    cdk::federation::FederationError::DkgTranscriptMismatch {
                        reason:
                            "FROST round-two contribution conflicts with the durable retry record"
                                .to_string(),
                    },
                ));
            }
            record
        }
        None => {
            let encrypted = package.seal(&storage_key, &mut OsRng).map_err(|_| {
                dkg_error_response(cdk::Error::Custom(
                    "failed to seal FROST round-two contribution".to_string(),
                ))
            })?;
            FederationFrostDkgRound2Record {
                context: payload.payload.context.clone(),
                sender: verified.sender,
                receiver: runtime_config.member_id(),
                encrypted_package: encrypted.to_bytes(),
            }
        }
    };
    state
        .persist_federation_frost_dkg_round2(&record, verified.sender, verified.sequence)
        .await
        .map_err(dkg_error_response)?;
    Ok(Json(FederationDkgSetupResponse::new(
        runtime_config.federation_id(),
        runtime_config.member_id(),
        kind,
        verified.sender,
    )))
}

#[instrument(skip_all)]
pub(super) async fn post_frost_dkg_confirmation(
    State(state): State<MintState>,
    Json(payload): Json<FederationSignedRequest<FederationFrostDkgConfirmationRequest>>,
) -> Result<Json<FederationDkgSetupResponse>, Response> {
    let kind = FederationRestMessageKind::FrostDkgConfirmation;
    let admission = admit_dkg_request(&state, &payload, kind).await?;
    let verified = admission.verified;
    let dkg_config = active_dkg_config(&state)?;
    let runtime_config = federation_runtime_config(&state)?;
    if payload.payload.context.ceremony_id != dkg_config.ceremony_id
        || payload.payload.context.root_key_epoch != dkg_config.root_key_epoch
    {
        return Err(federation_error_response(
            cdk::federation::FederationError::DkgTranscriptMismatch {
                reason: "FROST confirmation belongs to another ceremony or root epoch".to_string(),
            },
        ));
    }
    let confirmed_public = payload
        .payload
        .validate(runtime_config.public_config(), verified.sender)
        .map_err(federation_error_response)?;
    let local_roots = state
        .mint
        .localstore()
        .get_federation_frost_root_records()
        .await
        .map_err(|error| dkg_error_response(error.into()))?;
    if let Some(local_root) = local_roots.iter().find(|root| {
        root.context == payload.payload.context && root.member_id == runtime_config.member_id()
    }) {
        let local_public = local_root
            .validate(runtime_config.public_config())
            .map_err(federation_error_response)?;
        if local_public.encode() != confirmed_public.encode() {
            return Err(federation_error_response(
                cdk::federation::FederationError::DkgTranscriptMismatch {
                    reason: format!(
                        "FROST root confirmation from member {} differs from the local root",
                        verified.sender
                    ),
                },
            ));
        }
    }
    state
        .persist_federation_frost_dkg_confirmation(
            &payload.payload,
            verified.sender,
            verified.sequence,
        )
        .await
        .map_err(dkg_error_response)?;
    Ok(Json(FederationDkgSetupResponse::new(
        runtime_config.federation_id(),
        runtime_config.member_id(),
        kind,
        verified.sender,
    )))
}

#[instrument(skip_all)]
pub(super) async fn post_dkg_transcript_signature(
    State(state): State<MintState>,
    Json(payload): Json<FederationSignedRequest<FederationDkgTranscriptSignatureRequest>>,
) -> Result<Json<FederationDkgSetupResponse>, Response> {
    let admission = admit_dkg_request(
        &state,
        &payload,
        FederationRestMessageKind::DkgTranscriptSignature,
    )
    .await?;
    let verified = admission.verified;
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
    let admission =
        admit_dkg_request(&state, &payload, FederationRestMessageKind::DkgActivation).await?;
    let verified = admission.verified;
    let dkg_config = active_dkg_config(&state)?;
    if payload.payload.ceremony_id != dkg_config.ceremony_id {
        return Err(federation_error_response(
            cdk::federation::FederationError::DkgTranscriptMismatch {
                reason: "DKG activation belongs to another ceremony".to_string(),
            },
        ));
    }
    let runtime_config = federation_runtime_config(&state)?;
    let exact_durable_replay = state
        .mint
        .localstore()
        .get_federation_dkg_activations()
        .await
        .map_err(|error| dkg_error_response(error.into()))?
        .into_iter()
        .any(|activation| activation == payload.payload && activation.signer == verified.sender);
    if !exact_durable_replay {
        let finalized = state
            .ready_federation_dkg_finalized_member_config()
            .ok_or_else(dkg_finalized_config_not_ready_response)?;
        payload
            .payload
            .validate_sender_against_validated_configs(
                runtime_config.public_config(),
                &finalized.public_config,
                verified.sender,
            )
            .map_err(federation_error_response)?;
    }
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

        assert_eq!(
            dkg_finalized_config_not_ready_response().status(),
            axum::http::StatusCode::TOO_EARLY
        );
    }
}
