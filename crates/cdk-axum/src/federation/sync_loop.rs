use std::collections::BTreeSet;

use cdk::federation::{
    FederationCheckpoint, FederationCheckpointKind, FederationCheckpointQuorum,
    FederationCheckpointSummary, FederationCheckpointSummaryResponse, FederationError,
    FederationJournalCatchUpRequest, FederationJournalCatchUpResponse, FederationJournalEntry,
    FederationPeerStatus, MemberConfigPublic, MemberId,
};
use cdk_http_client::{Async, Transport};
use tokio::time::{self, Instant};

use crate::MintState;

use super::aleph_bft_runtime::{
    MAX_ALEPH_BFT_SESSION_BACKUP_BYTES, MAX_ALEPH_BFT_SESSION_BACKUP_CHUNKS,
};
use super::peer_client::FederationRestPeerClient;
use super::sync_config::{
    federation_peer_sync_disabled_by_env, federation_peer_sync_initial_delay,
    federation_peer_sync_interval, FederationPeerReconnectBackoff, FederationPeerSyncConfig,
};
#[cfg(feature = "prometheus")]
use super::u64_to_i64_saturating;
use super::{local_peer_order_status, local_peer_status};

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
    let member_rank = public_config
        .members
        .keys()
        .position(|member| *member == local_member)
        .unwrap_or_default();
    let initial_delay = federation_peer_sync_initial_delay(
        sync_config.peer_sync_interval,
        member_rank,
        public_config.members.len(),
    );
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
        if !initial_delay.is_zero() {
            time::sleep(initial_delay).await;
        }
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
    let round_started_at = Instant::now();
    let mut synced = 0;
    let mut evidence_peers = Vec::with_capacity(peers.len());
    let mut peer_statuses = Vec::with_capacity(peers.len());
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
                peer_statuses.push(status.clone());
                if let Some(backoff) = backoff.as_mut() {
                    backoff.record_success(peer.member_id);
                }
                record_federation_peer_sync_metrics(
                    state,
                    peer.member_id,
                    true,
                    Some(status.operation_count),
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
    let checkpoint_updates = exchange_checkpoint_round(state, client, &evidence_peers).await;
    if checkpoint_updates > 0 {
        tracing::debug!(
            checkpoint_updates,
            "Federation peer sync exchanged checkpoint evidence"
        );
    }
    reconcile_federation_startup_with_quorum(
        state,
        client.sync_config,
        round_started_at,
        &peer_statuses,
    )
    .await;
    synced
}

pub(super) async fn exchange_checkpoint_round<T>(
    state: &MintState,
    client: &FederationRestPeerClient<T>,
    peers: &[&MemberConfigPublic],
) -> usize
where
    T: Transport,
{
    if peers.is_empty() {
        return 0;
    }
    let startup_recovery = !matches!(
        state.federation_sync_status().await,
        Some(crate::FederationSyncStatus::Healthy)
    );
    if startup_recovery
        && matches!(
            state.federation.stable_checkpoint_boundary().await,
            Ok(Some(_))
        )
    {
        state.federation.request_recovery_checkpoint();
    }
    let request_recovery_checkpoint = state.federation.recovery_checkpoint_is_requested();
    let trusted_scheduled_sequence = match state
        .mint
        .localstore()
        .get_trusted_federation_checkpoint_quorum()
        .await
    {
        Ok(Some(quorum)) => {
            // Trusted quorums are verified during startup and before atomic promotion. Polling
            // reads only the scalar scheduled sequence and must not repeat cryptographic work.
            quorum.checkpoint.scheduled_checkpoint_sequence
        }
        Ok(None) => 0,
        Err(err) => {
            tracing::warn!("Could not load trusted checkpoint for discovery: {}", err);
            return 0;
        }
    };
    let mut updates = 0;
    let mut peer_summaries = Vec::with_capacity(peers.len());
    for &peer in peers {
        match client
            .checkpoint_summary(
                peer,
                request_recovery_checkpoint,
                Some(trusted_scheduled_sequence),
            )
            .await
        {
            Ok(summary) => peer_summaries.push((peer, summary)),
            Err(err) => tracing::warn!(
                member_id = %peer.member_id,
                "Federation checkpoint summary request failed: {}",
                err
            ),
        }
    }

    if let Err(err) = maybe_capture_recovery_checkpoint(state, &peer_summaries).await {
        tracing::warn!("Could not capture recovery checkpoint: {}", err);
    }

    for (peer, summary) in &peer_summaries {
        for (peer_quorum, require_exact_local_copy) in [
            (summary.next_scheduled_quorum.as_ref(), true),
            (summary.latest_scheduled_quorum.as_ref(), false),
            (summary.latest_recovery_quorum.as_ref(), false),
        ] {
            let Some(peer_quorum) = peer_quorum else {
                continue;
            };
            let already_stored = if require_exact_local_copy {
                local_checkpoint_quorum_is_stored(state, peer_quorum.checkpoint_id).await
            } else {
                local_checkpoint_quorum_is_current(state, peer_quorum).await
            };
            if already_stored {
                continue;
            }
            match client
                .fetch_checkpoint(peer, peer_quorum.checkpoint_id)
                .await
            {
                Ok(response) => {
                    let Some(quorum) = response.quorum.as_ref() else {
                        tracing::warn!(
                            member_id = %peer.member_id,
                            checkpoint_id = %peer_quorum.checkpoint_id,
                            "Federation peer advertised a checkpoint quorum but did not serve it"
                        );
                        continue;
                    };
                    match state.federation.persist_checkpoint_quorum(quorum).await {
                        Ok(_) => updates += 1,
                        Err(err) => tracing::warn!(
                            member_id = %peer.member_id,
                            checkpoint_id = %peer_quorum.checkpoint_id,
                            "Could not persist fetched federation checkpoint quorum: {}",
                            err
                        ),
                    }
                }
                Err(err) => tracing::warn!(
                    member_id = %peer.member_id,
                    checkpoint_id = %peer_quorum.checkpoint_id,
                    "Federation checkpoint quorum fetch failed: {}",
                    err
                ),
            }
        }
    }

    for kind in [
        FederationCheckpointKind::Scheduled,
        FederationCheckpointKind::Recovery,
    ] {
        updates += exchange_checkpoint_candidate(state, client, &peer_summaries, kind).await;
    }

    disseminate_latest_checkpoint_quorums(state, client, &peer_summaries, &mut updates).await;
    updates
}

