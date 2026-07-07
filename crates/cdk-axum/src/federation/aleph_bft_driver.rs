use std::collections::BTreeSet;

use cdk::federation::{
    FederationAlephBftFinalizedUnitRequest, FederationAlephBftFinalizedUnitResponse,
    FederationAlephBftUnitPayload, FederationConsensusBackendKind,
    FederationSubmitOperationResponse, MemberConfigPublic, MemberId,
};
use cdk_http_client::{Async, Transport};

use crate::MintState;

use super::peer_client::FederationRestPeerClient;
use super::sync_config::{federation_peer_sync_interval, FederationPeerSyncConfig};

/// Bounded work policy for one AlephBFT driver tick.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) struct FederationAlephBftDriverTickConfig {
    /// Maximum finalized units a single driver wakeup may broadcast and apply.
    pub max_finalized_units_per_tick: usize,
    /// Maximum remote peer deliveries attempted for one finalized unit.
    pub max_remote_peer_deliveries_per_unit: usize,
}

impl Default for FederationAlephBftDriverTickConfig {
    fn default() -> Self {
        Self {
            max_finalized_units_per_tick: 1,
            max_remote_peer_deliveries_per_unit: 64,
        }
    }
}

impl FederationAlephBftDriverTickConfig {
    fn validate(self) -> Result<(), cdk::Error> {
        if self.max_finalized_units_per_tick == 0 {
            return Err(cdk::Error::Custom(
                "AlephBFT driver max finalized units per tick must be greater than zero"
                    .to_string(),
            ));
        }
        if self.max_remote_peer_deliveries_per_unit == 0 {
            return Err(cdk::Error::Custom(
                "AlephBFT driver max remote peer deliveries per unit must be greater than zero"
                    .to_string(),
            ));
        }

        Ok(())
    }
}

/// Summary of bounded AlephBFT driver work completed during one tick.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) struct FederationAlephBftDriverTickOutcome {
    /// Number of finalized units broadcast and applied locally.
    pub finalized_units: usize,
    /// Number of local operation responses delivered by applied units.
    pub local_responses: usize,
    /// Number of remote member acknowledgements collected.
    pub remote_accepted_members: usize,
    /// Number of remote member delivery failures seen.
    pub remote_failed_members: usize,
    /// Number of remote members intentionally left for later catch-up.
    pub remote_skipped_members: usize,
    /// True when the local proposal queue became empty during this tick.
    pub reached_idle: bool,
}

/// Router-startup decision for the AlephBFT driver loop.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FederationAlephBftDriverStartupDecision {
    /// The mint is not configured as a federation member.
    NoFederationConfig,
    /// Federation is configured with a different consensus backend.
    NonAlephBftBackend {
        /// Configured consensus backend.
        backend: FederationConsensusBackendKind,
    },
    /// AlephBFT is configured, but no remote peer can receive finalized units.
    NoRemotePeers {
        /// Local federation member.
        local_member: MemberId,
    },
    /// Spawn the driver loop for the configured remote peers.
    Spawn {
        /// Local federation member.
        local_member: MemberId,
        /// Number of remote peers the driver should broadcast to.
        peer_count: usize,
    },
}

/// Decide whether router startup should spawn the AlephBFT driver loop.
pub(crate) fn federation_aleph_bft_driver_startup_decision(
    state: &MintState,
) -> FederationAlephBftDriverStartupDecision {
    let Some(config) = state.mint.federation_config() else {
        return FederationAlephBftDriverStartupDecision::NoFederationConfig;
    };
    let backend = config.consensus_backend();
    if backend != FederationConsensusBackendKind::AlephBft {
        return FederationAlephBftDriverStartupDecision::NonAlephBftBackend { backend };
    }

    let local_member = config.member_id();
    let peer_count = config
        .public_config()
        .members
        .values()
        .filter(|member| member.member_id != local_member)
        .count();
    if peer_count == 0 {
        return FederationAlephBftDriverStartupDecision::NoRemotePeers { local_member };
    }

    FederationAlephBftDriverStartupDecision::Spawn {
        local_member,
        peer_count,
    }
}

/// Spawn the bounded AlephBFT finalized-unit driver loop.
///
/// The loop sleeps on the configured interval, runs a bounded tick, and leaves
/// failed proposals queued for later retries.
pub(crate) fn spawn_federation_aleph_bft_driver_loop_with_config(
    state: MintState,
    sync_config: FederationPeerSyncConfig,
    driver_config: FederationAlephBftDriverTickConfig,
) {
    if let Err(err) = sync_config.validate() {
        tracing::error!("Could not start AlephBFT federation driver loop: {}", err);
        return;
    }
    if let Err(err) = driver_config.validate() {
        tracing::error!("Could not start AlephBFT federation driver loop: {}", err);
        return;
    }

    match federation_aleph_bft_driver_startup_decision(&state) {
        FederationAlephBftDriverStartupDecision::Spawn { .. } => {}
        FederationAlephBftDriverStartupDecision::NoFederationConfig
        | FederationAlephBftDriverStartupDecision::NonAlephBftBackend { .. }
        | FederationAlephBftDriverStartupDecision::NoRemotePeers { .. } => return,
    }

    let Some(config) = state.mint.federation_config().cloned() else {
        return;
    };
    let public_config = config.public_config().clone();
    let local_member = config.member_id();
    let peers = public_config
        .members
        .values()
        .filter(|member| member.member_id != local_member)
        .cloned()
        .collect::<Vec<_>>();
    if peers.is_empty() {
        return;
    }

    let private_config = config.private_config().clone();
    tokio::spawn(async move {
        let client = match FederationRestPeerClient::new_with_config(
            Async::default(),
            &public_config,
            private_config,
            sync_config,
        ) {
            Ok(client) => client,
            Err(err) => {
                tracing::error!("Could not start AlephBFT federation driver loop: {}", err);
                return;
            }
        };
        let mut interval = federation_peer_sync_interval(sync_config);

        loop {
            interval.tick().await;
            match run_federation_aleph_bft_driver_tick(&state, &client, &peers, driver_config).await
            {
                Ok(outcome) => {
                    if outcome.finalized_units > 0 {
                        tracing::debug!(
                            finalized_units = outcome.finalized_units,
                            local_responses = outcome.local_responses,
                            remote_accepted_members = outcome.remote_accepted_members,
                            remote_failed_members = outcome.remote_failed_members,
                            remote_skipped_members = outcome.remote_skipped_members,
                            reached_idle = outcome.reached_idle,
                            "AlephBFT federation driver tick completed"
                        );
                    }
                }
                Err(err) => {
                    tracing::warn!("AlephBFT federation driver tick failed: {}", err);
                }
            }
        }
    });
}

