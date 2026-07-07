use std::collections::BTreeSet;

use cdk::federation::{
    FederationConsensusEngine, FederationJournalCatchUpRequest, FederationJournalCatchUpResponse,
    FederationJournalEntry, FederationOperationEnvelope, FederationPeerStatus,
    FederationSessionOutcome, FederationStateDigest, FederationSubmitOperationRequest,
    FederationSubmitOperationResponse, MemberConfigPublic, MemberId,
};
use cdk_http_client::{Async, Transport};
use tokio::time::{self, Instant};

use crate::MintState;

use super::peer_client::FederationRestPeerClient;
use super::sync_config::{
    federation_peer_sync_disabled_by_env, federation_peer_sync_interval,
    FederationPeerReconnectBackoff, FederationPeerSyncConfig,
};
#[cfg(feature = "prometheus")]
use super::u64_to_i64_saturating;
use super::{
    local_peer_order_status, local_peer_status, session_outcome_quorum_status,
    verify_session_outcome_evidence_entries, SessionOutcomeQuorumStatus,
};

pub(crate) fn spawn_federation_peer_sync_loop_with_config(
    state: MintState,
    sync_config: FederationPeerSyncConfig,
) {
    if let Err(err) = sync_config.validate() {
        tracing::error!("Could not start federation peer sync loop: {}", err);
        return;
    }
    if federation_peer_sync_disabled_by_env() {
        tracing::warn!("Federation peer sync loop disabled by environment");
        return;
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
                tracing::error!("Could not start federation peer sync loop: {}", err);
                return;
            }
        };
        let mut interval = federation_peer_sync_interval(sync_config);
        let mut reconnect_backoff = FederationPeerReconnectBackoff::default();

        loop {
            interval.tick().await;
            run_federation_peer_sync_round_with_backoff(
                &state,
                &client,
                &peers,
                &mut reconnect_backoff,
            )
            .await;
        }
    });
}

#[cfg_attr(not(test), allow(dead_code))]
pub(super) async fn run_federation_peer_sync_round<T>(
    state: &MintState,
    client: &FederationRestPeerClient<T>,
    peers: &[MemberConfigPublic],
) -> usize
where
    T: Transport,
{
    run_federation_peer_sync_attempts(state, client, peers, None).await
}

pub(super) async fn run_federation_peer_sync_round_with_backoff<T>(
    state: &MintState,
    client: &FederationRestPeerClient<T>,
    peers: &[MemberConfigPublic],
    backoff: &mut FederationPeerReconnectBackoff,
) -> usize
where
    T: Transport,
{
    run_federation_peer_sync_attempts(state, client, peers, Some(backoff)).await
}

async fn run_federation_peer_sync_attempts<T>(
    state: &MintState,
    client: &FederationRestPeerClient<T>,
    peers: &[MemberConfigPublic],
    mut backoff: Option<&mut FederationPeerReconnectBackoff>,
) -> usize
where
    T: Transport,
{
    let mut synced = 0;
    let mut evidence_peers = Vec::with_capacity(peers.len());
    for peer in peers {
        let now = Instant::now();
        if let Some(backoff) = backoff.as_ref() {
            if !backoff.should_attempt(peer.member_id, now) {
                tracing::debug!(
                    member_id = %peer.member_id,
                    "Federation peer sync skipped until reconnect backoff expires"
                );
                continue;
            }
        }
        match client.sync_once_from_member(state, peer).await {
            Ok(status) => {
                synced += 1;
                evidence_peers.push(peer);
                if let Some(backoff) = backoff.as_mut() {
                    backoff.record_success(peer.member_id);
                }
                record_federation_peer_sync_metrics(
                    state,
                    peer.member_id,
                    true,
                    Some(status.operation_count),
                );
                tracing::debug!(
                    member_id = %peer.member_id,
                    operation_count = status.operation_count,
                    latest_index = ?status.latest_index,
                    "Federation peer sync completed"
                );
            }
            Err(err) => {
                let reconnect_after = backoff.as_mut().map(|backoff| {
                    backoff.record_failure(peer.member_id, client.sync_config, Instant::now())
                });
                record_federation_peer_sync_metrics(state, peer.member_id, false, None);
                match reconnect_after {
                    Some(delay) => {
                        tracing::warn!(
                            member_id = %peer.member_id,
                            reconnect_after_ms = delay.as_millis(),
                            "Federation peer sync failed: {}",
                            err
                        );
                    }
                    None => {
                        tracing::warn!(
                            member_id = %peer.member_id,
                            "Federation peer sync failed: {}",
                            err
                        );
                    }
                }
            }
        }
    }
    if evidence_peers.is_empty() {
        return synced;
    }

    let collected =
        collect_session_outcome_evidence_round(state, client, &evidence_peers, backoff).await;
    if collected > 0 {
        tracing::debug!(
            session_outcomes = collected,
            "Federation peer sync collected session outcome evidence"
        );
    }
    synced
}