async fn local_checkpoint_quorum_is_stored(
    state: &MintState,
    checkpoint_id: cdk::federation::FederationCheckpointId,
) -> bool {
    match state
        .mint
        .localstore()
        .get_federation_checkpoint_quorum(checkpoint_id)
        .await
    {
        Ok(Some(_)) => true,
        Ok(None) => false,
        Err(err) => {
            tracing::warn!(
                checkpoint_id = %checkpoint_id,
                "Could not inspect local federation checkpoint quorum: {}",
                err
            );
            false
        }
    }
}

async fn next_local_checkpoint_catch_up_target(
    state: &MintState,
) -> anyhow::Result<Option<FederationCheckpointQuorum>> {
    let runtime_config =
        state
            .mint
            .federation_config()
            .ok_or(FederationError::RuntimeUnavailable {
                component: "mint configuration",
            })?;
    let public_config = runtime_config.public_config();
    let local_member = runtime_config.member_id();
    let db = state.mint.localstore();
    let current = db.get_trusted_federation_checkpoint_quorum().await?;
    if let Some(current) = current.as_ref() {
        super::verify_checkpoint_quorum_with_metrics(current, public_config, local_member)?;
    }
    let scheduled_sequence = current
        .as_ref()
        .map_or(0, |quorum| quorum.checkpoint.scheduled_checkpoint_sequence);
    if let Some(next) = db
        .get_next_federation_scheduled_checkpoint_quorum(scheduled_sequence)
        .await?
    {
        super::verify_checkpoint_quorum_with_metrics(&next, public_config, local_member)?;
        next.checkpoint
            .validate_promotion_from(current.as_ref().map(|quorum| &quorum.checkpoint))?;
        return Ok(Some(next));
    }
    let Some(recovery) = db
        .get_latest_federation_checkpoint_quorum(FederationCheckpointKind::Recovery)
        .await?
    else {
        return Ok(None);
    };
    super::verify_checkpoint_quorum_with_metrics(&recovery, public_config, local_member)?;
    recovery
        .checkpoint
        .validate_promotion_from(current.as_ref().map(|quorum| &quorum.checkpoint))?;
    let frontier = db.get_federation_checkpoint_frontier().await?;
    if recovery.checkpoint.journal_next_index <= frontier.journal_next_index
        && recovery.checkpoint.finalized_next_item_index <= frontier.finalized_next_item_index
    {
        return Ok(None);
    }
    Ok(Some(recovery))
}

