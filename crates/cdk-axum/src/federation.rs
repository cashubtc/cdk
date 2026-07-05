//! Private federation member REST routes.

use std::collections::{BTreeMap, BTreeSet};
use std::str::FromStr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use axum::extract::{Json, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use axum::Router;
use cdk::federation::{
    FederatedKeyset, FederationConfig, FederationConsensusEngine, FederationDkgCommitmentRequest,
    FederationDkgRevealRequest, FederationDkgSecretShareRequest, FederationDkgSetupResponse,
    FederationDkgTranscriptSignatureRequest, FederationError, FederationId,
    FederationJournalCatchUpRequest, FederationJournalCatchUpResponse, FederationJournalDigest,
    FederationJournalEntry, FederationMaterializedStateRow,
    FederationMaterializedStateRowFingerprint, FederationMaterializedStateSnapshot,
    FederationOperation, FederationOperationEnvelope, FederationOperationId,
    FederationPaymentObservationQuarantineRecord, FederationPaymentObservationRecord,
    FederationPaymentObservationReport, FederationPaymentObservationReportSummary,
    FederationPeerStatus, FederationPeerStatusRequest, FederationRestMessageKind,
    FederationRestSequenceTracker, FederationSessionOutcome, FederationSessionOutcomeQuorum,
    FederationSignedRequest, FederationStateDigest, FederationSubmitOperationRequest,
    FederationSubmitOperationResponse, InMemoryFederationJournal, MemberConfigPrivate,
    MemberConfigPublic, MemberId, FEDERATION_REST_API_VERSION,
};
use cdk_http_client::{Async, Transport};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::time::{self, Duration, Instant, MissedTickBehavior};
use tracing::instrument;

use crate::MintState;

type SequenceHandle = std::sync::Arc<tokio::sync::Mutex<FederationRestSequenceTracker>>;

const DEFAULT_FEDERATION_PEER_SYNC_INTERVAL: Duration = Duration::from_secs(5);
const DEFAULT_FEDERATION_PEER_REQUEST_TIMEOUT: Duration = Duration::from_secs(10);
const DEFAULT_FEDERATION_OPERATION_SUBMISSION_TIMEOUT: Duration = Duration::from_secs(10);
const FEDERATION_DISABLE_PEER_SYNC_ENV_VAR: &str = "CDK_FEDERATION_DISABLE_PEER_SYNC";
const DEFAULT_FEDERATION_SESSION_OUTCOME_EVIDENCE_CHUNK_SIZE: usize = 1000;
const DEFAULT_FEDERATION_CATCH_UP_LIMIT: u16 = 100;
const MAX_FEDERATION_CATCH_UP_LIMIT: u16 = 1000;
const DEFAULT_FEDERATION_STATE_DIGEST_RECHECK_ATTEMPTS: usize = 20;
const DEFAULT_FEDERATION_STATE_DIGEST_RECHECK_DELAY: Duration = Duration::from_millis(100);
const DEFAULT_FEDERATION_RECONNECT_INITIAL_BACKOFF: Duration = Duration::from_secs(1);
const DEFAULT_FEDERATION_RECONNECT_MAX_BACKOFF: Duration = Duration::from_secs(60);
const DEFAULT_FEDERATION_PRIVATE_REST_INBOUND_CONCURRENCY_LIMIT: usize = 128;
const DEFAULT_FEDERATION_PRIVATE_REST_OUTBOUND_CONCURRENCY_LIMIT: usize = 64;
const DEFAULT_FEDERATION_STATE_DIGEST_CHECKPOINT_RETENTION: usize = 10_000;
static FEDERATION_REST_SEQUENCE_SEED: AtomicU64 = AtomicU64::new(0);

/// Private federation peer synchronization policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FederationPeerSyncConfig {
    /// Interval between background private REST peer sync rounds.
    pub peer_sync_interval: Duration,
    /// Timeout for private peer status, catch-up, DKG, and evidence requests.
    pub peer_request_timeout: Duration,
    /// Timeout for forwarding a locally received operation to the primary member.
    pub operation_submission_timeout: Duration,
    /// Maximum number of accepted journal entries requested per catch-up page.
    pub catch_up_limit: u16,
    /// Number of materialized-state digest rechecks after journal catch-up.
    pub digest_recheck_attempts: usize,
    /// Delay between materialized-state digest rechecks.
    pub digest_recheck_delay: Duration,
    /// Number of journal entries covered by each session-outcome evidence chunk.
    pub session_outcome_evidence_chunk_size: usize,
    /// Initial reconnect backoff after a peer sync failure.
    pub reconnect_initial_backoff: Duration,
    /// Maximum reconnect backoff after repeated peer sync failures.
    pub reconnect_max_backoff: Duration,
    /// Maximum concurrent private REST requests accepted by this member.
    pub private_rest_inbound_concurrency_limit: usize,
    /// Maximum concurrent private REST requests sent by one peer client.
    pub private_rest_outbound_concurrency_limit: usize,
    /// Number of materialized state digest checkpoints retained locally.
    pub state_digest_checkpoint_retention: usize,
}

impl Default for FederationPeerSyncConfig {
    fn default() -> Self {
        Self {
            peer_sync_interval: DEFAULT_FEDERATION_PEER_SYNC_INTERVAL,
            peer_request_timeout: DEFAULT_FEDERATION_PEER_REQUEST_TIMEOUT,
            operation_submission_timeout: DEFAULT_FEDERATION_OPERATION_SUBMISSION_TIMEOUT,
            catch_up_limit: DEFAULT_FEDERATION_CATCH_UP_LIMIT,
            digest_recheck_attempts: DEFAULT_FEDERATION_STATE_DIGEST_RECHECK_ATTEMPTS,
            digest_recheck_delay: DEFAULT_FEDERATION_STATE_DIGEST_RECHECK_DELAY,
            session_outcome_evidence_chunk_size:
                DEFAULT_FEDERATION_SESSION_OUTCOME_EVIDENCE_CHUNK_SIZE,
            reconnect_initial_backoff: DEFAULT_FEDERATION_RECONNECT_INITIAL_BACKOFF,
            reconnect_max_backoff: DEFAULT_FEDERATION_RECONNECT_MAX_BACKOFF,
            private_rest_inbound_concurrency_limit:
                DEFAULT_FEDERATION_PRIVATE_REST_INBOUND_CONCURRENCY_LIMIT,
            private_rest_outbound_concurrency_limit:
                DEFAULT_FEDERATION_PRIVATE_REST_OUTBOUND_CONCURRENCY_LIMIT,
            state_digest_checkpoint_retention: DEFAULT_FEDERATION_STATE_DIGEST_CHECKPOINT_RETENTION,
        }
    }
}

impl FederationPeerSyncConfig {
    /// Validate the synchronization policy before starting private peer sync.
    pub fn validate(&self) -> anyhow::Result<()> {
        if self.peer_sync_interval.is_zero() {
            anyhow::bail!("federation peer sync interval must be greater than zero");
        }
        if self.peer_request_timeout.is_zero() {
            anyhow::bail!("federation peer request timeout must be greater than zero");
        }
        if self.operation_submission_timeout.is_zero() {
            anyhow::bail!("federation operation submission timeout must be greater than zero");
        }
        if self.catch_up_limit == 0 {
            anyhow::bail!("federation catch-up limit must be greater than zero");
        }
        if self.digest_recheck_attempts == 0 {
            anyhow::bail!("federation digest recheck attempts must be greater than zero");
        }
        if self.digest_recheck_delay.is_zero() {
            anyhow::bail!("federation digest recheck delay must be greater than zero");
        }
        if self.session_outcome_evidence_chunk_size == 0 {
            anyhow::bail!(
                "federation session outcome evidence chunk size must be greater than zero"
            );
        }
        if self.session_outcome_evidence_chunk_size > usize::from(u16::MAX) {
            anyhow::bail!(
                "federation session outcome evidence chunk size must fit in a catch-up request limit"
            );
        }
        if self.reconnect_initial_backoff.is_zero() {
            anyhow::bail!("federation reconnect initial backoff must be greater than zero");
        }
        if self.reconnect_max_backoff.is_zero() {
            anyhow::bail!("federation reconnect max backoff must be greater than zero");
        }
        if self.reconnect_max_backoff < self.reconnect_initial_backoff {
            anyhow::bail!(
                "federation reconnect max backoff must be greater than or equal to the initial backoff"
            );
        }
        if self.private_rest_inbound_concurrency_limit == 0 {
            anyhow::bail!(
                "federation private REST inbound concurrency limit must be greater than zero"
            );
        }
        if self.private_rest_outbound_concurrency_limit == 0 {
            anyhow::bail!(
                "federation private REST outbound concurrency limit must be greater than zero"
            );
        }
        if self.state_digest_checkpoint_retention == 0 {
            anyhow::bail!("federation state digest checkpoint retention must be greater than zero");
        }

        Ok(())
    }
}