/// Run one bounded AlephBFT driver tick.
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) async fn run_federation_aleph_bft_driver_tick<T>(
    state: &MintState,
    client: &FederationRestPeerClient<T>,
    peers: &[MemberConfigPublic],
    driver_config: FederationAlephBftDriverTickConfig,
) -> Result<FederationAlephBftDriverTickOutcome, cdk::Error>
where
    T: Transport,
{
    driver_config.validate()?;

    let mut outcome = FederationAlephBftDriverTickOutcome::default();
    for _ in 0..driver_config.max_finalized_units_per_tick {
        let Some(broadcast) = broadcast_next_aleph_bft_finalized_unit_round_with_config(
            state,
            client,
            peers,
            driver_config,
        )
        .await?
        else {
            outcome.reached_idle = true;
            break;
        };

        outcome.finalized_units = outcome.finalized_units.saturating_add(1);
        outcome.local_responses = outcome
            .local_responses
            .saturating_add(broadcast.local_responses.len());
        outcome.remote_accepted_members = outcome
            .remote_accepted_members
            .saturating_add(broadcast.remote_accepted_members.len());
        outcome.remote_failed_members = outcome
            .remote_failed_members
            .saturating_add(broadcast.remote_failed_members.len());
        outcome.remote_skipped_members = outcome
            .remote_skipped_members
            .saturating_add(broadcast.remote_skipped_members.len());
    }

    Ok(outcome)
}

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
    /// Remote members not contacted because the delivery round had enough quorum.
    pub remote_skipped_members: Vec<MemberId>,
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
    broadcast_next_aleph_bft_finalized_unit_round_with_config(
        state,
        client,
        peers,
        FederationAlephBftDriverTickConfig::default(),
    )
    .await
}

/// Broadcast the next previewed local AlephBFT unit with an explicit driver policy.
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) async fn broadcast_next_aleph_bft_finalized_unit_round_with_config<T>(
    state: &MintState,
    client: &FederationRestPeerClient<T>,
    peers: &[MemberConfigPublic],
    driver_config: FederationAlephBftDriverTickConfig,
) -> Result<Option<FederationAlephBftFinalizedUnitBroadcastOutcome>, cdk::Error>
where
    T: Transport,
{
    driver_config.validate()?;
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

    let required = usize::from(config.public_config().threshold_params.consensus_threshold);
    let required_remote = required.saturating_sub(1);
    if driver_config.max_remote_peer_deliveries_per_unit < required_remote {
        return Err(cdk::Error::Custom(format!(
            "AlephBFT finalized-unit peer delivery limit {} is below {required_remote} remote members required for quorum",
            driver_config.max_remote_peer_deliveries_per_unit
        )));
    }

    let request = FederationAlephBftFinalizedUnitRequest::new(unit.clone());
    let local_member = config.member_id();
    let mut remote_accepted_members = Vec::new();
    let mut remote_failed_members = Vec::new();
    let mut remote_responses = Vec::new();
    let mut attempted_members = BTreeSet::new();
    for peer in peers {
        if peer.member_id == local_member {
            continue;
        }
        if attempted_members.contains(&peer.member_id) {
            continue;
        }
        if attempted_members.len() >= driver_config.max_remote_peer_deliveries_per_unit {
            break;
        }
        attempted_members.insert(peer.member_id);
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
        if remote_accepted_members.len().saturating_add(1) >= required {
            break;
        }
    }

    let accepted_with_local = remote_accepted_members.len().saturating_add(1);
    if accepted_with_local < required {
        let remote_skipped_members =
            remote_members_not_attempted(peers, local_member, &attempted_members);
        return Err(cdk::Error::Custom(format!(
            "AlephBFT finalized-unit broadcast reached {accepted_with_local} of {required} required members; remote failures {}; remote skipped {}",
            remote_failed_members.len(),
            remote_skipped_members.len()
        )));
    }
    let remote_skipped_members =
        remote_members_not_attempted(peers, local_member, &attempted_members);

    let local_responses = state
        .drain_and_apply_local_federation_aleph_bft_finalized_unit(&unit)
        .await?;

    Ok(Some(FederationAlephBftFinalizedUnitBroadcastOutcome {
        unit,
        remote_accepted_members,
        remote_failed_members,
        remote_responses,
        remote_skipped_members,
        local_responses,
    }))
}

fn remote_members_not_attempted(
    peers: &[MemberConfigPublic],
    local_member: MemberId,
    attempted_members: &BTreeSet<MemberId>,
) -> Vec<MemberId> {
    peers
        .iter()
        .filter(|peer| peer.member_id != local_member)
        .filter(|peer| !attempted_members.contains(&peer.member_id))
        .map(|peer| peer.member_id)
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}