async fn exchange_checkpoint_candidate<T>(
    state: &MintState,
    client: &FederationRestPeerClient<T>,
    peer_summaries: &[(&MemberConfigPublic, FederationCheckpointSummaryResponse)],
    kind: FederationCheckpointKind,
) -> usize
where
    T: Transport,
{
    let Some(candidate) = local_pending_checkpoint(state, kind).await else {
        return 0;
    };
    let checkpoint_id = match candidate.id() {
        Ok(checkpoint_id) => checkpoint_id,
        Err(err) => {
            tracing::warn!("Could not identify local federation checkpoint: {}", err);
            return 0;
        }
    };
    let mut stored_signers = match state
        .mint
        .localstore()
        .get_federation_checkpoint_signatures(checkpoint_id)
        .await
    {
        Ok(signatures) => signatures
            .into_iter()
            .map(|signature| signature.signer)
            .collect::<BTreeSet<_>>(),
        Err(err) => {
            tracing::warn!(
                checkpoint_id = %checkpoint_id,
                "Could not load federation checkpoint signatures: {}",
                err
            );
            return 0;
        }
    };
    let mut updates = 0;
    for (peer, summary) in peer_summaries {
        let peer_candidate = match kind {
            FederationCheckpointKind::Scheduled => summary.latest_scheduled_candidate.as_ref(),
            FederationCheckpointKind::Recovery => summary.latest_recovery_candidate.as_ref(),
        };
        if stored_signers.contains(&peer.member_id)
            || peer_candidate
                .is_none_or(|peer_candidate| peer_candidate.checkpoint_id != checkpoint_id)
        {
            continue;
        }
        match client.request_checkpoint_signature(peer, &candidate).await {
            Ok(response) => {
                let Some(signature) = response.signature.as_ref() else {
                    continue;
                };
                match state
                    .federation
                    .persist_checkpoint_signature(checkpoint_id, signature)
                    .await
                {
                    Ok(quorum) => {
                        updates += 1;
                        stored_signers.insert(signature.signer);
                        if quorum.is_some() {
                            break;
                        }
                    }
                    Err(err) => tracing::warn!(
                        member_id = %peer.member_id,
                        checkpoint_id = %checkpoint_id,
                        "Could not persist federation checkpoint signature: {}",
                        err
                    ),
                }
            }
            Err(err) => tracing::warn!(
                member_id = %peer.member_id,
                checkpoint_id = %checkpoint_id,
                "Federation checkpoint signature request failed: {}",
                err
            ),
        }
    }
    updates
}

async fn local_checkpoint_quorum_is_current(
    state: &MintState,
    peer_quorum: &FederationCheckpointSummary,
) -> bool {
    let db = state.mint.localstore();
    let same_kind = match db
        .get_latest_federation_checkpoint_quorum(peer_quorum.kind)
        .await
    {
        Ok(Some(local_quorum)) => {
            let Ok(local_summary) =
                FederationCheckpointSummary::from_checkpoint(&local_quorum.checkpoint)
            else {
                return false;
            };
            checkpoint_summary_covers(&local_summary, peer_quorum)
        }
        Ok(None) => false,
        Err(err) => {
            tracing::warn!("Could not load local federation checkpoint quorum: {}", err);
            false
        }
    };
    if same_kind || peer_quorum.kind == FederationCheckpointKind::Scheduled {
        return same_kind;
    }
    match db
        .get_latest_federation_checkpoint_quorum(FederationCheckpointKind::Scheduled)
        .await
    {
        Ok(Some(local_quorum)) => {
            FederationCheckpointSummary::from_checkpoint(&local_quorum.checkpoint)
                .is_ok_and(|local_summary| checkpoint_summary_covers(&local_summary, peer_quorum))
        }
        Ok(None) => false,
        Err(err) => {
            tracing::warn!("Could not load local scheduled checkpoint quorum: {}", err);
            false
        }
    }
}

fn checkpoint_summary_covers(
    local: &FederationCheckpointSummary,
    candidate: &FederationCheckpointSummary,
) -> bool {
    local.checkpoint_id == candidate.checkpoint_id
        || local.scheduled_checkpoint_sequence > candidate.scheduled_checkpoint_sequence
        || (local.scheduled_checkpoint_sequence == candidate.scheduled_checkpoint_sequence
            && local.finalized_next_item_index >= candidate.finalized_next_item_index
            && local.journal_next_index >= candidate.journal_next_index)
}

