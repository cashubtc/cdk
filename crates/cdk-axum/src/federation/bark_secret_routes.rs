//! HTTP routes for bark secret.

use std::time::{SystemTime, UNIX_EPOCH};

use axum::extract::{Json, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use cdk::federation::{
    FederationBarkEvidenceAttestationRequest, FederationBarkEvidenceAttestationResponse,
    FederationBarkEvidencePhase, FederationBarkPrepareReceiveSecretRequest,
    FederationBarkReceiveSecretAcknowledgement, FederationRestMessageKind, FederationSignedRequest,
};
use cdk::mint::QuoteId;
use tracing::instrument;

use crate::MintState;

use super::{admit_private_request, federation_error_response, federation_runtime_config};

#[instrument(skip_all)]
pub async fn post_bark_receive_secret(
    State(state): State<MintState>,
    Json(payload): Json<FederationSignedRequest<FederationBarkPrepareReceiveSecretRequest>>,
) -> Result<Json<FederationSignedRequest<FederationBarkReceiveSecretAcknowledgement>>, Response> {
    let kind = FederationRestMessageKind::ApplicationSecretPrepare;
    let verified = admit_private_request(&state, &payload, kind).await?;
    state
        .ensure_federation_ready_for_authoritative_read()
        .await
        .map_err(bark_secret_unavailable_response)?;
    let runtime_config = federation_runtime_config(&state)?;
    let public_config = runtime_config.public_config().clone();
    let private_config = runtime_config.private_config().clone();
    let federation_id = runtime_config.federation_id();
    let store = state
        .federation
        .bark_receive_secrets()
        .ok_or_else(bark_secret_store_unavailable_response)?;
    let request = payload.payload.clone();
    let sender = verified.sender();
    let received_at = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| bark_secret_store_unavailable_response())?
        .as_secs();
    let acknowledgement = tokio::task::spawn_blocking(move || {
        store.prepare_receive_secret(
            &request,
            sender,
            &public_config,
            &private_config,
            received_at,
        )
    })
    .await
    .map_err(|_| bark_secret_store_unavailable_response())?
    .map_err(bark_secret_request_response)?;
    state
        .record_federation_sequence(kind, verified.sender(), verified.sequence())
        .await;
    let runtime_config = federation_runtime_config(&state)?;
    let response = FederationSignedRequest::new_signed(
        federation_id,
        verified.sender(),
        verified.sequence(),
        kind,
        acknowledgement,
        runtime_config.private_config(),
    )
    .map_err(federation_error_response)?;
    Ok(Json(response))
}