pub(crate) fn create_federation_router() -> Router<MintState> {
    Router::new()
        .route("/status", post(post_status))
        .route("/local-status", post(post_local_status))
        .route("/dkg/commitments", post(post_dkg_commitment))
        .route("/dkg/reveals", post(post_dkg_reveal))
        .route("/dkg/secret-shares", post(post_dkg_secret_share))
        .route(
            "/dkg/transcript-signatures",
            post(post_dkg_transcript_signature),
        )
        .route("/operations", post(post_operation))
        .route("/journal", post(post_journal))
}

fn acquire_private_rest_admission(
    state: &MintState,
    kind: FederationRestMessageKind,
) -> Result<Option<tokio::sync::OwnedSemaphorePermit>, Response> {
    let Some(admission) = state.federation_private_rest_admission.as_ref() else {
        return Ok(None);
    };
    match admission.clone().try_acquire_owned() {
        Ok(permit) => Ok(Some(permit)),
        Err(tokio::sync::TryAcquireError::NoPermits) => {
            tracing::warn!(
                request_kind = ?kind,
                "Federation private REST inbound queue is full"
            );
            Err((
                StatusCode::TOO_MANY_REQUESTS,
                "federation private REST inbound queue is full",
            )
                .into_response())
        }
        Err(tokio::sync::TryAcquireError::Closed) => {
            tracing::error!(
                request_kind = ?kind,
                "Federation private REST inbound admission is closed"
            );
            Err((
                StatusCode::SERVICE_UNAVAILABLE,
                "federation private REST inbound admission is closed",
            )
                .into_response())
        }
    }
}

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

fn federation_peer_sync_interval(sync_config: FederationPeerSyncConfig) -> time::Interval {
    let mut interval = time::interval(sync_config.peer_sync_interval);
    interval.set_missed_tick_behavior(MissedTickBehavior::Delay);
    interval
}

fn federation_peer_sync_disabled_by_env() -> bool {
    match std::env::var(FEDERATION_DISABLE_PEER_SYNC_ENV_VAR) {
        Ok(value) => federation_peer_sync_disabled_value(&value),
        Err(_) => false,
    }
}

fn federation_peer_sync_disabled_value(value: &str) -> bool {
    matches!(
        value,
        "1" | "true" | "TRUE" | "True" | "yes" | "YES" | "Yes" | "on" | "ON" | "On"
    )
}

#[derive(Debug, Clone, Copy)]
struct FederationPeerReconnectBackoffEntry {
    failures: u32,
    retry_after: Instant,
}

#[derive(Debug, Default)]
struct FederationPeerReconnectBackoff {
    entries: BTreeMap<MemberId, FederationPeerReconnectBackoffEntry>,
}

impl FederationPeerReconnectBackoff {
    fn should_attempt(&self, peer: MemberId, now: Instant) -> bool {
        self.entries
            .get(&peer)
            .map(|entry| now >= entry.retry_after)
            .unwrap_or(true)
    }

    fn record_success(&mut self, peer: MemberId) {
        self.entries.remove(&peer);
    }

    fn record_failure(
        &mut self,
        peer: MemberId,
        sync_config: FederationPeerSyncConfig,
        now: Instant,
    ) -> Duration {
        let entry =
            self.entries
                .entry(peer)
                .or_insert_with(|| FederationPeerReconnectBackoffEntry {
                    failures: 0,
                    retry_after: now,
                });
        let delay = federation_reconnect_backoff_delay(sync_config, entry.failures);
        entry.failures = entry.failures.saturating_add(1);
        entry.retry_after = now + delay;
        delay
    }
}

fn federation_reconnect_backoff_delay(
    sync_config: FederationPeerSyncConfig,
    failure_count: u32,
) -> Duration {
    let mut delay = sync_config.reconnect_initial_backoff;
    for _ in 0..failure_count {
        match delay.checked_mul(2) {
            Some(next) if next < sync_config.reconnect_max_backoff => {
                delay = next;
            }
            _ => return sync_config.reconnect_max_backoff,
        }
    }
    delay.min(sync_config.reconnect_max_backoff)
}

#[cfg_attr(not(test), allow(dead_code))]
async fn run_federation_peer_sync_round<T>(
    state: &MintState,
    client: &FederationRestPeerClient<T>,
    peers: &[MemberConfigPublic],
) -> usize
where
    T: Transport,
{
    run_federation_peer_sync_attempts(state, client, peers, None).await
}