async fn maybe_capture_recovery_checkpoint(
    state: &MintState,
    peer_summaries: &[(&MemberConfigPublic, FederationCheckpointSummaryResponse)],
) -> Result<(), cdk::Error> {
    if !state.federation.recovery_checkpoint_is_requested() {
        return Ok(());
    }
    let Some(local_boundary) = state.federation.stable_checkpoint_boundary().await? else {
        state
            .federation
            .observe_recovery_checkpoint_boundary(None)
            .await;
        return Ok(());
    };
    let required = state
        .mint
        .federation_config()
        .map(|config| usize::from(config.public_config().threshold_params.consensus_threshold))
        .ok_or(FederationError::RuntimeUnavailable {
            component: "mint configuration",
        })?;
    let matching_peers = peer_summaries
        .iter()
        .filter(|(_, summary)| summary.stable_tip.as_ref() == Some(&local_boundary))
        .count();
    if matching_peers.saturating_add(1) < required {
        state
            .federation
            .observe_recovery_checkpoint_boundary(None)
            .await;
        return Ok(());
    }
    if !state
        .federation
        .observe_recovery_checkpoint_boundary(Some(&local_boundary))
        .await
    {
        return Ok(());
    }
    state
        .federation
        .capture_recovery_checkpoint(&local_boundary)
        .await?;
    Ok(())
}

async fn local_pending_checkpoint(
    state: &MintState,
    kind: FederationCheckpointKind,
) -> Option<FederationCheckpoint> {
    let db = state.mint.localstore();
    let candidate = match db.get_latest_federation_checkpoint(kind).await {
        Ok(candidate) => candidate?,
        Err(err) => {
            tracing::warn!("Could not load local checkpoint candidate: {}", err);
            return None;
        }
    };
    let checkpoint_id = match candidate.id() {
        Ok(checkpoint_id) => checkpoint_id,
        Err(err) => {
            tracing::warn!("Could not identify local checkpoint candidate: {}", err);
            return None;
        }
    };
    match db.get_federation_checkpoint_quorum(checkpoint_id).await {
        Ok(Some(_)) => None,
        Ok(None) => Some(candidate),
        Err(err) => {
            tracing::warn!("Could not inspect local checkpoint quorum: {}", err);
            None
        }
    }
}

async fn disseminate_latest_checkpoint_quorums<T>(
    state: &MintState,
    client: &FederationRestPeerClient<T>,
    peer_summaries: &[(&MemberConfigPublic, FederationCheckpointSummaryResponse)],
    updates: &mut usize,
) where
    T: Transport,
{
    for kind in [
        FederationCheckpointKind::Scheduled,
        FederationCheckpointKind::Recovery,
    ] {
        let quorum = match state
            .mint
            .localstore()
            .get_latest_federation_checkpoint_quorum(kind)
            .await
        {
            Ok(Some(quorum)) => quorum,
            Ok(None) => continue,
            Err(err) => {
                tracing::warn!(
                    "Could not load checkpoint quorum for dissemination: {}",
                    err
                );
                continue;
            }
        };
        let checkpoint_id = match quorum.checkpoint.id() {
            Ok(checkpoint_id) => checkpoint_id,
            Err(err) => {
                tracing::warn!(
                    "Could not identify checkpoint quorum for dissemination: {}",
                    err
                );
                continue;
            }
        };
        for (peer, summary) in peer_summaries {
            let peer_quorum = match kind {
                FederationCheckpointKind::Scheduled => summary.latest_scheduled_quorum.as_ref(),
                FederationCheckpointKind::Recovery => summary.latest_recovery_quorum.as_ref(),
            };
            if peer_quorum.is_some_and(|summary| summary.checkpoint_id == checkpoint_id) {
                continue;
            }
            match client.publish_checkpoint_quorum(peer, &quorum).await {
                Ok(()) => *updates += 1,
                Err(err) => tracing::warn!(
                    member_id = %peer.member_id,
                    checkpoint_id = %checkpoint_id,
                    "Federation checkpoint quorum publication failed: {}",
                    err
                ),
            }
        }
    }
}