#[cfg(feature = "prometheus")]
fn record_federation_peer_sync_metrics(
    state: &MintState,
    peer: MemberId,
    connected: bool,
    operation_count: Option<u64>,
) {
    let Some(config) = state.mint.federation_config() else {
        return;
    };
    let member = config.member_id().to_string();
    let peer = peer.to_string();
    cdk_prometheus::METRICS.set_federation_peer_connected(&member, &peer, connected);
    if let Some(operation_count) = operation_count {
        cdk_prometheus::METRICS.set_federation_peer_operation_count(
            &member,
            &peer,
            u64_to_i64_saturating(operation_count),
        );
    }
}

#[cfg(not(feature = "prometheus"))]
fn record_federation_peer_sync_metrics(
    _state: &MintState,
    _peer: MemberId,
    _connected: bool,
    _operation_count: Option<u64>,
) {
}

async fn collect_session_outcome_evidence_round<T>(
    state: &MintState,
    client: &FederationRestPeerClient<T>,
    peers: &[&MemberConfigPublic],
    mut backoff: Option<&mut FederationPeerReconnectBackoff>,
) -> usize
where
    T: Transport,
{
    let entries = match state
        .mint
        .localstore()
        .get_federation_journal_entries()
        .await
    {
        Ok(entries) => entries,
        Err(err) => {
            tracing::warn!(
                "Could not load local federation journal for evidence: {}",
                err
            );
            return 0;
        }
    };
    if entries.is_empty() {
        return 0;
    }

    let Some(runtime_config) = state.mint.federation_config() else {
        return 0;
    };
    let operation_count = entries.len() as u64;
    let mut quorum_status = match session_outcome_quorum_status(state).await {
        Ok(Some(quorum)) if quorum.from_index == 0 && quorum.operation_count >= operation_count => {
            state
                .mark_federation_healthy_if_session_outcome_verified(quorum.operation_count)
                .await;
            return 0;
        }
        Ok(quorum) => quorum,
        Err(err) => {
            tracing::warn!(
                "Could not inspect federation session outcome quorum: {}",
                err
            );
            None
        }
    };
    let entries_to_collect = unverified_session_outcome_entries(&entries, quorum_status.as_ref());
    if entries_to_collect.is_empty() {
        return 0;
    }

    let mut collected = match local_peer_status(state).await {
        Ok(peer_status) => {
            persist_local_session_outcome_evidence_chunks(
                state,
                runtime_config,
                entries_to_collect,
                peer_status.latest_order_digest,
                peer_status.state_digest,
                client.sync_config.session_outcome_evidence_chunk_size,
            )
            .await
        }
        Err(err) => {
            tracing::warn!(
                "Could not read local federation status for session outcome evidence: {}",
                err
            );
            0
        }
    };

    quorum_status = match session_outcome_quorum_status(state).await {
        Ok(Some(quorum)) if quorum.from_index == 0 && quorum.operation_count >= operation_count => {
            state
                .mark_federation_healthy_if_session_outcome_verified(quorum.operation_count)
                .await;
            return 0;
        }
        Ok(quorum) => quorum,
        Err(err) => {
            tracing::warn!(
                "Could not inspect federation session outcome quorum: {}",
                err
            );
            None
        }
    };

    for &peer in peers {
        let now = Instant::now();
        if let Some(backoff) = backoff.as_ref() {
            if !backoff.should_attempt(peer.member_id, now) {
                tracing::debug!(
                    member_id = %peer.member_id,
                    "Federation session outcome evidence skipped until reconnect backoff expires"
                );
                continue;
            }
        }
        let entries_to_collect =
            unverified_session_outcome_entries(&entries, quorum_status.as_ref());
        if entries_to_collect.is_empty() {
            break;
        }
        let evidence = collect_session_outcome_evidence_from_peer(
            state,
            client,
            peer,
            entries_to_collect,
            client.sync_config.session_outcome_evidence_chunk_size,
        )
        .await;
        collected += evidence.collected;
        if evidence.peer_failed {
            if let Some(backoff) = backoff.as_mut() {
                backoff.record_failure(peer.member_id, client.sync_config, Instant::now());
            }
        }
        quorum_status = match session_outcome_quorum_status(state).await {
            Ok(Some(quorum)) if quorum.from_index == 0 => Some(quorum),
            Ok(_) => None,
            Err(err) => {
                tracing::warn!(
                    "Could not inspect federation session outcome quorum after peer collection: {}",
                    err
                );
                None
            }
        };
    }

    mark_healthy_after_session_outcome_quorum(state, operation_count).await;

    collected
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
struct SessionOutcomeEvidenceCollection {
    collected: usize,
    peer_failed: bool,
}

fn unverified_session_outcome_entries<'a>(
    entries: &'a [FederationJournalEntry],
    quorum_status: Option<&SessionOutcomeQuorumStatus>,
) -> &'a [FederationJournalEntry] {
    let Some(quorum) = quorum_status else {
        return entries;
    };
    if quorum.from_index != 0 {
        return entries;
    }
    let next_index = quorum.latest_index.saturating_add(1);
    let start = entries
        .iter()
        .position(|entry| entry.index >= next_index)
        .unwrap_or(entries.len());
    &entries[start..]
}