async fn run_federation_peer_sync_round_with_backoff<T>(
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

async fn submit_operation_to_primary_if_needed_with_transport<T>(
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

async fn persist_local_session_outcome_evidence_chunks(
    state: &MintState,
    runtime_config: &cdk::mint::FederationRuntimeConfig,
    entries: &[FederationJournalEntry],
    latest_order_digest: FederationJournalDigest,
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

#[instrument(skip_all)]
async fn post_status(
    State(state): State<MintState>,
    Json(payload): Json<FederationSignedRequest<FederationPeerStatusRequest>>,
) -> Result<Json<FederationPeerStatus>, Response> {
    let _permit = acquire_private_rest_admission(&state, FederationRestMessageKind::Status)?;
    let verified =
        verify_federation_request(&state, &payload, FederationRestMessageKind::Status).await?;
    state
        .persist_federation_sequence(verified.sender, verified.sequence)
        .await
        .map_err(cdk_error_response)?;
    let peer_status = local_peer_order_status(&state)
        .await
        .map_err(anyhow_response)?;
    Ok(Json(peer_status))
}

#[instrument(skip_all)]
async fn post_local_status(
    State(state): State<MintState>,
    Json(payload): Json<FederationSignedRequest<FederationPeerStatusRequest>>,
) -> Result<Json<FederationLocalStatusResponse>, Response> {
    let _permit = acquire_private_rest_admission(&state, FederationRestMessageKind::Status)?;
    let verified =
        verify_federation_request(&state, &payload, FederationRestMessageKind::Status).await?;
    state
        .persist_federation_sequence(verified.sender, verified.sequence)
        .await
        .map_err(cdk_error_response)?;
    let (peer_status, materialized_state_rows) = if payload.payload.include_materialized_rows {
        local_peer_status_with_materialized_rows(&state)
            .await
            .map_err(anyhow_response)?
    } else {
        (
            local_peer_status(&state).await.map_err(anyhow_response)?,
            Vec::new(),
        )
    };
    let sync_status = state.federation_sync_status().await.unwrap_or_default();
    let session_outcome_quorum = session_outcome_quorum_status(&state)
        .await
        .map_err(cdk_error_response)?;
    let payment_observation_reports = payment_observation_report_summaries(&state)
        .await
        .map_err(cdk_error_response)?;
    let pending_mempool_items = state.federation_pending_mempool_len().await;
    let invalid_operation_alerts = state.federation_invalid_operation_alert_summaries().await;
    let response = FederationLocalStatusResponse::new(
        peer_status,
        sync_status,
        session_outcome_quorum,
        payment_observation_reports,
        materialized_state_rows,
        usize_to_u64_saturating(pending_mempool_items),
        invalid_operation_alerts,
    );
    #[cfg(feature = "prometheus")]
    record_federation_local_status_metrics(&response);
    Ok(Json(response))
}

#[instrument(skip_all)]
async fn post_dkg_commitment(
    State(state): State<MintState>,
    Json(payload): Json<FederationSignedRequest<FederationDkgCommitmentRequest>>,
) -> Result<Json<FederationDkgSetupResponse>, Response> {
    let _permit = acquire_private_rest_admission(&state, FederationRestMessageKind::DkgCommitment)?;
    let verified =
        verify_federation_request(&state, &payload, FederationRestMessageKind::DkgCommitment)
            .await?;
    let runtime_config = federation_runtime_config(&state)?;
    payload
        .payload
        .validate_sender(runtime_config.public_config(), verified.sender)
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
        .map_err(cdk_error_response)?;

    Ok(Json(FederationDkgSetupResponse::new(
        federation_id,
        local_member,
        FederationRestMessageKind::DkgCommitment,
        payload.payload.commitment.member_id,
    )))
}

#[instrument(skip_all)]
async fn post_dkg_reveal(
    State(state): State<MintState>,
    Json(payload): Json<FederationSignedRequest<FederationDkgRevealRequest>>,
) -> Result<Json<FederationDkgSetupResponse>, Response> {
    let _permit = acquire_private_rest_admission(&state, FederationRestMessageKind::DkgReveal)?;
    let verified =
        verify_federation_request(&state, &payload, FederationRestMessageKind::DkgReveal).await?;
    let runtime_config = federation_runtime_config(&state)?;
    payload
        .payload
        .validate_sender(runtime_config.public_config(), verified.sender)
        .map_err(federation_error_response)?;
    let federation_id = runtime_config.federation_id();
    let local_member = runtime_config.member_id();
    state
        .persist_federation_dkg_reveal(&payload.payload.reveal, verified.sender, verified.sequence)
        .await
        .map_err(cdk_error_response)?;

    Ok(Json(FederationDkgSetupResponse::new(
        federation_id,
        local_member,
        FederationRestMessageKind::DkgReveal,
        payload.payload.reveal.member_id,
    )))
}

#[instrument(skip_all)]
async fn post_dkg_secret_share(
    State(state): State<MintState>,
    Json(payload): Json<FederationSignedRequest<FederationDkgSecretShareRequest>>,
) -> Result<Json<FederationDkgSetupResponse>, Response> {
    let _permit =
        acquire_private_rest_admission(&state, FederationRestMessageKind::DkgSecretShare)?;
    let verified =
        verify_federation_request(&state, &payload, FederationRestMessageKind::DkgSecretShare)
            .await?;
    let runtime_config = federation_runtime_config(&state)?;
    payload
        .payload
        .validate_sender_receiver(
            runtime_config.public_config(),
            verified.sender,
            runtime_config.member_id(),
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
        .map_err(cdk_error_response)?;

    Ok(Json(FederationDkgSetupResponse::new(
        federation_id,
        local_member,
        FederationRestMessageKind::DkgSecretShare,
        payload.payload.sender,
    )))
}

#[instrument(skip_all)]
async fn post_dkg_transcript_signature(
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
        .map_err(cdk_error_response)?;

    Ok(Json(FederationDkgSetupResponse::new(
        federation_id,
        local_member,
        FederationRestMessageKind::DkgTranscriptSignature,
        payload.payload.signer,
    )))
}

#[instrument(skip_all)]
async fn post_operation(
    State(state): State<MintState>,
    Json(payload): Json<FederationSignedRequest<FederationSubmitOperationRequest>>,
) -> Result<Json<FederationSubmitOperationResponse>, Response> {
    let _permit =
        acquire_private_rest_admission(&state, FederationRestMessageKind::SubmitOperation)?;
    let verified =
        verify_federation_request(&state, &payload, FederationRestMessageKind::SubmitOperation)
            .await?;
    if let Some(response) = submit_operation_to_primary_if_needed(
        &state,
        payload.payload.envelope.clone(),
        Some((verified.sender, verified.sequence)),
    )
    .await
    .map_err(cdk_error_response)?
    {
        tracing::debug!(
            "Forwarded federation operation {} to primary member",
            response.operation_id
        );
        return Ok(Json(response));
    }

    let response = state
        .submit_federation_envelope_with_sequence(
            payload.payload.envelope,
            Some((verified.sender, verified.sequence)),
        )
        .await
        .map_err(cdk_error_response)?;
    tracing::debug!("Accepted federation operation {}", response.operation_id);
    Ok(Json(response))
}

#[instrument(skip_all)]
async fn post_journal(
    State(state): State<MintState>,
    Json(payload): Json<FederationSignedRequest<FederationJournalCatchUpRequest>>,
) -> Result<Json<FederationJournalCatchUpResponse>, Response> {
    let _permit = acquire_private_rest_admission(&state, FederationRestMessageKind::CatchUp)?;
    let verified =
        verify_federation_request(&state, &payload, FederationRestMessageKind::CatchUp).await?;
    state
        .persist_federation_sequence(verified.sender, verified.sequence)
        .await
        .map_err(cdk_error_response)?;
    let private_config = federation_runtime_config(&state)?.private_config().clone();
    tracing::debug!(
        next_index = payload.payload.next_index,
        limit = ?payload.payload.limit,
        "Received federation journal catch-up request"
    );
    let (mut response, entries) = durable_catch_up_response(&state, &payload.payload).await?;
    response.state_digest = materialized_state_digest(&state, &entries)
        .await
        .map_err(anyhow_response)?;
    let response = response
        .with_signed_session_outcome(&private_config)
        .map_err(federation_error_response)?;
    Ok(Json(response))
}

async fn durable_catch_up_response(
    state: &MintState,
    request: &FederationJournalCatchUpRequest,
) -> Result<
    (
        FederationJournalCatchUpResponse,
        Vec<FederationJournalEntry>,
    ),
    Response,
> {
    let runtime_config = federation_runtime_config(state)?;
    let journal = durable_federation_journal(state)
        .await
        .map_err(anyhow_response)?;

    if let Some(known_order_digest) = request.known_order_digest {
        if let Some(expected_digest) = journal.order_digest_before(request.next_index) {
            if expected_digest != known_order_digest {
                return Err(federation_error_response(
                    FederationError::CatchUpDigestMismatch {
                        index: request.next_index,
                        expected: expected_digest,
                        found: known_order_digest,
                    },
                ));
            }
        }
    }

    let response = FederationJournalCatchUpResponse::from_journal(
        runtime_config.federation_id(),
        &journal,
        request,
        DEFAULT_FEDERATION_CATCH_UP_LIMIT,
        MAX_FEDERATION_CATCH_UP_LIMIT,
    );
    let entries = journal.entries().to_vec();

    Ok((response, entries))
}

async fn verify_federation_request<T>(
    state: &MintState,
    request: &FederationSignedRequest<T>,
    expected_kind: FederationRestMessageKind,
) -> Result<VerifiedFederationRequest, Response>
where
    T: Serialize,
{
    if request.kind != expected_kind {
        return Err(federation_error_response(
            FederationError::RestMessageKindMismatch {
                expected: expected_kind,
                found: request.kind,
            },
        ));
    }

    let runtime_config = federation_runtime_config(state)?;
    let sequences = federation_sequences(state)?;
    let sequences = sequences.lock().await;
    sequences
        .verify_next(
            request,
            runtime_config.public_config(),
            runtime_config.member_id(),
        )
        .map_err(federation_error_response)?;

    Ok(VerifiedFederationRequest {
        sender: request.sender,
        sequence: request.sequence,
    })
}

#[derive(Debug, Clone, Copy)]
struct VerifiedFederationRequest {
    sender: cdk::federation::MemberId,
    sequence: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct FederationLocalStatusResponse {
    pub(crate) version: u16,
    pub(crate) peer_status: FederationPeerStatus,
    pub(crate) sync_state: String,
    pub(crate) sync_ready: bool,
    pub(crate) sync_message: String,
    #[serde(default)]
    pub(crate) pending_mempool_items: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) session_outcome_quorum_operation_count: Option<u64>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) session_outcome_quorum_signers: Vec<cdk::federation::MemberId>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) payment_observation_reports: Vec<FederationPaymentObservationReportSummary>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) materialized_state_rows: Vec<FederationMaterializedStateRowFingerprint>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) invalid_operation_alerts: Vec<crate::FederationInvalidOperationAlertSummary>,
}

