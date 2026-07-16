use std::collections::BTreeSet;

use cdk::federation::{
    FederationAlephBftBackupRequest, FederationCheckpoint, FederationCheckpointKind,
    FederationCheckpointQuorum, FederationCheckpointSummary, FederationCheckpointSummaryResponse,
    FederationError, FederationJournalCatchUpRequest, FederationJournalCatchUpResponse,
    FederationPeerStatus, MemberConfigPublic, MemberId,
    FEDERATION_MAX_CHECKPOINT_CATCH_UP_REQUEST_BYTES,
    FEDERATION_MAX_CHECKPOINT_CATCH_UP_TRANSFER_BYTES,
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
    if startup_recovery {
        state.federation.request_recovery_checkpoint().await;
    }
    let request_recovery_checkpoint = state.federation.recovery_checkpoint_is_requested().await;
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

    if let Err(err) = state
        .federation
        .propose_recovery_barrier_if_requested()
        .await
    {
        tracing::warn!("Could not propose recovery checkpoint barrier: {}", err);
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
    let verified_current = current
        .as_ref()
        .map(|current| {
            super::verify_checkpoint_quorum_with_metrics(current, public_config, local_member)
        })
        .transpose()?;
    let scheduled_sequence = verified_current.as_ref().map_or(0, |quorum| {
        quorum.checkpoint().scheduled_checkpoint_sequence
    });
    if let Some(next) = db
        .get_next_federation_scheduled_checkpoint_quorum(scheduled_sequence)
        .await?
    {
        let verified_next =
            super::verify_checkpoint_quorum_with_metrics(&next, public_config, local_member)?;
        verified_next.checkpoint().validate_promotion_from(
            verified_current
                .as_ref()
                .map(|current| current.checkpoint()),
        )?;
        return Ok(Some(next));
    }
    let Some(recovery) = db
        .get_latest_federation_checkpoint_quorum(FederationCheckpointKind::Recovery)
        .await?
    else {
        return Ok(None);
    };
    let verified_recovery =
        super::verify_checkpoint_quorum_with_metrics(&recovery, public_config, local_member)?;
    verified_recovery.checkpoint().validate_promotion_from(
        verified_current
            .as_ref()
            .map(|current| current.checkpoint()),
    )?;
    let frontier = db.get_federation_checkpoint_frontier().await?;
    if frontier
        .ensure_covers_checkpoint(verified_recovery.checkpoint())
        .is_ok()
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
            local_summary.covers(peer_quorum)
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
                .is_ok_and(|local_summary| local_summary.covers(peer_quorum))
        }
        Ok(None) => false,
        Err(err) => {
            tracing::warn!("Could not load local scheduled checkpoint quorum: {}", err);
            false
        }
    }
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
    let Some(sync_status) = state.federation_sync_status().await else {
        return;
    };
    let live_session_active = state.federation.live_session_active().await;
    if !quorum_reconciliation_is_required(&sync_status, live_session_active) {
        return;
    }
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

pub(super) fn quorum_reconciliation_is_required(
    sync_status: &crate::FederationSyncStatus,
    live_session_active: bool,
) -> bool {
    match sync_status {
        crate::FederationSyncStatus::Healthy if live_session_active => false,
        crate::FederationSyncStatus::Halted { .. } => false,
        crate::FederationSyncStatus::Healthy
        | crate::FederationSyncStatus::AwaitingQuorum { .. }
        | crate::FederationSyncStatus::Lagging { .. }
        | crate::FederationSyncStatus::CatchingUp { .. } => true,
    }
}

pub(super) fn quorum_evidence_is_current(
    observed_at: Instant,
    now: Instant,
    max_age: std::time::Duration,
) -> bool {
    now.saturating_duration_since(observed_at) <= max_age
}

pub(super) fn checked_checkpoint_transfer_bytes(
    transferred: u64,
    page_bytes: u64,
    maximum: u64,
) -> Result<u64, FederationError> {
    let actual =
        transferred
            .checked_add(page_bytes)
            .ok_or(FederationError::CheckpointCatchUpTooLarge {
                resource: "transfer batch",
                actual: u64::MAX,
                max: maximum,
            })?;
    if actual > maximum {
        return Err(FederationError::CheckpointCatchUpTooLarge {
            resource: "transfer batch",
            actual,
            max: maximum,
        });
    }
    Ok(actual)
}