pub(crate) async fn submit_operation_to_primary_if_needed(
    state: &MintState,
    envelope: FederationOperationEnvelope,
    sequence: Option<(MemberId, u64)>,
) -> Result<Option<FederationSubmitOperationResponse>, cdk::Error> {
    submit_operation_to_primary_if_needed_with_transport(
        state,
        envelope,
        sequence,
        Async::default(),
    )
    .await
}

pub(super) async fn submit_operation_to_primary_if_needed_with_transport<T>(
    state: &MintState,
    envelope: FederationOperationEnvelope,
    sequence: Option<(MemberId, u64)>,
    transport: T,
) -> Result<Option<FederationSubmitOperationResponse>, cdk::Error>
where
    T: Transport,
{
    let Some(runtime_config) = state.mint.federation_config() else {
        return Ok(None);
    };
    let Some(primary_member) = runtime_config.public_config().members.values().next() else {
        return Err(cdk::Error::Custom(
            "federation config has an empty member roster".to_string(),
        ));
    };
    if primary_member.member_id == runtime_config.member_id() {
        return Ok(None);
    }

    state.validate_federation_envelope_admission(&envelope)?;
    let request = FederationSubmitOperationRequest::new(envelope);
    request
        .ensure_admissible_size()
        .map_err(|err| cdk::Error::Custom(err.to_string()))?;
    let operation_id = request
        .operation_id()
        .map_err(|err| cdk::Error::Custom(err.to_string()))?;
    let client = FederationRestPeerClient::new_with_config(
        transport,
        runtime_config.public_config(),
        runtime_config.private_config().clone(),
        state.federation_peer_sync_config.unwrap_or_default(),
    )
    .map_err(|err| cdk::Error::Custom(err.to_string()))?;
    let response = client
        .submit_operation(primary_member, &request)
        .await
        .map_err(|err| cdk::Error::Custom(err.to_string()))?;

    client
        .sync_once_from_member(state, primary_member)
        .await
        .map_err(|err| cdk::Error::Custom(err.to_string()))?;
    let primary_peers = [primary_member];
    collect_session_outcome_evidence_round(state, &client, &primary_peers, None).await;

    if let Some((sender, sequence)) = sequence {
        state.persist_federation_sequence(sender, sequence).await?;
    }

    if response.operation_id != operation_id {
        return Err(cdk::Error::Custom(format!(
            "federation primary returned operation id {} for submitted operation {}",
            response.operation_id, operation_id
        )));
    }

    Ok(Some(response))
}