impl FederationLocalStatusResponse {
    fn new(
        peer_status: FederationPeerStatus,
        sync_status: crate::FederationSyncStatus,
        session_outcome_quorum: Option<SessionOutcomeQuorumStatus>,
        payment_observation_reports: Vec<FederationPaymentObservationReportSummary>,
        materialized_state_rows: Vec<FederationMaterializedStateRowFingerprint>,
        pending_mempool_items: u64,
        invalid_operation_alerts: Vec<crate::FederationInvalidOperationAlertSummary>,
    ) -> Self {
        let session_outcome_quorum_operation_count = session_outcome_quorum
            .as_ref()
            .map(|quorum| quorum.operation_count);
        let session_outcome_quorum_signers = session_outcome_quorum
            .map(|quorum| quorum.signers.into_iter().collect())
            .unwrap_or_default();
        Self {
            version: FEDERATION_REST_API_VERSION,
            peer_status,
            sync_state: sync_status.state_name().to_string(),
            sync_ready: sync_status.is_ready(),
            sync_message: sync_status.not_ready_message(),
            pending_mempool_items,
            session_outcome_quorum_operation_count,
            session_outcome_quorum_signers,
            payment_observation_reports,
            materialized_state_rows,
            invalid_operation_alerts,
        }
    }
}

#[cfg(feature = "prometheus")]
fn record_federation_local_status_metrics(response: &FederationLocalStatusResponse) {
    const SYNC_STATES: &[&str] = &[
        "healthy",
        "lagging",
        "catching_up",
        "verifying_session_outcome",
        "materialized_state_mismatch",
        "halted",
    ];
    const PAYMENT_REPORT_STATUSES: &[&str] =
        &["empty", "awaiting_quorum", "quorum_reached", "conflicting"];
    const PAYMENT_REPORT_ACTIONS: &[&str] = &["none", "inspect", "quarantine"];

    let member = response.peer_status.member_id.to_string();
    cdk_prometheus::METRICS.set_federation_consensus_operation_count(
        &member,
        u64_to_i64_saturating(response.peer_status.operation_count),
    );
    cdk_prometheus::METRICS.set_federation_pending_mempool_items(
        &member,
        u64_to_i64_saturating(response.pending_mempool_items),
    );
    cdk_prometheus::METRICS.set_federation_sync_ready(&member, response.sync_ready);
    for state in SYNC_STATES {
        cdk_prometheus::METRICS.set_federation_sync_state(
            &member,
            state,
            *state == response.sync_state,
        );
    }
    cdk_prometheus::METRICS.set_federation_session_outcome_quorum_operation_count(
        &member,
        response
            .session_outcome_quorum_operation_count
            .map(u64_to_i64_saturating)
            .unwrap_or(0),
    );

    for status in PAYMENT_REPORT_STATUSES {
        for action in PAYMENT_REPORT_ACTIONS {
            cdk_prometheus::METRICS
                .set_federation_payment_observation_report_count(&member, status, action, 0);
        }
    }

    let mut report_counts = BTreeMap::<(String, String), i64>::new();
    for report in &response.payment_observation_reports {
        let status = report.status.to_string();
        let action = report.recommended_action.to_string();
        match report_counts.get_mut(&(status.clone(), action.clone())) {
            Some(count) => *count += 1,
            None => {
                report_counts.insert((status, action), 1);
            }
        }
    }
    for ((status, action), count) in report_counts {
        cdk_prometheus::METRICS
            .set_federation_payment_observation_report_count(&member, &status, &action, count);
    }
}

#[cfg(feature = "prometheus")]
fn u64_to_i64_saturating(value: u64) -> i64 {
    match i64::try_from(value) {
        Ok(value) => value,
        Err(_) => i64::MAX,
    }
}