async fn reconcile_federation_startup_with_quorum(
    state: &MintState,
    sync_config: FederationPeerSyncConfig,
    evidence_observed_at: Instant,
    peer_statuses: &[FederationPeerStatus],
) {
    if matches!(
        state.federation_sync_status().await,
        Some(crate::FederationSyncStatus::Halted { .. })
    ) {
        return;
    }
    let live_session_active = state.federation.live_session_active().await;
    let Some(runtime_config) = state.mint.federation_config() else {
        return;
    };
    let local_status = match local_peer_status(state).await {
        Ok(status) => status,
        Err(err) => {
            tracing::warn!(
                "Could not inspect local federation status for quorum startup: {}",
                err
            );
            return;
        }
    };
    let required = usize::from(
        runtime_config
            .public_config()
            .threshold_params
            .consensus_threshold,
    );
    let max_evidence_age = sync_config.peer_request_timeout.saturating_mul(u32::from(
        runtime_config.public_config().threshold_params.member_count,
    ));
    if !quorum_evidence_is_current(evidence_observed_at, Instant::now(), max_evidence_age) {
        state
            .mark_federation_awaiting_quorum(local_status.operation_count)
            .await;
        return;
    }
    let target_session = quorum_compatible_session(&local_status, peer_statuses, required);
    let Some(target_session) = target_session else {
        state
            .mark_federation_awaiting_quorum(local_status.operation_count)
            .await;
        return;
    };

    if live_session_active {
        if target_session == local_status.aleph_bft_session {
            state.mark_federation_healthy().await;
        } else {
            state
                .mark_federation_awaiting_quorum(local_status.operation_count)
                .await;
        }
        return;
    }

    if target_session > local_status.aleph_bft_session {
        let Some(consensus) = state.federation.consensus.as_ref() else {
            return;
        };
        let mut consensus = consensus.lock().await;
        if let Err(err) = consensus.begin_aleph_bft_session(target_session) {
            tracing::warn!(
                target_session,
                "Could not advance federation consensus to quorum session: {}",
                err
            );
            return;
        }
    }

    match super::start_federation_aleph_bft_live_session(state, sync_config).await {
        Ok(Some(_)) => {}
        Ok(None) if state.federation.live_session_active().await => {}
        Ok(None) => {}
        Err(err) => {
            tracing::warn!("Could not start quorum-validated AlephBFT session: {}", err);
        }
    }
}

pub(super) fn quorum_evidence_is_current(
    observed_at: Instant,
    now: Instant,
    max_age: std::time::Duration,
) -> bool {
    now.saturating_duration_since(observed_at) <= max_age
}

pub(super) fn quorum_compatible_session(
    local_status: &FederationPeerStatus,
    peer_statuses: &[FederationPeerStatus],
    required: usize,
) -> Option<u64> {
    peer_statuses
        .iter()
        .filter(|candidate| {
            candidate.aleph_bft_session >= local_status.aleph_bft_session
                && peer_status_has_same_state(local_status, candidate)
        })
        .map(|candidate| candidate.aleph_bft_session)
        .filter(|candidate_session| {
            let matching_peers = peer_statuses
                .iter()
                .filter(|status| {
                    status.aleph_bft_session == *candidate_session
                        && peer_status_has_same_state(local_status, status)
                })
                .count();
            matching_peers.saturating_add(1) >= required
        })
        .max()
}