#[instrument(skip_all)]
pub async fn post_bark_evidence_attestation(
    State(state): State<MintState>,
    Json(payload): Json<FederationSignedRequest<FederationBarkEvidenceAttestationRequest>>,
) -> Result<Json<FederationSignedRequest<FederationBarkEvidenceAttestationResponse>>, Response> {
    let kind = FederationRestMessageKind::ApplicationEvidenceAttestation;
    let verified = admit_private_request(&state, &payload, kind).await?;
    state
        .ensure_federation_ready_for_authoritative_read()
        .await
        .map_err(bark_secret_unavailable_response)?;
    let runtime_config = federation_runtime_config(&state)?;
    let public_config = runtime_config.public_config().clone();
    let federation_id = runtime_config.federation_id();
    let local_member = runtime_config.private_config().member_id;
    payload
        .payload
        .validate_relayed(&public_config, verified.sender())
        .map_err(federation_error_response)?;
    let action = state
        .mint
        .federation_bark_action(payload.payload.binding.action_id)
        .await
        .map_err(bark_secret_unavailable_response)?
        .ok_or_else(|| {
            (
                StatusCode::BAD_REQUEST,
                "federation Bark evidence action is unknown",
            )
                .into_response()
        })?;
    let members = public_config.members.keys().copied().collect::<Vec<_>>();
    let actor_index = members
        .iter()
        .position(|member| *member == action.actor)
        .ok_or_else(bark_secret_store_unavailable_response)?;
    let local_index = members
        .iter()
        .position(|member| *member == local_member)
        .ok_or_else(bark_secret_store_unavailable_response)?;
    let collector_fallback_round = if local_index >= actor_index {
        local_index - actor_index
    } else {
        members.len() - actor_index + local_index
    };
    let collector_fallback_round = u32::try_from(collector_fallback_round)
        .map_err(|_| bark_secret_store_unavailable_response())?;
    let store = state
        .federation
        .bark_receive_secrets()
        .ok_or_else(bark_secret_store_unavailable_response)?;
    let request = payload.payload.clone();
    let store_config = public_config.clone();
    tokio::task::spawn_blocking(move || {
        store.persist_evidence_attestation(&request.binding, request.attestation, &store_config)?;
        Ok::<_, cdk_federation::FederatedBarkError>(())
    })
    .await
    .map_err(|_| bark_secret_store_unavailable_response())?
    .map_err(bark_evidence_request_response)?;
    let response_payload = FederationBarkEvidenceAttestationResponse::new(
        &payload.payload.binding,
        payload.payload.attestation.signer,
    )
    .map_err(federation_error_response)?;
    state
        .record_federation_sequence(kind, verified.sender(), verified.sequence())
        .await;
    let runtime_config = federation_runtime_config(&state)?;
    let response = FederationSignedRequest::new_signed(
        federation_id,
        verified.sender(),
        verified.sequence(),
        kind,
        response_payload,
        runtime_config.private_config(),
    )
    .map_err(federation_error_response)?;
    let phase = payload.payload.binding.phase;
    let quote = payload.payload.binding.quote.parse::<QuoteId>();
    if let Ok(quote) = quote {
        let wake_state = state.clone();
        state
            .federation
            .tasks
            .spawn("Bark evidence wake", false, async move {
                let result = match phase {
                FederationBarkEvidencePhase::ReceiveHtlcsReady
                | FederationBarkEvidencePhase::ReceiveClaimed => {
                    super::bark_incoming::advance_federated_bark_mint_quote_from_evidence_delivery(
                        &wake_state,
                        &quote,
                        collector_fallback_round,
                    )
                    .await
                }
                FederationBarkEvidencePhase::SendSucceeded => {
                    super::bark_outgoing::advance_federated_bark_melt_quote_from_evidence_delivery(
                        &wake_state,
                        &quote,
                        collector_fallback_round,
                    )
                    .await
                }
            };
                if let Err(error) = result {
                    tracing::debug!(error = %error, "Bark evidence wake did not advance quote");
                }
            });
    }
    Ok(Json(response))
}

fn bark_secret_request_response(error: cdk_federation::FederatedBarkError) -> Response {
    let status = match error {
        cdk_federation::FederatedBarkError::ReceiveSecretRequestInvalid
        | cdk_federation::FederatedBarkError::ReceiveSecretConflict
        | cdk_federation::FederatedBarkError::ReceiveSecretRecordInvalid => StatusCode::BAD_REQUEST,
        _ => StatusCode::SERVICE_UNAVAILABLE,
    };
    tracing::warn!(
        status = status.as_u16(),
        "Federation Bark receive-secret request failed"
    );
    (status, "federation Bark receive-secret request failed").into_response()
}

fn bark_evidence_request_response(error: cdk_federation::FederatedBarkError) -> Response {
    let status = match error {
        cdk_federation::FederatedBarkError::EvidenceAttestationConflict
        | cdk_federation::FederatedBarkError::EvidenceAttestationInvalid => StatusCode::BAD_REQUEST,
        _ => StatusCode::SERVICE_UNAVAILABLE,
    };
    tracing::warn!(
        status = status.as_u16(),
        "Federation Bark evidence-attestation request failed"
    );
    (
        status,
        "federation Bark evidence-attestation request failed",
    )
        .into_response()
}

fn bark_secret_store_unavailable_response() -> Response {
    (
        StatusCode::SERVICE_UNAVAILABLE,
        "federation Bark receive-secret store is unavailable",
    )
        .into_response()
}

fn bark_secret_unavailable_response(error: cdk::Error) -> Response {
    let _ = error;
    (
        StatusCode::SERVICE_UNAVAILABLE,
        "federation Bark receive-secret service is unavailable",
    )
        .into_response()
}