fn usize_to_u64_saturating(value: usize) -> u64 {
    match u64::try_from(value) {
        Ok(value) => value,
        Err(_) => u64::MAX,
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct SessionOutcomeQuorumStatus {
    signers: BTreeSet<cdk::federation::MemberId>,
    from_index: u64,
    latest_index: u64,
    operation_count: u64,
}

impl From<FederationSessionOutcomeQuorum> for SessionOutcomeQuorumStatus {
    fn from(quorum: FederationSessionOutcomeQuorum) -> Self {
        Self {
            signers: quorum.signers,
            from_index: quorum.from_index,
            latest_index: quorum.latest_index,
            operation_count: quorum.operation_count,
        }
    }
}

impl SessionOutcomeQuorumStatus {
    fn append(&mut self, quorum: &FederationSessionOutcomeQuorum) {
        self.signers.extend(quorum.signers.iter().copied());
        self.latest_index = quorum.latest_index;
        self.operation_count = self
            .latest_index
            .checked_sub(self.from_index)
            .and_then(|span| span.checked_add(1))
            .unwrap_or(u64::MAX);
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct SessionOutcomeRangeKey {
    from_index: u64,
    operation_count: u64,
    latest_index: u64,
    previous_order_digest: FederationJournalDigest,
    range_order_digest: FederationJournalDigest,
    latest_order_digest: FederationJournalDigest,
    state_digest: FederationStateDigest,
    operation_ids: Vec<FederationOperationId>,
}

impl From<&FederationSessionOutcome> for SessionOutcomeRangeKey {
    fn from(outcome: &FederationSessionOutcome) -> Self {
        Self {
            from_index: outcome.from_index,
            operation_count: outcome.operation_count,
            latest_index: outcome.latest_index,
            previous_order_digest: outcome.previous_order_digest,
            range_order_digest: outcome.range_order_digest,
            latest_order_digest: outcome.latest_order_digest,
            state_digest: outcome.state_digest,
            operation_ids: outcome.operation_ids.clone(),
        }
    }
}

async fn session_outcome_quorum_status(
    state: &MintState,
) -> Result<Option<SessionOutcomeQuorumStatus>, cdk::Error> {
    let Some(runtime_config) = state.mint.federation_config() else {
        return Ok(None);
    };
    let db = state.mint.localstore();
    let entries = db.get_federation_journal_entries().await?;
    let outcomes = db.get_federation_session_outcomes().await?;
    if entries.is_empty() || outcomes.is_empty() {
        return Ok(None);
    }

    let entries_by_index = entries
        .into_iter()
        .map(|entry| (entry.index, entry))
        .collect::<BTreeMap<_, _>>();
    let mut grouped_outcomes: BTreeMap<SessionOutcomeRangeKey, Vec<FederationSessionOutcome>> =
        BTreeMap::new();
    for outcome in outcomes {
        grouped_outcomes
            .entry(SessionOutcomeRangeKey::from(&outcome))
            .or_default()
            .push(outcome);
    }

    let mut quorums_by_start: BTreeMap<u64, Vec<FederationSessionOutcomeQuorum>> = BTreeMap::new();
    for (range, outcomes) in grouped_outcomes {
        let Some(entries) = entries_for_session_outcome_range(&entries_by_index, &range) else {
            continue;
        };
        let response = FederationJournalCatchUpResponse {
            version: FEDERATION_REST_API_VERSION,
            federation_id: runtime_config.public_config().federation_id,
            from_index: range.from_index,
            entries,
            latest_index: Some(range.latest_index),
            latest_order_digest: range.latest_order_digest,
            state_digest: range.state_digest,
            session_outcome: None,
            complete: true,
        };

        let quorum = match response
            .verify_session_outcome_quorum(runtime_config.public_config(), &outcomes)
        {
            Ok(quorum) => quorum,
            Err(FederationError::SessionOutcomeQuorumNotReached { .. }) => continue,
            Err(_) => continue,
        };
        quorums_by_start
            .entry(quorum.from_index)
            .or_default()
            .push(quorum);
    }

    Ok(contiguous_session_outcome_quorum_prefix(&quorums_by_start))
}

async fn payment_observation_report_summaries(
    state: &MintState,
) -> Result<Vec<FederationPaymentObservationReportSummary>, cdk::Error> {
    let Some(runtime_config) = state.mint.federation_config() else {
        return Ok(Vec::new());
    };
    let db = state.mint.localstore();
    let entries = db.get_federation_journal_entries().await?;
    let mut report_keys = BTreeSet::new();

    for entry in &entries {
        if let Some(record) = FederationPaymentObservationRecord::from_journal_entry(entry) {
            report_keys.insert((
                record.observation.context(),
                record.observation.quote().to_string(),
            ));
        }
        if let Some(record) =
            FederationPaymentObservationQuarantineRecord::from_journal_entry(entry)
        {
            report_keys.insert((record.quarantine.context, record.quarantine.quote.clone()));
        }
    }
    let quarantines = entries
        .iter()
        .filter_map(FederationPaymentObservationQuarantineRecord::from_journal_entry)
        .collect::<Vec<_>>();

    let mut reports = Vec::with_capacity(report_keys.len());
    let stale_observation_secs = runtime_config
        .public_config()
        .payment_observation_policy
        .stale_observation_secs;
    let now = current_unix_time();
    for (context, quote) in report_keys {
        let required = runtime_config
            .public_config()
            .payment_verification_policy(context)
            .required_matching_observations();
        let observations = db
            .get_federation_payment_observations(context, &quote)
            .await?;
        let report = FederationPaymentObservationReport::from_records_with_quarantines(
            context,
            &quote,
            required,
            &observations,
            &quarantines,
        );
        let quote_age_secs =
            payment_observation_quote_age_secs(state, context, &quote, now).await?;
        reports.push(report.summary_with_quote_age(quote_age_secs, stale_observation_secs));
    }

    Ok(reports)
}

async fn payment_observation_quote_age_secs(
    state: &MintState,
    context: cdk::federation::FederationPaymentObservationContext,
    quote: &str,
    now: u64,
) -> Result<Option<u64>, cdk::Error> {
    let Ok(quote_id) = cdk::mint::QuoteId::from_str(quote) else {
        return Ok(None);
    };
    let db = state.mint.localstore();
    match context {
        cdk::federation::FederationPaymentObservationContext::MintQuote => Ok(db
            .get_mint_quote(&quote_id)
            .await?
            .map(|quote| now.saturating_sub(quote.created_time))),
        cdk::federation::FederationPaymentObservationContext::MeltQuote => Ok(db
            .get_melt_quote(&quote_id)
            .await?
            .map(|quote| now.saturating_sub(quote.created_time))),
    }
}

fn current_unix_time() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or_default()
}

fn contiguous_session_outcome_quorum_prefix(
    quorums_by_start: &BTreeMap<u64, Vec<FederationSessionOutcomeQuorum>>,
) -> Option<SessionOutcomeQuorumStatus> {
    let mut next_index = 0;
    let mut status: Option<SessionOutcomeQuorumStatus> = None;
    while let Some(quorums) = quorums_by_start.get(&next_index) {
        let Some(best) = quorums.iter().max_by_key(|quorum| quorum.latest_index) else {
            break;
        };
        match &mut status {
            Some(status) => status.append(best),
            None => status = Some(best.clone().into()),
        }
        if best.latest_index < next_index {
            break;
        }
        let Some(after_best) = best.latest_index.checked_add(1) else {
            break;
        };
        next_index = after_best;
    }

    status
}

fn entries_for_session_outcome_range(
    entries_by_index: &BTreeMap<u64, FederationJournalEntry>,
    range: &SessionOutcomeRangeKey,
) -> Option<Vec<FederationJournalEntry>> {
    let range_span = range
        .latest_index
        .checked_sub(range.from_index)
        .and_then(|span| span.checked_add(1))?;
    if range.operation_count == 0
        || range.operation_count != range_span
        || usize::try_from(range.operation_count).ok()? > entries_by_index.len()
    {
        return None;
    }

    let count = usize::try_from(range.operation_count).ok()?;
    let mut entries = Vec::with_capacity(count);
    for offset in 0..range.operation_count {
        let index = range.from_index.checked_add(offset)?;
        entries.push(entries_by_index.get(&index)?.clone());
    }
    Some(entries)
}

fn federation_runtime_config(
    state: &MintState,
) -> Result<&cdk::mint::FederationRuntimeConfig, Response> {
    state
        .mint
        .federation_config()
        .ok_or_else(federation_not_configured)
}

fn federation_sequences(state: &MintState) -> Result<SequenceHandle, Response> {
    state
        .federation_sequences
        .clone()
        .ok_or_else(federation_not_configured)
}

fn federation_not_configured() -> Response {
    (
        StatusCode::NOT_FOUND,
        "mint is not configured as a federation member",
    )
        .into_response()
}

fn federation_error_response(err: FederationError) -> Response {
    let status = match err {
        FederationError::ConflictingOperation { .. } => StatusCode::CONFLICT,
        FederationError::OperationFederationMismatch { .. }
        | FederationError::CatchUpFederationMismatch { .. }
        | FederationError::CatchUpIndexMismatch { .. }
        | FederationError::CatchUpDigestMismatch { .. }
        | FederationError::CatchUpOperationIdMismatch { .. }
        | FederationError::JournalConflictKeysMismatch { .. } => StatusCode::BAD_REQUEST,
        _ => StatusCode::BAD_REQUEST,
    };
    tracing::warn!("Federation request failed: {}", err);
    (status, err.to_string()).into_response()
}

fn cdk_error_response(err: cdk::Error) -> Response {
    match err {
        cdk::Error::Federation(err) => federation_error_response(err),
        err => {
            tracing::error!("Federation storage mutation failed: {}", err);
            (StatusCode::INTERNAL_SERVER_ERROR, err.to_string()).into_response()
        }
    }
}

fn anyhow_response(err: anyhow::Error) -> Response {
    tracing::error!("Federation status failed: {}", err);
    (StatusCode::INTERNAL_SERVER_ERROR, err.to_string()).into_response()
}

#[derive(Debug, Clone)]
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) struct FederationRestPeerClient<T = Async> {
    transport: T,
    public_config: FederationConfig,
    federation_id: FederationId,
    private_config: MemberConfigPrivate,
    sync_config: FederationPeerSyncConfig,
    next_sequence: Arc<AtomicU64>,
    outbound_admission: Arc<tokio::sync::Semaphore>,
}

#[cfg_attr(not(test), allow(dead_code))]
impl<T> FederationRestPeerClient<T>
where
    T: Transport,
{
    pub(crate) fn new(
        transport: T,
        public_config: &FederationConfig,
        private_config: MemberConfigPrivate,
    ) -> anyhow::Result<Self> {
        Self::new_with_config(
            transport,
            public_config,
            private_config,
            FederationPeerSyncConfig::default(),
        )
    }

    pub(crate) fn new_with_config(
        transport: T,
        public_config: &FederationConfig,
        private_config: MemberConfigPrivate,
        sync_config: FederationPeerSyncConfig,
    ) -> anyhow::Result<Self> {
        sync_config.validate()?;
        private_config.validate_against_public(public_config)?;
        Ok(Self {
            transport,
            public_config: public_config.clone(),
            federation_id: public_config.federation_id,
            private_config,
            sync_config,
            next_sequence: Arc::new(AtomicU64::new(initial_federation_rest_sequence())),
            outbound_admission: Arc::new(tokio::sync::Semaphore::new(
                sync_config.private_rest_outbound_concurrency_limit,
            )),
        })
    }

    async fn http_post_with_timeout<P, R>(
        &self,
        member: &MemberConfigPublic,
        endpoint: &str,
        operation: &str,
        payload: &P,
        timeout: Duration,
    ) -> anyhow::Result<R>
    where
        P: Serialize + Send + Sync,
        R: DeserializeOwned,
    {
        let _permit = match self.outbound_admission.clone().try_acquire_owned() {
            Ok(permit) => permit,
            Err(tokio::sync::TryAcquireError::NoPermits) => {
                anyhow::bail!(
                    "federation outbound REST queue is full for peer {} {}",
                    member.member_id,
                    operation
                );
            }
            Err(tokio::sync::TryAcquireError::Closed) => {
                anyhow::bail!(
                    "federation outbound REST admission is closed for peer {} {}",
                    member.member_id,
                    operation
                );
            }
        };
        let request =
            self.transport
                .http_post(federation_endpoint(member, endpoint)?, None, payload);
        match time::timeout(timeout, request).await {
            Ok(result) => Ok(result?),
            Err(_) => anyhow::bail!(
                "federation peer {} {} timed out after {:?}",
                member.member_id,
                operation,
                timeout
            ),
        }
    }

    pub(crate) async fn peer_status(
        &self,
        member: &MemberConfigPublic,
    ) -> anyhow::Result<FederationPeerStatus> {
        let request = self.signed_request(
            member,
            FederationRestMessageKind::Status,
            FederationPeerStatusRequest::default(),
        )?;
        let status = self
            .http_post_with_timeout(
                member,
                "status",
                "status request",
                &request,
                self.sync_config.peer_request_timeout,
            )
            .await?;
        validate_peer_status(self.federation_id, member, &status)?;
        Ok(status)
    }

    pub(crate) async fn submit_operation(
        &self,
        member: &MemberConfigPublic,
        request: &FederationSubmitOperationRequest,
    ) -> anyhow::Result<FederationSubmitOperationResponse> {
        request.ensure_admissible_size()?;
        let expected_operation_id = request.operation_id()?;
        let request = self.signed_request(
            member,
            FederationRestMessageKind::SubmitOperation,
            request.clone(),
        )?;
        let response: FederationSubmitOperationResponse = self
            .http_post_with_timeout(
                member,
                "operations",
                "operation submission",
                &request,
                self.sync_config.operation_submission_timeout,
            )
            .await?;

        if response.operation_id != expected_operation_id {
            anyhow::bail!(
                "federation peer {} returned operation id {} for submitted operation {}",
                member.member_id,
                response.operation_id,
                expected_operation_id
            );
        }

        Ok(response)
    }

    pub(crate) async fn submit_dkg_commitment(
        &self,
        member: &MemberConfigPublic,
        request: &FederationDkgCommitmentRequest,
    ) -> anyhow::Result<FederationDkgSetupResponse> {
        request.validate_sender(&self.public_config, self.private_config.member_id)?;
        let request =
            self.signed_request(member, FederationRestMessageKind::DkgCommitment, *request)?;
        let response: FederationDkgSetupResponse = self
            .http_post_with_timeout(
                member,
                "dkg/commitments",
                "DKG commitment submission",
                &request,
                self.sync_config.peer_request_timeout,
            )
            .await?;
        response.validate(
            self.federation_id,
            member.member_id,
            FederationRestMessageKind::DkgCommitment,
            self.private_config.member_id,
        )?;
        Ok(response)
    }

    pub(crate) async fn submit_dkg_reveal(
        &self,
        member: &MemberConfigPublic,
        request: &FederationDkgRevealRequest,
    ) -> anyhow::Result<FederationDkgSetupResponse> {
        request.validate_sender(&self.public_config, self.private_config.member_id)?;
        let request = self.signed_request(
            member,
            FederationRestMessageKind::DkgReveal,
            request.clone(),
        )?;
        let response: FederationDkgSetupResponse = self
            .http_post_with_timeout(
                member,
                "dkg/reveals",
                "DKG reveal submission",
                &request,
                self.sync_config.peer_request_timeout,
            )
            .await?;
        response.validate(
            self.federation_id,
            member.member_id,
            FederationRestMessageKind::DkgReveal,
            self.private_config.member_id,
        )?;
        Ok(response)
    }

    pub(crate) async fn submit_dkg_secret_share(
        &self,
        member: &MemberConfigPublic,
        request: &FederationDkgSecretShareRequest,
    ) -> anyhow::Result<FederationDkgSetupResponse> {
        request.validate_sender_receiver(
            &self.public_config,
            self.private_config.member_id,
            member.member_id,
        )?;
        let request = self.signed_request(
            member,
            FederationRestMessageKind::DkgSecretShare,
            request.clone(),
        )?;
        let response: FederationDkgSetupResponse = self
            .http_post_with_timeout(
                member,
                "dkg/secret-shares",
                "DKG secret-share submission",
                &request,
                self.sync_config.peer_request_timeout,
            )
            .await?;
        response.validate(
            self.federation_id,
            member.member_id,
            FederationRestMessageKind::DkgSecretShare,
            self.private_config.member_id,
        )?;
        Ok(response)
    }

    pub(crate) async fn submit_dkg_transcript_signature(
        &self,
        member: &MemberConfigPublic,
        request: &FederationDkgTranscriptSignatureRequest,
    ) -> anyhow::Result<FederationDkgSetupResponse> {
        request.validate_sender(&self.public_config, self.private_config.member_id)?;
        let request = self.signed_request(
            member,
            FederationRestMessageKind::DkgTranscriptSignature,
            *request,
        )?;
        let response: FederationDkgSetupResponse = self
            .http_post_with_timeout(
                member,
                "dkg/transcript-signatures",
                "DKG transcript-signature submission",
                &request,
                self.sync_config.peer_request_timeout,
            )
            .await?;
        response.validate(
            self.federation_id,
            member.member_id,
            FederationRestMessageKind::DkgTranscriptSignature,
            self.private_config.member_id,
        )?;
        Ok(response)
    }

    pub(crate) async fn catch_up(
        &self,
        member: &MemberConfigPublic,
        request: &FederationJournalCatchUpRequest,
    ) -> anyhow::Result<FederationJournalCatchUpResponse> {
        let request =
            self.signed_request(member, FederationRestMessageKind::CatchUp, request.clone())?;
        let response: FederationJournalCatchUpResponse = self
            .http_post_with_timeout(
                member,
                "journal",
                "catch-up request",
                &request,
                self.sync_config.peer_request_timeout,
            )
            .await?;
        if response.version != FEDERATION_REST_API_VERSION {
            anyhow::bail!(
                "federation peer {} returned unsupported REST version {}",
                member.member_id,
                response.version
            );
        }
        if response.federation_id != self.federation_id {
            anyhow::bail!(
                "federation peer {} returned catch-up for federation {}, expected {}",
                member.member_id,
                response.federation_id,
                self.federation_id
            );
        }
        response.verify_session_outcome(member)?;
        Ok(response)
    }

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
            consensus
                .catch_up_request()
                .with_limit(self.sync_config.catch_up_limit)
        };
        let response = self.catch_up(member, &request).await?;
        let applied = state.apply_federation_catch_up_response(&response).await?;
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

    fn signed_request<P>(
        &self,
        member: &MemberConfigPublic,
        kind: FederationRestMessageKind,
        payload: P,
    ) -> anyhow::Result<FederationSignedRequest<P>>
    where
        P: Serialize,
    {
        Ok(FederationSignedRequest::new_signed(
            self.federation_id,
            member.member_id,
            next_federation_rest_sequence(&self.next_sequence),
            kind,
            payload,
            &self.private_config,
        )?)
    }
}

fn federation_rest_sequence_time_seed() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| {
            duration
                .as_secs()
                .saturating_mul(1_000_000_000)
                .saturating_add(u64::from(duration.subsec_nanos()))
        })
        .unwrap_or_default()
}