pub(super) async fn persist_local_session_outcome_evidence_chunks(
    state: &MintState,
    runtime_config: &cdk::mint::FederationRuntimeConfig,
    entries: &[FederationJournalEntry],
    latest_order_digest: cdk::federation::FederationJournalDigest,
    state_digest: FederationStateDigest,
    chunk_size: usize,
) -> usize {
    let mut persisted = 0;
    for chunk in entries.chunks(chunk_size) {
        let outcome = match FederationSessionOutcome::new_signed(
            runtime_config.federation_id(),
            chunk,
            latest_order_digest,
            state_digest,
            runtime_config.private_config(),
        ) {
            Ok(outcome) => outcome,
            Err(err) => {
                tracing::warn!(
                    "Could not sign local federation session outcome evidence: {}",
                    err
                );
                continue;
            }
        };
        match state.persist_federation_session_outcome(&outcome).await {
            Ok(()) => persisted += 1,
            Err(err) => {
                tracing::warn!(
                    "Could not persist local federation session outcome evidence: {}",
                    err
                );
            }
        }
    }

    persisted
}

async fn collect_session_outcome_evidence_from_peer<T>(
    state: &MintState,
    client: &FederationRestPeerClient<T>,
    peer: &MemberConfigPublic,
    entries: &[FederationJournalEntry],
    chunk_size: usize,
) -> SessionOutcomeEvidenceCollection
where
    T: Transport,
{
    let mut collected = 0;
    for chunk in entries.chunks(chunk_size) {
        let Some(first) = chunk.first() else {
            continue;
        };
        let request = FederationJournalCatchUpRequest::new(first.index)
            .with_limit(chunk.len() as u16)
            .with_known_order_digest(first.previous_order_digest);
        let response = match client.catch_up(peer, &request).await {
            Ok(response) => response,
            Err(err) => {
                tracing::warn!(
                    member_id = %peer.member_id,
                    "Federation session outcome evidence request failed: {}",
                    err
                );
                return SessionOutcomeEvidenceCollection {
                    collected,
                    peer_failed: true,
                };
            }
        };
        if let Err(err) = verify_session_outcome_evidence_entries(chunk, &response) {
            tracing::warn!(
                member_id = %peer.member_id,
                "Federation session outcome evidence did not match local journal: {}",
                err
            );
            return SessionOutcomeEvidenceCollection {
                collected,
                peer_failed: true,
            };
        }
        let Some(outcome) = response.session_outcome.as_ref() else {
            continue;
        };
        match state.persist_federation_session_outcome(outcome).await {
            Ok(()) => collected += 1,
            Err(err) => {
                tracing::warn!(
                    member_id = %peer.member_id,
                    "Could not persist federation session outcome evidence: {}",
                    err
                );
            }
        }
    }

    SessionOutcomeEvidenceCollection {
        collected,
        peer_failed: false,
    }
}