pub(super) fn checked_checkpoint_page_cursors(
    next_journal_index: u64,
    next_finalized_item_index: u64,
    journal_progress: u64,
    finalized_progress: u64,
) -> Result<(u64, u64), FederationError> {
    if journal_progress == 0 && finalized_progress == 0 {
        return Err(FederationError::CatchUpNoProgress {
            index: next_journal_index,
        });
    }
    let next_journal_index = next_journal_index
        .checked_add(journal_progress)
        .ok_or_else(|| FederationError::CheckpointInvalid {
            reason: "checkpoint catch-up journal cursor overflow".to_string(),
        })?;
    let next_finalized_item_index = next_finalized_item_index
        .checked_add(finalized_progress)
        .ok_or_else(|| FederationError::CheckpointInvalid {
            reason: "checkpoint catch-up finalized cursor overflow".to_string(),
        })?;
    Ok((next_journal_index, next_finalized_item_index))
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
                && local_status.has_same_committed_history_and_audit(candidate)
        })
        .map(|candidate| candidate.aleph_bft_session)
        .filter(|candidate_session| {
            let matching_peers = peer_statuses
                .iter()
                .filter(|status| {
                    status.aleph_bft_session == *candidate_session
                        && local_status.has_same_committed_history_and_audit(status)
                })
                .count();
            matching_peers.saturating_add(1) >= required
        })
        .max()
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

/// Owns checkpoint target application and bounded page acquisition from one source.
///
/// It may perform peer requests only before calling the runtime application path;
/// it never holds the operation-application or consensus lock across network I/O.
struct FederationCheckpointCatchUpManager<'a, T> {
    client: &'a FederationRestPeerClient<T>,
}

impl<'a, T> FederationCheckpointCatchUpManager<'a, T> {
    fn new(client: &'a FederationRestPeerClient<T>) -> Self {
        Self { client }
    }
}