fn initial_federation_rest_sequence() -> u64 {
    let now = federation_rest_sequence_time_seed();

    loop {
        let last = FEDERATION_REST_SEQUENCE_SEED.load(Ordering::Relaxed);
        let candidate = now.max(last.saturating_add(1));
        match FEDERATION_REST_SEQUENCE_SEED.compare_exchange(
            last,
            candidate,
            Ordering::Relaxed,
            Ordering::Relaxed,
        ) {
            Ok(_) => return candidate,
            Err(_) => continue,
        }
    }
}

fn next_federation_rest_sequence(local_sequence: &AtomicU64) -> u64 {
    loop {
        let now = federation_rest_sequence_time_seed();
        let local_last = local_sequence.load(Ordering::Relaxed);
        let global_last = FEDERATION_REST_SEQUENCE_SEED.load(Ordering::Relaxed);
        let candidate = now
            .max(local_last.saturating_add(1))
            .max(global_last.saturating_add(1));
        match FEDERATION_REST_SEQUENCE_SEED.compare_exchange(
            global_last,
            candidate,
            Ordering::Relaxed,
            Ordering::Relaxed,
        ) {
            Ok(_) => {
                local_sequence.store(candidate, Ordering::Relaxed);
                return candidate;
            }
            Err(_) => continue,
        }
    }
}