async fn collect_catch_up_session_outcome_quorum<T>(
    state: &MintState,
    client: &FederationRestPeerClient<T>,
    source_member: &MemberConfigPublic,
    response: &FederationJournalCatchUpResponse,
) -> anyhow::Result<Vec<FederationSessionOutcome>>
where
    T: Transport,
{
    if response.entries.is_empty() {
        return Ok(Vec::new());
    }

    let Some(runtime_config) = state.mint.federation_config() else {
        anyhow::bail!("mint is not configured as a federation member");
    };
    let public_config = runtime_config.public_config();
    let required = usize::from(public_config.threshold_params.consensus_threshold);
    let mut signers = BTreeSet::new();
    if let Some(session_outcome) = response.session_outcome.as_ref() {
        signers.insert(session_outcome.signer);
    }
    if signers.len() >= required {
        return Ok(Vec::new());
    }

    let first = response
        .entries
        .first()
        .ok_or_else(|| anyhow::anyhow!("missing entries for catch-up evidence"))?;
    let limit = u16::try_from(response.entries.len()).map_err(|_| {
        anyhow::anyhow!(
            "catch-up response carried too many entries for an evidence request: {}",
            response.entries.len()
        )
    })?;
    let request = FederationJournalCatchUpRequest::new(response.from_index)
        .with_limit(limit)
        .with_known_order_digest(first.previous_order_digest);
    let mut additional_outcomes = Vec::new();

    for peer in public_config.members.values() {
        if peer.member_id == runtime_config.member_id()
            || peer.member_id == source_member.member_id
            || signers.contains(&peer.member_id)
        {
            continue;
        }

        let peer_response = match client.catch_up(peer, &request).await {
            Ok(response) => response,
            Err(err) => {
                tracing::warn!(
                    member_id = %peer.member_id,
                    "Federation catch-up quorum evidence request failed: {}",
                    err
                );
                continue;
            }
        };
        if let Err(err) = verify_session_outcome_evidence_entries(&response.entries, &peer_response)
        {
            tracing::warn!(
                member_id = %peer.member_id,
                "Federation catch-up quorum evidence did not match peer journal range: {}",
                err
            );
            continue;
        }
        let Some(session_outcome) = peer_response.session_outcome else {
            continue;
        };
        if let Err(err) = session_outcome.verify(
            peer,
            response.federation_id,
            response.from_index,
            &response.entries,
            response.latest_order_digest,
            response.state_digest,
        ) {
            tracing::warn!(
                member_id = %peer.member_id,
                "Federation catch-up quorum evidence signature failed: {}",
                err
            );
            continue;
        }
        if signers.insert(session_outcome.signer) {
            additional_outcomes.push(session_outcome);
        }
        if signers.len() >= required {
            break;
        }
    }

    Ok(additional_outcomes)
}

async fn mark_healthy_after_session_outcome_quorum(state: &MintState, operation_count: u64) {
    match session_outcome_quorum_status(state).await {
        Ok(Some(quorum)) if quorum.from_index == 0 && quorum.operation_count >= operation_count => {
            state
                .mark_federation_healthy_if_session_outcome_verified(quorum.operation_count)
                .await;
        }
        Ok(_) => {}
        Err(err) => {
            tracing::warn!(
                "Could not verify federation session outcome quorum after collection: {}",
                err
            );
        }
    }
}

