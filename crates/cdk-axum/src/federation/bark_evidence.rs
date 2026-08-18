//! Shared private-attestation collection for federated Bark evidence.

use cdk::federation::{
    FederationBarkActionId, FederationBarkEvidenceAttestationRequest,
    FederationBarkEvidenceBinding, FederationBarkEvidenceCertificate, FederationBarkEvidencePhase,
    FederationBarkMemberObservation, FederationOperationId, MemberId,
};

use crate::MintState;

use super::latency::{
    FederationLatencyBackend, FederationLatencyCorrelation, FederationLatencyDirection,
    FederationLatencyOutcome, FederationLatencyPhase, FederationLatencyTimer,
};
use super::peer_client::FederationRestPeerClient;

#[allow(clippy::too_many_arguments)]
pub(super) async fn collect_bark_evidence_certificate(
    state: &MintState,
    action_id: FederationBarkActionId,
    actor: MemberId,
    phase: FederationBarkEvidencePhase,
    evidence_operation_id: FederationOperationId,
    attempt: u32,
    observation: &FederationBarkMemberObservation,
    required: u16,
    collector_fallback_round: u32,
    direction: FederationLatencyDirection,
) -> Result<Option<FederationBarkEvidenceCertificate>, cdk::Error> {
    let timer = FederationLatencyTimer::start(
        FederationLatencyCorrelation::Action,
        action_id.to_string(),
        FederationLatencyBackend::Bark,
        direction,
        FederationLatencyPhase::BarkEvidenceCollection,
    );
    let result = collect_bark_evidence_certificate_inner(
        state,
        actor,
        phase,
        evidence_operation_id,
        attempt,
        observation,
        required,
        collector_fallback_round,
    )
    .await;
    timer.finish(match &result {
        Ok(Some(_)) => FederationLatencyOutcome::Success,
        Ok(None) => FederationLatencyOutcome::Pending,
        Err(_) => FederationLatencyOutcome::Error,
    });
    result
}

#[allow(clippy::too_many_arguments)]
async fn collect_bark_evidence_certificate_inner(
    state: &MintState,
    actor: MemberId,
    phase: FederationBarkEvidencePhase,
    evidence_operation_id: FederationOperationId,
    attempt: u32,
    observation: &FederationBarkMemberObservation,
    required: u16,
    collector_fallback_round: u32,
) -> Result<Option<FederationBarkEvidenceCertificate>, cdk::Error> {
    let federation = state.mint.federation_config().ok_or(cdk::Error::Internal)?;
    let (binding, _) = persist_local_bark_evidence_attestation(
        state,
        phase,
        evidence_operation_id,
        attempt,
        observation,
    )?;
    let store = state
        .federation
        .bark_receive_secrets()
        .ok_or(cdk::Error::Internal)?;
    let collector = bark_evidence_collector(
        federation.public_config().members.keys().copied(),
        actor,
        collector_fallback_round,
    )
    .ok_or(cdk::Error::Internal)?;
    let attestations = store
        .evidence_attestations(&binding, federation.public_config())
        .map_err(|_| cdk::Error::Internal)?;

    if federation.member_id() != collector {
        let collector_member = federation
            .public_config()
            .member(collector)
            .ok_or(cdk::Error::Internal)?;
        let peer_client = FederationRestPeerClient::new_with_validated_config(
            state.federation.transport.clone(),
            federation.public_config(),
            federation.private_config().clone(),
            state
                .federation
                .peer_sync_config
                .ok_or(cdk::Error::Internal)?,
        )
        .map_err(|_| cdk::Error::Internal)?;
        for attestation in attestations {
            let request =
                FederationBarkEvidenceAttestationRequest::new(binding.clone(), attestation);
            if peer_client
                .deliver_bark_evidence_attestation(collector_member, &request)
                .await
                .is_err()
            {
                tracing::debug!(
                    collector = %collector,
                    "Bark evidence attestation primary delivery failed"
                );
                if let Some(retry_collector) = bark_evidence_retry_collector(
                    federation.public_config().members.keys().copied(),
                    actor,
                    collector_fallback_round,
                    required,
                ) {
                    let retry_member = federation
                        .public_config()
                        .member(retry_collector)
                        .ok_or(cdk::Error::Internal)?;
                    if peer_client
                        .deliver_bark_evidence_attestation(retry_member, &request)
                        .await
                        .is_err()
                    {
                        tracing::debug!(
                            collector = %retry_collector,
                            "Bark evidence attestation remains durable for a later delivery retry"
                        );
                    }
                }
            }
        }
        return Ok(None);
    }

    if attestations.len() < usize::from(required) {
        return Ok(None);
    }
    FederationBarkEvidenceCertificate::new(
        binding,
        attestations,
        federation.public_config(),
        required,
    )
    .map(Some)
    .map_err(cdk::Error::from)
}

pub(super) fn persist_local_bark_evidence_attestation(
    state: &MintState,
    phase: FederationBarkEvidencePhase,
    evidence_operation_id: FederationOperationId,
    attempt: u32,
    observation: &FederationBarkMemberObservation,
) -> Result<
    (
        FederationBarkEvidenceBinding,
        cdk::federation::FederationBarkEvidenceAttestation,
    ),
    cdk::Error,
> {
    let federation = state.mint.federation_config().ok_or(cdk::Error::Internal)?;
    let binding = FederationBarkEvidenceBinding::from_observation(
        federation.public_config(),
        phase,
        evidence_operation_id,
        attempt,
        observation,
    )?;
    let attestation = cdk::federation::FederationBarkEvidenceAttestation::new_signed(
        &binding,
        federation.public_config(),
        federation.private_config(),
    )?;
    state
        .federation
        .bark_receive_secrets()
        .ok_or(cdk::Error::Internal)?
        .persist_evidence_attestation(&binding, attestation, federation.public_config())
        .map_err(|_| cdk::Error::Internal)?;
    Ok((binding, attestation))
}

pub(super) fn bark_evidence_collector(
    members: impl IntoIterator<Item = MemberId>,
    actor: MemberId,
    fallback_round: u32,
) -> Option<MemberId> {
    let members = members.into_iter().collect::<Vec<_>>();
    let actor_index = members.iter().position(|member| *member == actor)?;
    let offset = usize::try_from(fallback_round)
        .unwrap_or(usize::MAX)
        .min(members.len().saturating_sub(1));
    members.get((actor_index + offset) % members.len()).copied()
}

pub(super) fn scheduled_bark_evidence_collector_round(fallback_round: u32, required: u16) -> u32 {
    match fallback_round {
        0 => 0,
        fallback_round => u32::from(required).saturating_add(fallback_round - 1),
    }
}

pub(super) fn bark_evidence_retry_collector(
    members: impl IntoIterator<Item = MemberId>,
    actor: MemberId,
    current_round: u32,
    required: u16,
) -> Option<MemberId> {
    let members = members.into_iter().collect::<Vec<_>>();
    let last_round = u32::try_from(members.len().checked_sub(1)?).ok()?;
    let retry_round = current_round.saturating_add(1).max(u32::from(required));
    if retry_round > last_round {
        return None;
    }
    bark_evidence_collector(members, actor, retry_round)
}