#[cfg_attr(not(test), allow(dead_code))]
fn federation_endpoint(member: &MemberConfigPublic, endpoint: &str) -> anyhow::Result<url::Url> {
    Ok(member
        .federation_api_url
        .join_paths(&["federation", "v1", endpoint])?)
}

#[cfg_attr(not(test), allow(dead_code))]
fn validate_peer_status(
    expected_federation_id: FederationId,
    member: &MemberConfigPublic,
    status: &FederationPeerStatus,
) -> anyhow::Result<()> {
    if status.version != FEDERATION_REST_API_VERSION {
        anyhow::bail!(
            "federation peer {} returned unsupported REST version {}",
            member.member_id,
            status.version
        );
    }
    if status.member_id != member.member_id {
        anyhow::bail!(
            "federation peer status member mismatch: expected {}, found {}",
            member.member_id,
            status.member_id
        );
    }
    if status.federation_id != expected_federation_id {
        anyhow::bail!(
            "federation peer status federation mismatch: expected {}, found {}",
            expected_federation_id,
            status.federation_id
        );
    }

    Ok(())
}

async fn local_peer_status(state: &MintState) -> anyhow::Result<FederationPeerStatus> {
    let (mut status, entries) = local_peer_status_and_entries(state).await?;
    status.state_digest = materialized_state_digest(state, &entries).await?;
    Ok(status)
}

