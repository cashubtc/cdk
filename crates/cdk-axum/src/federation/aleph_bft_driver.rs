use cdk::federation::{
    FederationAlephBftFinalizedUnitRequest, FederationAlephBftFinalizedUnitResponse,
    FederationAlephBftUnitPayload, FederationConsensusBackendKind,
    FederationSubmitOperationResponse, MemberConfigPublic, MemberId,
};
use cdk_http_client::Transport;

use crate::MintState;

use super::peer_client::FederationRestPeerClient;

/// Result of one local AlephBFT finalized-unit broadcast round.
#[derive(Debug, Clone)]
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) struct FederationAlephBftFinalizedUnitBroadcastOutcome {
    /// Canonical unit payload accepted by the round.
    pub unit: FederationAlephBftUnitPayload,
    /// Remote members that accepted the exact unit.
    pub remote_accepted_members: Vec<MemberId>,
    /// Remote members that failed to accept the unit in this round.
    pub remote_failed_members: Vec<MemberId>,
    /// Responses returned by accepting remote members.
    pub remote_responses: Vec<FederationAlephBftFinalizedUnitResponse>,
    /// Local operation responses after draining and applying the unit.
    pub local_responses: Vec<FederationSubmitOperationResponse>,
}

/// Broadcast the next previewed local AlephBFT unit and apply it after quorum delivery.
///
/// This is a driver-facing handoff helper, not a production BFT ordering engine.
/// The live AlephBFT task still owns finality. This helper protects the CDK
/// boundary around that task: preview without draining, deliver the exact unit
/// over signed private REST, and only drain/apply the local queue once enough
/// federation members have accepted the same payload.
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) async fn broadcast_next_aleph_bft_finalized_unit_round<T>(
    state: &MintState,
    client: &FederationRestPeerClient<T>,
    peers: &[MemberConfigPublic],
) -> Result<Option<FederationAlephBftFinalizedUnitBroadcastOutcome>, cdk::Error>
where
    T: Transport,
{
    let config = state.mint.federation_config().ok_or_else(|| {
        cdk::Error::Custom("AlephBFT broadcast requested without federation config".to_string())
    })?;
    if config.consensus_backend() != FederationConsensusBackendKind::AlephBft {
        return Err(cdk::Error::Custom(format!(
            "AlephBFT broadcast requested for {} consensus backend",
            config.consensus_backend()
        )));
    }

    let unit = {
        let consensus = state.federation_consensus.as_ref().ok_or_else(|| {
            cdk::Error::Custom("federation mint is missing its consensus handle".to_string())
        })?;
        let consensus = consensus.lock().await;
        consensus.preview_next_aleph_bft_unit_payload()?
    };
    let Some(unit) = unit else {
        return Ok(None);
    };

    let request = FederationAlephBftFinalizedUnitRequest::new(unit.clone());
    let local_member = config.member_id();
    let mut remote_accepted_members = Vec::new();
    let mut remote_failed_members = Vec::new();
    let mut remote_responses = Vec::new();
    for peer in peers {
        if peer.member_id == local_member {
            continue;
        }
        match client.submit_aleph_bft_finalized_unit(peer, &request).await {
            Ok(response) => {
                remote_accepted_members.push(peer.member_id);
                remote_responses.push(response);
            }
            Err(err) => {
                tracing::warn!(
                    member_id = %peer.member_id,
                    unit_index = unit.unit_index,
                    body_hash = %unit.body_hash,
                    "AlephBFT finalized-unit broadcast failed: {}",
                    err
                );
                remote_failed_members.push(peer.member_id);
            }
        }
    }

    let required = usize::from(config.public_config().threshold_params.consensus_threshold);
    let accepted_with_local = remote_accepted_members.len().saturating_add(1);
    if accepted_with_local < required {
        return Err(cdk::Error::Custom(format!(
            "AlephBFT finalized-unit broadcast reached {accepted_with_local} of {required} required members"
        )));
    }

    let local_responses = state
        .drain_and_apply_local_federation_aleph_bft_finalized_unit(&unit)
        .await?;

    Ok(Some(FederationAlephBftFinalizedUnitBroadcastOutcome {
        unit,
        remote_accepted_members,
        remote_failed_members,
        remote_responses,
        local_responses,
    }))
}
