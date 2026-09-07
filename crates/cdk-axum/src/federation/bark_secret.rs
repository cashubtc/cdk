//! Authenticated private transport for protected Bark receive secrets.

use std::time::{SystemTime, UNIX_EPOCH};

use axum::extract::{Json, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use cdk::federation::{
    FederationBarkEvidenceAttestationRequest, FederationBarkEvidenceAttestationResponse,
    FederationBarkEvidencePhase, FederationBarkPrepareReceiveSecretRequest,
    FederationBarkReceiveSecret, FederationBarkReceiveSecretAcknowledgement,
    FederationBarkReceiveSecretBinding, FederationConfig, FederationRestMessageKind,
    FederationSignedRequest, MemberConfigPublic,
};
use cdk::mint::QuoteId;
use futures::future::join_all;
use tracing::instrument;

use crate::MintState;

use super::{admit_private_request, federation_error_response, federation_runtime_config};

/// Bounded collector for one exact protected receive-secret replication set.
#[derive(Debug)]
#[allow(dead_code)]
pub(crate) struct FederationBarkReceiveSecretCoordinator<'a, T> {
    client: &'a super::peer_client::FederationRestPeerClient<T>,
}

#[allow(dead_code)]
impl<'a, T> FederationBarkReceiveSecretCoordinator<'a, T>
where
    T: cdk_http_client::Transport,
{
    pub(crate) fn new(client: &'a super::peer_client::FederationRestPeerClient<T>) -> Self {
        Self { client }
    }

    /// Replicate an exact preimage to a sorted eligible set and require a
    /// signature threshold including the designated invoice actor.
    pub(crate) async fn distribute(
        &self,
        config: &FederationConfig,
        binding: &FederationBarkReceiveSecretBinding,
        secret: &FederationBarkReceiveSecret,
        eligible: &[MemberConfigPublic],
        initial: Vec<FederationBarkReceiveSecretAcknowledgement>,
    ) -> anyhow::Result<Vec<FederationBarkReceiveSecretAcknowledgement>> {
        binding.validate(config)?;
        let mut previous = None;
        for member in eligible {
            if previous.is_some_and(|previous| previous >= member.member_id)
                || config.member(member.member_id) != Some(member)
            {
                anyhow::bail!("Bark receive-secret recipients are not a sorted configured set");
            }
            previous = Some(member.member_id);
        }
        let mut acknowledgements = initial;
        let results = join_all(eligible.iter().map(|member| {
            let request = FederationBarkPrepareReceiveSecretRequest::new(
                binding.clone(),
                member.member_id,
                *secret.as_bytes(),
            );
            async move {
                (
                    member.member_id,
                    self.client
                        .prepare_bark_receive_secret(member, &request)
                        .await,
                )
            }
        }))
        .await;
        for (member, result) in results {
            match result {
                Ok(acknowledgement) => acknowledgements.push(acknowledgement),
                Err(error) => {
                    tracing::warn!(
                        member = %member,
                        error = %error,
                        "Bark receive-secret replication failed"
                    );
                }
            }
        }
        acknowledgements.sort_by_key(|acknowledgement| acknowledgement.member);
        if acknowledgements
            .windows(2)
            .any(|pair| pair[0].member >= pair[1].member)
        {
            anyhow::bail!("Bark receive-secret acknowledgements are not unique");
        }
        for acknowledgement in &acknowledgements {
            acknowledgement.verify(config, binding)?;
        }
        if acknowledgements.len() < usize::from(config.threshold_params.signature_threshold)
            || !acknowledgements
                .iter()
                .any(|acknowledgement| acknowledgement.member == binding.actor)
        {
            anyhow::bail!("Bark receive-secret acknowledgement threshold was not reached");
        }
        Ok(acknowledgements)
    }
}

#[instrument(skip_all)]
pub(super) async fn post_bark_receive_secret(
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
pub(super) async fn post_bark_evidence_attestation(
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
    let projection = state
        .mint
        .federation_bark_projection_snapshot()
        .await
        .map_err(bark_secret_unavailable_response)?;
    let action = projection
        .actions
        .iter()
        .find(|action| action.action_id == payload.payload.binding.action_id)
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
        Ok::<_, cdk_bark::Error>(())
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
            .payment_observer
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

fn bark_secret_request_response(error: cdk_bark::Error) -> Response {
    let status = match error {
        cdk_bark::Error::ReceiveSecretRequestInvalid
        | cdk_bark::Error::ReceiveSecretConflict
        | cdk_bark::Error::ReceiveSecretRecordInvalid => StatusCode::BAD_REQUEST,
        _ => StatusCode::SERVICE_UNAVAILABLE,
    };
    tracing::warn!(
        status = status.as_u16(),
        "Federation Bark receive-secret request failed"
    );
    (status, "federation Bark receive-secret request failed").into_response()
}

fn bark_evidence_request_response(error: cdk_bark::Error) -> Response {
    let status = match error {
        cdk_bark::Error::EvidenceAttestationConflict
        | cdk_bark::Error::EvidenceAttestationInvalid => StatusCode::BAD_REQUEST,
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