async fn local_peer_order_status(state: &MintState) -> anyhow::Result<FederationPeerStatus> {
    let (status, _) = local_peer_status_and_entries(state).await?;
    Ok(status)
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

async fn local_peer_status_with_materialized_rows(
    state: &MintState,
) -> anyhow::Result<(
    FederationPeerStatus,
    Vec<FederationMaterializedStateRowFingerprint>,
)> {
    let (mut status, entries) = local_peer_status_and_entries(state).await?;
    let snapshot = materialized_state_snapshot(state, &entries).await?;
    status.state_digest = snapshot.digest()?;
    let materialized_state_rows = snapshot.row_fingerprints()?;
    Ok((status, materialized_state_rows))
}

async fn local_peer_status_and_entries(
    state: &MintState,
) -> anyhow::Result<(FederationPeerStatus, Vec<FederationJournalEntry>)> {
    let runtime_config = state
        .mint
        .federation_config()
        .ok_or_else(|| anyhow::anyhow!("mint is not configured as a federation member"))?;
    let journal = durable_federation_journal(state).await?;
    let status = FederationPeerStatus::from_journal_with_backend(
        runtime_config.federation_id(),
        runtime_config.member_id(),
        &journal,
        runtime_config.consensus_backend(),
    );
    let entries = journal.entries().to_vec();
    Ok((status, entries))
}

async fn durable_federation_journal(
    state: &MintState,
) -> anyhow::Result<InMemoryFederationJournal> {
    let entries = state
        .mint
        .localstore()
        .get_federation_journal_entries()
        .await?;
    Ok(InMemoryFederationJournal::from_entries(entries)?)
}

pub(crate) async fn materialized_state_digest(
    state: &MintState,
    entries: &[FederationJournalEntry],
) -> anyhow::Result<FederationStateDigest> {
    Ok(materialized_state_snapshot(state, entries)
        .await?
        .digest()?)
}

async fn materialized_state_snapshot(
    state: &MintState,
    entries: &[FederationJournalEntry],
) -> anyhow::Result<FederationMaterializedStateSnapshot> {
    let db = state.mint.localstore();
    let mut snapshot = FederationMaterializedStateSnapshot::new();
    let mut mint_quotes = BTreeSet::new();
    let mut melt_quotes = BTreeSet::new();
    let mut payment_observation_quotes = BTreeSet::new();
    let mut payment_quarantines = BTreeSet::new();
    let mut proof_ys = BTreeSet::new();
    let mut blinded_messages = BTreeSet::new();
    let mut keyset_rotations = Vec::new();

    for entry in entries {
        match &entry.envelope.operation {
            FederationOperation::MintQuote(operation) => {
                mint_quotes.insert(operation.quote.clone());
            }
            FederationOperation::MintQuotePayment(observation) => {
                mint_quotes.insert(observation.quote.clone());
                payment_observation_quotes.insert((
                    cdk::federation::FederationPaymentObservationContext::MintQuote,
                    observation.quote.clone(),
                ));
            }
            FederationOperation::Mint(operation) => {
                mint_quotes.extend(
                    operation
                        .request
                        .quote_ids()
                        .into_iter()
                        .map(str::to_string),
                );
                match &operation.request {
                    cdk::federation::FederationMintRequest::Single(request) => {
                        blinded_messages
                            .extend(request.outputs.iter().map(|output| output.blinded_secret));
                    }
                    cdk::federation::FederationMintRequest::Batch(request) => {
                        blinded_messages
                            .extend(request.outputs.iter().map(|output| output.blinded_secret));
                    }
                }
            }
            FederationOperation::MeltQuote(operation) => {
                melt_quotes.insert(operation.quote.clone());
            }
            FederationOperation::MeltQuotePayment(observation) => {
                melt_quotes.insert(observation.quote.clone());
                payment_observation_quotes.insert((
                    cdk::federation::FederationPaymentObservationContext::MeltQuote,
                    observation.quote.clone(),
                ));
            }
            FederationOperation::PaymentObservationQuarantine(operation) => {
                payment_observation_quotes.insert((operation.context, operation.quote.clone()));
                payment_quarantines.insert((operation.context, operation.quote.clone()));
                match operation.context {
                    cdk::federation::FederationPaymentObservationContext::MintQuote => {
                        mint_quotes.insert(operation.quote.clone());
                    }
                    cdk::federation::FederationPaymentObservationContext::MeltQuote => {
                        melt_quotes.insert(operation.quote.clone());
                    }
                }
            }
            FederationOperation::Melt(operation) => {
                melt_quotes.insert(operation.request.quote().clone());
                for proof in operation.request.inputs() {
                    proof_ys.insert(proof.y()?);
                }
                if let Some(outputs) = operation.request.outputs() {
                    blinded_messages.extend(outputs.iter().map(|output| output.blinded_secret));
                }
            }
            FederationOperation::Swap(operation) => {
                for proof in operation.request.inputs() {
                    proof_ys.insert(proof.y()?);
                }
                blinded_messages.extend(
                    operation
                        .request
                        .outputs()
                        .iter()
                        .map(|output| output.blinded_secret),
                );
            }
            FederationOperation::Restore(operation) => {
                blinded_messages.extend(
                    operation
                        .request
                        .outputs
                        .iter()
                        .map(|output| output.blinded_secret),
                );
            }
            FederationOperation::KeysetRotation(operation) => {
                keyset_rotations.push((entry.index, entry.operation_id, operation.keyset.clone()));
            }
        }
    }

    for (operation_index, operation_id, keyset) in keyset_rotations {
        snapshot.push(federation_keyset_materialized_row(
            operation_index,
            operation_id,
            &keyset,
        )?);
    }

    for quote in mint_quotes {
        let quote_id = cdk::mint::QuoteId::from_str(&quote)?;
        match db.get_mint_quote(&quote_id).await? {
            Some(quote) => snapshot.push(
                FederationMaterializedStateRow::new("mint_quotes", quote.id.to_string())
                    .with_field("amount", optional_amount_value(quote.amount.as_ref()))
                    .with_field("unit", quote.unit.to_string())
                    .with_field("request", quote.request.clone())
                    .with_field("expiry", quote.expiry.to_string())
                    .with_field("amount_paid", quote.amount_paid().value().to_string())
                    .with_field("amount_issued", quote.amount_issued().value().to_string())
                    .with_field("state", quote.state().to_string())
                    .with_field("payment_method", quote.payment_method.to_string()),
            ),
            None => snapshot.push(
                FederationMaterializedStateRow::new("mint_quotes", quote)
                    .with_field("missing", "true"),
            ),
        }
    }

    for quote in melt_quotes {
        let quote_id = cdk::mint::QuoteId::from_str(&quote)?;
        match db.get_melt_quote(&quote_id).await? {
            Some(quote) => snapshot.push(
                FederationMaterializedStateRow::new("melt_quotes", quote.id.to_string())
                    .with_field("amount", quote.amount().value().to_string())
                    .with_field("fee_reserve", quote.fee_reserve().value().to_string())
                    .with_field("unit", quote.unit.to_string())
                    .with_field("request", quote.request.to_string())
                    .with_field("state", quote.state.to_string())
                    .with_field("payment_method", quote.payment_method.to_string())
                    .with_field("payment_proof", quote.payment_proof.unwrap_or_default()),
            ),
            None => snapshot.push(
                FederationMaterializedStateRow::new("melt_quotes", quote)
                    .with_field("missing", "true"),
            ),
        }
    }

    for (context, quote) in payment_quarantines {
        snapshot.push(
            FederationMaterializedStateRow::new(
                "payment_observation_quarantines",
                format!("{context}:{quote}"),
            )
            .with_field("context", context.to_string())
            .with_field("quote", quote),
        );
    }

    for (context, quote) in payment_observation_quotes {
        let records = db
            .get_federation_payment_observations(context, &quote)
            .await?;
        if records.is_empty() {
            snapshot.push(
                FederationMaterializedStateRow::new(
                    "payment_observations",
                    format!("{context}:{quote}"),
                )
                .with_field("missing", "true"),
            );
            continue;
        }

        for record in records {
            let key = format!(
                "{}:{}:{}:{}",
                record.observation.context(),
                record.observation.quote(),
                record.operation_index,
                record.operation_id
            );
            snapshot.push(
                FederationMaterializedStateRow::new("payment_observations", key)
                    .with_field("context", record.observation.context().to_string())
                    .with_field("quote", record.observation.quote().to_string())
                    .with_field("operation_index", record.operation_index.to_string())
                    .with_field("operation_id", record.operation_id.to_string())
                    .with_field("observer", record.observation.observer().to_string())
                    .with_field("state", record.observation.state_label())
                    .with_field("evidence_hash", payment_observation_evidence_hash(&record)?),
            );
        }
    }

    if !proof_ys.is_empty() {
        let proof_ys = proof_ys.into_iter().collect::<Vec<_>>();
        let proof_states = db.get_proofs_states(&proof_ys).await?;
        for (y, state) in proof_ys.into_iter().zip(proof_states) {
            let mut row = FederationMaterializedStateRow::new("proof_states", y.to_hex());
            match state {
                Some(state) => row.insert_field("state", state.to_string()),
                None => row.insert_field("missing", "true"),
            }
            snapshot.push(row);
        }
    }

    if !blinded_messages.is_empty() {
        let blinded_messages = blinded_messages.into_iter().collect::<Vec<_>>();
        let blind_signatures = db.get_blind_signatures(&blinded_messages).await?;
        for (blinded_message, signature) in blinded_messages.into_iter().zip(blind_signatures) {
            let mut row =
                FederationMaterializedStateRow::new("blind_signatures", blinded_message.to_hex());
            match signature {
                Some(signature) => {
                    row.insert_field("amount", signature.amount.to_string());
                    row.insert_field("keyset_id", signature.keyset_id.to_string());
                }
                None => row.insert_field("missing", "true"),
            }
            snapshot.push(row);
        }
    }

    Ok(snapshot)
}

fn federation_keyset_materialized_row(
    operation_index: u64,
    operation_id: FederationOperationId,
    keyset: &FederatedKeyset,
) -> anyhow::Result<FederationMaterializedStateRow> {
    Ok(
        FederationMaterializedStateRow::new("federation_keysets", keyset.id.to_string())
            .with_field("operation_index", operation_index.to_string())
            .with_field("operation_id", operation_id.to_string())
            .with_field("unit", keyset.unit.to_string())
            .with_field("active", keyset.active.to_string())
            .with_field("input_fee_ppk", keyset.input_fee_ppk.to_string())
            .with_field(
                "final_expiry",
                keyset
                    .final_expiry
                    .map(|expiry| expiry.to_string())
                    .unwrap_or_default(),
            )
            .with_field(
                "aggregate_keys",
                serde_json::to_string(&keyset.aggregate_keys)?,
            )
            .with_field(
                "public_shares",
                serde_json::to_string(&keyset.public_shares)?,
            ),
    )
}

fn optional_amount_value(amount: Option<&cdk::Amount<cdk::nuts::CurrencyUnit>>) -> String {
    amount
        .map(|amount| amount.value().to_string())
        .unwrap_or_default()
}

fn payment_observation_evidence_hash(
    record: &FederationPaymentObservationRecord,
) -> anyhow::Result<String> {
    let canonical = serde_json::to_vec(&record.observation)?;
    Ok(hex_encode(&Sha256::digest(canonical)))
}

fn hex_encode(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut encoded = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        encoded.push(HEX[(byte >> 4) as usize] as char);
        encoded.push(HEX[(byte & 0x0f) as usize] as char);
    }
    encoded
}

fn verify_session_outcome_evidence_entries(
    local_entries: &[FederationJournalEntry],
    response: &FederationJournalCatchUpResponse,
) -> anyhow::Result<()> {
    let first = local_entries
        .first()
        .ok_or_else(|| anyhow::anyhow!("missing local entries for evidence response"))?;
    if response.from_index != first.index {
        anyhow::bail!(
            "evidence response started at {}, expected {}",
            response.from_index,
            first.index
        );
    }
    if response.entries.len() != local_entries.len() {
        anyhow::bail!(
            "evidence response carried {} entries, expected {}",
            response.entries.len(),
            local_entries.len()
        );
    }

    for (local, peer) in local_entries.iter().zip(&response.entries) {
        let peer_operation_id = peer.envelope.operation_id()?;
        if peer.index != local.index
            || peer.operation_id != local.operation_id
            || peer_operation_id != local.operation_id
            || peer.previous_order_digest != local.previous_order_digest
            || peer.order_digest != local.order_digest
        {
            anyhow::bail!(
                "evidence entry mismatch at local index {} and peer index {}",
                local.index,
                peer.index
            );
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests;