fn peer_status_has_same_state(local: &FederationPeerStatus, peer: &FederationPeerStatus) -> bool {
    local.federation_id == peer.federation_id
        && local.aleph_bft_next_item_index == peer.aleph_bft_next_item_index
        && local.operation_count == peer.operation_count
        && local.latest_index == peer.latest_index
        && local.latest_order_digest == peer.latest_order_digest
        && local.application_digest == peer.application_digest
        && local.latest_audited_checkpoint_id == peer.latest_audited_checkpoint_id
        && local.materialized_state_digest == peer.materialized_state_digest
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

impl<T> FederationRestPeerClient<T>
where
    T: Transport,
{
    pub(crate) async fn catch_up_state_to_checkpoint_from_member(
        &self,
        state: &MintState,
        member: &MemberConfigPublic,
        quorum: &FederationCheckpointQuorum,
    ) -> anyhow::Result<(
        FederationJournalCatchUpResponse,
        Vec<FederationJournalEntry>,
    )> {
        let runtime_config =
            state
                .mint
                .federation_config()
                .ok_or(FederationError::RuntimeUnavailable {
                    component: "mint configuration",
                })?;
        let public_config = runtime_config.public_config();
        super::verify_checkpoint_quorum_with_metrics(
            quorum,
            public_config,
            runtime_config.member_id(),
        )?;
        let checkpoint = &quorum.checkpoint;
        let checkpoint_id = checkpoint.id()?;
        let consensus = state.federation.consensus.as_ref().ok_or_else(|| {
            FederationError::RuntimeUnavailable {
                component: "consensus",
            }
        })?;
        let (initial_journal_index, initial_finalized_item_index, mut known_order_digest) = {
            let consensus = consensus.lock().await;
            let status = consensus.aleph_bft_status()?;
            (
                consensus.journal().next_index(),
                status.next_item_index,
                consensus.journal().latest_order_digest(),
            )
        };
        if initial_journal_index > checkpoint.journal_next_index
            || initial_finalized_item_index > checkpoint.finalized_next_item_index
        {
            return Err(FederationError::CheckpointInvalid {
                reason: "local catch-up frontier is ahead of the selected target checkpoint"
                    .to_string(),
            }
            .into());
        }

        let mut next_journal_index = initial_journal_index;
        let mut next_finalized_item_index = initial_finalized_item_index;
        let mut aggregate: Option<FederationJournalCatchUpResponse> = None;
        while next_journal_index < checkpoint.journal_next_index
            || next_finalized_item_index < checkpoint.finalized_next_item_index
        {
            let request = FederationJournalCatchUpRequest::new(next_journal_index)
                .with_target_checkpoint(checkpoint_id)
                .with_limit(self.sync_config.catch_up_limit)
                .with_known_order_digest(known_order_digest)
                .with_aleph_bft_next_item_index(next_finalized_item_index);
            let mut page = self
                .catch_up_to_checkpoint(member, &request, checkpoint)
                .await?;
            if page.from_index != next_journal_index
                || page.aleph_bft_from_item_index != next_finalized_item_index
            {
                return Err(FederationError::CheckpointInvalid {
                    reason: "checkpoint catch-up page does not start at the requested cursors"
                        .to_string(),
                }
                .into());
            }
            if let Some(first) = page.entries.first() {
                if first.previous_order_digest != known_order_digest {
                    return Err(FederationError::CatchUpDigestMismatch {
                        index: first.index,
                        expected: known_order_digest,
                        found: first.previous_order_digest,
                    }
                    .into());
                }
            }
            let journal_progress = page.entries.len() as u64;
            let finalized_progress = page.aleph_bft_finalized_items.len() as u64;
            if journal_progress == 0 && finalized_progress == 0 {
                return Err(FederationError::CatchUpNoProgress {
                    index: next_journal_index,
                }
                .into());
            }
            next_journal_index = next_journal_index
                .checked_add(journal_progress)
                .ok_or_else(|| FederationError::CheckpointInvalid {
                    reason: "checkpoint catch-up journal cursor overflow".to_string(),
                })?;
            next_finalized_item_index = next_finalized_item_index
                .checked_add(finalized_progress)
                .ok_or_else(|| FederationError::CheckpointInvalid {
                reason: "checkpoint catch-up finalized cursor overflow".to_string(),
            })?;
            if let Some(last) = page.entries.last() {
                known_order_digest = last.order_digest;
            }
            match aggregate.as_mut() {
                Some(aggregate) => {
                    aggregate.entries.append(&mut page.entries);
                    aggregate
                        .aleph_bft_finalized_items
                        .append(&mut page.aleph_bft_finalized_items);
                    aggregate.complete = page.complete;
                    aggregate.aleph_bft_finalized_items_complete =
                        page.aleph_bft_finalized_items_complete;
                }
                None => aggregate = Some(page),
            }
        }

        let response = aggregate.unwrap_or_else(|| {
            FederationJournalCatchUpResponse::from_journal(
                checkpoint.federation_id,
                &cdk::federation::InMemoryFederationJournal::from_base(
                    checkpoint.journal_next_index,
                    checkpoint.journal_order_digest,
                ),
                checkpoint.materialized_state_digest,
                &FederationJournalCatchUpRequest::new(checkpoint.journal_next_index)
                    .with_target_checkpoint(checkpoint_id)
                    .with_aleph_bft_next_item_index(checkpoint.finalized_next_item_index),
                self.sync_config.catch_up_limit,
                self.sync_config.catch_up_limit,
            )
        });
        response.validate_checkpoint_binding(checkpoint)?;
        let applied = state
            .apply_federation_checkpoint_catch_up_response(&response, quorum)
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

        if peer_requires_catch_up(&local_status, &peer_status)? {
            if should_defer_catch_up_to_live_aleph_bft(state).await {
                tracing::debug!(
                    member_id = %member.member_id,
                    local_operation_count = local_status.operation_count,
                    peer_operation_count = peer_status.operation_count,
                    "Deferring peer catch-up to the healthy live AlephBFT session"
                );
                return Ok(peer_status);
            }
            state
                .mark_federation_lagging(
                    member.member_id,
                    local_status.operation_count,
                    peer_status.operation_count,
                )
                .await;
            state.mark_federation_catching_up(member.member_id).await;

            while peer_requires_catch_up(&local_status, &peer_status)? {
                let Some(target) = next_local_checkpoint_catch_up_target(state).await? else {
                    state.federation.request_recovery_checkpoint();
                    state
                        .mark_federation_lagging(
                            member.member_id,
                            local_status.operation_count,
                            peer_status.operation_count,
                        )
                        .await;
                    return Ok(peer_status);
                };
                let before_status = local_status.clone();
                if let Err(err) = self
                    .catch_up_state_to_checkpoint_from_member(state, member, &target)
                    .await
                {
                    state
                        .mark_federation_lagging(
                            member.member_id,
                            before_status.operation_count,
                            peer_status.operation_count,
                        )
                        .await;
                    return Err(err);
                }

                local_status = local_peer_order_status(state).await?;
                recovered_from_peer = true;
                if !status_made_catch_up_progress(&before_status, &local_status)? {
                    let before_finalized_item = format_aleph_bft_next_item_index(&before_status);
                    super::halt_current_federation_aleph_bft_session(
                        state,
                        &format!(
                            "peer {} returned no catch-up progress from operation {} / finalized item {}",
                            member.member_id, before_status.operation_count, before_finalized_item
                        ),
                    )
                    .await;
                    return Err(FederationError::CatchUpNoProgress {
                        index: before_status.operation_count,
                    }
                    .into());
                }
            }
        }

        local_status = local_peer_order_status(state).await?;
        let order_digest_mismatch =
            local_status.latest_order_digest != peer_status.latest_order_digest;
        if local_status.operation_count == peer_status.operation_count && order_digest_mismatch {
            super::halt_current_federation_aleph_bft_session(
                state,
                &format!(
                    "peer {} digest mismatch at operation count {}",
                    member.member_id, peer_status.operation_count
                ),
            )
            .await;
            return Err(FederationError::CatchUpDigestMismatch {
                index: peer_status.operation_count.saturating_sub(1),
                expected: local_status.latest_order_digest,
                found: peer_status.latest_order_digest,
            }
            .into());
        }

        let local_full_status = local_peer_status(state).await?;
        if !state.federation.live_session_active().await
            && peer_status.aleph_bft_session == local_full_status.aleph_bft_session
            && peer_status_has_same_state(&local_full_status, &peer_status)
            && peer_status.aleph_bft_next_item_index.unwrap_or(0) > 0
        {
            self.recover_aleph_bft_backup_from_member(
                state,
                member,
                peer_status.aleph_bft_session,
                recovered_from_peer,
            )
            .await?;
        }

        Ok(peer_status)
    }

    async fn recover_aleph_bft_backup_from_member(
        &self,
        state: &MintState,
        member: &MemberConfigPublic,
        session: u64,
        replace_existing: bool,
    ) -> anyhow::Result<()> {
        let db = state.mint.localstore();
        if !replace_existing
            && !db
                .get_federation_aleph_bft_backup_chunks(session)
                .await?
                .is_empty()
        {
            return Ok(());
        }

        let consensus = state.federation.consensus.as_ref().ok_or_else(|| {
            FederationError::RuntimeUnavailable {
                component: "consensus",
            }
        })?;
        let mut chunks = Vec::new();
        let mut next_chunk = 0u64;
        let mut total_bytes = 0usize;
        loop {
            let request = {
                let consensus = consensus.lock().await;
                let mut request = consensus
                    .catch_up_request()
                    .with_limit(self.sync_config.catch_up_limit)
                    .with_aleph_bft_backup(session, next_chunk);
                if let Ok(status) = consensus.aleph_bft_status() {
                    request = request.with_aleph_bft_next_item_index(status.next_item_index);
                }
                request
            };
            let response = self.catch_up(member, &request).await?;
            let backup = response.aleph_bft_backup.ok_or_else(|| {
                anyhow::anyhow!(
                    "federation member {} omitted requested AlephBFT session {} backup",
                    member.member_id,
                    session
                )
            })?;
            if backup.session != session || backup.from_chunk != next_chunk {
                anyhow::bail!(
                    "federation member {} returned AlephBFT backup page session/chunk {}/{}; expected {}/{}",
                    member.member_id,
                    backup.session,
                    backup.from_chunk,
                    session,
                    next_chunk
                );
            }
            if backup.chunks.iter().any(Vec::is_empty) {
                anyhow::bail!(
                    "federation member {} returned an empty AlephBFT backup chunk",
                    member.member_id
                );
            }
            if backup.chunks.is_empty() && !backup.complete {
                anyhow::bail!(
                    "federation member {} returned no AlephBFT backup progress",
                    member.member_id
                );
            }
            for chunk in backup.chunks {
                total_bytes = total_bytes
                    .checked_add(chunk.len())
                    .ok_or_else(|| anyhow::anyhow!("AlephBFT backup byte count overflowed"))?;
                if total_bytes > MAX_ALEPH_BFT_SESSION_BACKUP_BYTES {
                    anyhow::bail!(
                        "AlephBFT session {} backup exceeds the {} byte recovery bound",
                        session,
                        MAX_ALEPH_BFT_SESSION_BACKUP_BYTES
                    );
                }
                chunks.push(chunk);
                if chunks.len() > MAX_ALEPH_BFT_SESSION_BACKUP_CHUNKS {
                    anyhow::bail!(
                        "AlephBFT session {} backup exceeds the {} chunk recovery bound",
                        session,
                        MAX_ALEPH_BFT_SESSION_BACKUP_CHUNKS
                    );
                }
                next_chunk = next_chunk
                    .checked_add(1)
                    .ok_or_else(|| anyhow::anyhow!("AlephBFT backup chunk count overflowed"))?;
            }
            if backup.complete {
                break;
            }
        }
        if chunks.is_empty() {
            anyhow::bail!(
                "federation member {} has no AlephBFT backup for progressed session {}",
                member.member_id,
                session
            );
        }

        let mut tx = db.begin_transaction().await?;
        if replace_existing {
            tx.replace_federation_aleph_bft_backup_chunks(session, &chunks)
                .await?;
        } else {
            for chunk in &chunks {
                tx.append_federation_aleph_bft_backup_bytes(session, chunk)
                    .await?;
            }
        }
        tx.commit().await?;
        tracing::info!(
            member_id = %member.member_id,
            session,
            backup_chunks = chunks.len(),
            backup_bytes = total_bytes,
            "Recovered AlephBFT live-session backup from federation peer"
        );
        Ok(())
    }
}

fn peer_requires_catch_up(
    local_status: &FederationPeerStatus,
    peer_status: &FederationPeerStatus,
) -> anyhow::Result<bool> {
    Ok(peer_status.operation_count > local_status.operation_count
        || peer_finalized_cursor_ahead(local_status, peer_status)?)
}

async fn should_defer_catch_up_to_live_aleph_bft(state: &MintState) -> bool {
    if !matches!(
        state.federation_sync_status().await,
        Some(crate::FederationSyncStatus::Healthy)
    ) {
        return false;
    }

    state.federation.live_session_active().await && state.federation.network_inbound_active().await
}

fn status_made_catch_up_progress(
    before_status: &FederationPeerStatus,
    after_status: &FederationPeerStatus,
) -> anyhow::Result<bool> {
    Ok(after_status.operation_count > before_status.operation_count
        || status_finalized_cursor_increased(before_status, after_status)?)
}

fn peer_finalized_cursor_ahead(
    local_status: &FederationPeerStatus,
    peer_status: &FederationPeerStatus,
) -> anyhow::Result<bool> {
    match (
        aleph_bft_next_item_index(local_status)?,
        aleph_bft_next_item_index(peer_status)?,
    ) {
        (Some(local_next_item_index), Some(peer_next_item_index)) => {
            Ok(peer_next_item_index > local_next_item_index)
        }
        _ => Ok(false),
    }
}

fn status_finalized_cursor_increased(
    before_status: &FederationPeerStatus,
    after_status: &FederationPeerStatus,
) -> anyhow::Result<bool> {
    match (
        aleph_bft_next_item_index(before_status)?,
        aleph_bft_next_item_index(after_status)?,
    ) {
        (Some(before_next_item_index), Some(after_next_item_index)) => {
            Ok(after_next_item_index > before_next_item_index)
        }
        _ => Ok(false),
    }
}

fn aleph_bft_next_item_index(status: &FederationPeerStatus) -> anyhow::Result<Option<u64>> {
    Ok(Some(status.aleph_bft_next_item_index.ok_or(
        FederationError::AlephBftFinalizedCursorMissing {
            member_id: status.member_id,
        },
    )?))
}

fn format_aleph_bft_next_item_index(status: &FederationPeerStatus) -> String {
    match status.aleph_bft_next_item_index {
        Some(index) => index.to_string(),
        None => "none".to_string(),
    }
}