impl<T> FederationRestPeerClient<T>
where
    T: Transport,
{
    pub(crate) async fn catch_up_state_from_member(
        &self,
        state: &MintState,
        member: &MemberConfigPublic,
    ) -> anyhow::Result<(
        FederationJournalCatchUpResponse,
        Vec<FederationJournalEntry>,
    )> {
        let consensus = state
            .federation_consensus
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("mint is not configured as a federation member"))?;
        let request = {
            let consensus = consensus.lock().await;
            let mut request = consensus
                .catch_up_request()
                .with_limit(self.sync_config.catch_up_limit);
            if let Ok(status) = consensus.aleph_bft_status() {
                request = request.with_aleph_bft_next_item_index(status.next_item_index);
            }
            request
        };
        let response = self.catch_up(member, &request).await?;
        let additional_outcomes =
            collect_catch_up_session_outcome_quorum(state, self, member, &response).await?;
        let applied = state
            .apply_federation_catch_up_response_with_session_outcome_quorum(
                &response,
                &additional_outcomes,
            )
            .await?;
        Ok((response, applied))
    }

    pub(crate) async fn sync_once_from_member(
        &self,
        state: &MintState,
        member: &MemberConfigPublic,
    ) -> anyhow::Result<FederationPeerStatus> {
        let peer_status = self.peer_status(member).await?;
        let mut local_status = local_peer_order_status(state).await?;
        let mut recovered_from_peer = false;
        let mut latest_catch_up_state_digest = None;

        ensure_peer_consensus_backend_matches(state, member, &local_status, &peer_status).await?;

        if peer_status.operation_count > local_status.operation_count {
            state
                .mark_federation_lagging(
                    member.member_id,
                    local_status.operation_count,
                    peer_status.operation_count,
                )
                .await;
            state.mark_federation_catching_up(member.member_id).await;

            while local_status.operation_count < peer_status.operation_count {
                let before = local_status.operation_count;
                match self.catch_up_state_from_member(state, member).await {
                    Ok((response, _applied)) => {
                        latest_catch_up_state_digest = Some(response.state_digest);
                    }
                    Err(err) => {
                        state
                            .mark_federation_lagging(
                                member.member_id,
                                before,
                                peer_status.operation_count,
                            )
                            .await;
                        return Err(err);
                    }
                }

                local_status = local_peer_order_status(state).await?;
                recovered_from_peer = true;
                if local_status.operation_count == before {
                    state
                        .mark_federation_halted(format!(
                            "peer {} returned no catch-up progress from operation {}",
                            member.member_id, before
                        ))
                        .await;
                    anyhow::bail!(
                        "federation peer {} returned no catch-up progress from operation {}",
                        member.member_id,
                        before
                    );
                }
            }
        }

        local_status = local_peer_order_status(state).await?;
        ensure_peer_consensus_backend_matches(state, member, &local_status, &peer_status).await?;
        let order_digest_mismatch =
            local_status.latest_order_digest != peer_status.latest_order_digest;
        if local_status.operation_count == peer_status.operation_count && order_digest_mismatch {
            state
                .mark_federation_halted(format!(
                    "peer {} digest mismatch at operation count {}",
                    member.member_id, peer_status.operation_count
                ))
                .await;
            anyhow::bail!(
                "federation peer {} digest mismatch at operation count {}",
                member.member_id,
                peer_status.operation_count
            );
        }

        if let (true, Some(expected_state_digest)) =
            (recovered_from_peer, latest_catch_up_state_digest)
        {
            wait_for_local_materialized_state_digest(
                state,
                member,
                local_status.operation_count,
                expected_state_digest,
                self.sync_config,
            )
            .await?;
        }

        if local_status.operation_count >= peer_status.operation_count {
            if recovered_from_peer && local_status.operation_count > 0 {
                state
                    .mark_federation_verifying_session_outcome(
                        member.member_id,
                        local_status.operation_count,
                    )
                    .await;
            } else {
                state
                    .mark_federation_healthy_if_caught_up(local_status.operation_count)
                    .await;
            }
        }

        Ok(local_status)
    }
}

async fn wait_for_local_materialized_state_digest(
    state: &MintState,
    member: &MemberConfigPublic,
    operation_count: u64,
    expected_state_digest: FederationStateDigest,
    sync_config: FederationPeerSyncConfig,
) -> anyhow::Result<()> {
    for attempt in 1..=sync_config.digest_recheck_attempts {
        let local_status = local_peer_status(state).await?;
        if local_status.state_digest == expected_state_digest {
            tracing::debug!(
                member_id = %member.member_id,
                operation_count,
                attempt,
                "Federation peer sync local materialized digest matched catch-up response"
            );
            return Ok(());
        }

        if attempt < sync_config.digest_recheck_attempts {
            time::sleep(sync_config.digest_recheck_delay).await;
        }
    }

    state
        .mark_federation_materialized_state_mismatch(member.member_id, operation_count)
        .await;
    anyhow::bail!(
        "federation peer {} materialized state digest mismatch at operation count {}",
        member.member_id,
        operation_count
    )
}

async fn ensure_peer_consensus_backend_matches(
    state: &MintState,
    member: &MemberConfigPublic,
    local_status: &FederationPeerStatus,
    peer_status: &FederationPeerStatus,
) -> anyhow::Result<()> {
    if local_status.consensus_backend == peer_status.consensus_backend {
        return Ok(());
    }

    state
        .mark_federation_halted(format!(
            "peer {} consensus backend mismatch: local {} peer {}",
            member.member_id, local_status.consensus_backend, peer_status.consensus_backend
        ))
        .await;
    anyhow::bail!(
        "federation peer {} consensus backend mismatch: local {} peer {}",
        member.member_id,
        local_status.consensus_backend,
        peer_status.consensus_backend
    );
}