impl<T> FederationCheckpointCatchUpManager<'_, T>
where
    T: Transport,
{
    pub(crate) async fn catch_up_state_to_checkpoint_from_member(
        &self,
        state: &MintState,
        member: &MemberConfigPublic,
        quorum: &FederationCheckpointQuorum,
    ) -> anyhow::Result<()> {
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
        let current_checkpoint = state
            .mint
            .localstore()
            .get_trusted_federation_checkpoint_quorum()
            .await?;
        checkpoint.validate_catch_up_batch_from(
            current_checkpoint.as_ref().map(|quorum| &quorum.checkpoint),
        )?;
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
        let mut transferred_bytes = 0u64;
        #[cfg(feature = "prometheus")]
        let mut peak_in_flight_bytes = 0u64;
        let mut page_count = 0u64;
        #[cfg(feature = "prometheus")]
        let metric_member = runtime_config.member_id().to_string();
        #[cfg(feature = "prometheus")]
        cdk_prometheus::METRICS
            .set_federation_checkpoint_catch_up_peak_in_flight_bytes(&metric_member, 0);
        while next_journal_index < checkpoint.journal_next_index
            || next_finalized_item_index < checkpoint.finalized_next_item_index
        {
            let request = FederationJournalCatchUpRequest::new(next_journal_index)
                .with_target_checkpoint(checkpoint_id)
                .with_limit(self.client.sync_config.catch_up_limit)
                .with_known_order_digest(known_order_digest)
                .with_aleph_bft_next_item_index(next_finalized_item_index);
            let request_bytes = serde_json::to_vec(&request)?.len();
            if request_bytes > FEDERATION_MAX_CHECKPOINT_CATCH_UP_REQUEST_BYTES {
                return Err(FederationError::CheckpointCatchUpTooLarge {
                    resource: "request",
                    actual: u64::try_from(request_bytes).unwrap_or(u64::MAX),
                    max: FEDERATION_MAX_CHECKPOINT_CATCH_UP_REQUEST_BYTES as u64,
                }
                .into());
            }
            let page = self
                .client
                .catch_up_to_checkpoint(member, &request, checkpoint)
                .await?;
            let page_bytes =
                u64::try_from(page.ensure_within_checkpoint_page_limit()?).unwrap_or(u64::MAX);
            transferred_bytes = checked_checkpoint_transfer_bytes(
                transferred_bytes,
                page_bytes,
                FEDERATION_MAX_CHECKPOINT_CATCH_UP_TRANSFER_BYTES,
            )?;
            #[cfg(feature = "prometheus")]
            {
                peak_in_flight_bytes = peak_in_flight_bytes.max(page_bytes);
                cdk_prometheus::METRICS.record_federation_checkpoint_work(
                    &metric_member,
                    cdk_prometheus::FederationCheckpointWork::CatchUpPagesFetched,
                    1,
                );
                cdk_prometheus::METRICS.record_federation_checkpoint_work(
                    &metric_member,
                    cdk_prometheus::FederationCheckpointWork::CatchUpCanonicalBytesFetched,
                    page_bytes,
                );
                cdk_prometheus::METRICS.set_federation_checkpoint_catch_up_peak_in_flight_bytes(
                    &metric_member,
                    u64_to_i64_saturating(peak_in_flight_bytes),
                );
            }
            page_count = page_count.saturating_add(1);
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
            let journal_progress = u64::try_from(page.entries.len()).map_err(|_| {
                FederationError::CheckpointInvalid {
                    reason: "checkpoint catch-up journal page length exceeds u64".to_string(),
                }
            })?;
            let finalized_progress =
                u64::try_from(page.aleph_bft_finalized_items.len()).map_err(|_| {
                    FederationError::CheckpointInvalid {
                        reason: "checkpoint catch-up finalized page length exceeds u64".to_string(),
                    }
                })?;
            (next_journal_index, next_finalized_item_index) = checked_checkpoint_page_cursors(
                next_journal_index,
                next_finalized_item_index,
                journal_progress,
                finalized_progress,
            )?;
            if let Some(last) = page.entries.last() {
                known_order_digest = last.order_digest;
            }
            state
                .apply_federation_checkpoint_catch_up_response(&page, quorum)
                .await?;
        }

        if page_count == 0 {
            let response = FederationJournalCatchUpResponse::from_journal(
                checkpoint.federation_id,
                &cdk::federation::InMemoryFederationJournal::from_base(
                    checkpoint.journal_next_index,
                    checkpoint.journal_order_digest,
                ),
                checkpoint.materialized_state_digest,
                &FederationJournalCatchUpRequest::new(checkpoint.journal_next_index)
                    .with_target_checkpoint(checkpoint_id)
                    .with_aleph_bft_next_item_index(checkpoint.finalized_next_item_index),
                self.client.sync_config.catch_up_limit,
                self.client.sync_config.catch_up_limit,
            );
            response.validate_checkpoint_binding(checkpoint)?;
            state
                .apply_federation_checkpoint_catch_up_response(&response, quorum)
                .await?;
        }
        Ok(())
    }
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
    ) -> anyhow::Result<()> {
        FederationCheckpointCatchUpManager::new(self)
            .catch_up_state_to_checkpoint_from_member(state, member, quorum)
            .await
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
                    state.federation.request_recovery_checkpoint().await;
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
                    #[cfg(feature = "prometheus")]
                    if let Some(config) = state.mint.federation_config() {
                        cdk_prometheus::METRICS.record_federation_checkpoint_work(
                            &config.member_id().to_string(),
                            cdk_prometheus::FederationCheckpointWork::CatchUpRetries,
                            1,
                        );
                    }
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
            && local_full_status.has_same_committed_state(&peer_status)
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

        let mut chunks = Vec::new();
        let mut next_chunk = 0u64;
        let mut total_bytes = 0usize;
        loop {
            let request = FederationAlephBftBackupRequest::new(session, next_chunk);
            let backup = self.aleph_bft_backup(member, request).await?;
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
