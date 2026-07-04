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
    FederatedKeyset, FederationConfig, FederationConsensusEngine, FederationConsensusRuntime,
    FederationDkgCommitmentRequest, FederationDkgRevealRequest, FederationDkgSecretShareRequest,
    FederationDkgSetupResponse, FederationDkgTranscriptSignatureRequest, FederationError,
    FederationId, FederationJournalCatchUpRequest, FederationJournalCatchUpResponse,
    FederationJournalDigest, FederationJournalEntry, FederationMaterializedStateRow,
    FederationMaterializedStateSnapshot, FederationOperation, FederationOperationEnvelope,
    FederationOperationId, FederationPaymentObservationQuarantineRecord,
    FederationPaymentObservationRecord, FederationPaymentObservationReport,
    FederationPaymentObservationReportSummary, FederationPeerStatus, FederationPeerStatusRequest,
    FederationRestMessageKind, FederationRestSequenceTracker, FederationSessionOutcome,
    FederationSessionOutcomeQuorum, FederationSignedRequest, FederationStateDigest,
    FederationSubmitOperationRequest, FederationSubmitOperationResponse, MemberConfigPrivate,
    MemberConfigPublic, MemberId, FEDERATION_REST_API_VERSION,
};
use cdk_http_client::{Async, Transport};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::time::{self, Duration, MissedTickBehavior};
use tracing::instrument;

use crate::MintState;

type ConsensusHandle = std::sync::Arc<tokio::sync::Mutex<FederationConsensusRuntime>>;
type SequenceHandle = std::sync::Arc<tokio::sync::Mutex<FederationRestSequenceTracker>>;

const FEDERATION_PEER_SYNC_INTERVAL: Duration = Duration::from_secs(5);
const FEDERATION_DISABLE_PEER_SYNC_ENV_VAR: &str = "CDK_FEDERATION_DISABLE_PEER_SYNC";
const FEDERATION_SESSION_OUTCOME_EVIDENCE_CHUNK_LIMIT: usize = 1000;
static FEDERATION_REST_SEQUENCE_SEED: AtomicU64 = AtomicU64::new(0);

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

pub(crate) fn spawn_federation_peer_sync_loop(state: MintState) {
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
        let client =
            match FederationRestPeerClient::new(Async::default(), &public_config, private_config) {
                Ok(client) => client,
                Err(err) => {
                    tracing::error!("Could not start federation peer sync loop: {}", err);
                    return;
                }
            };
        let mut interval = time::interval(FEDERATION_PEER_SYNC_INTERVAL);
        interval.set_missed_tick_behavior(MissedTickBehavior::Delay);

        loop {
            interval.tick().await;
            run_federation_peer_sync_round(&state, &client, &peers).await;
        }
    });
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

async fn run_federation_peer_sync_round<T>(
    state: &MintState,
    client: &FederationRestPeerClient<T>,
    peers: &[MemberConfigPublic],
) -> usize
where
    T: Transport,
{
    let mut synced = 0;
    for peer in peers {
        match client.sync_once_from_member(state, peer).await {
            Ok(status) => {
                synced += 1;
                tracing::debug!(
                    member_id = %peer.member_id,
                    operation_count = status.operation_count,
                    latest_index = ?status.latest_index,
                    "Federation peer sync completed"
                );
            }
            Err(err) => {
                tracing::warn!(
                    member_id = %peer.member_id,
                    "Federation peer sync failed: {}",
                    err
                );
            }
        }
    }
    let collected = collect_session_outcome_evidence_round(state, client, peers).await;
    if collected > 0 {
        tracing::debug!(
            session_outcomes = collected,
            "Federation peer sync collected session outcome evidence"
        );
    }
    synced
}

async fn collect_session_outcome_evidence_round<T>(
    state: &MintState,
    client: &FederationRestPeerClient<T>,
    peers: &[MemberConfigPublic],
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
    let mut collected = match local_peer_status(state).await {
        Ok(peer_status) => {
            persist_local_session_outcome_evidence_chunks(
                state,
                runtime_config,
                &entries,
                peer_status.latest_order_digest,
                peer_status.state_digest,
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

    let operation_count = entries.len() as u64;
    match session_outcome_quorum_status(state).await {
        Ok(Some(quorum)) if quorum.from_index == 0 && quorum.operation_count >= operation_count => {
            state
                .mark_federation_healthy_if_session_outcome_verified(quorum.operation_count)
                .await;
            return 0;
        }
        Ok(_) => {}
        Err(err) => {
            tracing::warn!(
                "Could not inspect federation session outcome quorum: {}",
                err
            );
        }
    }

    for peer in peers {
        collected +=
            collect_session_outcome_evidence_from_peer(state, client, peer, &entries).await;
    }

    mark_healthy_after_session_outcome_quorum(state, operation_count).await;

    collected
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
    let client = FederationRestPeerClient::new(
        transport,
        runtime_config.public_config(),
        runtime_config.private_config().clone(),
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
) -> usize {
    let mut persisted = 0;
    for chunk in entries.chunks(FEDERATION_SESSION_OUTCOME_EVIDENCE_CHUNK_LIMIT) {
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
) -> usize
where
    T: Transport,
{
    let mut collected = 0;
    for chunk in entries.chunks(FEDERATION_SESSION_OUTCOME_EVIDENCE_CHUNK_LIMIT) {
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
                continue;
            }
        };
        if let Err(err) = verify_session_outcome_evidence_entries(chunk, &response) {
            tracing::warn!(
                member_id = %peer.member_id,
                "Federation session outcome evidence did not match local journal: {}",
                err
            );
            continue;
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

    collected
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
    let verified =
        verify_federation_request(&state, &payload, FederationRestMessageKind::Status).await?;
    state
        .persist_federation_sequence(verified.sender, verified.sequence)
        .await
        .map_err(cdk_error_response)?;
    let peer_status = local_peer_status(&state).await.map_err(anyhow_response)?;
    Ok(Json(peer_status))
}

#[instrument(skip_all)]
async fn post_local_status(
    State(state): State<MintState>,
    Json(payload): Json<FederationSignedRequest<FederationPeerStatusRequest>>,
) -> Result<Json<FederationLocalStatusResponse>, Response> {
    let verified =
        verify_federation_request(&state, &payload, FederationRestMessageKind::Status).await?;
    state
        .persist_federation_sequence(verified.sender, verified.sequence)
        .await
        .map_err(cdk_error_response)?;
    let peer_status = local_peer_status(&state).await.map_err(anyhow_response)?;
    let sync_status = state.federation_sync_status().await.unwrap_or_default();
    let session_outcome_quorum = session_outcome_quorum_status(&state)
        .await
        .map_err(cdk_error_response)?;
    let payment_observation_reports = payment_observation_report_summaries(&state)
        .await
        .map_err(cdk_error_response)?;
    let response = FederationLocalStatusResponse::new(
        peer_status,
        sync_status,
        session_outcome_quorum,
        payment_observation_reports,
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
    let verified =
        verify_federation_request(&state, &payload, FederationRestMessageKind::CatchUp).await?;
    state
        .persist_federation_sequence(verified.sender, verified.sequence)
        .await
        .map_err(cdk_error_response)?;
    let consensus = federation_consensus(&state)?;
    let private_config = federation_runtime_config(&state)?.private_config().clone();
    tracing::debug!(
        next_index = payload.payload.next_index,
        limit = ?payload.payload.limit,
        "Received federation journal catch-up request"
    );
    let (mut response, entries) = {
        let consensus = consensus.lock().await;
        (
            consensus
                .catch_up_checked(&payload.payload)
                .map_err(federation_error_response)?,
            consensus.journal().entries().to_vec(),
        )
    };
    response.state_digest = materialized_state_digest(&state, &entries)
        .await
        .map_err(anyhow_response)?;
    let response = response
        .with_signed_session_outcome(&private_config)
        .map_err(federation_error_response)?;
    Ok(Json(response))
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) session_outcome_quorum_operation_count: Option<u64>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) session_outcome_quorum_signers: Vec<cdk::federation::MemberId>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) payment_observation_reports: Vec<FederationPaymentObservationReportSummary>,
}

impl FederationLocalStatusResponse {
    fn new(
        peer_status: FederationPeerStatus,
        sync_status: crate::FederationSyncStatus,
        session_outcome_quorum: Option<SessionOutcomeQuorumStatus>,
        payment_observation_reports: Vec<FederationPaymentObservationReportSummary>,
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
            session_outcome_quorum_operation_count,
            session_outcome_quorum_signers,
            payment_observation_reports,
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
        self.operation_count = self.latest_index.saturating_sub(self.from_index) + 1;
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
        next_index = best.latest_index.saturating_add(1);
    }

    status
}

fn entries_for_session_outcome_range(
    entries_by_index: &BTreeMap<u64, FederationJournalEntry>,
    range: &SessionOutcomeRangeKey,
) -> Option<Vec<FederationJournalEntry>> {
    if range.operation_count == 0
        || range.latest_index < range.from_index
        || range.operation_count != range.latest_index.saturating_sub(range.from_index) + 1
    {
        return None;
    }

    let mut entries = Vec::with_capacity(range.operation_count as usize);
    for index in range.from_index..=range.latest_index {
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

fn federation_consensus(state: &MintState) -> Result<ConsensusHandle, Response> {
    state
        .federation_consensus
        .clone()
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
    next_sequence: Arc<AtomicU64>,
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
        private_config.validate_against_public(public_config)?;
        Ok(Self {
            transport,
            public_config: public_config.clone(),
            federation_id: public_config.federation_id,
            private_config,
            next_sequence: Arc::new(AtomicU64::new(initial_federation_rest_sequence())),
        })
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
            .transport
            .http_post(federation_endpoint(member, "status")?, None, &request)
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
            .transport
            .http_post(federation_endpoint(member, "operations")?, None, &request)
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
            .transport
            .http_post(
                federation_endpoint(member, "dkg/commitments")?,
                None,
                &request,
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
            .transport
            .http_post(federation_endpoint(member, "dkg/reveals")?, None, &request)
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
            .transport
            .http_post(
                federation_endpoint(member, "dkg/secret-shares")?,
                None,
                &request,
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
            .transport
            .http_post(
                federation_endpoint(member, "dkg/transcript-signatures")?,
                None,
                &request,
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
            .transport
            .http_post(federation_endpoint(member, "journal")?, None, &request)
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
    ) -> anyhow::Result<Vec<FederationJournalEntry>> {
        let consensus = state
            .federation_consensus
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("mint is not configured as a federation member"))?;
        let request = {
            let consensus = consensus.lock().await;
            consensus.catch_up_request()
        };
        let response = self.catch_up(member, &request).await?;
        Ok(state.apply_federation_catch_up_response(&response).await?)
    }

    pub(crate) async fn sync_once_from_member(
        &self,
        state: &MintState,
        member: &MemberConfigPublic,
    ) -> anyhow::Result<FederationPeerStatus> {
        let peer_status = self.peer_status(member).await?;
        let mut local_status = local_peer_status(state).await?;
        let mut recovered_from_peer = false;

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
                if let Err(err) = self.catch_up_state_from_member(state, member).await {
                    state
                        .mark_federation_lagging(
                            member.member_id,
                            before,
                            peer_status.operation_count,
                        )
                        .await;
                    return Err(err);
                }

                local_status = local_peer_status(state).await?;
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

        local_status = local_peer_status(state).await?;
        let order_digest_mismatch =
            local_status.latest_order_digest != peer_status.latest_order_digest;
        let state_digest_mismatch = local_status.state_digest != peer_status.state_digest;
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
        if local_status.operation_count == peer_status.operation_count && state_digest_mismatch {
            state
                .mark_federation_materialized_state_mismatch(
                    member.member_id,
                    peer_status.operation_count,
                )
                .await;
            anyhow::bail!(
                "federation peer {} materialized state digest mismatch at operation count {}",
                member.member_id,
                peer_status.operation_count
            );
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
    let consensus = state
        .federation_consensus
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("mint is not configured as a federation member"))?;
    let (mut status, entries) = {
        let consensus = consensus.lock().await;
        (
            consensus.peer_status(),
            consensus.journal().entries().to_vec(),
        )
    };
    status.state_digest = materialized_state_digest(state, &entries).await?;
    Ok(status)
}

async fn materialized_state_digest(
    state: &MintState,
    entries: &[FederationJournalEntry],
) -> anyhow::Result<FederationStateDigest> {
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

    Ok(snapshot.digest()?)
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
mod tests {
    use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
    use std::str::FromStr;
    use std::sync::{Arc, Mutex};

    use async_trait::async_trait;
    use bip39::Mnemonic;
    use cdk::cdk_payment::PaymentIdentifier;
    use cdk::federation::{
        generate_trusted_dealer_configs, FederationDkgSetupMethod, FederationDkgTranscript,
        FederationDkgTranscriptSignature, FederationKeysetRotationOperation,
        FederationMintQuotePaymentObservation, FederationOperation, FederationOperationEnvelope,
        FederationOperationKind, FederationOperationStatus, FederationPaymentObservationPolicy,
        FederationPaymentObservationQuarantineReason, FederationPaymentObservationReportAction,
        FederationPaymentTrustModel, FederationRestoreOperation, InMemoryFederationConsensus,
        MemberConfigPrivate, MemberId, ThresholdParams, TrustedDealerKeysetSpec,
        TrustedDealerMember,
    };
    use cdk::mint::{MintBuilder, MintInput, MintMeltLimits, MintQuote, QuoteId};
    use cdk::mint_url::MintUrl;
    use cdk::nuts::nut00::KnownMethod;
    use cdk::nuts::nut01::{evaluate_secret_key_coefficients, BlsG1PublicKey, BlsSecretKey};
    use cdk::nuts::nut22::AuthToken;
    use cdk::nuts::{
        BlindedMessage, CurrencyUnit, MeltQuoteBolt11Request, MeltQuoteState,
        MintQuoteBolt11Request, MintQuoteBolt11Response, MintQuoteState, MintRequest,
        PaymentMethod, PublicKey, RestoreRequest, SecretKey,
    };
    use cdk::types::FeeReserve;
    use cdk::Amount;
    use cdk_fake_wallet::{create_fake_invoice, FakeInvoiceDescription, FakeWallet};
    use cdk_http_client::HttpError;
    use serde::de::DeserializeOwned;
    use serde::Serialize;

    use super::*;

    #[test]
    fn federation_peer_sync_disable_value_accepts_truthy_values() {
        for value in [
            "1", "true", "TRUE", "True", "yes", "YES", "Yes", "on", "ON", "On",
        ] {
            assert!(federation_peer_sync_disabled_value(value));
        }

        for value in ["", "0", "false", "no", "off"] {
            assert!(!federation_peer_sync_disabled_value(value));
        }
    }

    #[derive(Debug, Default)]
    struct MockFederationTransportState {
        status_response: Option<FederationPeerStatus>,
        submit_response: Option<FederationSubmitOperationResponse>,
        dkg_setup_responses: VecDeque<FederationDkgSetupResponse>,
        catch_up_responses: VecDeque<FederationJournalCatchUpResponse>,
        status_requests: Vec<FederationSignedRequest<FederationPeerStatusRequest>>,
        dkg_commitment_requests: Vec<FederationSignedRequest<FederationDkgCommitmentRequest>>,
        dkg_reveal_requests: Vec<FederationSignedRequest<FederationDkgRevealRequest>>,
        dkg_secret_share_requests: Vec<FederationSignedRequest<FederationDkgSecretShareRequest>>,
        dkg_transcript_signature_requests:
            Vec<FederationSignedRequest<FederationDkgTranscriptSignatureRequest>>,
        submitted_operations: Vec<FederationSignedRequest<FederationSubmitOperationRequest>>,
        catch_up_requests: Vec<FederationSignedRequest<FederationJournalCatchUpRequest>>,
    }

    #[derive(Debug, Clone, Default)]
    struct MockFederationTransport {
        state: Arc<Mutex<MockFederationTransportState>>,
    }

    struct TempMintDbFile {
        path: String,
    }

    impl TempMintDbFile {
        fn new(name: &str) -> Self {
            let path = std::env::temp_dir()
                .join(format!("{name}-{}.sqlite", uuid::Uuid::new_v4()))
                .to_string_lossy()
                .to_string();

            Self { path }
        }

        fn path(&self) -> &str {
            &self.path
        }
    }

    impl Drop for TempMintDbFile {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.path);
            let _ = std::fs::remove_file(format!("{}-shm", self.path));
            let _ = std::fs::remove_file(format!("{}-wal", self.path));
        }
    }

    impl MockFederationTransport {
        fn with_status_response(self, response: FederationPeerStatus) -> Self {
            self.state
                .lock()
                .expect("mock transport lock")
                .status_response = Some(response);
            self
        }

        fn with_submit_response(self, response: FederationSubmitOperationResponse) -> Self {
            self.state
                .lock()
                .expect("mock transport lock")
                .submit_response = Some(response);
            self
        }

        fn with_dkg_setup_response(self, response: FederationDkgSetupResponse) -> Self {
            self.state
                .lock()
                .expect("mock transport lock")
                .dkg_setup_responses
                .push_back(response);
            self
        }

        fn with_catch_up_response(self, response: FederationJournalCatchUpResponse) -> Self {
            self.state
                .lock()
                .expect("mock transport lock")
                .catch_up_responses
                .push_back(response);
            self
        }

        fn status_requests(&self) -> Vec<FederationSignedRequest<FederationPeerStatusRequest>> {
            self.state
                .lock()
                .expect("mock transport lock")
                .status_requests
                .clone()
        }

        fn dkg_commitment_requests(
            &self,
        ) -> Vec<FederationSignedRequest<FederationDkgCommitmentRequest>> {
            self.state
                .lock()
                .expect("mock transport lock")
                .dkg_commitment_requests
                .clone()
        }

        fn dkg_reveal_requests(&self) -> Vec<FederationSignedRequest<FederationDkgRevealRequest>> {
            self.state
                .lock()
                .expect("mock transport lock")
                .dkg_reveal_requests
                .clone()
        }

        fn dkg_secret_share_requests(
            &self,
        ) -> Vec<FederationSignedRequest<FederationDkgSecretShareRequest>> {
            self.state
                .lock()
                .expect("mock transport lock")
                .dkg_secret_share_requests
                .clone()
        }

        fn dkg_transcript_signature_requests(
            &self,
        ) -> Vec<FederationSignedRequest<FederationDkgTranscriptSignatureRequest>> {
            self.state
                .lock()
                .expect("mock transport lock")
                .dkg_transcript_signature_requests
                .clone()
        }

        fn submitted_operations(
            &self,
        ) -> Vec<FederationSignedRequest<FederationSubmitOperationRequest>> {
            self.state
                .lock()
                .expect("mock transport lock")
                .submitted_operations
                .clone()
        }

        fn catch_up_requests(
            &self,
        ) -> Vec<FederationSignedRequest<FederationJournalCatchUpRequest>> {
            self.state
                .lock()
                .expect("mock transport lock")
                .catch_up_requests
                .clone()
        }
    }

    #[async_trait]
    impl Transport for MockFederationTransport {
        fn with_proxy(
            &mut self,
            _proxy: url::Url,
            _host_matcher: Option<&str>,
            _accept_invalid_certs: bool,
        ) -> Result<(), HttpError> {
            Ok(())
        }

        async fn http_get<R>(&self, url: url::Url, _auth: Option<AuthToken>) -> Result<R, HttpError>
        where
            R: DeserializeOwned,
        {
            Err(HttpError::Other(format!(
                "unexpected GET path {}",
                url.path()
            )))
        }

        async fn http_post<P, R>(
            &self,
            url: url::Url,
            _auth_token: Option<AuthToken>,
            payload: &P,
        ) -> Result<R, HttpError>
        where
            P: Serialize + Send + Sync,
            R: DeserializeOwned,
        {
            let mut state = self.state.lock().expect("mock transport lock");
            match url.path() {
                path if path.ends_with("/federation/v1/status") => {
                    let request = serde_json::from_value::<
                        FederationSignedRequest<FederationPeerStatusRequest>,
                    >(serde_json::to_value(payload)?)?;
                    state.status_requests.push(request);
                    let response = state
                        .status_response
                        .clone()
                        .ok_or_else(|| HttpError::Other("missing status response".to_string()))?;
                    json_response(response)
                }
                path if path.ends_with("/federation/v1/dkg/commitments") => {
                    let request = serde_json::from_value::<
                        FederationSignedRequest<FederationDkgCommitmentRequest>,
                    >(serde_json::to_value(payload)?)?;
                    state.dkg_commitment_requests.push(request);
                    let response = state.dkg_setup_responses.pop_front().ok_or_else(|| {
                        HttpError::Other("missing DKG setup response".to_string())
                    })?;
                    json_response(response)
                }
                path if path.ends_with("/federation/v1/dkg/reveals") => {
                    let request = serde_json::from_value::<
                        FederationSignedRequest<FederationDkgRevealRequest>,
                    >(serde_json::to_value(payload)?)?;
                    state.dkg_reveal_requests.push(request);
                    let response = state.dkg_setup_responses.pop_front().ok_or_else(|| {
                        HttpError::Other("missing DKG setup response".to_string())
                    })?;
                    json_response(response)
                }
                path if path.ends_with("/federation/v1/dkg/secret-shares") => {
                    let request = serde_json::from_value::<
                        FederationSignedRequest<FederationDkgSecretShareRequest>,
                    >(serde_json::to_value(payload)?)?;
                    state.dkg_secret_share_requests.push(request);
                    let response = state.dkg_setup_responses.pop_front().ok_or_else(|| {
                        HttpError::Other("missing DKG setup response".to_string())
                    })?;
                    json_response(response)
                }
                path if path.ends_with("/federation/v1/dkg/transcript-signatures") => {
                    let request = serde_json::from_value::<
                        FederationSignedRequest<FederationDkgTranscriptSignatureRequest>,
                    >(serde_json::to_value(payload)?)?;
                    state.dkg_transcript_signature_requests.push(request);
                    let response = state.dkg_setup_responses.pop_front().ok_or_else(|| {
                        HttpError::Other("missing DKG setup response".to_string())
                    })?;
                    json_response(response)
                }
                path if path.ends_with("/federation/v1/operations") => {
                    let request = serde_json::from_value::<
                        FederationSignedRequest<FederationSubmitOperationRequest>,
                    >(serde_json::to_value(payload)?)?;
                    state.submitted_operations.push(request);
                    let response = state
                        .submit_response
                        .clone()
                        .ok_or_else(|| HttpError::Other("missing submit response".to_string()))?;
                    json_response(response)
                }
                path if path.ends_with("/federation/v1/journal") => {
                    let request = serde_json::from_value::<
                        FederationSignedRequest<FederationJournalCatchUpRequest>,
                    >(serde_json::to_value(payload)?)?;
                    state.catch_up_requests.push(request);
                    let response = state
                        .catch_up_responses
                        .pop_front()
                        .ok_or_else(|| HttpError::Other("missing catch-up response".to_string()))?;
                    json_response(response)
                }
                path => Err(HttpError::Other(format!("unexpected POST path {path}"))),
            }
        }
    }

    fn json_response<T, R>(value: T) -> Result<R, HttpError>
    where
        T: Serialize,
        R: DeserializeOwned,
    {
        Ok(serde_json::from_value(serde_json::to_value(value)?)?)
    }

    fn member_id(value: u16) -> MemberId {
        MemberId::new(value).expect("member id")
    }

    fn private_member(value: u16) -> MemberConfigPrivate {
        MemberConfigPrivate {
            federation_id: None,
            member_id: member_id(value),
            identity_secret_key: SecretKey::from_slice(&[value as u8; 32])
                .expect("identity secret"),
            keyset_secret_shares: BTreeMap::new(),
            setup_transcript_hash: None,
        }
    }

    fn transport_config() -> (FederationConfig, MemberConfigPrivate, MemberConfigPrivate) {
        transport_config_with_federation_api_urls(
            MintUrl::from_str("https://fed1.example.com/api").expect("federation api url"),
            MintUrl::from_str("https://fed2.example.com/api").expect("federation api url"),
        )
    }

    fn transport_config_with_federation_api_urls(
        first_federation_api_url: MintUrl,
        second_federation_api_url: MintUrl,
    ) -> (FederationConfig, MemberConfigPrivate, MemberConfigPrivate) {
        let members = vec![
            TrustedDealerMember::new(
                member_id(1),
                MintUrl::from_str("https://mint1.example.com").expect("public mint url"),
                first_federation_api_url,
                SecretKey::from_slice(&[1; 32]).expect("identity secret"),
            ),
            TrustedDealerMember::new(
                member_id(2),
                MintUrl::from_str("https://mint2.example.com").expect("public mint url"),
                second_federation_api_url,
                SecretKey::from_slice(&[2; 32]).expect("identity secret"),
            ),
        ];
        let keyset = TrustedDealerKeysetSpec::new(
            CurrencyUnit::Sat,
            true,
            0,
            None,
            BTreeMap::from([(
                Amount::from(1),
                vec![
                    BlsSecretKey::from_reduced_bytes(&[11; 32]),
                    BlsSecretKey::from_reduced_bytes(&[12; 32]),
                ],
            )]),
        );
        let bundle = generate_trusted_dealer_configs(
            ThresholdParams::new(2, 2, 2).expect("threshold params"),
            members,
            vec![keyset],
        )
        .expect("trusted dealer bundle");
        let first_private = bundle
            .private_configs
            .get(&member_id(1))
            .expect("first private config")
            .clone();
        let second_private = bundle
            .private_configs
            .get(&member_id(2))
            .expect("second private config")
            .clone();
        (bundle.public_config, first_private, second_private)
    }

    async fn federation_route_state_with_local_private() -> (
        MintState,
        FederationConfig,
        MemberConfigPrivate,
        MemberConfigPrivate,
    ) {
        let (public_config, local_private, peer_private) = transport_config();
        let state =
            federation_route_state_for_private(public_config.clone(), local_private.clone()).await;

        (state, public_config, local_private, peer_private)
    }

    async fn federation_route_state_for_private(
        public_config: FederationConfig,
        local_private: MemberConfigPrivate,
    ) -> MintState {
        let db = Arc::new(cdk_sqlite::mint::memory::empty().await.expect("mint db"));
        let mnemonic = Mnemonic::generate(12).expect("mnemonic");
        federation_route_state_for_private_with_db(
            public_config,
            local_private,
            db,
            &mnemonic.to_seed_normalized(""),
        )
        .await
    }

    async fn federation_route_state_for_private_with_db(
        public_config: FederationConfig,
        local_private: MemberConfigPrivate,
        db: Arc<cdk_sqlite::MintSqliteDatabase>,
        seed: &[u8],
    ) -> MintState {
        let runtime_config =
            cdk::mint::FederationRuntimeConfig::new(public_config.clone(), local_private.clone())
                .expect("runtime config");
        let mut builder = MintBuilder::new(db.clone())
            .with_batch_minting(Some(10), Some(vec![KnownMethod::Bolt11.to_string()]))
            .with_federation_config(runtime_config.clone());
        let fake = FakeWallet::new(
            FeeReserve {
                min_fee_reserve: 1.into(),
                percent_fee_reserve: 0.0,
            },
            HashMap::default(),
            HashSet::default(),
            0,
            CurrencyUnit::Sat,
        );
        builder
            .add_payment_processor(
                CurrencyUnit::Sat,
                PaymentMethod::Known(KnownMethod::Bolt11),
                MintMeltLimits::new(1, 10_000),
                Arc::new(fake),
            )
            .await
            .expect("payment processor");
        let mint = builder.build_with_seed(db, seed).await.expect("mint");
        MintState {
            mint: Arc::new(mint),
            cache: Arc::new(crate::cache::HttpCache::default()),
            federation_consensus: Some(Arc::new(tokio::sync::Mutex::new(
                FederationConsensusRuntime::in_memory(
                    runtime_config.federation_id(),
                    runtime_config.member_id(),
                ),
            ))),
            federation_sequences: Some(Arc::new(tokio::sync::Mutex::new(
                FederationRestSequenceTracker::new(),
            ))),
            federation_sync: Some(Arc::new(tokio::sync::Mutex::new(
                crate::FederationSyncState::default(),
            ))),
            federation_notifications: Some(Arc::new(
                crate::FederationOperationNotifications::default(),
            )),
            federation_finalizer: None,
        }
    }

    async fn federation_route_state() -> (MintState, FederationConfig, MemberConfigPrivate) {
        let (state, public_config, _, peer_private) =
            federation_route_state_with_local_private().await;
        (state, public_config, peer_private)
    }

    fn signed_operation_request(
        public_config: &FederationConfig,
        private_config: &MemberConfigPrivate,
        sequence: u64,
        kind: FederationRestMessageKind,
        envelope: FederationOperationEnvelope,
    ) -> FederationSignedRequest<FederationSubmitOperationRequest> {
        FederationSignedRequest::new_signed(
            public_config.federation_id,
            member_id(1),
            sequence,
            kind,
            FederationSubmitOperationRequest::new(envelope),
            private_config,
        )
        .expect("signed operation request")
    }

    fn signed_status_request(
        public_config: &FederationConfig,
        private_config: &MemberConfigPrivate,
        receiver: MemberId,
        sequence: u64,
    ) -> FederationSignedRequest<FederationPeerStatusRequest> {
        FederationSignedRequest::new_signed(
            public_config.federation_id,
            receiver,
            sequence,
            FederationRestMessageKind::Status,
            FederationPeerStatusRequest::default(),
            private_config,
        )
        .expect("signed status request")
    }

    fn signed_catch_up_request(
        public_config: &FederationConfig,
        private_config: &MemberConfigPrivate,
        receiver: MemberId,
        sequence: u64,
        request: FederationJournalCatchUpRequest,
    ) -> FederationSignedRequest<FederationJournalCatchUpRequest> {
        FederationSignedRequest::new_signed(
            public_config.federation_id,
            receiver,
            sequence,
            FederationRestMessageKind::CatchUp,
            request,
            private_config,
        )
        .expect("signed catch-up request")
    }

    fn signed_catch_up_response(
        response: FederationJournalCatchUpResponse,
        private_config: &MemberConfigPrivate,
    ) -> FederationJournalCatchUpResponse {
        response
            .with_signed_session_outcome(private_config)
            .expect("signed catch-up response")
    }

    fn dkg_reveal(
        public_config: &FederationConfig,
        member_id: MemberId,
        seed: u8,
    ) -> cdk::federation::FederationDkgParticipantCommitmentReveal {
        cdk::federation::FederationDkgParticipantCommitmentReveal {
            federation_id: public_config.federation_id,
            member_id,
            coefficient_commitments: BTreeMap::from([(
                Amount::from(1),
                vec![
                    BlsSecretKey::from_reduced_bytes(&[seed; 32]).public_key_g2(),
                    BlsSecretKey::from_reduced_bytes(&[seed + 10; 32]).public_key_g2(),
                ],
            )]),
        }
    }

    fn dkg_commitment_request(
        public_config: &FederationConfig,
        member_id: MemberId,
        seed: u8,
    ) -> FederationDkgCommitmentRequest {
        let reveal = dkg_reveal(public_config, member_id, seed);
        let commitment = cdk::federation::FederationDkgParticipantCommitment::from_reveal(&reveal)
            .expect("dkg commitment");
        FederationDkgCommitmentRequest::new(commitment)
    }

    fn dkg_reveal_request(
        public_config: &FederationConfig,
        member_id: MemberId,
        seed: u8,
    ) -> FederationDkgRevealRequest {
        FederationDkgRevealRequest::new(dkg_reveal(public_config, member_id, seed))
    }

    fn dkg_secret_share_request(
        public_config: &FederationConfig,
        sender: MemberId,
        receiver: MemberId,
        seed: u8,
    ) -> FederationDkgSecretShareRequest {
        FederationDkgSecretShareRequest::new(
            public_config.federation_id,
            sender,
            receiver,
            BTreeMap::from([(
                Amount::from(1),
                evaluate_secret_key_coefficients(
                    receiver.to_bls_signer_id(),
                    &[
                        BlsSecretKey::from_reduced_bytes(&[seed; 32]),
                        BlsSecretKey::from_reduced_bytes(&[seed + 10; 32]),
                    ],
                )
                .expect("dkg secret share"),
            )]),
        )
    }

    fn signed_dkg_commitment_request(
        public_config: &FederationConfig,
        private_config: &MemberConfigPrivate,
        receiver: MemberId,
        sequence: u64,
        request: FederationDkgCommitmentRequest,
    ) -> FederationSignedRequest<FederationDkgCommitmentRequest> {
        FederationSignedRequest::new_signed(
            public_config.federation_id,
            receiver,
            sequence,
            FederationRestMessageKind::DkgCommitment,
            request,
            private_config,
        )
        .expect("signed DKG commitment request")
    }

    fn signed_dkg_reveal_request(
        public_config: &FederationConfig,
        private_config: &MemberConfigPrivate,
        receiver: MemberId,
        sequence: u64,
        request: FederationDkgRevealRequest,
    ) -> FederationSignedRequest<FederationDkgRevealRequest> {
        FederationSignedRequest::new_signed(
            public_config.federation_id,
            receiver,
            sequence,
            FederationRestMessageKind::DkgReveal,
            request,
            private_config,
        )
        .expect("signed DKG reveal request")
    }

    fn signed_dkg_secret_share_request(
        public_config: &FederationConfig,
        private_config: &MemberConfigPrivate,
        receiver: MemberId,
        sequence: u64,
        request: FederationDkgSecretShareRequest,
    ) -> FederationSignedRequest<FederationDkgSecretShareRequest> {
        FederationSignedRequest::new_signed(
            public_config.federation_id,
            receiver,
            sequence,
            FederationRestMessageKind::DkgSecretShare,
            request,
            private_config,
        )
        .expect("signed DKG secret-share request")
    }

    async fn persist_complete_dkg_round(
        state: &MintState,
        public_config: &FederationConfig,
        first_private: &MemberConfigPrivate,
        second_private: &MemberConfigPrivate,
    ) {
        for (private_config, seed) in [(first_private, 11), (second_private, 21)] {
            let commitment =
                dkg_commitment_request(public_config, private_config.member_id, seed).commitment;
            state
                .persist_federation_dkg_commitment(&commitment, private_config.member_id, 1)
                .await
                .expect("persist DKG commitment");
            let reveal = dkg_reveal_request(public_config, private_config.member_id, seed).reveal;
            state
                .persist_federation_dkg_reveal(&reveal, private_config.member_id, 2)
                .await
                .expect("persist DKG reveal");
        }
    }

    fn dkg_transcript_signature_request(
        public_config: &FederationConfig,
        private_config: &MemberConfigPrivate,
    ) -> FederationDkgTranscriptSignatureRequest {
        let transcript = public_config
            .setup_transcript
            .as_ref()
            .expect("setup transcript");
        let signature = cdk::federation::FederationDkgTranscriptSignature::sign(
            transcript.transcript_hash,
            private_config,
        )
        .expect("transcript signature");
        FederationDkgTranscriptSignatureRequest::new(
            public_config.federation_id,
            private_config.member_id,
            transcript.transcript_hash,
            signature,
        )
    }

    fn signed_dkg_transcript_signature_request(
        public_config: &FederationConfig,
        private_config: &MemberConfigPrivate,
        receiver: MemberId,
        sequence: u64,
        request: FederationDkgTranscriptSignatureRequest,
    ) -> FederationSignedRequest<FederationDkgTranscriptSignatureRequest> {
        FederationSignedRequest::new_signed(
            public_config.federation_id,
            receiver,
            sequence,
            FederationRestMessageKind::DkgTranscriptSignature,
            request,
            private_config,
        )
        .expect("signed DKG transcript signature request")
    }

    async fn consensus_operation_count(state: &MintState) -> u64 {
        state
            .federation_consensus
            .as_ref()
            .expect("consensus")
            .lock()
            .await
            .peer_status()
            .operation_count
    }

    async fn peer_status_with_materialized_digest(
        state: &MintState,
        consensus: &InMemoryFederationConsensus,
    ) -> FederationPeerStatus {
        let mut status = consensus.peer_status();
        status.state_digest = materialized_state_digest(state, consensus.journal().entries())
            .await
            .expect("materialized peer status digest");
        status
    }

    async fn catch_up_response_with_materialized_digest(
        state: &MintState,
        consensus: &InMemoryFederationConsensus,
        request: &FederationJournalCatchUpRequest,
    ) -> FederationJournalCatchUpResponse {
        let mut response = consensus
            .catch_up_checked(request)
            .expect("peer catch-up response");
        response.state_digest = materialized_state_digest(state, consensus.journal().entries())
            .await
            .expect("materialized catch-up state digest");
        response
    }

    fn payment_observation_envelope(federation_id: FederationId) -> FederationOperationEnvelope {
        FederationOperationEnvelope::new(
            federation_id,
            FederationOperation::MintQuotePayment(FederationMintQuotePaymentObservation {
                quote: "quote-paid".to_string(),
                observer: MemberId::new(1).expect("member id"),
                method: PaymentMethod::Known(KnownMethod::Bolt11),
                state: MintQuoteState::Paid,
                amount_paid: Amount::from(21),
                amount_issued: Amount::ZERO,
                payment_proof: Some("fake-wallet-paid".to_string()),
            }),
        )
    }

    fn oversized_payment_observation_envelope(
        federation_id: FederationId,
    ) -> FederationOperationEnvelope {
        let mut envelope = payment_observation_envelope(federation_id);
        let FederationOperation::MintQuotePayment(observation) = &mut envelope.operation else {
            panic!("mint quote payment observation");
        };
        observation.payment_proof =
            Some("x".repeat(cdk::federation::FEDERATION_MAX_OPERATION_ENVELOPE_BYTES));
        envelope
    }

    fn mint_quote_envelope(state: &MintState, quote_id: &QuoteId) -> FederationOperationEnvelope {
        let config = state.mint.federation_config().expect("federation config");
        config.mint_quote_operation(
            quote_id.clone(),
            cdk::mint::MintQuoteRequest::Bolt11(MintQuoteBolt11Request {
                amount: Amount::from(1),
                unit: CurrencyUnit::Sat,
                description: None,
                pubkey: None,
            }),
            cdk::mint::MintQuoteResponse::Bolt11(MintQuoteBolt11Response {
                quote: quote_id.to_string(),
                request: format!("invoice-{quote_id}"),
                amount: Some(Amount::from(1)),
                unit: Some(CurrencyUnit::Sat),
                state: MintQuoteState::Unpaid,
                expiry: Some(10_000),
                pubkey: None,
            }),
        )
    }

    fn paid_observation_envelope(
        state: &MintState,
        quote_id: &QuoteId,
    ) -> FederationOperationEnvelope {
        let config = state.mint.federation_config().expect("federation config");
        config.mint_quote_payment_observation(
            quote_id.clone(),
            PaymentMethod::Known(KnownMethod::Bolt11),
            MintQuoteState::Paid,
            Amount::from(1),
            Amount::ZERO,
            Some(format!("paid-{quote_id}")),
        )
    }

    fn mint_payment_observation_envelope_for_member(
        state: &MintState,
        quote_id: &QuoteId,
        observer: u16,
        amount_paid: u64,
        payment_proof: &str,
    ) -> FederationOperationEnvelope {
        let config = state.mint.federation_config().expect("federation config");
        FederationOperationEnvelope::new(
            config.federation_id(),
            FederationOperation::MintQuotePayment(FederationMintQuotePaymentObservation {
                quote: quote_id.to_string(),
                observer: member_id(observer),
                method: PaymentMethod::Known(KnownMethod::Bolt11),
                state: MintQuoteState::Paid,
                amount_paid: Amount::from(amount_paid),
                amount_issued: Amount::ZERO,
                payment_proof: Some(payment_proof.to_string()),
            }),
        )
    }

    fn mint_output(state: &MintState, quote_id: &QuoteId) -> BlindedMessage {
        let config = state.mint.federation_config().expect("federation config");
        BlindedMessage::new(
            Amount::from(1),
            config.public_config().keysets[0].id,
            PublicKey::from(BlsG1PublicKey::hash_to_curve(
                format!("federation replay output {quote_id}").as_bytes(),
            )),
        )
    }

    fn aggregate_secret_for_amount(amount: u64) -> BlsSecretKey {
        match amount {
            1 => BlsSecretKey::from_reduced_bytes(&[11; 32]),
            _ => panic!("missing aggregate secret fixture for amount {amount}"),
        }
    }

    fn proof_for_amount(state: &MintState, amount: u64, label: &str) -> cdk::nuts::Proof {
        let config = state.mint.federation_config().expect("federation config");
        let keyset = &config.public_config().keysets[0];
        let secret = cdk::secret::Secret::new(format!("federation replay proof {label}"));
        let signature = BlsG1PublicKey::hash_to_curve(secret.as_bytes())
            .mul(&aggregate_secret_for_amount(amount));

        cdk::nuts::Proof::new(
            Amount::from(amount),
            keyset.id,
            secret,
            PublicKey::from(signature),
        )
    }

    fn swap_output(state: &MintState, label: &str) -> BlindedMessage {
        let config = state.mint.federation_config().expect("federation config");
        BlindedMessage::new(
            Amount::from(1),
            config.public_config().keysets[0].id,
            PublicKey::from(BlsG1PublicKey::hash_to_curve(
                format!("federation replay swap output {label}").as_bytes(),
            )),
        )
    }

    async fn melt_quote(state: &MintState, amount_msat: u64) -> QuoteId {
        melt_quote_with_description(state, amount_msat, "".to_string()).await
    }

    async fn melt_quote_with_description(
        state: &MintState,
        amount_msat: u64,
        description: String,
    ) -> QuoteId {
        let response = state
            .mint
            .get_melt_quote(cdk::MeltQuoteRequest::Bolt11(MeltQuoteBolt11Request {
                request: create_fake_invoice(amount_msat, description),
                unit: CurrencyUnit::Sat,
                options: None,
            }))
            .await
            .expect("melt quote");

        match response {
            cdk::MeltQuoteCreateResponse::Bolt11(response) => response.quote,
            _ => panic!("expected bolt11 melt quote"),
        }
    }

    fn melt_payment_observation_envelope(
        state: &MintState,
        quote_id: &QuoteId,
        state_observed: MeltQuoteState,
        payment_proof: Option<String>,
    ) -> FederationOperationEnvelope {
        let config = state.mint.federation_config().expect("federation config");
        config.melt_quote_payment_observation(
            quote_id.clone(),
            PaymentMethod::Known(KnownMethod::Bolt11),
            state_observed,
            Amount::from(1),
            Amount::from(1),
            payment_proof,
            None,
        )
    }

    fn restore_envelope(
        federation_id: FederationId,
        label: &'static str,
    ) -> FederationOperationEnvelope {
        restore_envelope_with_output(
            federation_id,
            PublicKey::from_hex(label).expect("public key"),
        )
    }

    fn indexed_restore_envelope(
        federation_id: FederationId,
        index: u64,
    ) -> FederationOperationEnvelope {
        restore_envelope_with_output(
            federation_id,
            PublicKey::from(BlsG1PublicKey::hash_to_curve(
                format!("federation evidence restore {index}").as_bytes(),
            )),
        )
    }

    fn restore_envelope_with_output(
        federation_id: FederationId,
        blinded_secret: PublicKey,
    ) -> FederationOperationEnvelope {
        FederationOperationEnvelope::new(
            federation_id,
            FederationOperation::Restore(cdk::federation::FederationRestoreOperation {
                request: RestoreRequest {
                    outputs: vec![cdk::nuts::BlindedMessage::new(
                        Amount::from(1),
                        cdk::nuts::Id::from_str("00bfa73302d12ffd").expect("keyset id"),
                        blinded_secret,
                    )],
                },
            }),
        )
    }

    #[tokio::test]
    async fn rest_peer_client_validates_peer_status() {
        let (public_config, local_private, _) = transport_config();
        let federation_id = public_config.federation_id;
        let member = public_config.member(member_id(2)).expect("peer member");
        let status = FederationPeerStatus::new(
            federation_id,
            member.member_id,
            0,
            None,
            cdk::federation::FederationJournalDigest::genesis(),
        );
        let transport = MockFederationTransport::default().with_status_response(status.clone());
        let client =
            FederationRestPeerClient::new(transport.clone(), &public_config, local_private)
                .expect("client");

        let response = client.peer_status(member).await.expect("peer status");

        assert_eq!(response, status);
        let requests = transport.status_requests();
        assert_eq!(requests.len(), 1);
        requests[0]
            .verify(&public_config, member.member_id)
            .expect("signed status request");
        assert_eq!(requests[0].sender, member_id(1));
        assert_eq!(requests[0].kind, FederationRestMessageKind::Status);
    }

    #[tokio::test]
    async fn rest_peer_client_initial_sequence_advances_across_clients() {
        let (public_config, local_private, _) = transport_config();
        let federation_id = public_config.federation_id;
        let member = public_config.member(member_id(2)).expect("peer member");
        let status = FederationPeerStatus::new(
            federation_id,
            member.member_id,
            0,
            None,
            cdk::federation::FederationJournalDigest::genesis(),
        );
        let transport = MockFederationTransport::default().with_status_response(status);
        let first_client =
            FederationRestPeerClient::new(transport.clone(), &public_config, local_private.clone())
                .expect("first client");
        first_client
            .peer_status(member)
            .await
            .expect("first peer status");

        let second_client =
            FederationRestPeerClient::new(transport.clone(), &public_config, local_private)
                .expect("second client");
        second_client
            .peer_status(member)
            .await
            .expect("second peer status");

        let requests = transport.status_requests();
        assert_eq!(requests.len(), 2);
        assert!(requests[0].sequence > 1);
        assert!(
            requests[1].sequence > requests[0].sequence,
            "fresh clients must not replay stale federation REST sequences"
        );
    }

    #[tokio::test]
    async fn rest_peer_client_older_client_sequence_catches_up_after_newer_client() {
        let (public_config, local_private, _) = transport_config();
        let federation_id = public_config.federation_id;
        let member = public_config.member(member_id(2)).expect("peer member");
        let status = FederationPeerStatus::new(
            federation_id,
            member.member_id,
            0,
            None,
            cdk::federation::FederationJournalDigest::genesis(),
        );
        let transport = MockFederationTransport::default().with_status_response(status);
        let older_client =
            FederationRestPeerClient::new(transport.clone(), &public_config, local_private.clone())
                .expect("older client");
        older_client
            .peer_status(member)
            .await
            .expect("older first peer status");

        let newer_client =
            FederationRestPeerClient::new(transport.clone(), &public_config, local_private)
                .expect("newer client");
        newer_client
            .peer_status(member)
            .await
            .expect("newer peer status");
        older_client
            .peer_status(member)
            .await
            .expect("older second peer status");

        let requests = transport.status_requests();
        assert_eq!(requests.len(), 3);
        assert!(
            requests[1].sequence > requests[0].sequence,
            "newer client should advance the process-wide REST sequence"
        );
        assert!(
            requests[2].sequence > requests[1].sequence,
            "older client must reserve a sequence above newer client traffic"
        );
    }

    #[tokio::test]
    async fn rest_peer_client_submits_dkg_setup_messages() {
        let (public_config, local_private, _) = transport_config();
        let member = public_config.member(member_id(2)).expect("peer member");
        let commitment_request =
            dkg_commitment_request(&public_config, local_private.member_id, 11);
        let reveal_request = dkg_reveal_request(&public_config, local_private.member_id, 11);
        let secret_share_request = dkg_secret_share_request(
            &public_config,
            local_private.member_id,
            member.member_id,
            11,
        );
        let transcript_signature_request =
            dkg_transcript_signature_request(&public_config, &local_private);
        let transport = MockFederationTransport::default()
            .with_dkg_setup_response(FederationDkgSetupResponse::new(
                public_config.federation_id,
                member.member_id,
                FederationRestMessageKind::DkgCommitment,
                local_private.member_id,
            ))
            .with_dkg_setup_response(FederationDkgSetupResponse::new(
                public_config.federation_id,
                member.member_id,
                FederationRestMessageKind::DkgReveal,
                local_private.member_id,
            ))
            .with_dkg_setup_response(FederationDkgSetupResponse::new(
                public_config.federation_id,
                member.member_id,
                FederationRestMessageKind::DkgSecretShare,
                local_private.member_id,
            ))
            .with_dkg_setup_response(FederationDkgSetupResponse::new(
                public_config.federation_id,
                member.member_id,
                FederationRestMessageKind::DkgTranscriptSignature,
                local_private.member_id,
            ));
        let client =
            FederationRestPeerClient::new(transport.clone(), &public_config, local_private.clone())
                .expect("client");

        client
            .submit_dkg_commitment(member, &commitment_request)
            .await
            .expect("submit DKG commitment");
        client
            .submit_dkg_reveal(member, &reveal_request)
            .await
            .expect("submit DKG reveal");
        client
            .submit_dkg_secret_share(member, &secret_share_request)
            .await
            .expect("submit DKG secret share");
        client
            .submit_dkg_transcript_signature(member, &transcript_signature_request)
            .await
            .expect("submit DKG transcript signature");

        let commitments = transport.dkg_commitment_requests();
        let reveals = transport.dkg_reveal_requests();
        let secret_shares = transport.dkg_secret_share_requests();
        let transcript_signatures = transport.dkg_transcript_signature_requests();
        assert_eq!(commitments.len(), 1);
        assert_eq!(reveals.len(), 1);
        assert_eq!(secret_shares.len(), 1);
        assert_eq!(transcript_signatures.len(), 1);
        commitments[0]
            .verify(&public_config, member.member_id)
            .expect("commitment signature");
        commitments[0]
            .payload
            .validate_sender(&public_config, commitments[0].sender)
            .expect("commitment payload owner");
        reveals[0]
            .verify(&public_config, member.member_id)
            .expect("reveal signature");
        reveals[0]
            .payload
            .validate_sender(&public_config, reveals[0].sender)
            .expect("reveal payload owner");
        secret_shares[0]
            .verify(&public_config, member.member_id)
            .expect("secret-share request");
        secret_shares[0]
            .payload
            .validate_sender_receiver(&public_config, secret_shares[0].sender, member.member_id)
            .expect("secret-share payload owner");
        transcript_signatures[0]
            .verify(&public_config, member.member_id)
            .expect("transcript signature request");
        transcript_signatures[0]
            .payload
            .validate_sender(&public_config, transcript_signatures[0].sender)
            .expect("transcript signature payload owner");
        assert_eq!(commitments[0].sender, local_private.member_id);
        assert_eq!(reveals[0].sender, local_private.member_id);
        assert_eq!(secret_shares[0].sender, local_private.member_id);
        assert_eq!(transcript_signatures[0].sender, local_private.member_id);
        assert!(reveals[0].sequence > commitments[0].sequence);
        assert!(secret_shares[0].sequence > reveals[0].sequence);
        assert!(transcript_signatures[0].sequence > secret_shares[0].sequence);
    }

    #[tokio::test]
    async fn rest_peer_client_submits_dkg_setup_messages_over_http() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind receiver federation route");
        let address = listener.local_addr().expect("receiver route address");
        let sender_federation_api_url =
            MintUrl::from_str("http://127.0.0.1:9").expect("sender federation api url");
        let receiver_federation_api_url =
            MintUrl::from_str(&format!("http://127.0.0.1:{}", address.port()))
                .expect("receiver federation api url");
        let (public_config, sender_private, receiver_private) =
            transport_config_with_federation_api_urls(
                sender_federation_api_url,
                receiver_federation_api_url,
            );
        let receiver_member = public_config
            .member(receiver_private.member_id)
            .expect("receiver member");
        let commitment_request =
            dkg_commitment_request(&public_config, sender_private.member_id, 11);
        let expected_commitment = commitment_request.commitment;
        let reveal_request = dkg_reveal_request(&public_config, sender_private.member_id, 11);
        let expected_reveal = reveal_request.reveal.clone();
        let secret_share_request = dkg_secret_share_request(
            &public_config,
            sender_private.member_id,
            receiver_private.member_id,
            11,
        );
        let expected_secret_share = secret_share_request.clone();
        let receiver_state =
            federation_route_state_for_private(public_config.clone(), receiver_private).await;
        let router = Router::new()
            .nest("/federation/v1", create_federation_router())
            .with_state(receiver_state.clone());
        let server = tokio::spawn(async move {
            axum::serve(listener, router)
                .await
                .expect("receiver federation route server");
        });
        tokio::task::yield_now().await;

        let client =
            FederationRestPeerClient::new(Async::default(), &public_config, sender_private)
                .expect("client");
        client
            .submit_dkg_commitment(receiver_member, &commitment_request)
            .await
            .expect("submit DKG commitment");
        client
            .submit_dkg_reveal(receiver_member, &reveal_request)
            .await
            .expect("submit DKG reveal");
        client
            .submit_dkg_secret_share(receiver_member, &secret_share_request)
            .await
            .expect("submit DKG secret share");

        let sequences = receiver_state
            .mint
            .localstore()
            .get_federation_member_sequences()
            .await
            .expect("persisted sequences");
        let sender_sequence = sequences
            .get(&member_id(1))
            .copied()
            .expect("persisted sender sequence");
        assert!(sender_sequence >= 3);
        let commitments = receiver_state
            .mint
            .localstore()
            .get_federation_dkg_commitments()
            .await
            .expect("persisted commitments");
        let reveals = receiver_state
            .mint
            .localstore()
            .get_federation_dkg_reveals()
            .await
            .expect("persisted reveals");
        let secret_shares = receiver_state
            .mint
            .localstore()
            .get_federation_dkg_secret_share_contributions()
            .await
            .expect("persisted secret-share contributions");
        assert_eq!(commitments, vec![expected_commitment]);
        assert_eq!(reveals, vec![expected_reveal]);
        assert_eq!(secret_shares, vec![expected_secret_share]);

        server.abort();
    }

    #[tokio::test]
    async fn rest_peer_client_rejects_status_wrong_federation() {
        let (public_config, local_private, _) = transport_config();
        let member = public_config.member(member_id(2)).expect("peer member");
        let status = FederationPeerStatus::new(
            super::FederationId::from_bytes([32; 32]),
            member.member_id,
            0,
            None,
            cdk::federation::FederationJournalDigest::genesis(),
        );
        let transport = MockFederationTransport::default().with_status_response(status);
        let client = FederationRestPeerClient::new(transport, &public_config, local_private)
            .expect("client");

        assert!(client.peer_status(member).await.is_err());
    }

    #[tokio::test]
    async fn rest_peer_client_submits_payment_observation_operations() {
        let (public_config, local_private, _) = transport_config();
        let federation_id = public_config.federation_id;
        let member = public_config.member(member_id(2)).expect("peer member");
        let envelope = payment_observation_envelope(federation_id);
        let request = FederationSubmitOperationRequest::new(envelope.clone());
        let mut peer_consensus = InMemoryFederationConsensus::new(federation_id, member.member_id);
        let submit_response = peer_consensus
            .submit_operation(FederationSubmitOperationRequest::new(envelope))
            .expect("peer submit response");
        let transport =
            MockFederationTransport::default().with_submit_response(submit_response.clone());
        let client =
            FederationRestPeerClient::new(transport.clone(), &public_config, local_private)
                .expect("client");

        let response = client
            .submit_operation(member, &request)
            .await
            .expect("submit operation");

        assert_eq!(response.operation_id, submit_response.operation_id);
        let submitted = transport.submitted_operations();
        assert_eq!(submitted.len(), 1);
        submitted[0]
            .verify(&public_config, member.member_id)
            .expect("signed submit request");
        assert_eq!(
            submitted[0].payload.operation_id().expect("operation id"),
            response.operation_id
        );
        assert_eq!(
            submitted[0].payload.envelope.operation.kind(),
            FederationOperationKind::MintQuotePayment
        );
    }

    #[tokio::test]
    async fn non_primary_submit_forwards_to_primary_and_catches_up() {
        let (public_config, primary_private, local_private) = transport_config();
        let federation_id = public_config.federation_id;
        let state =
            federation_route_state_for_private(public_config.clone(), local_private.clone()).await;
        let primary_member = public_config.member(member_id(1)).expect("primary member");
        let blinded_secret = PublicKey::from(BlsG1PublicKey::hash_to_curve(
            b"non primary forwarded restore",
        ));
        let envelope = restore_envelope_with_output(federation_id, blinded_secret);
        let operation_id = envelope.operation_id().expect("operation id");

        let mut primary_consensus =
            InMemoryFederationConsensus::new(federation_id, primary_member.member_id);
        let submit_response = primary_consensus
            .submit_operation(FederationSubmitOperationRequest::new(envelope.clone()))
            .expect("primary submit response");
        let local_catch_up_request = state
            .federation_consensus
            .as_ref()
            .expect("local consensus")
            .lock()
            .await
            .catch_up_request();
        let mut catch_up_response = primary_consensus
            .catch_up_checked(&local_catch_up_request)
            .expect("primary catch-up response");
        let state_digest =
            FederationMaterializedStateSnapshot::from_rows([FederationMaterializedStateRow::new(
                "blind_signatures",
                blinded_secret.to_hex(),
            )
            .with_field("missing", "true")])
            .digest()
            .expect("state digest");
        catch_up_response.state_digest = state_digest;
        let catch_up_response = signed_catch_up_response(catch_up_response, &primary_private);
        let primary_status = FederationPeerStatus::new_with_state_digest(
            federation_id,
            primary_member.member_id,
            primary_consensus.peer_status().operation_count,
            primary_consensus.peer_status().latest_index,
            primary_consensus.peer_status().latest_order_digest,
            state_digest,
        );
        let transport = MockFederationTransport::default()
            .with_submit_response(submit_response.clone())
            .with_status_response(primary_status)
            .with_catch_up_response(catch_up_response);

        let response = submit_operation_to_primary_if_needed_with_transport(
            &state,
            envelope,
            None,
            transport.clone(),
        )
        .await
        .expect("forward to primary")
        .expect("non-primary should forward");

        assert_eq!(response.operation_id, operation_id);
        assert_eq!(response.operation_id, submit_response.operation_id);
        let submitted = transport.submitted_operations();
        assert_eq!(submitted.len(), 1);
        submitted[0]
            .verify(&public_config, primary_member.member_id)
            .expect("signed primary submit");
        assert_eq!(submitted[0].sender, local_private.member_id);
        assert_eq!(submitted[0].receiver, primary_member.member_id);
        let persisted = state
            .mint
            .localstore()
            .get_federation_journal_entries()
            .await
            .expect("persisted journal");
        assert_eq!(persisted.len(), 1);
        assert_eq!(persisted[0].operation_id, operation_id);
    }

    #[tokio::test]
    async fn rest_peer_client_rejects_oversized_operation_before_broadcast() {
        let (public_config, local_private, _) = transport_config();
        let member = public_config.member(member_id(2)).expect("peer member");
        let request = FederationSubmitOperationRequest::new(
            oversized_payment_observation_envelope(public_config.federation_id),
        );
        let transport = MockFederationTransport::default();
        let client =
            FederationRestPeerClient::new(transport.clone(), &public_config, local_private)
                .expect("client");

        let err = client
            .submit_operation(member, &request)
            .await
            .expect_err("oversized operation should not be broadcast");

        assert!(matches!(
            err.downcast_ref::<FederationError>(),
            Some(FederationError::OperationEnvelopeTooLarge { .. })
        ));
        assert!(transport.submitted_operations().is_empty());
    }

    #[tokio::test]
    async fn rest_peer_client_catches_up_local_consensus_from_member() {
        let (state, public_config, local_private, peer_private) =
            federation_route_state_with_local_private().await;
        let federation_id = public_config.federation_id;
        let peer_member = public_config.member(member_id(2)).expect("peer member");
        let first = restore_envelope(
            federation_id,
            "0204bbffa045f28ec836117a29ea0a00d77f1d692e38cf94f72a5145bfda6d8f41",
        );
        let second = restore_envelope(
            federation_id,
            "025f0615ccba96f810582a6885ffdb04bd57c96dbc590f5aa560447b31258988d7",
        );
        let mut peer_consensus =
            InMemoryFederationConsensus::new(federation_id, peer_member.member_id);
        state
            .submit_federation_envelope(first.clone())
            .await
            .expect("local first operation");
        peer_consensus
            .submit_envelope(first)
            .expect("peer first operation");
        peer_consensus
            .submit_envelope(second)
            .expect("peer second operation");
        let local_consensus = state.federation_consensus.as_ref().expect("consensus");
        let catch_up_response = peer_consensus
            .catch_up_checked(&local_consensus.lock().await.catch_up_request())
            .expect("peer catch-up response");
        let transport = MockFederationTransport::default()
            .with_catch_up_response(signed_catch_up_response(catch_up_response, &peer_private));
        let client =
            FederationRestPeerClient::new(transport.clone(), &public_config, local_private)
                .expect("client");

        let applied = client
            .catch_up_state_from_member(&state, peer_member)
            .await
            .expect("catch up");

        assert_eq!(applied.len(), 1);
        assert_eq!(
            local_consensus
                .lock()
                .await
                .peer_status()
                .latest_order_digest,
            peer_consensus.peer_status().latest_order_digest
        );
        let persisted = state
            .mint
            .localstore()
            .get_federation_journal_entries()
            .await
            .expect("persisted journal");
        assert_eq!(persisted.len(), 2);
        assert_eq!(persisted[1].operation_id, applied[0].operation_id);
        let persisted_outcomes = state
            .mint
            .localstore()
            .get_federation_session_outcomes()
            .await
            .expect("persisted session outcomes");
        assert_eq!(persisted_outcomes.len(), 1);
        assert_eq!(persisted_outcomes[0].from_index, 1);
        assert_eq!(persisted_outcomes[0].signer, peer_member.member_id);
        let requests = transport.catch_up_requests();
        assert_eq!(requests.len(), 1);
        requests[0]
            .verify(&public_config, peer_member.member_id)
            .expect("signed catch-up request");
        assert_eq!(requests[0].payload.next_index, 1);
    }

    #[tokio::test]
    async fn rest_peer_client_tolerates_already_applied_catch_up_overlap() {
        let (state, public_config, local_private, peer_private) =
            federation_route_state_with_local_private().await;
        let federation_id = public_config.federation_id;
        let peer_member = public_config.member(member_id(2)).expect("peer member");
        let first = restore_envelope(
            federation_id,
            "0204bbffa045f28ec836117a29ea0a00d77f1d692e38cf94f72a5145bfda6d8f41",
        );
        let second = restore_envelope(
            federation_id,
            "025f0615ccba96f810582a6885ffdb04bd57c96dbc590f5aa560447b31258988d7",
        );
        let mut peer_consensus =
            InMemoryFederationConsensus::new(federation_id, peer_member.member_id);
        state
            .submit_federation_envelope(first.clone())
            .await
            .expect("local first operation");
        peer_consensus
            .submit_envelope(first)
            .expect("peer first operation");
        peer_consensus
            .submit_envelope(second.clone())
            .expect("peer second operation");
        let local_consensus = state.federation_consensus.as_ref().expect("consensus");
        let catch_up_response = peer_consensus
            .catch_up_checked(&local_consensus.lock().await.catch_up_request())
            .expect("peer catch-up response");

        state
            .submit_federation_envelope(second)
            .await
            .expect("background sync applied second operation first");
        let transport = MockFederationTransport::default()
            .with_catch_up_response(signed_catch_up_response(catch_up_response, &peer_private));
        let client =
            FederationRestPeerClient::new(transport.clone(), &public_config, local_private)
                .expect("client");

        let applied = client
            .catch_up_state_from_member(&state, peer_member)
            .await
            .expect("stale catch-up response is idempotent");

        assert!(applied.is_empty());
        assert_eq!(
            local_consensus.lock().await.peer_status().operation_count,
            2
        );
        let persisted_outcomes = state
            .mint
            .localstore()
            .get_federation_session_outcomes()
            .await
            .expect("persisted session outcomes");
        assert_eq!(persisted_outcomes.len(), 1);
        assert_eq!(persisted_outcomes[0].from_index, 1);
        assert_eq!(persisted_outcomes[0].signer, peer_member.member_id);
        assert_eq!(transport.catch_up_requests()[0].payload.next_index, 2);
    }

    #[tokio::test]
    async fn rest_peer_client_rejects_unsigned_nonempty_catch_up_response() {
        let (state, public_config, local_private, _) =
            federation_route_state_with_local_private().await;
        let federation_id = public_config.federation_id;
        let peer_member = public_config.member(member_id(2)).expect("peer member");
        let first = restore_envelope(
            federation_id,
            "0204bbffa045f28ec836117a29ea0a00d77f1d692e38cf94f72a5145bfda6d8f41",
        );
        let second = restore_envelope(
            federation_id,
            "025f0615ccba96f810582a6885ffdb04bd57c96dbc590f5aa560447b31258988d7",
        );
        let mut peer_consensus =
            InMemoryFederationConsensus::new(federation_id, peer_member.member_id);
        state
            .submit_federation_envelope(first.clone())
            .await
            .expect("local first operation");
        peer_consensus
            .submit_envelope(first)
            .expect("peer first operation");
        peer_consensus
            .submit_envelope(second)
            .expect("peer second operation");
        let local_consensus = state.federation_consensus.as_ref().expect("consensus");
        let catch_up_response = peer_consensus
            .catch_up_checked(&local_consensus.lock().await.catch_up_request())
            .expect("peer catch-up response");
        assert!(catch_up_response.session_outcome.is_none());
        let transport =
            MockFederationTransport::default().with_catch_up_response(catch_up_response);
        let client = FederationRestPeerClient::new(transport, &public_config, local_private)
            .expect("client");

        let err = client
            .catch_up_state_from_member(&state, peer_member)
            .await
            .expect_err("unsigned response must fail");

        assert!(matches!(
            err.downcast_ref::<FederationError>(),
            Some(FederationError::SessionOutcomeMismatch { .. })
        ));
        assert_eq!(
            local_consensus.lock().await.peer_status().operation_count,
            1
        );
    }

    #[tokio::test]
    async fn lagging_member_refuses_public_federation_submit_until_healthy() {
        let (state, public_config, _) = federation_route_state().await;
        let peer_member = public_config.member(member_id(2)).expect("peer member");
        state
            .mark_federation_lagging(peer_member.member_id, 0, 1)
            .await;
        let envelope = restore_envelope(
            public_config.federation_id,
            "0204bbffa045f28ec836117a29ea0a00d77f1d692e38cf94f72a5145bfda6d8f41",
        );

        let blocked = state.submit_federation_envelope(envelope.clone()).await;

        assert!(matches!(blocked, Err(cdk::Error::Custom(_))));
        assert!(matches!(
            state.federation_sync_status().await,
            Some(crate::FederationSyncStatus::Lagging { .. })
        ));

        state.mark_federation_healthy().await;
        state
            .submit_federation_envelope(envelope)
            .await
            .expect("healthy member can submit");
        assert!(matches!(
            state.federation_sync_status().await,
            Some(crate::FederationSyncStatus::Healthy)
        ));
    }

    #[tokio::test]
    async fn rest_peer_client_sync_requires_quorum_evidence_before_public_submit() {
        let (state, public_config, local_private, peer_private) =
            federation_route_state_with_local_private().await;
        let federation_id = public_config.federation_id;
        let peer_member = public_config.member(member_id(2)).expect("peer member");
        let peer_entry = restore_envelope(
            federation_id,
            "0204bbffa045f28ec836117a29ea0a00d77f1d692e38cf94f72a5145bfda6d8f41",
        );
        let mut peer_consensus =
            InMemoryFederationConsensus::new(federation_id, peer_member.member_id);
        peer_consensus
            .submit_envelope(peer_entry)
            .expect("peer accepted operation");
        let local_consensus = state.federation_consensus.as_ref().expect("consensus");
        let catch_up_request = local_consensus.lock().await.catch_up_request();
        let catch_up_response =
            catch_up_response_with_materialized_digest(&state, &peer_consensus, &catch_up_request)
                .await;
        let peer_status = peer_status_with_materialized_digest(&state, &peer_consensus).await;
        let transport = MockFederationTransport::default()
            .with_status_response(peer_status)
            .with_catch_up_response(signed_catch_up_response(catch_up_response, &peer_private));
        let client = FederationRestPeerClient::new(transport, &public_config, local_private)
            .expect("client");

        let synced_status = client
            .sync_once_from_member(&state, peer_member)
            .await
            .expect("peer sync");

        assert_eq!(synced_status.operation_count, 1);
        assert!(matches!(
            state.federation_sync_status().await,
            Some(crate::FederationSyncStatus::VerifyingSessionOutcome {
                operation_count: 1,
                ..
            })
        ));

        let next = restore_envelope(
            federation_id,
            "025f0615ccba96f810582a6885ffdb04bd57c96dbc590f5aa560447b31258988d7",
        );
        let blocked = state.submit_federation_envelope(next).await;
        assert!(matches!(blocked, Err(cdk::Error::Custom(_))));
    }

    #[tokio::test]
    async fn peer_sync_round_catches_up_from_configured_peer() {
        let (state, public_config, local_private, peer_private) =
            federation_route_state_with_local_private().await;
        let federation_id = public_config.federation_id;
        let peer_member = public_config.member(member_id(2)).expect("peer member");
        let peer_entry = restore_envelope(
            federation_id,
            "0204bbffa045f28ec836117a29ea0a00d77f1d692e38cf94f72a5145bfda6d8f41",
        );
        let mut peer_consensus =
            InMemoryFederationConsensus::new(federation_id, peer_member.member_id);
        peer_consensus
            .submit_envelope(peer_entry)
            .expect("peer accepted operation");
        let local_consensus = state.federation_consensus.as_ref().expect("consensus");
        let catch_up_request = local_consensus.lock().await.catch_up_request();
        let catch_up_response =
            catch_up_response_with_materialized_digest(&state, &peer_consensus, &catch_up_request)
                .await;
        let peer_status = peer_status_with_materialized_digest(&state, &peer_consensus).await;
        let transport = MockFederationTransport::default()
            .with_status_response(peer_status)
            .with_catch_up_response(signed_catch_up_response(catch_up_response, &peer_private));
        let client = FederationRestPeerClient::new(transport, &public_config, local_private)
            .expect("client");

        let synced =
            run_federation_peer_sync_round(&state, &client, std::slice::from_ref(peer_member))
                .await;

        assert_eq!(synced, 1);
        assert_eq!(
            local_consensus.lock().await.peer_status().operation_count,
            1
        );
        assert!(matches!(
            state.federation_sync_status().await,
            Some(crate::FederationSyncStatus::Healthy)
        ));

        let next = restore_envelope(
            federation_id,
            "025f0615ccba96f810582a6885ffdb04bd57c96dbc590f5aa560447b31258988d7",
        );
        state
            .submit_federation_envelope(next)
            .await
            .expect("quorum-verified member can submit");
    }

    #[tokio::test]
    async fn peer_sync_round_collects_session_outcome_evidence_from_equal_peer() {
        let (state, public_config, local_private, peer_private) =
            federation_route_state_with_local_private().await;
        let federation_id = public_config.federation_id;
        let peer_member = public_config.member(member_id(2)).expect("peer member");
        let entry = restore_envelope(
            federation_id,
            "0204bbffa045f28ec836117a29ea0a00d77f1d692e38cf94f72a5145bfda6d8f41",
        );
        state
            .submit_federation_envelope(entry.clone())
            .await
            .expect("local accepted operation");
        let mut peer_consensus =
            InMemoryFederationConsensus::new(federation_id, peer_member.member_id);
        peer_consensus
            .submit_envelope(entry)
            .expect("peer accepted operation");

        assert!(session_outcome_quorum_status(&state)
            .await
            .expect("quorum status")
            .is_none());

        let catch_up_request = FederationJournalCatchUpRequest::new(0)
            .with_limit(1)
            .with_known_order_digest(FederationJournalDigest::genesis());
        let catch_up_response =
            catch_up_response_with_materialized_digest(&state, &peer_consensus, &catch_up_request)
                .await;
        let peer_status = peer_status_with_materialized_digest(&state, &peer_consensus).await;
        let transport = MockFederationTransport::default()
            .with_status_response(peer_status)
            .with_catch_up_response(signed_catch_up_response(catch_up_response, &peer_private));
        let client =
            FederationRestPeerClient::new(transport.clone(), &public_config, local_private.clone())
                .expect("client");

        let synced =
            run_federation_peer_sync_round(&state, &client, std::slice::from_ref(peer_member))
                .await;

        assert_eq!(synced, 1);
        assert_eq!(transport.catch_up_requests().len(), 1);
        let quorum = session_outcome_quorum_status(&state)
            .await
            .expect("quorum status")
            .expect("quorum evidence");
        assert_eq!(quorum.operation_count, 1);
        assert_eq!(quorum.signers.len(), 2);
        assert!(quorum.signers.contains(&local_private.member_id));
        assert!(quorum.signers.contains(&peer_private.member_id));
    }

    #[tokio::test]
    async fn peer_sync_round_collects_chunked_session_outcome_evidence() {
        let (state, public_config, local_private, peer_private) =
            federation_route_state_with_local_private().await;
        let federation_id = public_config.federation_id;
        let peer_member = public_config.member(member_id(2)).expect("peer member");
        let mut peer_consensus =
            InMemoryFederationConsensus::new(federation_id, peer_member.member_id);
        for index in 0..=FEDERATION_SESSION_OUTCOME_EVIDENCE_CHUNK_LIMIT as u64 {
            let entry = indexed_restore_envelope(federation_id, index);
            state
                .submit_federation_envelope(entry.clone())
                .await
                .expect("local accepted operation");
            peer_consensus
                .submit_envelope(entry)
                .expect("peer accepted operation");
        }
        state
            .mark_federation_verifying_session_outcome(peer_member.member_id, 1001)
            .await;

        let entries = state
            .mint
            .localstore()
            .get_federation_journal_entries()
            .await
            .expect("journal entries");
        assert_eq!(
            entries.len(),
            FEDERATION_SESSION_OUTCOME_EVIDENCE_CHUNK_LIMIT + 1
        );
        let first_request = FederationJournalCatchUpRequest::new(0)
            .with_limit(FEDERATION_SESSION_OUTCOME_EVIDENCE_CHUNK_LIMIT as u16)
            .with_known_order_digest(FederationJournalDigest::genesis());
        let second_request = FederationJournalCatchUpRequest::new(
            FEDERATION_SESSION_OUTCOME_EVIDENCE_CHUNK_LIMIT as u64,
        )
        .with_limit(1)
        .with_known_order_digest(
            entries[FEDERATION_SESSION_OUTCOME_EVIDENCE_CHUNK_LIMIT].previous_order_digest,
        );
        let first_response =
            catch_up_response_with_materialized_digest(&state, &peer_consensus, &first_request)
                .await;
        let second_response =
            catch_up_response_with_materialized_digest(&state, &peer_consensus, &second_request)
                .await;
        let peer_status = peer_status_with_materialized_digest(&state, &peer_consensus).await;
        let transport = MockFederationTransport::default()
            .with_status_response(peer_status)
            .with_catch_up_response(signed_catch_up_response(first_response, &peer_private))
            .with_catch_up_response(signed_catch_up_response(second_response, &peer_private));
        let client =
            FederationRestPeerClient::new(transport.clone(), &public_config, local_private.clone())
                .expect("client");

        let synced =
            run_federation_peer_sync_round(&state, &client, std::slice::from_ref(peer_member))
                .await;

        assert_eq!(synced, 1);
        let requests = transport.catch_up_requests();
        assert_eq!(requests.len(), 2);
        assert_eq!(requests[0].payload.next_index, 0);
        assert_eq!(
            requests[0].payload.limit,
            Some(FEDERATION_SESSION_OUTCOME_EVIDENCE_CHUNK_LIMIT as u16)
        );
        assert_eq!(
            requests[1].payload.next_index,
            FEDERATION_SESSION_OUTCOME_EVIDENCE_CHUNK_LIMIT as u64
        );
        assert_eq!(requests[1].payload.limit, Some(1));
        let quorum = session_outcome_quorum_status(&state)
            .await
            .expect("quorum status")
            .expect("chunked quorum evidence");
        assert_eq!(quorum.from_index, 0);
        assert_eq!(quorum.operation_count, 1001);
        assert_eq!(quorum.latest_index, 1000);
        assert!(quorum.signers.contains(&local_private.member_id));
        assert!(quorum.signers.contains(&peer_private.member_id));
        assert!(matches!(
            state.federation_sync_status().await,
            Some(crate::FederationSyncStatus::Healthy)
        ));
    }

    #[tokio::test]
    async fn peer_sync_round_does_not_clear_higher_known_lag_with_lower_peer() {
        let (state, public_config, local_private, _) =
            federation_route_state_with_local_private().await;
        let federation_id = public_config.federation_id;
        let peer_member = public_config.member(member_id(2)).expect("peer member");
        state
            .mark_federation_lagging(peer_member.member_id, 0, 2)
            .await;
        let status = FederationPeerStatus::new_with_state_digest(
            federation_id,
            peer_member.member_id,
            0,
            None,
            cdk::federation::FederationJournalDigest::genesis(),
            cdk::federation::FederationStateDigest::genesis(),
        );
        let transport = MockFederationTransport::default().with_status_response(status);
        let client = FederationRestPeerClient::new(transport, &public_config, local_private)
            .expect("client");

        let synced =
            run_federation_peer_sync_round(&state, &client, std::slice::from_ref(peer_member))
                .await;

        assert_eq!(synced, 1);
        assert!(matches!(
            state.federation_sync_status().await,
            Some(crate::FederationSyncStatus::Lagging {
                peer_operation_count: 2,
                ..
            })
        ));
    }

    #[tokio::test]
    async fn rest_peer_client_halts_on_equal_height_digest_mismatch() {
        let (state, public_config, local_private, _) =
            federation_route_state_with_local_private().await;
        let federation_id = public_config.federation_id;
        let peer_member = public_config.member(member_id(2)).expect("peer member");
        let peer_entry = restore_envelope(
            federation_id,
            "0204bbffa045f28ec836117a29ea0a00d77f1d692e38cf94f72a5145bfda6d8f41",
        );
        let local_entry = restore_envelope(
            federation_id,
            "025f0615ccba96f810582a6885ffdb04bd57c96dbc590f5aa560447b31258988d7",
        );
        let mut peer_consensus =
            InMemoryFederationConsensus::new(federation_id, peer_member.member_id);
        peer_consensus
            .submit_envelope(peer_entry)
            .expect("peer accepted operation");
        state
            .submit_federation_envelope(local_entry)
            .await
            .expect("local accepted operation");
        let transport =
            MockFederationTransport::default().with_status_response(peer_consensus.peer_status());
        let client = FederationRestPeerClient::new(transport, &public_config, local_private)
            .expect("client");

        let sync = client.sync_once_from_member(&state, peer_member).await;

        assert!(sync.is_err());
        assert!(matches!(
            state.federation_sync_status().await,
            Some(crate::FederationSyncStatus::Halted { .. })
        ));

        let blocked = state
            .submit_federation_envelope(restore_envelope(
                federation_id,
                "03b45d181a961a91a284e8c64eb72839d43300afaa4e6b3076f5183bb14f50c2cc",
            ))
            .await;
        assert!(matches!(blocked, Err(cdk::Error::Custom(_))));
    }

    #[tokio::test]
    async fn rest_peer_client_marks_equal_height_state_digest_mismatch_not_ready() {
        let (state, public_config, local_private, _) =
            federation_route_state_with_local_private().await;
        let federation_id = public_config.federation_id;
        let peer_member = public_config.member(member_id(2)).expect("peer member");
        let status = FederationPeerStatus::new_with_state_digest(
            federation_id,
            peer_member.member_id,
            0,
            None,
            cdk::federation::FederationJournalDigest::genesis(),
            cdk::federation::FederationStateDigest::from_bytes([9; 32]),
        );
        let transport = MockFederationTransport::default().with_status_response(status);
        let client = FederationRestPeerClient::new(transport, &public_config, local_private)
            .expect("client");

        let sync = client.sync_once_from_member(&state, peer_member).await;

        assert!(sync.is_err());
        assert!(matches!(
            state.federation_sync_status().await,
            Some(crate::FederationSyncStatus::MaterializedStateMismatch { .. })
        ));

        let blocked = state
            .submit_federation_envelope(restore_envelope(
                federation_id,
                "03b45d181a961a91a284e8c64eb72839d43300afaa4e6b3076f5183bb14f50c2cc",
            ))
            .await;
        assert!(matches!(blocked, Err(cdk::Error::Custom(_))));
    }

    #[tokio::test]
    async fn rest_peer_client_recovers_from_transient_equal_height_state_digest_mismatch() {
        let (state, public_config, local_private, _) =
            federation_route_state_with_local_private().await;
        let federation_id = public_config.federation_id;
        let peer_member = public_config.member(member_id(2)).expect("peer member");
        let mismatched_status = FederationPeerStatus::new_with_state_digest(
            federation_id,
            peer_member.member_id,
            0,
            None,
            cdk::federation::FederationJournalDigest::genesis(),
            cdk::federation::FederationStateDigest::from_bytes([9; 32]),
        );
        let transport = MockFederationTransport::default().with_status_response(mismatched_status);
        let client =
            FederationRestPeerClient::new(transport.clone(), &public_config, local_private)
                .expect("client");

        let sync = client.sync_once_from_member(&state, peer_member).await;

        assert!(sync.is_err());
        assert!(matches!(
            state.federation_sync_status().await,
            Some(crate::FederationSyncStatus::MaterializedStateMismatch { .. })
        ));

        let local_status = local_peer_status(&state).await.expect("local status");
        let converged_status = FederationPeerStatus::new_with_state_digest(
            federation_id,
            peer_member.member_id,
            local_status.operation_count,
            local_status.latest_index,
            local_status.latest_order_digest,
            local_status.state_digest,
        );
        transport.with_status_response(converged_status);

        client
            .sync_once_from_member(&state, peer_member)
            .await
            .expect("second sync recovers");

        assert!(matches!(
            state.federation_sync_status().await,
            Some(crate::FederationSyncStatus::Healthy)
        ));
    }

    #[tokio::test]
    async fn catch_up_applies_mint_quote_operation_to_local_store() {
        let (state, public_config, _) = federation_route_state().await;
        let peer_member = public_config.member(member_id(2)).expect("peer member");
        let quote_id = QuoteId::new();
        let envelope = mint_quote_envelope(&state, &quote_id);
        let operation_id = envelope.operation_id().expect("operation id");
        let mut peer_consensus =
            InMemoryFederationConsensus::new(public_config.federation_id, peer_member.member_id);
        peer_consensus
            .submit_envelope(envelope)
            .expect("peer accepted quote");
        let local_consensus = state.federation_consensus.as_ref().expect("consensus");
        let catch_up_response = peer_consensus
            .catch_up_checked(&local_consensus.lock().await.catch_up_request())
            .expect("catch-up response");

        state
            .apply_federation_catch_up_response(&catch_up_response)
            .await
            .expect("apply catch-up");

        let quote = state
            .mint
            .localstore()
            .get_mint_quote(&quote_id)
            .await
            .expect("quote lookup")
            .expect("quote persisted");
        assert_eq!(quote.request, format!("invoice-{quote_id}"));

        let persisted = state
            .mint
            .localstore()
            .get_federation_journal_entries()
            .await
            .expect("journal lookup");
        assert_eq!(persisted.len(), 1);
        assert_eq!(persisted[0].status, FederationOperationStatus::Applied);
        assert_eq!(
            local_consensus
                .lock()
                .await
                .journal()
                .entry_by_operation_id(operation_id)
                .expect("runtime entry")
                .status,
            FederationOperationStatus::Applied
        );
    }

    #[tokio::test]
    async fn catch_up_applies_payment_quarantine_and_local_status_reports_it() {
        let (state, public_config, peer_private) = federation_route_state().await;
        let config = state
            .mint
            .federation_config()
            .expect("federation config")
            .clone();
        let local_member = config.member_id();
        let peer_member = public_config.member(member_id(2)).expect("peer member");
        let quote_id = QuoteId::new();
        let first_observation =
            mint_payment_observation_envelope_for_member(&state, &quote_id, 1, 1, "settled");
        let first_operation_id = first_observation
            .operation_id()
            .expect("first operation id");
        let conflicting_observation = mint_payment_observation_envelope_for_member(
            &state,
            &quote_id,
            2,
            2,
            "different-settlement",
        );
        let conflicting_operation_id = conflicting_observation
            .operation_id()
            .expect("conflicting operation id");
        let quarantine = config.payment_observation_quarantine(
            cdk::federation::FederationPaymentObservationContext::MintQuote,
            quote_id.clone(),
            FederationPaymentObservationQuarantineReason::ConflictingEvidence,
            vec![first_operation_id, conflicting_operation_id],
        );
        let quarantine_operation_id = quarantine.operation_id().expect("quarantine operation id");
        let mut peer_consensus =
            InMemoryFederationConsensus::new(public_config.federation_id, peer_member.member_id);
        for envelope in [
            mint_quote_envelope(&state, &quote_id),
            first_observation,
            conflicting_observation,
            quarantine,
        ] {
            peer_consensus
                .submit_envelope(envelope)
                .expect("peer accepted operation");
        }
        let local_consensus = state.federation_consensus.as_ref().expect("consensus");
        let catch_up_response = peer_consensus
            .catch_up_checked(&local_consensus.lock().await.catch_up_request())
            .expect("catch-up response");

        let applied = state
            .apply_federation_catch_up_response(&catch_up_response)
            .await
            .expect("apply catch-up");

        assert_eq!(applied.len(), 4);
        let persisted = state
            .mint
            .localstore()
            .get_federation_journal_entries()
            .await
            .expect("journal lookup");
        assert_eq!(persisted.len(), 4);
        assert!(persisted
            .iter()
            .all(|entry| entry.status == FederationOperationStatus::Applied));

        let request = signed_status_request(&public_config, &peer_private, local_member, 1);
        let response = post_local_status(State(state), Json(request))
            .await
            .expect("local status response")
            .0;

        assert_eq!(response.payment_observation_reports.len(), 1);
        let report = &response.payment_observation_reports[0];
        assert_eq!(
            report.context,
            cdk::federation::FederationPaymentObservationContext::MintQuote
        );
        assert_eq!(report.quote, quote_id.to_string());
        assert_eq!(report.total_observations, 2);
        assert_eq!(
            report.status,
            cdk::federation::FederationPaymentObservationReportStatus::Conflicting
        );
        assert!(report.quarantined);
        assert_eq!(
            report.recommended_action,
            FederationPaymentObservationReportAction::Quarantine
        );
        assert_eq!(
            report.quarantine_operation_ids,
            vec![quarantine_operation_id]
        );
        assert_eq!(report.quarantine_operation_indices, vec![3]);
        assert_eq!(
            report.quarantine_reasons,
            vec![FederationPaymentObservationQuarantineReason::ConflictingEvidence]
        );

        let status_json = serde_json::to_string(&response).expect("status json");
        assert!(!status_json.contains("settled"));
        assert!(!status_json.contains("different-settlement"));
    }

    #[tokio::test]
    async fn catch_up_applies_melt_quote_operation_to_local_store() {
        let (state, public_config, _) = federation_route_state().await;
        let config = state
            .mint
            .federation_config()
            .expect("federation config")
            .clone();
        let peer_member = public_config.member(member_id(2)).expect("peer member");
        let quote_id = QuoteId::new();
        let invoice = create_fake_invoice(1000, "".to_string());
        let request = cdk::MeltQuoteRequest::Bolt11(MeltQuoteBolt11Request {
            request: invoice.clone(),
            unit: CurrencyUnit::Sat,
            options: None,
        });
        let envelope = config.melt_quote_operation(quote_id.clone(), request);
        let operation_id = envelope.operation_id().expect("operation id");
        let mut peer_consensus =
            InMemoryFederationConsensus::new(public_config.federation_id, peer_member.member_id);
        peer_consensus
            .submit_envelope(envelope)
            .expect("peer accepted melt quote");
        let local_consensus = state.federation_consensus.as_ref().expect("consensus");
        let catch_up_response = peer_consensus
            .catch_up_checked(&local_consensus.lock().await.catch_up_request())
            .expect("catch-up response");

        state
            .apply_federation_catch_up_response(&catch_up_response)
            .await
            .expect("apply catch-up");

        let quote = state
            .mint
            .localstore()
            .get_melt_quote(&quote_id)
            .await
            .expect("quote lookup")
            .expect("quote persisted");
        assert_eq!(quote.id, quote_id);
        assert_eq!(
            quote.payment_method,
            PaymentMethod::Known(KnownMethod::Bolt11)
        );
        assert_eq!(quote.request.to_string(), invoice.to_string());
        assert_eq!(quote.state, MeltQuoteState::Unpaid);
        assert_eq!(quote.amount(), Amount::new(1, CurrencyUnit::Sat));
        assert_eq!(quote.fee_reserve(), Amount::new(1, CurrencyUnit::Sat));

        let persisted = state
            .mint
            .localstore()
            .get_federation_journal_entries()
            .await
            .expect("journal lookup");
        assert_eq!(persisted.len(), 1);
        assert_eq!(persisted[0].status, FederationOperationStatus::Applied);
        assert_eq!(
            local_consensus
                .lock()
                .await
                .journal()
                .entry_by_operation_id(operation_id)
                .expect("runtime entry")
                .status,
            FederationOperationStatus::Applied
        );
    }

    #[tokio::test]
    async fn catch_up_applies_swap_operation_to_local_store() {
        let (state, public_config, _) = federation_route_state().await;
        let config = state
            .mint
            .federation_config()
            .expect("federation config")
            .clone();
        let peer_member = public_config.member(member_id(2)).expect("peer member");
        let input = proof_for_amount(&state, 1, "catch-up");
        let input_y = input.y().expect("input y");
        let output = swap_output(&state, "catch-up");
        let request = cdk::nuts::SwapRequest::new(vec![input], vec![output.clone()]);
        let envelope = config
            .swap_operation(request.clone())
            .expect("swap envelope");
        let operation_id = envelope.operation_id().expect("operation id");
        let mut peer_consensus =
            InMemoryFederationConsensus::new(public_config.federation_id, peer_member.member_id);
        peer_consensus
            .submit_envelope(envelope)
            .expect("peer accepted swap");
        let local_consensus = state.federation_consensus.as_ref().expect("consensus");
        let catch_up_response = peer_consensus
            .catch_up_checked(&local_consensus.lock().await.catch_up_request())
            .expect("catch-up response");

        state
            .apply_federation_catch_up_response(&catch_up_response)
            .await
            .expect("apply catch-up");

        let proof_states = state
            .mint
            .localstore()
            .get_proofs_states(&[input_y])
            .await
            .expect("proof state lookup");
        assert_eq!(proof_states, vec![Some(cdk::nuts::State::Spent)]);

        let signatures = state
            .mint
            .localstore()
            .get_blind_signatures(&[output.blinded_secret])
            .await
            .expect("signature lookup");
        let expected_response = config
            .swap_share_response(operation_id, &request)
            .expect("expected share response");
        assert_eq!(
            signatures,
            vec![Some(expected_response.signatures[0].clone())]
        );

        let persisted = state
            .mint
            .localstore()
            .get_federation_journal_entries()
            .await
            .expect("journal lookup");
        assert_eq!(persisted.len(), 1);
        assert_eq!(persisted[0].status, FederationOperationStatus::Applied);
        assert_eq!(
            local_consensus
                .lock()
                .await
                .journal()
                .entry_by_operation_id(operation_id)
                .expect("runtime entry")
                .status,
            FederationOperationStatus::Applied
        );
    }

    #[tokio::test]
    async fn catch_up_applies_melt_operation_to_local_store() {
        let (state, public_config, _) = federation_route_state().await;
        let config = state
            .mint
            .federation_config()
            .expect("federation config")
            .clone();
        let peer_member = public_config.member(member_id(2)).expect("peer member");
        let quote_id = melt_quote(&state, 1000).await;
        let inputs = vec![
            proof_for_amount(&state, 1, "melt-catch-up-a"),
            proof_for_amount(&state, 1, "melt-catch-up-b"),
            proof_for_amount(&state, 1, "melt-catch-up-c"),
        ];
        let input_ys = inputs
            .iter()
            .map(|proof| proof.y().expect("input y"))
            .collect::<Vec<_>>();
        let mut change_output = swap_output(&state, "melt-catch-up-change");
        change_output.amount = Amount::ZERO;
        let request = cdk::nuts::MeltRequest::new(
            quote_id.clone(),
            inputs,
            Some(vec![change_output.clone()]),
        );
        let envelope = config.melt_operation(request).expect("melt envelope");
        let operation_id = envelope.operation_id().expect("operation id");
        let mut peer_consensus =
            InMemoryFederationConsensus::new(public_config.federation_id, peer_member.member_id);
        peer_consensus
            .submit_envelope(envelope)
            .expect("peer accepted melt");
        let local_consensus = state.federation_consensus.as_ref().expect("consensus");
        let catch_up_response = peer_consensus
            .catch_up_checked(&local_consensus.lock().await.catch_up_request())
            .expect("catch-up response");

        state
            .apply_federation_catch_up_response(&catch_up_response)
            .await
            .expect("apply catch-up");

        let quote = state
            .mint
            .localstore()
            .get_melt_quote(&quote_id)
            .await
            .expect("quote lookup")
            .expect("quote persisted");
        assert_eq!(quote.state, MeltQuoteState::Paid);

        let proof_states = state
            .mint
            .localstore()
            .get_proofs_states(&input_ys)
            .await
            .expect("proof state lookup");
        assert_eq!(
            proof_states,
            vec![
                Some(cdk::nuts::State::Spent),
                Some(cdk::nuts::State::Spent),
                Some(cdk::nuts::State::Spent),
            ]
        );

        let signatures = state
            .mint
            .localstore()
            .get_blind_signatures(&[change_output.blinded_secret])
            .await
            .expect("signature lookup");
        let signature = signatures[0].clone().expect("change share persisted");
        assert_eq!(signature.amount, Amount::from(1));
        assert_eq!(signature.keyset_id, change_output.keyset_id);

        let persisted = state
            .mint
            .localstore()
            .get_federation_journal_entries()
            .await
            .expect("journal lookup");
        assert_eq!(persisted.len(), 1);
        assert_eq!(persisted[0].status, FederationOperationStatus::Applied);
        assert_eq!(
            local_consensus
                .lock()
                .await
                .journal()
                .entry_by_operation_id(operation_id)
                .expect("runtime entry")
                .status,
            FederationOperationStatus::Applied
        );
    }

    #[tokio::test]
    async fn catch_up_applies_paid_melt_payment_observation_to_pending_melt() {
        let (state, public_config, _) = federation_route_state().await;
        let config = state
            .mint
            .federation_config()
            .expect("federation config")
            .clone();
        let peer_member = public_config.member(member_id(2)).expect("peer member");
        let fake_description = FakeInvoiceDescription {
            pay_invoice_state: MeltQuoteState::Pending,
            check_payment_state: MeltQuoteState::Pending,
            pay_err: false,
            check_err: false,
        };
        let quote_id = melt_quote_with_description(
            &state,
            1000,
            serde_json::to_string(&fake_description).expect("fake invoice description"),
        )
        .await;
        let inputs = vec![
            proof_for_amount(&state, 1, "paid-observation-a"),
            proof_for_amount(&state, 1, "paid-observation-b"),
            proof_for_amount(&state, 1, "paid-observation-c"),
        ];
        let input_ys = inputs
            .iter()
            .map(|proof| proof.y().expect("input y"))
            .collect::<Vec<_>>();
        let mut change_output = swap_output(&state, "paid-observation-change");
        change_output.amount = Amount::ZERO;
        let melt_request = cdk::nuts::MeltRequest::new(
            quote_id.clone(),
            inputs,
            Some(vec![change_output.clone()]),
        );
        let melt_envelope = config.melt_operation(melt_request).expect("melt envelope");
        let melt_operation_id = melt_envelope.operation_id().expect("melt operation id");
        let paid_envelope = melt_payment_observation_envelope(
            &state,
            &quote_id,
            MeltQuoteState::Paid,
            Some("federation-paid-preimage".to_string()),
        );
        let paid_operation_id = paid_envelope.operation_id().expect("paid operation id");
        let mut peer_consensus =
            InMemoryFederationConsensus::new(public_config.federation_id, peer_member.member_id);
        peer_consensus
            .submit_envelope(melt_envelope)
            .expect("peer accepted pending melt");
        peer_consensus
            .submit_envelope(paid_envelope)
            .expect("peer accepted paid observation");
        let local_consensus = state.federation_consensus.as_ref().expect("consensus");
        let catch_up_response = peer_consensus
            .catch_up_checked(&local_consensus.lock().await.catch_up_request())
            .expect("catch-up response");

        state
            .apply_federation_catch_up_response(&catch_up_response)
            .await
            .expect("apply catch-up");

        let quote = state
            .mint
            .localstore()
            .get_melt_quote(&quote_id)
            .await
            .expect("quote lookup")
            .expect("quote persisted");
        assert_eq!(quote.state, MeltQuoteState::Paid);
        assert_eq!(
            quote.payment_proof.as_deref(),
            Some("federation-paid-preimage")
        );

        let proof_states = state
            .mint
            .localstore()
            .get_proofs_states(&input_ys)
            .await
            .expect("proof state lookup");
        assert_eq!(
            proof_states,
            vec![
                Some(cdk::nuts::State::Spent),
                Some(cdk::nuts::State::Spent),
                Some(cdk::nuts::State::Spent),
            ]
        );

        let signatures = state
            .mint
            .localstore()
            .get_blind_signatures(&[change_output.blinded_secret])
            .await
            .expect("signature lookup");
        let signature = signatures[0].clone().expect("change share persisted");
        assert_eq!(signature.amount, Amount::from(1));
        assert_eq!(signature.keyset_id, change_output.keyset_id);

        let persisted = state
            .mint
            .localstore()
            .get_federation_journal_entries()
            .await
            .expect("journal lookup");
        assert_eq!(persisted.len(), 2);
        assert!(persisted
            .iter()
            .all(|entry| entry.status == FederationOperationStatus::Applied));
        let runtime = local_consensus.lock().await;
        assert_eq!(
            runtime
                .journal()
                .entry_by_operation_id(melt_operation_id)
                .expect("melt runtime entry")
                .status,
            FederationOperationStatus::Applied
        );
        assert_eq!(
            runtime
                .journal()
                .entry_by_operation_id(paid_operation_id)
                .expect("paid runtime entry")
                .status,
            FederationOperationStatus::Applied
        );
    }

    #[tokio::test]
    async fn catch_up_resolves_pending_melt_after_later_paid_observation() {
        let (state, public_config, _) = federation_route_state().await;
        let config = state
            .mint
            .federation_config()
            .expect("federation config")
            .clone();
        let peer_member = public_config.member(member_id(2)).expect("peer member");
        let fake_description = FakeInvoiceDescription {
            pay_invoice_state: MeltQuoteState::Pending,
            check_payment_state: MeltQuoteState::Pending,
            pay_err: false,
            check_err: false,
        };
        let quote_id = melt_quote_with_description(
            &state,
            1000,
            serde_json::to_string(&fake_description).expect("fake invoice description"),
        )
        .await;
        let inputs = vec![
            proof_for_amount(&state, 1, "late-paid-observation-a"),
            proof_for_amount(&state, 1, "late-paid-observation-b"),
            proof_for_amount(&state, 1, "late-paid-observation-c"),
        ];
        let input_ys = inputs
            .iter()
            .map(|proof| proof.y().expect("input y"))
            .collect::<Vec<_>>();
        let mut change_output = swap_output(&state, "late-paid-observation-change");
        change_output.amount = Amount::ZERO;
        let melt_request = cdk::nuts::MeltRequest::new(
            quote_id.clone(),
            inputs,
            Some(vec![change_output.clone()]),
        );
        let melt_envelope = config.melt_operation(melt_request).expect("melt envelope");
        let paid_envelope = melt_payment_observation_envelope(
            &state,
            &quote_id,
            MeltQuoteState::Paid,
            Some("late-paid-observation-preimage".to_string()),
        );
        let mut peer_consensus =
            InMemoryFederationConsensus::new(public_config.federation_id, peer_member.member_id);
        peer_consensus
            .submit_envelope(melt_envelope)
            .expect("peer accepted pending melt");

        let local_consensus = state.federation_consensus.as_ref().expect("consensus");
        let pending_response = peer_consensus
            .catch_up_checked(&local_consensus.lock().await.catch_up_request())
            .expect("pending catch-up response");
        state
            .apply_federation_catch_up_response(&pending_response)
            .await
            .expect("apply pending catch-up");

        let pending_quote = state
            .mint
            .check_melt_quote(&quote_id)
            .await
            .expect("pending quote status");
        assert_eq!(pending_quote.state(), MeltQuoteState::Pending);
        assert!(pending_quote.change().is_none());
        let pending_states = state
            .mint
            .localstore()
            .get_proofs_states(&input_ys)
            .await
            .expect("pending proof states");
        assert_eq!(
            pending_states,
            vec![
                Some(cdk::nuts::State::Pending),
                Some(cdk::nuts::State::Pending),
                Some(cdk::nuts::State::Pending),
            ]
        );
        assert!(state
            .mint
            .localstore()
            .get_blind_signatures(&[change_output.blinded_secret])
            .await
            .expect("pending change signature lookup")[0]
            .is_none());

        peer_consensus
            .submit_envelope(paid_envelope)
            .expect("peer accepted paid observation");
        let paid_response = peer_consensus
            .catch_up_checked(&local_consensus.lock().await.catch_up_request())
            .expect("paid catch-up response");
        state
            .apply_federation_catch_up_response(&paid_response)
            .await
            .expect("apply paid catch-up");

        let paid_quote = state
            .mint
            .check_melt_quote(&quote_id)
            .await
            .expect("paid quote status");
        assert_eq!(paid_quote.state(), MeltQuoteState::Paid);
        let change = paid_quote.change().expect("paid quote change");
        assert_eq!(change.len(), 1);
        assert_eq!(change[0].amount, Amount::from(1));
        assert_eq!(change[0].keyset_id, change_output.keyset_id);

        let spent_states = state
            .mint
            .localstore()
            .get_proofs_states(&input_ys)
            .await
            .expect("spent proof states");
        assert_eq!(
            spent_states,
            vec![
                Some(cdk::nuts::State::Spent),
                Some(cdk::nuts::State::Spent),
                Some(cdk::nuts::State::Spent),
            ]
        );
        assert!(
            state
                .mint
                .localstore()
                .get_melt_saga_by_quote_id(&quote_id)
                .await
                .expect("saga lookup")
                .is_none(),
            "paid observation catch-up should clear pending melt saga metadata"
        );
        assert_eq!(
            local_consensus.lock().await.peer_status().operation_count,
            2
        );
    }

    #[tokio::test]
    async fn catch_up_rolls_back_pending_melt_after_later_failed_observation() {
        let (state, public_config, _) = federation_route_state().await;
        let config = state
            .mint
            .federation_config()
            .expect("federation config")
            .clone();
        let peer_member = public_config.member(member_id(2)).expect("peer member");
        let fake_description = FakeInvoiceDescription {
            pay_invoice_state: MeltQuoteState::Pending,
            check_payment_state: MeltQuoteState::Pending,
            pay_err: false,
            check_err: false,
        };
        let quote_id = melt_quote_with_description(
            &state,
            1000,
            serde_json::to_string(&fake_description).expect("fake invoice description"),
        )
        .await;
        let inputs = vec![
            proof_for_amount(&state, 1, "late-failed-observation-a"),
            proof_for_amount(&state, 1, "late-failed-observation-b"),
            proof_for_amount(&state, 1, "late-failed-observation-c"),
        ];
        let input_ys = inputs
            .iter()
            .map(|proof| proof.y().expect("input y"))
            .collect::<Vec<_>>();
        let mut change_output = swap_output(&state, "late-failed-observation-change");
        change_output.amount = Amount::ZERO;
        let melt_request = cdk::nuts::MeltRequest::new(
            quote_id.clone(),
            inputs,
            Some(vec![change_output.clone()]),
        );
        let melt_envelope = config.melt_operation(melt_request).expect("melt envelope");
        let failed_envelope =
            melt_payment_observation_envelope(&state, &quote_id, MeltQuoteState::Failed, None);
        let mut peer_consensus =
            InMemoryFederationConsensus::new(public_config.federation_id, peer_member.member_id);
        peer_consensus
            .submit_envelope(melt_envelope)
            .expect("peer accepted pending melt");

        let local_consensus = state.federation_consensus.as_ref().expect("consensus");
        let pending_response = peer_consensus
            .catch_up_checked(&local_consensus.lock().await.catch_up_request())
            .expect("pending catch-up response");
        state
            .apply_federation_catch_up_response(&pending_response)
            .await
            .expect("apply pending catch-up");

        let pending_quote = state
            .mint
            .check_melt_quote(&quote_id)
            .await
            .expect("pending quote status");
        assert_eq!(pending_quote.state(), MeltQuoteState::Pending);
        assert!(pending_quote.change().is_none());
        let pending_states = state
            .mint
            .localstore()
            .get_proofs_states(&input_ys)
            .await
            .expect("pending proof states");
        assert_eq!(
            pending_states,
            vec![
                Some(cdk::nuts::State::Pending),
                Some(cdk::nuts::State::Pending),
                Some(cdk::nuts::State::Pending),
            ]
        );

        peer_consensus
            .submit_envelope(failed_envelope)
            .expect("peer accepted failed observation");
        let failed_response = peer_consensus
            .catch_up_checked(&local_consensus.lock().await.catch_up_request())
            .expect("failed catch-up response");
        state
            .apply_federation_catch_up_response(&failed_response)
            .await
            .expect("apply failed catch-up");

        let failed_quote = state
            .mint
            .check_melt_quote(&quote_id)
            .await
            .expect("failed quote status");
        assert_eq!(failed_quote.state(), MeltQuoteState::Unpaid);
        assert!(failed_quote.payment_proof().is_none());
        assert!(failed_quote.change().is_none());
        let released_states = state
            .mint
            .localstore()
            .get_proofs_states(&input_ys)
            .await
            .expect("released proof states");
        assert_eq!(released_states, vec![None, None, None]);
        assert!(state
            .mint
            .localstore()
            .get_blind_signatures(&[change_output.blinded_secret])
            .await
            .expect("failed change signature lookup")[0]
            .is_none());
        assert!(
            state
                .mint
                .localstore()
                .get_melt_saga_by_quote_id(&quote_id)
                .await
                .expect("saga lookup")
                .is_none(),
            "failed observation catch-up should clear pending melt saga metadata"
        );
        assert_eq!(
            local_consensus.lock().await.peer_status().operation_count,
            2
        );
    }

    #[tokio::test]
    async fn restart_replay_preserves_pending_melt_then_catches_up_paid_observation() {
        let (mut state, public_config, _) = federation_route_state().await;
        let config = state
            .mint
            .federation_config()
            .expect("federation config")
            .clone();
        let peer_member = public_config.member(member_id(2)).expect("peer member");
        let fake_description = FakeInvoiceDescription {
            pay_invoice_state: MeltQuoteState::Pending,
            check_payment_state: MeltQuoteState::Pending,
            pay_err: false,
            check_err: false,
        };
        let quote_id = melt_quote_with_description(
            &state,
            1000,
            serde_json::to_string(&fake_description).expect("fake invoice description"),
        )
        .await;
        let inputs = vec![
            proof_for_amount(&state, 1, "restart-paid-observation-a"),
            proof_for_amount(&state, 1, "restart-paid-observation-b"),
            proof_for_amount(&state, 1, "restart-paid-observation-c"),
        ];
        let input_ys = inputs
            .iter()
            .map(|proof| proof.y().expect("input y"))
            .collect::<Vec<_>>();
        let mut change_output = swap_output(&state, "restart-paid-observation-change");
        change_output.amount = Amount::ZERO;
        let melt_request = cdk::nuts::MeltRequest::new(
            quote_id.clone(),
            inputs,
            Some(vec![change_output.clone()]),
        );
        let melt_envelope = config.melt_operation(melt_request).expect("melt envelope");
        let paid_envelope = melt_payment_observation_envelope(
            &state,
            &quote_id,
            MeltQuoteState::Paid,
            Some("restart-paid-observation-preimage".to_string()),
        );
        let mut peer_consensus =
            InMemoryFederationConsensus::new(public_config.federation_id, peer_member.member_id);
        peer_consensus
            .submit_envelope(melt_envelope)
            .expect("peer accepted pending melt");

        let local_consensus = state.federation_consensus.as_ref().expect("consensus");
        let pending_response = peer_consensus
            .catch_up_checked(&local_consensus.lock().await.catch_up_request())
            .expect("pending catch-up response");
        state
            .apply_federation_catch_up_response(&pending_response)
            .await
            .expect("apply pending catch-up");

        let pending_quote = state
            .mint
            .check_melt_quote(&quote_id)
            .await
            .expect("pending quote status before replay");
        assert_eq!(pending_quote.state(), MeltQuoteState::Pending);
        assert!(pending_quote.change().is_none());
        assert!(state
            .mint
            .localstore()
            .get_melt_saga_by_quote_id(&quote_id)
            .await
            .expect("saga lookup before replay")
            .is_some());

        let (replayed_consensus, replayed_sequences) =
            crate::create_federation_runtime_state(&state.mint)
                .await
                .expect("recreate federation runtime state");
        state.federation_consensus = replayed_consensus;
        state.federation_sequences = replayed_sequences;
        assert_eq!(
            state
                .federation_consensus
                .as_ref()
                .expect("replayed consensus")
                .lock()
                .await
                .peer_status()
                .operation_count,
            1
        );
        let replayed_pending_quote = state
            .mint
            .check_melt_quote(&quote_id)
            .await
            .expect("pending quote status after replay");
        assert_eq!(replayed_pending_quote.state(), MeltQuoteState::Pending);
        assert!(replayed_pending_quote.change().is_none());

        peer_consensus
            .submit_envelope(paid_envelope)
            .expect("peer accepted paid observation");
        let replayed_consensus = state
            .federation_consensus
            .as_ref()
            .expect("replayed consensus");
        let paid_response = peer_consensus
            .catch_up_checked(&replayed_consensus.lock().await.catch_up_request())
            .expect("paid catch-up response after replay");
        state
            .apply_federation_catch_up_response(&paid_response)
            .await
            .expect("apply paid catch-up after replay");

        let paid_quote = state
            .mint
            .check_melt_quote(&quote_id)
            .await
            .expect("paid quote status after replay");
        assert_eq!(paid_quote.state(), MeltQuoteState::Paid);
        let change = paid_quote.change().expect("paid quote change after replay");
        assert_eq!(change.len(), 1);
        assert_eq!(change[0].amount, Amount::from(1));
        assert_eq!(change[0].keyset_id, change_output.keyset_id);

        let proof_states = state
            .mint
            .localstore()
            .get_proofs_states(&input_ys)
            .await
            .expect("proof state lookup after replay");
        assert_eq!(
            proof_states,
            vec![
                Some(cdk::nuts::State::Spent),
                Some(cdk::nuts::State::Spent),
                Some(cdk::nuts::State::Spent),
            ]
        );
        assert!(
            state
                .mint
                .localstore()
                .get_melt_saga_by_quote_id(&quote_id)
                .await
                .expect("saga lookup after paid catch-up")
                .is_none(),
            "paid observation after replay should clear pending melt saga metadata"
        );
        assert_eq!(
            state
                .federation_consensus
                .as_ref()
                .expect("final consensus")
                .lock()
                .await
                .peer_status()
                .operation_count,
            2
        );
    }

    #[tokio::test]
    async fn restart_replay_preserves_pending_melt_then_catches_up_failed_observation() {
        let (mut state, public_config, _) = federation_route_state().await;
        let config = state
            .mint
            .federation_config()
            .expect("federation config")
            .clone();
        let peer_member = public_config.member(member_id(2)).expect("peer member");
        let fake_description = FakeInvoiceDescription {
            pay_invoice_state: MeltQuoteState::Pending,
            check_payment_state: MeltQuoteState::Pending,
            pay_err: false,
            check_err: false,
        };
        let quote_id = melt_quote_with_description(
            &state,
            1000,
            serde_json::to_string(&fake_description).expect("fake invoice description"),
        )
        .await;
        let inputs = vec![
            proof_for_amount(&state, 1, "restart-failed-observation-a"),
            proof_for_amount(&state, 1, "restart-failed-observation-b"),
            proof_for_amount(&state, 1, "restart-failed-observation-c"),
        ];
        let input_ys = inputs
            .iter()
            .map(|proof| proof.y().expect("input y"))
            .collect::<Vec<_>>();
        let mut change_output = swap_output(&state, "restart-failed-observation-change");
        change_output.amount = Amount::ZERO;
        let melt_request = cdk::nuts::MeltRequest::new(
            quote_id.clone(),
            inputs,
            Some(vec![change_output.clone()]),
        );
        let melt_envelope = config.melt_operation(melt_request).expect("melt envelope");
        let failed_envelope =
            melt_payment_observation_envelope(&state, &quote_id, MeltQuoteState::Failed, None);
        let mut peer_consensus =
            InMemoryFederationConsensus::new(public_config.federation_id, peer_member.member_id);
        peer_consensus
            .submit_envelope(melt_envelope)
            .expect("peer accepted pending melt");

        let local_consensus = state.federation_consensus.as_ref().expect("consensus");
        let pending_response = peer_consensus
            .catch_up_checked(&local_consensus.lock().await.catch_up_request())
            .expect("pending catch-up response");
        state
            .apply_federation_catch_up_response(&pending_response)
            .await
            .expect("apply pending catch-up");

        let pending_quote = state
            .mint
            .check_melt_quote(&quote_id)
            .await
            .expect("pending quote status before replay");
        assert_eq!(pending_quote.state(), MeltQuoteState::Pending);
        assert!(pending_quote.change().is_none());
        assert!(state
            .mint
            .localstore()
            .get_melt_saga_by_quote_id(&quote_id)
            .await
            .expect("saga lookup before replay")
            .is_some());

        let (replayed_consensus, replayed_sequences) =
            crate::create_federation_runtime_state(&state.mint)
                .await
                .expect("recreate federation runtime state");
        state.federation_consensus = replayed_consensus;
        state.federation_sequences = replayed_sequences;
        assert_eq!(
            state
                .federation_consensus
                .as_ref()
                .expect("replayed consensus")
                .lock()
                .await
                .peer_status()
                .operation_count,
            1
        );
        let replayed_pending_quote = state
            .mint
            .check_melt_quote(&quote_id)
            .await
            .expect("pending quote status after replay");
        assert_eq!(replayed_pending_quote.state(), MeltQuoteState::Pending);
        assert!(replayed_pending_quote.change().is_none());

        peer_consensus
            .submit_envelope(failed_envelope)
            .expect("peer accepted failed observation");
        let replayed_consensus = state
            .federation_consensus
            .as_ref()
            .expect("replayed consensus");
        let failed_response = peer_consensus
            .catch_up_checked(&replayed_consensus.lock().await.catch_up_request())
            .expect("failed catch-up response after replay");
        state
            .apply_federation_catch_up_response(&failed_response)
            .await
            .expect("apply failed catch-up after replay");

        let rolled_back_quote = state
            .mint
            .check_melt_quote(&quote_id)
            .await
            .expect("rolled back quote status after replay");
        assert_eq!(rolled_back_quote.state(), MeltQuoteState::Unpaid);
        assert!(rolled_back_quote.payment_proof().is_none());
        assert!(rolled_back_quote.change().is_none());
        let proof_states = state
            .mint
            .localstore()
            .get_proofs_states(&input_ys)
            .await
            .expect("proof state lookup after rollback");
        assert_eq!(proof_states, vec![None, None, None]);
        assert!(state
            .mint
            .localstore()
            .get_blind_signatures(&[change_output.blinded_secret])
            .await
            .expect("failed change signature lookup after replay")[0]
            .is_none());
        assert!(
            state
                .mint
                .localstore()
                .get_melt_saga_by_quote_id(&quote_id)
                .await
                .expect("saga lookup after failed catch-up")
                .is_none(),
            "failed observation after replay should clear pending melt saga metadata"
        );
        assert_eq!(
            state
                .federation_consensus
                .as_ref()
                .expect("final consensus")
                .lock()
                .await
                .peer_status()
                .operation_count,
            2
        );
    }

    #[tokio::test]
    async fn sqlite_restart_replays_pending_melt_then_catches_up_paid_observation() {
        let (public_config, local_private, _) = transport_config();
        let db_file = TempMintDbFile::new("cdk-axum-fed-pending-melt-restart");
        let mnemonic = Mnemonic::generate(12).expect("mnemonic");
        let seed = mnemonic.to_seed_normalized("");
        let db = Arc::new(
            cdk_sqlite::MintSqliteDatabase::new(db_file.path())
                .await
                .expect("mint db"),
        );
        let state = federation_route_state_for_private_with_db(
            public_config.clone(),
            local_private.clone(),
            db,
            &seed,
        )
        .await;
        let config = state
            .mint
            .federation_config()
            .expect("federation config")
            .clone();
        let peer_member = public_config.member(member_id(2)).expect("peer member");
        let fake_description = FakeInvoiceDescription {
            pay_invoice_state: MeltQuoteState::Pending,
            check_payment_state: MeltQuoteState::Pending,
            pay_err: false,
            check_err: false,
        };
        let quote_id = melt_quote_with_description(
            &state,
            1000,
            serde_json::to_string(&fake_description).expect("fake invoice description"),
        )
        .await;
        let inputs = vec![
            proof_for_amount(&state, 1, "sqlite-restart-paid-observation-a"),
            proof_for_amount(&state, 1, "sqlite-restart-paid-observation-b"),
            proof_for_amount(&state, 1, "sqlite-restart-paid-observation-c"),
        ];
        let input_ys = inputs
            .iter()
            .map(|proof| proof.y().expect("input y"))
            .collect::<Vec<_>>();
        let mut change_output = swap_output(&state, "sqlite-restart-paid-observation-change");
        change_output.amount = Amount::ZERO;
        let melt_request = cdk::nuts::MeltRequest::new(
            quote_id.clone(),
            inputs,
            Some(vec![change_output.clone()]),
        );
        let melt_envelope = config.melt_operation(melt_request).expect("melt envelope");
        let paid_envelope = melt_payment_observation_envelope(
            &state,
            &quote_id,
            MeltQuoteState::Paid,
            Some("sqlite-restart-paid-observation-preimage".to_string()),
        );
        let mut peer_consensus =
            InMemoryFederationConsensus::new(public_config.federation_id, peer_member.member_id);
        peer_consensus
            .submit_envelope(melt_envelope)
            .expect("peer accepted pending melt");

        let pending_response = {
            let local_consensus = state.federation_consensus.as_ref().expect("consensus");
            peer_consensus
                .catch_up_checked(&local_consensus.lock().await.catch_up_request())
                .expect("pending catch-up response")
        };
        state
            .apply_federation_catch_up_response(&pending_response)
            .await
            .expect("apply pending catch-up");
        assert_eq!(
            state
                .mint
                .check_melt_quote(&quote_id)
                .await
                .expect("pending quote before restart")
                .state(),
            MeltQuoteState::Pending
        );
        assert!(state
            .mint
            .localstore()
            .get_melt_saga_by_quote_id(&quote_id)
            .await
            .expect("saga lookup before restart")
            .is_some());
        drop(state);

        let restarted_db = Arc::new(
            cdk_sqlite::MintSqliteDatabase::new(db_file.path())
                .await
                .expect("restarted mint db"),
        );
        let mut restarted_state = federation_route_state_for_private_with_db(
            public_config.clone(),
            local_private,
            restarted_db,
            &seed,
        )
        .await;
        let (replayed_consensus, replayed_sequences) =
            crate::create_federation_runtime_state(&restarted_state.mint)
                .await
                .expect("recreate federation runtime state from sqlite");
        restarted_state.federation_consensus = replayed_consensus;
        restarted_state.federation_sequences = replayed_sequences;
        assert_eq!(
            restarted_state
                .federation_consensus
                .as_ref()
                .expect("replayed consensus")
                .lock()
                .await
                .peer_status()
                .operation_count,
            1
        );

        let restarted_pending_quote = restarted_state
            .mint
            .check_melt_quote(&quote_id)
            .await
            .expect("pending quote after sqlite restart");
        assert_eq!(restarted_pending_quote.state(), MeltQuoteState::Pending);
        assert!(restarted_pending_quote.change().is_none());
        assert!(restarted_state
            .mint
            .localstore()
            .get_melt_saga_by_quote_id(&quote_id)
            .await
            .expect("saga lookup after sqlite restart")
            .is_some());

        peer_consensus
            .submit_envelope(paid_envelope)
            .expect("peer accepted paid observation");
        let paid_response = {
            let replayed_consensus = restarted_state
                .federation_consensus
                .as_ref()
                .expect("replayed consensus");
            peer_consensus
                .catch_up_checked(&replayed_consensus.lock().await.catch_up_request())
                .expect("paid catch-up response after sqlite restart")
        };
        restarted_state
            .apply_federation_catch_up_response(&paid_response)
            .await
            .expect("apply paid catch-up after sqlite restart");

        let paid_quote = restarted_state
            .mint
            .check_melt_quote(&quote_id)
            .await
            .expect("paid quote after sqlite restart");
        assert_eq!(paid_quote.state(), MeltQuoteState::Paid);
        let change = paid_quote
            .change()
            .expect("paid quote change after restart");
        assert_eq!(change.len(), 1);
        assert_eq!(change[0].amount, Amount::from(1));
        assert_eq!(change[0].keyset_id, change_output.keyset_id);
        let proof_states = restarted_state
            .mint
            .localstore()
            .get_proofs_states(&input_ys)
            .await
            .expect("proof state lookup after sqlite restart");
        assert_eq!(
            proof_states,
            vec![
                Some(cdk::nuts::State::Spent),
                Some(cdk::nuts::State::Spent),
                Some(cdk::nuts::State::Spent),
            ]
        );
        assert!(
            restarted_state
                .mint
                .localstore()
                .get_melt_saga_by_quote_id(&quote_id)
                .await
                .expect("saga lookup after paid catch-up")
                .is_none(),
            "paid observation after sqlite restart should clear pending melt saga metadata"
        );
        assert_eq!(
            restarted_state
                .federation_consensus
                .as_ref()
                .expect("final consensus")
                .lock()
                .await
                .peer_status()
                .operation_count,
            2
        );
    }

    #[tokio::test]
    async fn sqlite_restart_replays_pending_melt_then_catches_up_failed_observation() {
        let (public_config, local_private, _) = transport_config();
        let db_file = TempMintDbFile::new("cdk-axum-fed-pending-melt-failed-restart");
        let mnemonic = Mnemonic::generate(12).expect("mnemonic");
        let seed = mnemonic.to_seed_normalized("");
        let db = Arc::new(
            cdk_sqlite::MintSqliteDatabase::new(db_file.path())
                .await
                .expect("mint db"),
        );
        let state = federation_route_state_for_private_with_db(
            public_config.clone(),
            local_private.clone(),
            db,
            &seed,
        )
        .await;
        let config = state
            .mint
            .federation_config()
            .expect("federation config")
            .clone();
        let peer_member = public_config.member(member_id(2)).expect("peer member");
        let fake_description = FakeInvoiceDescription {
            pay_invoice_state: MeltQuoteState::Pending,
            check_payment_state: MeltQuoteState::Pending,
            pay_err: false,
            check_err: false,
        };
        let quote_id = melt_quote_with_description(
            &state,
            1000,
            serde_json::to_string(&fake_description).expect("fake invoice description"),
        )
        .await;
        let inputs = vec![
            proof_for_amount(&state, 1, "sqlite-restart-failed-observation-a"),
            proof_for_amount(&state, 1, "sqlite-restart-failed-observation-b"),
            proof_for_amount(&state, 1, "sqlite-restart-failed-observation-c"),
        ];
        let input_ys = inputs
            .iter()
            .map(|proof| proof.y().expect("input y"))
            .collect::<Vec<_>>();
        let mut change_output = swap_output(&state, "sqlite-restart-failed-observation-change");
        change_output.amount = Amount::ZERO;
        let melt_request = cdk::nuts::MeltRequest::new(
            quote_id.clone(),
            inputs,
            Some(vec![change_output.clone()]),
        );
        let melt_envelope = config.melt_operation(melt_request).expect("melt envelope");
        let failed_envelope =
            melt_payment_observation_envelope(&state, &quote_id, MeltQuoteState::Failed, None);
        let mut peer_consensus =
            InMemoryFederationConsensus::new(public_config.federation_id, peer_member.member_id);
        peer_consensus
            .submit_envelope(melt_envelope)
            .expect("peer accepted pending melt");

        let pending_response = {
            let local_consensus = state.federation_consensus.as_ref().expect("consensus");
            peer_consensus
                .catch_up_checked(&local_consensus.lock().await.catch_up_request())
                .expect("pending catch-up response")
        };
        state
            .apply_federation_catch_up_response(&pending_response)
            .await
            .expect("apply pending catch-up");
        assert_eq!(
            state
                .mint
                .check_melt_quote(&quote_id)
                .await
                .expect("pending quote before restart")
                .state(),
            MeltQuoteState::Pending
        );
        assert!(state
            .mint
            .localstore()
            .get_melt_saga_by_quote_id(&quote_id)
            .await
            .expect("saga lookup before restart")
            .is_some());
        drop(state);

        let restarted_db = Arc::new(
            cdk_sqlite::MintSqliteDatabase::new(db_file.path())
                .await
                .expect("restarted mint db"),
        );
        let mut restarted_state = federation_route_state_for_private_with_db(
            public_config.clone(),
            local_private,
            restarted_db,
            &seed,
        )
        .await;
        let (replayed_consensus, replayed_sequences) =
            crate::create_federation_runtime_state(&restarted_state.mint)
                .await
                .expect("recreate federation runtime state from sqlite");
        restarted_state.federation_consensus = replayed_consensus;
        restarted_state.federation_sequences = replayed_sequences;
        assert_eq!(
            restarted_state
                .federation_consensus
                .as_ref()
                .expect("replayed consensus")
                .lock()
                .await
                .peer_status()
                .operation_count,
            1
        );

        let restarted_pending_quote = restarted_state
            .mint
            .check_melt_quote(&quote_id)
            .await
            .expect("pending quote after sqlite restart");
        assert_eq!(restarted_pending_quote.state(), MeltQuoteState::Pending);
        assert!(restarted_pending_quote.change().is_none());
        assert!(restarted_state
            .mint
            .localstore()
            .get_melt_saga_by_quote_id(&quote_id)
            .await
            .expect("saga lookup after sqlite restart")
            .is_some());

        peer_consensus
            .submit_envelope(failed_envelope)
            .expect("peer accepted failed observation");
        let failed_response = {
            let replayed_consensus = restarted_state
                .federation_consensus
                .as_ref()
                .expect("replayed consensus");
            peer_consensus
                .catch_up_checked(&replayed_consensus.lock().await.catch_up_request())
                .expect("failed catch-up response after sqlite restart")
        };
        restarted_state
            .apply_federation_catch_up_response(&failed_response)
            .await
            .expect("apply failed catch-up after sqlite restart");

        let rolled_back_quote = restarted_state
            .mint
            .check_melt_quote(&quote_id)
            .await
            .expect("rolled back quote after sqlite restart");
        assert_eq!(rolled_back_quote.state(), MeltQuoteState::Unpaid);
        assert!(rolled_back_quote.payment_proof().is_none());
        assert!(rolled_back_quote.change().is_none());
        let proof_states = restarted_state
            .mint
            .localstore()
            .get_proofs_states(&input_ys)
            .await
            .expect("proof state lookup after sqlite restart");
        assert_eq!(proof_states, vec![None, None, None]);
        assert!(restarted_state
            .mint
            .localstore()
            .get_blind_signatures(&[change_output.blinded_secret])
            .await
            .expect("failed change signature lookup after sqlite restart")[0]
            .is_none());
        assert!(
            restarted_state
                .mint
                .localstore()
                .get_melt_saga_by_quote_id(&quote_id)
                .await
                .expect("saga lookup after failed catch-up")
                .is_none(),
            "failed observation after sqlite restart should clear pending melt saga metadata"
        );
        assert_eq!(
            restarted_state
                .federation_consensus
                .as_ref()
                .expect("final consensus")
                .lock()
                .await
                .peer_status()
                .operation_count,
            2
        );
    }

    #[tokio::test]
    async fn catch_up_applies_unpaid_melt_payment_observation_to_pending_melt() {
        let (state, public_config, _) = federation_route_state().await;
        let config = state
            .mint
            .federation_config()
            .expect("federation config")
            .clone();
        let peer_member = public_config.member(member_id(2)).expect("peer member");
        let fake_description = FakeInvoiceDescription {
            pay_invoice_state: MeltQuoteState::Pending,
            check_payment_state: MeltQuoteState::Pending,
            pay_err: false,
            check_err: false,
        };
        let quote_id = melt_quote_with_description(
            &state,
            1000,
            serde_json::to_string(&fake_description).expect("fake invoice description"),
        )
        .await;
        let inputs = vec![
            proof_for_amount(&state, 1, "unpaid-observation-a"),
            proof_for_amount(&state, 1, "unpaid-observation-b"),
        ];
        let input_ys = inputs
            .iter()
            .map(|proof| proof.y().expect("input y"))
            .collect::<Vec<_>>();
        let melt_request = cdk::nuts::MeltRequest::new(quote_id.clone(), inputs, None);
        let melt_envelope = config.melt_operation(melt_request).expect("melt envelope");
        let unpaid_envelope =
            melt_payment_observation_envelope(&state, &quote_id, MeltQuoteState::Unpaid, None);
        let mut peer_consensus =
            InMemoryFederationConsensus::new(public_config.federation_id, peer_member.member_id);
        peer_consensus
            .submit_envelope(melt_envelope)
            .expect("peer accepted pending melt");
        peer_consensus
            .submit_envelope(unpaid_envelope)
            .expect("peer accepted unpaid observation");
        let local_consensus = state.federation_consensus.as_ref().expect("consensus");
        let catch_up_response = peer_consensus
            .catch_up_checked(&local_consensus.lock().await.catch_up_request())
            .expect("catch-up response");

        state
            .apply_federation_catch_up_response(&catch_up_response)
            .await
            .expect("apply catch-up");

        let quote = state
            .mint
            .localstore()
            .get_melt_quote(&quote_id)
            .await
            .expect("quote lookup")
            .expect("quote persisted");
        assert_eq!(quote.state, MeltQuoteState::Unpaid);
        assert!(quote.payment_proof.is_none());
        assert!(
            state
                .mint
                .localstore()
                .get_melt_saga_by_quote_id(&quote_id)
                .await
                .expect("saga lookup")
                .is_none(),
            "rollback should remove pending melt saga metadata"
        );

        let proof_states = state
            .mint
            .localstore()
            .get_proofs_states(&input_ys)
            .await
            .expect("proof state lookup");
        assert_eq!(proof_states, vec![None, None]);

        let persisted = state
            .mint
            .localstore()
            .get_federation_journal_entries()
            .await
            .expect("journal lookup");
        assert_eq!(persisted.len(), 2);
        assert!(persisted
            .iter()
            .all(|entry| entry.status == FederationOperationStatus::Applied));
    }

    #[tokio::test]
    async fn offline_member_catches_up_multiple_operations_and_converges() {
        let (state, public_config, _) = federation_route_state().await;
        let config = state
            .mint
            .federation_config()
            .expect("federation config")
            .clone();
        let peer_member = public_config.member(member_id(2)).expect("peer member");
        let mut peer_consensus =
            InMemoryFederationConsensus::new(public_config.federation_id, peer_member.member_id);

        let mint_quote_id = QuoteId::new();
        let minted_output = mint_output(&state, &mint_quote_id);
        let mint_request = MintInput::Single(MintRequest {
            quote: mint_quote_id.clone(),
            outputs: vec![minted_output.clone()],
            signature: None,
        });

        let swap_input = proof_for_amount(&state, 1, "offline-catch-up-swap-input");
        let swap_input_y = swap_input.y().expect("swap input y");
        let swapped_output = swap_output(&state, "offline-catch-up-swap-output");
        let swap_request =
            cdk::nuts::SwapRequest::new(vec![swap_input], vec![swapped_output.clone()]);

        let fake_description = FakeInvoiceDescription {
            pay_invoice_state: MeltQuoteState::Pending,
            check_payment_state: MeltQuoteState::Pending,
            pay_err: false,
            check_err: false,
        };
        let melt_quote_id = QuoteId::new();
        let melt_invoice = create_fake_invoice(
            1000,
            serde_json::to_string(&fake_description).expect("fake invoice description"),
        );
        let melt_quote_envelope = config.melt_quote_operation(
            melt_quote_id.clone(),
            cdk::MeltQuoteRequest::Bolt11(MeltQuoteBolt11Request {
                request: melt_invoice,
                unit: CurrencyUnit::Sat,
                options: None,
            }),
        );
        let melt_inputs = vec![
            proof_for_amount(&state, 1, "offline-catch-up-melt-a"),
            proof_for_amount(&state, 1, "offline-catch-up-melt-b"),
            proof_for_amount(&state, 1, "offline-catch-up-melt-c"),
        ];
        let melt_input_ys = melt_inputs
            .iter()
            .map(|proof| proof.y().expect("melt input y"))
            .collect::<Vec<_>>();
        let mut melt_change_output = swap_output(&state, "offline-catch-up-melt-change");
        melt_change_output.amount = Amount::ZERO;
        let melt_request = cdk::nuts::MeltRequest::new(
            melt_quote_id.clone(),
            melt_inputs,
            Some(vec![melt_change_output.clone()]),
        );
        let melt_envelope = config.melt_operation(melt_request).expect("melt envelope");
        let melt_payment_envelope = melt_payment_observation_envelope(
            &state,
            &melt_quote_id,
            MeltQuoteState::Paid,
            Some("offline-catch-up-preimage".to_string()),
        );

        for envelope in [
            mint_quote_envelope(&state, &mint_quote_id),
            paid_observation_envelope(&state, &mint_quote_id),
            config
                .mint_operation(mint_request.clone())
                .expect("mint envelope"),
            config
                .swap_operation(swap_request.clone())
                .expect("swap envelope"),
            melt_quote_envelope,
            melt_envelope,
            melt_payment_envelope,
        ] {
            peer_consensus
                .submit_envelope(envelope)
                .expect("peer accepted operation");
        }

        let local_consensus = state.federation_consensus.as_ref().expect("consensus");
        assert_eq!(
            local_consensus.lock().await.peer_status().operation_count,
            0
        );
        let catch_up_response = peer_consensus
            .catch_up_checked(&local_consensus.lock().await.catch_up_request())
            .expect("peer catch-up response");

        let applied = state
            .apply_federation_catch_up_response(&catch_up_response)
            .await
            .expect("apply offline catch-up");

        assert_eq!(applied.len(), 7);
        assert_eq!(
            local_consensus
                .lock()
                .await
                .peer_status()
                .latest_order_digest,
            peer_consensus.peer_status().latest_order_digest
        );

        let persisted = state
            .mint
            .localstore()
            .get_federation_journal_entries()
            .await
            .expect("journal lookup");
        assert_eq!(persisted.len(), 7);
        assert!(persisted
            .iter()
            .all(|entry| entry.status == FederationOperationStatus::Applied));

        let quote = state
            .mint
            .localstore()
            .get_mint_quote(&mint_quote_id)
            .await
            .expect("mint quote lookup")
            .expect("mint quote persisted");
        assert_eq!(quote.state(), MintQuoteState::Issued);

        let mint_signatures = state
            .mint
            .localstore()
            .get_blind_signatures(&[minted_output.blinded_secret])
            .await
            .expect("mint signature lookup");
        assert!(mint_signatures[0].is_some());

        let swap_states = state
            .mint
            .localstore()
            .get_proofs_states(&[swap_input_y])
            .await
            .expect("swap state lookup");
        assert_eq!(swap_states, vec![Some(cdk::nuts::State::Spent)]);
        let swap_signatures = state
            .mint
            .localstore()
            .get_blind_signatures(&[swapped_output.blinded_secret])
            .await
            .expect("swap signature lookup");
        assert!(swap_signatures[0].is_some());

        let melt_quote = state
            .mint
            .localstore()
            .get_melt_quote(&melt_quote_id)
            .await
            .expect("melt quote lookup")
            .expect("melt quote persisted");
        assert_eq!(melt_quote.state, MeltQuoteState::Paid);
        assert_eq!(
            melt_quote.payment_proof.as_deref(),
            Some("offline-catch-up-preimage")
        );
        assert!(
            state
                .mint
                .localstore()
                .get_melt_saga_by_quote_id(&melt_quote_id)
                .await
                .expect("melt saga lookup")
                .is_none(),
            "paid observation replay should complete pending melt saga metadata"
        );

        let melt_states = state
            .mint
            .localstore()
            .get_proofs_states(&melt_input_ys)
            .await
            .expect("melt state lookup");
        assert_eq!(
            melt_states,
            vec![
                Some(cdk::nuts::State::Spent),
                Some(cdk::nuts::State::Spent),
                Some(cdk::nuts::State::Spent),
            ]
        );
        let melt_change_signatures = state
            .mint
            .localstore()
            .get_blind_signatures(&[melt_change_output.blinded_secret])
            .await
            .expect("melt change signature lookup");
        assert!(melt_change_signatures[0].is_some());

        let conflicting_output = swap_output(&state, "offline-catch-up-conflicting-mint");
        let conflicting_mint = config
            .mint_operation(MintInput::Single(MintRequest {
                quote: mint_quote_id,
                outputs: vec![conflicting_output],
                signature: None,
            }))
            .expect("conflicting mint envelope");
        let conflict = state.submit_federation_envelope(conflicting_mint).await;
        assert!(matches!(
            conflict,
            Err(cdk::Error::Federation(
                FederationError::ConflictingOperation { .. }
            ))
        ));
    }

    #[tokio::test]
    async fn diverged_member_rejects_peer_catch_up_digest_mismatch() {
        let (state, public_config, _) = federation_route_state().await;
        let peer_member = public_config.member(member_id(2)).expect("peer member");
        let peer_entry = restore_envelope(
            public_config.federation_id,
            "0204bbffa045f28ec836117a29ea0a00d77f1d692e38cf94f72a5145bfda6d8f41",
        );
        let local_entry = restore_envelope(
            public_config.federation_id,
            "025f0615ccba96f810582a6885ffdb04bd57c96dbc590f5aa560447b31258988d7",
        );
        let mut peer_consensus =
            InMemoryFederationConsensus::new(public_config.federation_id, peer_member.member_id);
        peer_consensus
            .submit_envelope(peer_entry)
            .expect("peer accepted operation");
        state
            .submit_federation_envelope(local_entry)
            .await
            .expect("local accepted divergent operation");

        let local_consensus = state.federation_consensus.as_ref().expect("consensus");
        let request = local_consensus.lock().await.catch_up_request();
        assert_eq!(request.next_index, 1);
        assert!(matches!(
            peer_consensus.catch_up_checked(&request),
            Err(FederationError::CatchUpDigestMismatch { .. })
        ));

        let unchecked_response = peer_consensus.catch_up(&request);
        let apply = state
            .apply_federation_catch_up_response(&unchecked_response)
            .await;
        assert!(matches!(
            apply,
            Err(cdk::Error::Federation(
                FederationError::CatchUpDigestMismatch { .. }
            ))
        ));
    }

    #[tokio::test]
    async fn federation_runtime_state_replays_persisted_journal_and_sequences() {
        let (state, public_config, peer_private) = federation_route_state().await;
        let peer_member = peer_private.member_id;
        let envelope = restore_envelope(
            public_config.federation_id,
            "0204bbffa045f28ec836117a29ea0a00d77f1d692e38cf94f72a5145bfda6d8f41",
        );
        let response = state
            .submit_federation_envelope(envelope)
            .await
            .expect("persist operation");
        state
            .persist_federation_sequence(peer_member, 42)
            .await
            .expect("persist sequence");

        let (consensus, sequences) = crate::create_federation_runtime_state(&state.mint)
            .await
            .expect("runtime state");
        let consensus = consensus.expect("consensus");
        let sequences = sequences.expect("sequences");
        let consensus = consensus.lock().await;
        let sequences = sequences.lock().await;

        assert_eq!(consensus.peer_status().operation_count, 1);
        assert_eq!(
            consensus
                .journal()
                .entries()
                .first()
                .expect("journal entry")
                .operation_id,
            response.operation_id
        );
        assert_eq!(sequences.sequences().get(&peer_member), Some(&42));
    }

    #[tokio::test]
    async fn submit_mint_quote_operation_applies_quote_to_local_store() {
        let (state, _, _) = federation_route_state().await;
        let quote_id = QuoteId::new();
        let envelope = mint_quote_envelope(&state, &quote_id);
        let operation_id = envelope.operation_id().expect("operation id");

        state
            .submit_federation_envelope(envelope)
            .await
            .expect("submit mint quote");

        let quote = state
            .mint
            .localstore()
            .get_mint_quote(&quote_id)
            .await
            .expect("quote lookup")
            .expect("quote persisted");
        assert_eq!(quote.request, format!("invoice-{quote_id}"));
        assert_eq!(quote.amount, Some(Amount::new(1, CurrencyUnit::Sat)));
        assert_eq!(quote.state(), MintQuoteState::Unpaid);

        let persisted = state
            .mint
            .localstore()
            .get_federation_journal_entries()
            .await
            .expect("journal lookup");
        assert_eq!(persisted[0].status, FederationOperationStatus::Applied);
        assert_eq!(
            state
                .federation_consensus
                .as_ref()
                .expect("consensus")
                .lock()
                .await
                .journal()
                .entry_by_operation_id(operation_id)
                .expect("runtime entry")
                .status,
            FederationOperationStatus::Applied
        );
    }

    #[tokio::test]
    async fn local_status_uses_materialized_database_state_digest() {
        let (state, _, _) = federation_route_state().await;
        let quote_id = QuoteId::new();
        state
            .submit_federation_envelope(mint_quote_envelope(&state, &quote_id))
            .await
            .expect("submit mint quote");

        let status = local_peer_status(&state).await.expect("local status");
        let consensus = state.federation_consensus.as_ref().expect("consensus");
        let journal_only_digest = consensus.lock().await.state_digest();

        assert_eq!(status.operation_count, 1);
        assert_ne!(
            status.state_digest, journal_only_digest,
            "status digest should include applied mint database rows, not only the accepted journal"
        );
        assert_ne!(status.state_digest, FederationStateDigest::genesis());
    }

    #[tokio::test]
    async fn materialized_state_digest_tracks_keyset_rotation_rows() {
        let (state, public_config, _) = federation_route_state().await;
        let consensus = state.federation_consensus.as_ref().expect("consensus");
        let before_entries = consensus.lock().await.journal().entries().to_vec();
        let before_digest = materialized_state_digest(&state, &before_entries)
            .await
            .expect("before digest");
        let keyset = public_config.keysets[0].clone();
        let envelope = FederationOperationEnvelope::new(
            public_config.federation_id,
            FederationOperation::KeysetRotation(FederationKeysetRotationOperation { keyset }),
        );
        let operation_id = envelope.operation_id().expect("operation id");

        state
            .submit_federation_envelope(envelope)
            .await
            .expect("submit keyset rotation");
        let after_entries = consensus.lock().await.journal().entries().to_vec();
        let after_digest = materialized_state_digest(&state, &after_entries)
            .await
            .expect("after digest");

        assert_ne!(before_digest, after_digest);
        assert_ne!(after_digest, FederationStateDigest::genesis());

        let persisted = state
            .mint
            .localstore()
            .get_federation_journal_entries()
            .await
            .expect("journal lookup");
        assert_eq!(
            persisted
                .iter()
                .find(|entry| entry.operation_id == operation_id)
                .expect("rotation entry")
                .status,
            FederationOperationStatus::Applied
        );
    }

    #[tokio::test]
    async fn materialized_state_digest_ignores_local_mint_payment_metadata() {
        let (first_state, _, _) = federation_route_state().await;
        let (second_state, _, _) = federation_route_state().await;
        let quote_id = QuoteId::new();
        let quote_envelope = mint_quote_envelope(&first_state, &quote_id);
        let paid_envelope = paid_observation_envelope(&first_state, &quote_id);

        for state in [&first_state, &second_state] {
            state
                .submit_federation_envelope(quote_envelope.clone())
                .await
                .expect("submit mint quote");
            state
                .submit_federation_envelope(paid_envelope.clone())
                .await
                .expect("submit paid observation");
        }

        let mut tx = second_state
            .mint
            .localstore()
            .begin_transaction()
            .await
            .expect("begin tx");
        let mut quote = tx
            .get_mint_quote(&quote_id)
            .await
            .expect("load quote")
            .expect("mint quote");
        quote
            .add_payment(
                Amount::from(0).with_unit(CurrencyUnit::Sat),
                "local-backend-zero-payment".to_string(),
                Some(42),
            )
            .expect("add local payment metadata");
        tx.update_mint_quote(&mut quote)
            .await
            .expect("persist local payment metadata");
        tx.commit().await.expect("commit tx");

        let first_entries = first_state
            .federation_consensus
            .as_ref()
            .expect("consensus")
            .lock()
            .await
            .journal()
            .entries()
            .to_vec();
        let second_entries = second_state
            .federation_consensus
            .as_ref()
            .expect("consensus")
            .lock()
            .await
            .journal()
            .entries()
            .to_vec();

        let first_digest = materialized_state_digest(&first_state, &first_entries)
            .await
            .expect("first digest");
        let second_digest = materialized_state_digest(&second_state, &second_entries)
            .await
            .expect("second digest");

        assert_eq!(first_digest, second_digest);
    }

    #[tokio::test]
    async fn materialized_state_digest_tracks_unfinalized_payment_observation_rows() {
        let (mut public_config, local_private, _) = transport_config();
        public_config.payment_observation_policy =
            FederationPaymentObservationPolicy::member_quorum(
                2,
                FederationPaymentTrustModel::SharedBackendReadOnly,
            );
        let state = federation_route_state_for_private(public_config, local_private).await;
        let quote_id = QuoteId::new();
        state
            .submit_federation_envelope(mint_quote_envelope(&state, &quote_id))
            .await
            .expect("submit mint quote");

        let consensus = state.federation_consensus.as_ref().expect("consensus");
        let before_entries = consensus.lock().await.journal().entries().to_vec();
        let before_digest = materialized_state_digest(&state, &before_entries)
            .await
            .expect("digest before observation");

        state
            .submit_federation_envelope(paid_observation_envelope(&state, &quote_id))
            .await
            .expect("submit unfinalized observation");
        let after_entries = consensus.lock().await.journal().entries().to_vec();
        let after_digest = materialized_state_digest(&state, &after_entries)
            .await
            .expect("digest after observation");

        assert_ne!(
            before_digest, after_digest,
            "accepted payment-observation evidence must affect the materialized digest before quote finality"
        );

        let quote = state
            .mint
            .localstore()
            .get_mint_quote(&quote_id)
            .await
            .expect("quote lookup")
            .expect("quote persisted");
        assert_eq!(quote.state(), MintQuoteState::Unpaid);
        assert_eq!(quote.amount_paid(), Amount::new(0, CurrencyUnit::Sat));

        let observations = state
            .mint
            .localstore()
            .get_federation_payment_observations(
                cdk::federation::FederationPaymentObservationContext::MintQuote,
                &quote_id.to_string(),
            )
            .await
            .expect("payment observations");
        assert_eq!(observations.len(), 1);
    }

    #[tokio::test]
    async fn materialized_state_digest_tracks_payment_observation_quarantine_rows() {
        let (state, _, _) = federation_route_state().await;
        let config = state
            .mint
            .federation_config()
            .expect("federation config")
            .clone();
        let quote_id = QuoteId::new();
        state
            .submit_federation_envelope(mint_quote_envelope(&state, &quote_id))
            .await
            .expect("submit mint quote");
        let first_response = state
            .submit_federation_envelope(mint_payment_observation_envelope_for_member(
                &state, &quote_id, 1, 1, "settled",
            ))
            .await
            .expect("submit first observation");
        let conflicting_response = state
            .submit_federation_envelope(mint_payment_observation_envelope_for_member(
                &state,
                &quote_id,
                2,
                2,
                "different-settlement",
            ))
            .await
            .expect("submit conflicting observation");

        let consensus = state.federation_consensus.as_ref().expect("consensus");
        let before_entries = consensus.lock().await.journal().entries().to_vec();
        let before_digest = materialized_state_digest(&state, &before_entries)
            .await
            .expect("digest before quarantine");

        state
            .submit_federation_envelope(config.payment_observation_quarantine(
                cdk::federation::FederationPaymentObservationContext::MintQuote,
                quote_id.clone(),
                FederationPaymentObservationQuarantineReason::ConflictingEvidence,
                vec![
                    first_response.operation_id,
                    conflicting_response.operation_id,
                ],
            ))
            .await
            .expect("submit quarantine");
        let after_entries = consensus.lock().await.journal().entries().to_vec();
        let after_digest = materialized_state_digest(&state, &after_entries)
            .await
            .expect("digest after quarantine");

        assert_ne!(
            before_digest, after_digest,
            "accepted payment-observation quarantine rows must affect the materialized digest"
        );
        assert_ne!(after_digest, FederationStateDigest::genesis());
    }

    #[tokio::test]
    async fn materialized_state_digest_tracks_swap_spend_and_output_share_rows() {
        let (state, public_config, _) = federation_route_state().await;
        let config = state
            .mint
            .federation_config()
            .expect("federation config")
            .clone();
        let peer_member = public_config.member(member_id(2)).expect("peer member");
        let input = proof_for_amount(&state, 1, "digest-swap");
        let input_y = input.y().expect("input y");
        let output = swap_output(&state, "digest-swap");
        let request = cdk::nuts::SwapRequest::new(vec![input], vec![output.clone()]);
        let envelope = config.swap_operation(request).expect("swap envelope");
        let mut peer_consensus =
            InMemoryFederationConsensus::new(public_config.federation_id, peer_member.member_id);
        peer_consensus
            .submit_envelope(envelope)
            .expect("peer accepted swap");
        let local_consensus = state.federation_consensus.as_ref().expect("consensus");
        let catch_up_request = local_consensus.lock().await.catch_up_request();
        let catch_up_response = peer_consensus
            .catch_up_checked(&catch_up_request)
            .expect("catch-up response");

        let before_digest = materialized_state_digest(&state, &catch_up_response.entries)
            .await
            .expect("before digest");
        state
            .apply_federation_catch_up_response(&catch_up_response)
            .await
            .expect("apply catch-up");
        let after_digest = materialized_state_digest(&state, &catch_up_response.entries)
            .await
            .expect("after digest");

        assert_ne!(before_digest, after_digest);
        assert_ne!(after_digest, FederationStateDigest::genesis());

        let proof_states = state
            .mint
            .localstore()
            .get_proofs_states(&[input_y])
            .await
            .expect("proof state lookup");
        assert_eq!(proof_states, vec![Some(cdk::nuts::State::Spent)]);

        let signatures = state
            .mint
            .localstore()
            .get_blind_signatures(&[output.blinded_secret])
            .await
            .expect("signature lookup");
        assert!(signatures[0].is_some());
    }

    #[tokio::test]
    async fn submit_federation_envelope_notifies_operation_acceptance() {
        let (mut state, _, _) = federation_route_state().await;
        state.start_federation_finalization_loop();
        let quote_id = QuoteId::new();
        let envelope = mint_quote_envelope(&state, &quote_id);
        let operation_id = envelope.operation_id().expect("operation id");
        let waiter_state = state.clone();
        let waiter = tokio::spawn(async move {
            waiter_state
                .wait_for_federation_operation_acceptance(operation_id)
                .await
                .expect("notified response")
        });
        tokio::task::yield_now().await;

        let response = state
            .submit_federation_envelope(envelope)
            .await
            .expect("submit mint quote");
        let notified = tokio::time::timeout(std::time::Duration::from_secs(1), waiter)
            .await
            .expect("notification waiter")
            .expect("notification task");

        assert_eq!(response.operation_id, operation_id);
        assert_eq!(notified.operation_id, operation_id);
        assert_eq!(notified.outcome.entry().operation_id, operation_id);
    }

    #[tokio::test]
    async fn submit_mint_operation_applies_issuance_and_persists_share() {
        let (state, _, _) = federation_route_state().await;
        let config = state
            .mint
            .federation_config()
            .expect("federation config")
            .clone();
        let quote_id = QuoteId::new();
        state
            .submit_federation_envelope(mint_quote_envelope(&state, &quote_id))
            .await
            .expect("submit mint quote");
        state
            .submit_federation_envelope(paid_observation_envelope(&state, &quote_id))
            .await
            .expect("submit paid observation");

        let output = mint_output(&state, &quote_id);
        let input = MintInput::Single(MintRequest {
            quote: quote_id.clone(),
            outputs: vec![output.clone()],
            signature: None,
        });
        let envelope = config.mint_operation(input.clone()).expect("mint envelope");
        let operation_id = envelope.operation_id().expect("operation id");
        state
            .submit_federation_envelope(envelope)
            .await
            .expect("submit mint");

        let quote = state
            .mint
            .localstore()
            .get_mint_quote(&quote_id)
            .await
            .expect("quote lookup")
            .expect("quote persisted");
        assert_eq!(quote.amount_paid(), Amount::new(1, CurrencyUnit::Sat));
        assert_eq!(quote.amount_issued(), Amount::new(1, CurrencyUnit::Sat));
        assert_eq!(quote.state(), MintQuoteState::Issued);

        let signatures = state
            .mint
            .localstore()
            .get_blind_signatures(&[output.blinded_secret])
            .await
            .expect("signature lookup");
        let persisted_signature = signatures[0].clone().expect("signature persisted");
        let expected_response = config
            .mint_share_response(operation_id, &input)
            .expect("expected share response");
        assert_eq!(persisted_signature, expected_response.signatures[0]);

        let persisted = state
            .mint
            .localstore()
            .get_federation_journal_entries()
            .await
            .expect("journal lookup");
        assert_eq!(
            persisted
                .iter()
                .find(|entry| entry.operation_id == operation_id)
                .expect("mint entry")
                .status,
            FederationOperationStatus::Applied
        );
    }

    #[tokio::test]
    async fn materialized_state_digest_ignores_member_local_signature_share_bytes() {
        let (public_config, first_private, second_private) = transport_config();
        let first_state =
            federation_route_state_for_private(public_config.clone(), first_private).await;
        let second_state =
            federation_route_state_for_private(public_config.clone(), second_private).await;
        let quote_id = QuoteId::new();
        let quote_envelope = mint_quote_envelope(&first_state, &quote_id);
        let paid_envelope = paid_observation_envelope(&first_state, &quote_id);
        let output = mint_output(&first_state, &quote_id);
        let input = MintInput::Single(MintRequest {
            quote: quote_id.clone(),
            outputs: vec![output.clone()],
            signature: None,
        });
        let mint_envelope = first_state
            .mint
            .federation_config()
            .expect("federation config")
            .mint_operation(input)
            .expect("mint envelope");

        for state in [&first_state, &second_state] {
            state
                .submit_federation_envelope_with_sequence(quote_envelope.clone(), None)
                .await
                .expect("submit mint quote");
            state
                .submit_federation_envelope_with_sequence(paid_envelope.clone(), None)
                .await
                .expect("submit paid observation");
            state
                .submit_federation_envelope_with_sequence(mint_envelope.clone(), None)
                .await
                .expect("submit mint");
        }

        let first_signature = first_state
            .mint
            .localstore()
            .get_blind_signatures(&[output.blinded_secret])
            .await
            .expect("first signature lookup")
            .remove(0)
            .expect("first signature");
        let second_signature = second_state
            .mint
            .localstore()
            .get_blind_signatures(&[output.blinded_secret])
            .await
            .expect("second signature lookup")
            .remove(0)
            .expect("second signature");
        assert_ne!(first_signature.c, second_signature.c);

        let first_entries = first_state
            .federation_consensus
            .as_ref()
            .expect("consensus")
            .lock()
            .await
            .journal()
            .entries()
            .to_vec();
        let second_entries = second_state
            .federation_consensus
            .as_ref()
            .expect("consensus")
            .lock()
            .await
            .journal()
            .entries()
            .to_vec();
        let first_digest = materialized_state_digest(&first_state, &first_entries)
            .await
            .expect("first digest");
        let second_digest = materialized_state_digest(&second_state, &second_entries)
            .await
            .expect("second digest");

        assert_eq!(first_digest, second_digest);
    }

    #[tokio::test]
    async fn materialized_state_digest_tracks_melt_quote_spend_and_change_rows() {
        let (state, public_config, _) = federation_route_state().await;
        let config = state
            .mint
            .federation_config()
            .expect("federation config")
            .clone();
        let peer_member = public_config.member(member_id(2)).expect("peer member");
        let quote_id = melt_quote(&state, 1000).await;
        let inputs = vec![
            proof_for_amount(&state, 1, "digest-melt-a"),
            proof_for_amount(&state, 1, "digest-melt-b"),
            proof_for_amount(&state, 1, "digest-melt-c"),
        ];
        let input_ys = inputs
            .iter()
            .map(|proof| proof.y().expect("input y"))
            .collect::<Vec<_>>();
        let mut change_output = swap_output(&state, "digest-melt-change");
        change_output.amount = Amount::ZERO;
        let request = cdk::nuts::MeltRequest::new(
            quote_id.clone(),
            inputs,
            Some(vec![change_output.clone()]),
        );
        let envelope = config.melt_operation(request).expect("melt envelope");
        let mut peer_consensus =
            InMemoryFederationConsensus::new(public_config.federation_id, peer_member.member_id);
        peer_consensus
            .submit_envelope(envelope)
            .expect("peer accepted melt");
        let local_consensus = state.federation_consensus.as_ref().expect("consensus");
        let catch_up_request = local_consensus.lock().await.catch_up_request();
        let catch_up_response = peer_consensus
            .catch_up_checked(&catch_up_request)
            .expect("catch-up response");

        let before_digest = materialized_state_digest(&state, &catch_up_response.entries)
            .await
            .expect("before digest");
        state
            .apply_federation_catch_up_response(&catch_up_response)
            .await
            .expect("apply catch-up");
        let after_digest = materialized_state_digest(&state, &catch_up_response.entries)
            .await
            .expect("after digest");

        assert_ne!(before_digest, after_digest);
        assert_ne!(after_digest, FederationStateDigest::genesis());

        let quote = state
            .mint
            .localstore()
            .get_melt_quote(&quote_id)
            .await
            .expect("quote lookup")
            .expect("quote persisted");
        assert_eq!(quote.state, MeltQuoteState::Paid);

        let proof_states = state
            .mint
            .localstore()
            .get_proofs_states(&input_ys)
            .await
            .expect("proof state lookup");
        assert_eq!(
            proof_states,
            vec![
                Some(cdk::nuts::State::Spent),
                Some(cdk::nuts::State::Spent),
                Some(cdk::nuts::State::Spent),
            ]
        );

        let signatures = state
            .mint
            .localstore()
            .get_blind_signatures(&[change_output.blinded_secret])
            .await
            .expect("signature lookup");
        assert!(signatures[0].is_some());
    }

    #[tokio::test]
    async fn materialized_state_digest_tracks_restore_output_share_rows() {
        let (state, public_config, _) = federation_route_state().await;
        let config = state
            .mint
            .federation_config()
            .expect("federation config")
            .clone();
        let peer_member = public_config.member(member_id(2)).expect("peer member");
        let quote_id = QuoteId::new();
        let output = mint_output(&state, &quote_id);
        let mut restore_output = output.clone();
        restore_output.amount = Amount::ZERO;
        let restore_envelope = FederationOperationEnvelope::new(
            public_config.federation_id,
            FederationOperation::Restore(FederationRestoreOperation {
                request: RestoreRequest {
                    outputs: vec![restore_output],
                },
            }),
        );
        let mut peer_consensus =
            InMemoryFederationConsensus::new(public_config.federation_id, peer_member.member_id);
        peer_consensus
            .submit_envelope(restore_envelope)
            .expect("peer accepted restore");
        let local_consensus = state.federation_consensus.as_ref().expect("consensus");
        let catch_up_request = local_consensus.lock().await.catch_up_request();
        let catch_up_response = peer_consensus
            .catch_up_checked(&catch_up_request)
            .expect("catch-up response");

        let missing_digest = materialized_state_digest(&state, &catch_up_response.entries)
            .await
            .expect("missing digest");
        state
            .submit_federation_envelope(mint_quote_envelope(&state, &quote_id))
            .await
            .expect("submit mint quote");
        state
            .submit_federation_envelope(paid_observation_envelope(&state, &quote_id))
            .await
            .expect("submit paid observation");
        let input = MintInput::Single(MintRequest {
            quote: quote_id,
            outputs: vec![output.clone()],
            signature: None,
        });
        state
            .submit_federation_envelope(config.mint_operation(input).expect("mint envelope"))
            .await
            .expect("submit mint");
        let materialized_digest = materialized_state_digest(&state, &catch_up_response.entries)
            .await
            .expect("materialized digest");

        assert_ne!(missing_digest, materialized_digest);
        assert_ne!(materialized_digest, FederationStateDigest::genesis());

        let signatures = state
            .mint
            .localstore()
            .get_blind_signatures(&[output.blinded_secret])
            .await
            .expect("signature lookup");
        assert!(signatures[0].is_some());
    }

    #[tokio::test]
    async fn private_local_status_route_returns_consensus_and_sync_state() {
        let (state, public_config, peer_private) = federation_route_state().await;
        let local_member = state
            .mint
            .federation_config()
            .expect("federation config")
            .member_id();
        state
            .submit_federation_envelope(restore_envelope(
                public_config.federation_id,
                "0204bbffa045f28ec836117a29ea0a00d77f1d692e38cf94f72a5145bfda6d8f41",
            ))
            .await
            .expect("submit restore");

        let request = signed_status_request(&public_config, &peer_private, local_member, 1);
        let response = post_local_status(State(state), Json(request))
            .await
            .expect("local status response")
            .0;

        assert_eq!(response.version, FEDERATION_REST_API_VERSION);
        assert_eq!(response.peer_status.member_id, local_member);
        assert_eq!(response.peer_status.operation_count, 1);
        assert_eq!(response.sync_state, "healthy");
        assert!(response.sync_ready);
        assert_eq!(response.sync_message, "federation member is healthy");
        assert_eq!(response.session_outcome_quorum_operation_count, None);
        assert!(response.session_outcome_quorum_signers.is_empty());
        assert!(response.payment_observation_reports.is_empty());
    }

    #[tokio::test]
    async fn private_local_status_route_reports_payment_observation_conflicts() {
        let (state, public_config, peer_private) = federation_route_state().await;
        let local_member = state
            .mint
            .federation_config()
            .expect("federation config")
            .member_id();
        let config = state
            .mint
            .federation_config()
            .expect("federation config")
            .clone();
        let quote_id = QuoteId::new();
        state
            .submit_federation_envelope(mint_quote_envelope(&state, &quote_id))
            .await
            .expect("submit mint quote");
        let first_response = state
            .submit_federation_envelope(mint_payment_observation_envelope_for_member(
                &state, &quote_id, 1, 1, "settled",
            ))
            .await
            .expect("submit first payment observation");
        let conflicting_response = state
            .submit_federation_envelope(mint_payment_observation_envelope_for_member(
                &state,
                &quote_id,
                2,
                2,
                "different-settlement",
            ))
            .await
            .expect("submit conflicting second payment observation");
        let quarantine_response = state
            .submit_federation_envelope(config.payment_observation_quarantine(
                cdk::federation::FederationPaymentObservationContext::MintQuote,
                quote_id.clone(),
                FederationPaymentObservationQuarantineReason::ConflictingEvidence,
                vec![
                    first_response.operation_id,
                    conflicting_response.operation_id,
                ],
            ))
            .await
            .expect("submit quarantine");

        let request = signed_status_request(&public_config, &peer_private, local_member, 1);
        let response = post_local_status(State(state), Json(request))
            .await
            .expect("local status response")
            .0;

        assert_eq!(response.payment_observation_reports.len(), 1);
        let report = &response.payment_observation_reports[0];
        assert_eq!(
            report.context,
            cdk::federation::FederationPaymentObservationContext::MintQuote
        );
        assert_eq!(report.quote, quote_id.to_string());
        assert_eq!(report.required, 1);
        assert_eq!(report.total_observations, 2);
        assert_eq!(
            report.status,
            cdk::federation::FederationPaymentObservationReportStatus::Conflicting
        );
        assert_eq!(report.groups.len(), 2);
        assert_eq!(report.groups[0].state, "PAID");
        assert_eq!(report.groups[0].observers, vec![member_id(1)]);
        assert!(report.groups[0].quorum_reached);
        assert_eq!(report.groups[0].conflicting_observers, vec![member_id(2)]);
        assert_eq!(
            report.conflicting_observers,
            vec![member_id(1), member_id(2)]
        );
        assert!(report.quarantined);
        assert_eq!(
            report.recommended_action,
            FederationPaymentObservationReportAction::Quarantine
        );
        assert_eq!(
            report.quarantine_operation_ids,
            vec![quarantine_response.operation_id]
        );
        assert_eq!(report.quarantine_operation_indices, vec![3]);
        assert_eq!(
            report.quarantine_reasons,
            vec![FederationPaymentObservationQuarantineReason::ConflictingEvidence]
        );

        let status_json = serde_json::to_string(&response).expect("status json");
        assert!(!status_json.contains("settled"));
        assert!(!status_json.contains("different-settlement"));
    }

    #[tokio::test]
    async fn private_local_status_route_reports_stale_payment_observations() {
        let (mut public_config, local_private, peer_private) = transport_config();
        public_config.payment_observation_policy =
            FederationPaymentObservationPolicy::member_quorum(
                2,
                FederationPaymentTrustModel::SharedBackendReadOnly,
            );
        public_config
            .payment_observation_policy
            .stale_observation_secs = 1;
        let state =
            federation_route_state_for_private(public_config.clone(), local_private.clone()).await;
        let local_member = local_private.member_id;
        let quote_id = QuoteId::new();
        let old_quote = MintQuote::new(
            Some(quote_id.clone()),
            format!("invoice-{quote_id}"),
            CurrencyUnit::Sat,
            Some(Amount::from(1).with_unit(CurrencyUnit::Sat)),
            10_000,
            PaymentIdentifier::QuoteId(quote_id.clone()),
            None,
            Amount::ZERO.with_unit(CurrencyUnit::Sat),
            Amount::ZERO.with_unit(CurrencyUnit::Sat),
            PaymentMethod::Known(KnownMethod::Bolt11),
            1,
            Vec::new(),
            Vec::new(),
            None,
        );
        let mut tx = state
            .mint
            .localstore()
            .begin_transaction()
            .await
            .expect("begin transaction");
        tx.add_mint_quote(old_quote).await.expect("add old quote");
        tx.commit().await.expect("commit old quote");

        state
            .submit_federation_envelope(mint_payment_observation_envelope_for_member(
                &state, &quote_id, 1, 1, "settled",
            ))
            .await
            .expect("submit first payment observation");

        let request = signed_status_request(&public_config, &peer_private, local_member, 1);
        let response = post_local_status(State(state), Json(request))
            .await
            .expect("local status response")
            .0;

        assert_eq!(response.payment_observation_reports.len(), 1);
        let report = &response.payment_observation_reports[0];
        assert_eq!(
            report.status,
            cdk::federation::FederationPaymentObservationReportStatus::AwaitingQuorum
        );
        assert_eq!(report.required, 2);
        assert_eq!(report.total_observations, 1);
        assert!(report.stale);
        assert!(report.quote_age_secs.expect("quote age") >= 1);
        assert_eq!(
            report.recommended_action,
            FederationPaymentObservationReportAction::Inspect
        );
        assert!(!report.groups[0].quorum_reached);

        let status_json = serde_json::to_string(&response).expect("status json");
        assert!(!status_json.contains("settled"));
    }

    #[tokio::test]
    async fn private_local_status_route_reports_session_outcome_quorum_evidence() {
        let (state, public_config, local_private, peer_private) =
            federation_route_state_with_local_private().await;
        let local_member = local_private.member_id;
        state
            .submit_federation_envelope(restore_envelope(
                public_config.federation_id,
                "0204bbffa045f28ec836117a29ea0a00d77f1d692e38cf94f72a5145bfda6d8f41",
            ))
            .await
            .expect("submit restore");
        let entries = state
            .mint
            .localstore()
            .get_federation_journal_entries()
            .await
            .expect("journal entries");
        let peer_status = local_peer_status(&state).await.expect("local status");
        let local_outcome = FederationSessionOutcome::new_signed(
            public_config.federation_id,
            &entries,
            peer_status.latest_order_digest,
            peer_status.state_digest,
            &local_private,
        )
        .expect("local session outcome");
        let peer_outcome = FederationSessionOutcome::new_signed(
            public_config.federation_id,
            &entries,
            peer_status.latest_order_digest,
            peer_status.state_digest,
            &peer_private,
        )
        .expect("peer session outcome");
        let mut tx = state
            .mint
            .localstore()
            .begin_transaction()
            .await
            .expect("begin transaction");
        tx.add_federation_session_outcome(&local_outcome)
            .await
            .expect("persist local outcome");
        tx.add_federation_session_outcome(&peer_outcome)
            .await
            .expect("persist peer outcome");
        tx.commit().await.expect("commit outcomes");

        let request = signed_status_request(&public_config, &peer_private, local_member, 1);
        let response = post_local_status(State(state), Json(request))
            .await
            .expect("local status response")
            .0;

        assert_eq!(response.peer_status.operation_count, 1);
        assert_eq!(response.session_outcome_quorum_operation_count, Some(1));
        assert_eq!(response.session_outcome_quorum_signers.len(), 2);
        assert!(response
            .session_outcome_quorum_signers
            .contains(&local_private.member_id));
        assert!(response
            .session_outcome_quorum_signers
            .contains(&peer_private.member_id));
    }

    #[tokio::test]
    async fn private_local_status_route_reports_lagging_sync_state() {
        let (state, public_config, peer_private) = federation_route_state().await;
        let local_member = state
            .mint
            .federation_config()
            .expect("federation config")
            .member_id();
        let peer_member = peer_private.member_id;
        state.mark_federation_lagging(peer_member, 0, 3).await;

        let request = signed_status_request(&public_config, &peer_private, local_member, 1);
        let response = post_local_status(State(state), Json(request))
            .await
            .expect("local status response")
            .0;

        assert_eq!(response.peer_status.operation_count, 0);
        assert_eq!(response.sync_state, "lagging");
        assert!(!response.sync_ready);
        assert!(response
            .sync_message
            .contains("federation member is lagging behind peer"));
    }

    #[tokio::test]
    async fn private_local_status_route_reports_verifying_session_outcome_state() {
        let (state, public_config, peer_private) = federation_route_state().await;
        let local_member = state
            .mint
            .federation_config()
            .expect("federation config")
            .member_id();
        let peer_member = peer_private.member_id;
        state
            .mark_federation_verifying_session_outcome(peer_member, 3)
            .await;

        let request = signed_status_request(&public_config, &peer_private, local_member, 1);
        let response = post_local_status(State(state), Json(request))
            .await
            .expect("local status response")
            .0;

        assert_eq!(response.peer_status.operation_count, 0);
        assert_eq!(response.sync_state, "verifying_session_outcome");
        assert!(!response.sync_ready);
        assert!(response
            .sync_message
            .contains("verifying quorum session outcome evidence"));
    }

    #[tokio::test]
    async fn private_journal_route_returns_signed_session_outcome() {
        let (state, public_config, peer_private) = federation_route_state().await;
        let local_member = state
            .mint
            .federation_config()
            .expect("federation config")
            .member_id();
        state
            .submit_federation_envelope(restore_envelope(
                public_config.federation_id,
                "0204bbffa045f28ec836117a29ea0a00d77f1d692e38cf94f72a5145bfda6d8f41",
            ))
            .await
            .expect("submit restore");
        let request = signed_catch_up_request(
            &public_config,
            &peer_private,
            local_member,
            1,
            FederationJournalCatchUpRequest::new(0),
        );

        let response = post_journal(State(state), Json(request))
            .await
            .expect("journal response")
            .0;

        assert_eq!(response.entries.len(), 1);
        assert!(response.session_outcome.is_some());
        response
            .verify_session_outcome(public_config.member(local_member).expect("local member"))
            .expect("session outcome verifies");
    }

    #[tokio::test]
    async fn private_dkg_setup_routes_accept_signed_peer_requests() {
        let (state, public_config, peer_private) = federation_route_state().await;
        let local_member = state
            .mint
            .federation_config()
            .expect("federation config")
            .member_id();
        let commitment_payload = dkg_commitment_request(&public_config, peer_private.member_id, 21);
        let expected_commitment = commitment_payload.commitment;
        let reveal_payload = dkg_reveal_request(&public_config, peer_private.member_id, 21);
        let expected_reveal = reveal_payload.reveal.clone();
        let secret_share_payload =
            dkg_secret_share_request(&public_config, peer_private.member_id, local_member, 21);
        let expected_secret_share = secret_share_payload.clone();
        let commitment_request = signed_dkg_commitment_request(
            &public_config,
            &peer_private,
            local_member,
            1,
            commitment_payload,
        );
        let reveal_request = signed_dkg_reveal_request(
            &public_config,
            &peer_private,
            local_member,
            2,
            reveal_payload,
        );
        let secret_share_request = signed_dkg_secret_share_request(
            &public_config,
            &peer_private,
            local_member,
            3,
            secret_share_payload,
        );

        let commitment_response =
            post_dkg_commitment(State(state.clone()), Json(commitment_request))
                .await
                .expect("commitment accepted")
                .0;
        let reveal_response = post_dkg_reveal(State(state.clone()), Json(reveal_request))
            .await
            .expect("reveal accepted")
            .0;
        let secret_share_response =
            post_dkg_secret_share(State(state.clone()), Json(secret_share_request))
                .await
                .expect("secret share accepted")
                .0;

        commitment_response
            .validate(
                public_config.federation_id,
                local_member,
                FederationRestMessageKind::DkgCommitment,
                peer_private.member_id,
            )
            .expect("commitment ack");
        reveal_response
            .validate(
                public_config.federation_id,
                local_member,
                FederationRestMessageKind::DkgReveal,
                peer_private.member_id,
            )
            .expect("reveal ack");
        secret_share_response
            .validate(
                public_config.federation_id,
                local_member,
                FederationRestMessageKind::DkgSecretShare,
                peer_private.member_id,
            )
            .expect("secret-share ack");
        let sequences = state
            .mint
            .localstore()
            .get_federation_member_sequences()
            .await
            .expect("persisted sequences");
        assert_eq!(sequences.get(&peer_private.member_id), Some(&3));
        let commitments = state
            .mint
            .localstore()
            .get_federation_dkg_commitments()
            .await
            .expect("persisted commitments");
        let reveals = state
            .mint
            .localstore()
            .get_federation_dkg_reveals()
            .await
            .expect("persisted reveals");
        let secret_shares = state
            .mint
            .localstore()
            .get_federation_dkg_secret_share_contributions()
            .await
            .expect("persisted secret-share contributions");
        assert_eq!(commitments, vec![expected_commitment]);
        assert_eq!(reveals, vec![expected_reveal]);
        assert_eq!(secret_shares, vec![expected_secret_share]);
    }

    #[tokio::test]
    async fn private_dkg_secret_share_route_rejects_wrong_receiver_before_sequence() {
        let (state, public_config, peer_private) = federation_route_state().await;
        let local_member = state
            .mint
            .federation_config()
            .expect("federation config")
            .member_id();
        let payload = dkg_secret_share_request(
            &public_config,
            peer_private.member_id,
            peer_private.member_id,
            21,
        );
        let request = signed_dkg_secret_share_request(
            &public_config,
            &peer_private,
            local_member,
            1,
            payload,
        );

        let response = post_dkg_secret_share(State(state.clone()), Json(request))
            .await
            .expect_err("wrong receiver must fail");

        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        let sequences = state
            .mint
            .localstore()
            .get_federation_member_sequences()
            .await
            .expect("persisted sequences");
        assert!(sequences.is_empty());
        let secret_shares = state
            .mint
            .localstore()
            .get_federation_dkg_secret_share_contributions()
            .await
            .expect("persisted secret-share contributions");
        assert!(secret_shares.is_empty());
    }

    #[tokio::test]
    async fn private_dkg_secret_share_route_rejects_conflict_before_sequence_persistence() {
        let (state, public_config, peer_private) = federation_route_state().await;
        let local_member = state
            .mint
            .federation_config()
            .expect("federation config")
            .member_id();
        let first_payload =
            dkg_secret_share_request(&public_config, peer_private.member_id, local_member, 21);
        let conflicting_payload =
            dkg_secret_share_request(&public_config, peer_private.member_id, local_member, 31);
        let first_request = signed_dkg_secret_share_request(
            &public_config,
            &peer_private,
            local_member,
            1,
            first_payload.clone(),
        );
        let conflicting_request = signed_dkg_secret_share_request(
            &public_config,
            &peer_private,
            local_member,
            2,
            conflicting_payload,
        );

        let _ = post_dkg_secret_share(State(state.clone()), Json(first_request))
            .await
            .expect("first secret share accepted");
        let response = post_dkg_secret_share(State(state.clone()), Json(conflicting_request))
            .await
            .expect_err("conflicting secret share must fail");

        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        let sequences = state
            .mint
            .localstore()
            .get_federation_member_sequences()
            .await
            .expect("persisted sequences");
        assert_eq!(sequences.get(&peer_private.member_id), Some(&1));
        let secret_shares = state
            .mint
            .localstore()
            .get_federation_dkg_secret_share_contributions()
            .await
            .expect("persisted secret-share contributions");
        assert_eq!(secret_shares, vec![first_payload]);
    }

    #[tokio::test]
    async fn private_dkg_secret_share_route_derives_member_keyset_share_when_ready() {
        let (state, public_config, local_private, peer_private) =
            federation_route_state_with_local_private().await;
        let local_member = local_private.member_id;
        persist_complete_dkg_round(&state, &public_config, &local_private, &peer_private).await;

        let local_contribution =
            dkg_secret_share_request(&public_config, local_member, local_member, 11);
        state
            .persist_federation_dkg_secret_share_contribution(&local_contribution, local_member, 3)
            .await
            .expect("persist local secret-share contribution");
        assert!(
            state
                .mint
                .localstore()
                .get_federation_dkg_member_keyset_shares()
                .await
                .expect("member keyset shares before final contribution")
                .is_empty(),
            "local keyset share should wait for every participant contribution"
        );

        let peer_contribution =
            dkg_secret_share_request(&public_config, peer_private.member_id, local_member, 21);
        let request = signed_dkg_secret_share_request(
            &public_config,
            &peer_private,
            local_member,
            3,
            peer_contribution.clone(),
        );

        let response = post_dkg_secret_share(State(state.clone()), Json(request))
            .await
            .expect("peer secret share accepted")
            .0;

        response
            .validate(
                public_config.federation_id,
                local_member,
                FederationRestMessageKind::DkgSecretShare,
                peer_private.member_id,
            )
            .expect("secret-share ack");
        let shares = state
            .mint
            .localstore()
            .get_federation_dkg_member_keyset_shares()
            .await
            .expect("member keyset shares");
        assert_eq!(shares.len(), 1);
        assert_eq!(shares[0].member_id, local_member);
        assert_eq!(shares[0].secret_shares.len(), 1);
        shares[0]
            .keyset
            .validate(public_config.threshold_params, &public_config.members)
            .expect("derived keyset validates");
        let contributions = state
            .mint
            .localstore()
            .get_federation_dkg_secret_share_contributions()
            .await
            .expect("stored secret-share contributions");
        assert_eq!(contributions, vec![local_contribution, peer_contribution]);
        let sequences = state
            .mint
            .localstore()
            .get_federation_member_sequences()
            .await
            .expect("persisted sequences");
        assert_eq!(sequences.get(&local_member), Some(&3));
        assert_eq!(sequences.get(&peer_private.member_id), Some(&3));
    }

    #[tokio::test]
    async fn private_dkg_secret_share_route_rejects_bad_share_during_completion() {
        let (state, public_config, local_private, peer_private) =
            federation_route_state_with_local_private().await;
        let local_member = local_private.member_id;
        persist_complete_dkg_round(&state, &public_config, &local_private, &peer_private).await;
        let local_contribution =
            dkg_secret_share_request(&public_config, local_member, local_member, 11);
        state
            .persist_federation_dkg_secret_share_contribution(&local_contribution, local_member, 3)
            .await
            .expect("persist local secret-share contribution");

        let bad_peer_contribution =
            dkg_secret_share_request(&public_config, peer_private.member_id, local_member, 31);
        let request = signed_dkg_secret_share_request(
            &public_config,
            &peer_private,
            local_member,
            3,
            bad_peer_contribution,
        );

        let response = post_dkg_secret_share(State(state.clone()), Json(request))
            .await
            .expect_err("bad secret share must fail");

        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        let shares = state
            .mint
            .localstore()
            .get_federation_dkg_member_keyset_shares()
            .await
            .expect("member keyset shares");
        assert!(shares.is_empty());
        let contributions = state
            .mint
            .localstore()
            .get_federation_dkg_secret_share_contributions()
            .await
            .expect("stored secret-share contributions");
        assert_eq!(contributions, vec![local_contribution]);
        let sequences = state
            .mint
            .localstore()
            .get_federation_member_sequences()
            .await
            .expect("persisted sequences");
        assert_eq!(sequences.get(&local_member), Some(&3));
        assert_eq!(sequences.get(&peer_private.member_id), Some(&2));
    }

    #[tokio::test]
    async fn federation_restart_activates_route_derived_dkg_records() {
        let (trusted_public_config, local_private, peer_private) = transport_config();
        let mut setup_public_config = trusted_public_config.clone();
        setup_public_config.keysets.clear();
        setup_public_config.setup_transcript = None;
        setup_public_config.validate().expect("setup public config");

        let mut setup_local_private = local_private.clone();
        setup_local_private.keyset_secret_shares.clear();
        setup_local_private.setup_transcript_hash = None;
        setup_local_private
            .validate_against_public(&setup_public_config)
            .expect("setup local private config");
        let mut setup_peer_private = peer_private.clone();
        setup_peer_private.keyset_secret_shares.clear();
        setup_peer_private.setup_transcript_hash = None;

        let db = Arc::new(cdk_sqlite::mint::memory::empty().await.expect("mint db"));
        let mnemonic = Mnemonic::generate(12).expect("mnemonic");
        let seed = mnemonic.to_seed_normalized("");
        let state = federation_route_state_for_private_with_db(
            setup_public_config.clone(),
            setup_local_private.clone(),
            db.clone(),
            &seed,
        )
        .await;
        let local_member = setup_local_private.member_id;
        persist_complete_dkg_round(
            &state,
            &setup_public_config,
            &setup_local_private,
            &setup_peer_private,
        )
        .await;
        let local_contribution =
            dkg_secret_share_request(&setup_public_config, local_member, local_member, 11);
        state
            .persist_federation_dkg_secret_share_contribution(&local_contribution, local_member, 3)
            .await
            .expect("persist local secret-share contribution");
        let peer_contribution = dkg_secret_share_request(
            &setup_public_config,
            setup_peer_private.member_id,
            local_member,
            21,
        );
        let peer_request = signed_dkg_secret_share_request(
            &setup_public_config,
            &setup_peer_private,
            local_member,
            3,
            peer_contribution,
        );
        let _ = post_dkg_secret_share(State(state.clone()), Json(peer_request))
            .await
            .expect("peer secret-share accepted");

        let keyset_share = state
            .mint
            .localstore()
            .get_federation_dkg_member_keyset_shares()
            .await
            .expect("route-derived keyset shares")
            .into_iter()
            .next()
            .expect("route-derived keyset share");
        let mut final_public_config = FederationConfig::new(
            setup_public_config.threshold_params,
            setup_public_config.members.clone(),
            vec![keyset_share.keyset.clone()],
        )
        .expect("final public config");
        let transcript_hash = FederationDkgTranscript::hash_for_config(
            FederationDkgSetupMethod::PedersenDkg,
            &final_public_config,
        )
        .expect("transcript hash");
        let transcript_signatures = [&setup_local_private, &setup_peer_private]
            .into_iter()
            .map(|private_config| {
                FederationDkgTranscriptSignatureRequest::new(
                    final_public_config.federation_id,
                    private_config.member_id,
                    transcript_hash,
                    FederationDkgTranscriptSignature::sign(transcript_hash, private_config)
                        .expect("transcript signature"),
                )
            })
            .collect::<Vec<_>>();
        let transcript = FederationDkgTranscript::from_signatures(
            FederationDkgSetupMethod::PedersenDkg,
            &final_public_config,
            transcript_signatures
                .iter()
                .map(|signature| (signature.signer, signature.signature))
                .collect(),
        )
        .expect("assembled transcript");
        final_public_config.setup_transcript = Some(transcript);
        final_public_config
            .validate()
            .expect("final public config with transcript");

        let mut tx = state
            .mint
            .localstore()
            .begin_transaction()
            .await
            .expect("begin transcript signatures");
        for signature in &transcript_signatures {
            tx.add_federation_dkg_transcript_signature(signature)
                .await
                .expect("persist transcript signature");
        }
        tx.commit().await.expect("commit transcript signatures");

        let restarted = federation_route_state_for_private_with_db(
            setup_public_config,
            setup_local_private,
            db,
            &seed,
        )
        .await;
        let runtime_config = restarted
            .mint
            .federation_config()
            .expect("activated federation runtime config");
        assert_eq!(
            runtime_config.public_config().keysets,
            final_public_config.keysets
        );
        assert_eq!(
            runtime_config.public_config().setup_transcript,
            final_public_config.setup_transcript
        );
        assert_eq!(
            runtime_config
                .private_config()
                .keyset_secret_shares
                .get(&keyset_share.keyset.id),
            Some(&keyset_share.secret_shares)
        );
    }

    #[tokio::test]
    async fn private_dkg_transcript_signature_route_accepts_signed_peer_request() {
        let (state, public_config, peer_private) = federation_route_state().await;
        let local_member = state
            .mint
            .federation_config()
            .expect("federation config")
            .member_id();
        let payload = dkg_transcript_signature_request(&public_config, &peer_private);
        let request = signed_dkg_transcript_signature_request(
            &public_config,
            &peer_private,
            local_member,
            1,
            payload,
        );

        let response = post_dkg_transcript_signature(State(state.clone()), Json(request))
            .await
            .expect("transcript signature accepted")
            .0;

        response
            .validate(
                public_config.federation_id,
                local_member,
                FederationRestMessageKind::DkgTranscriptSignature,
                peer_private.member_id,
            )
            .expect("transcript signature ack");
        let sequences = state
            .mint
            .localstore()
            .get_federation_member_sequences()
            .await
            .expect("persisted sequences");
        assert_eq!(sequences.get(&peer_private.member_id), Some(&1));
        let signatures = state
            .mint
            .localstore()
            .get_federation_dkg_transcript_signatures()
            .await
            .expect("persisted transcript signatures");
        assert_eq!(signatures, vec![payload]);
    }

    #[tokio::test]
    async fn private_dkg_transcript_signature_route_rejects_bad_signature_before_sequence() {
        let (state, public_config, local_private, peer_private) =
            federation_route_state_with_local_private().await;
        let local_member = local_private.member_id;
        let transcript_hash = public_config
            .setup_transcript
            .as_ref()
            .expect("setup transcript")
            .transcript_hash;
        let bad_signature = cdk::federation::FederationDkgTranscriptSignature::sign(
            transcript_hash,
            &local_private,
        )
        .expect("bad transcript signature");
        let payload = FederationDkgTranscriptSignatureRequest::new(
            public_config.federation_id,
            peer_private.member_id,
            transcript_hash,
            bad_signature,
        );
        let request = signed_dkg_transcript_signature_request(
            &public_config,
            &peer_private,
            local_member,
            1,
            payload,
        );

        let response = post_dkg_transcript_signature(State(state.clone()), Json(request))
            .await
            .expect_err("bad transcript signature must fail");

        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        let sequences = state
            .mint
            .localstore()
            .get_federation_member_sequences()
            .await
            .expect("persisted sequences");
        assert!(sequences.is_empty());
        let signatures = state
            .mint
            .localstore()
            .get_federation_dkg_transcript_signatures()
            .await
            .expect("persisted transcript signatures");
        assert!(signatures.is_empty());
    }

    #[tokio::test]
    async fn private_dkg_setup_route_rejects_sender_body_mismatch_before_sequence_persistence() {
        let (state, public_config, peer_private) = federation_route_state().await;
        let local_member = state
            .mint
            .federation_config()
            .expect("federation config")
            .member_id();
        let mismatched_request = signed_dkg_reveal_request(
            &public_config,
            &peer_private,
            local_member,
            1,
            dkg_reveal_request(&public_config, local_member, 11),
        );

        let response = post_dkg_reveal(State(state.clone()), Json(mismatched_request))
            .await
            .expect_err("mismatched setup request must fail");

        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        let sequences = state
            .mint
            .localstore()
            .get_federation_member_sequences()
            .await
            .expect("persisted sequences");
        assert!(sequences.is_empty());
    }

    #[tokio::test]
    async fn private_dkg_reveal_route_rejects_missing_commitment_before_sequence_persistence() {
        let (state, public_config, peer_private) = federation_route_state().await;
        let local_member = state
            .mint
            .federation_config()
            .expect("federation config")
            .member_id();
        let reveal_request = signed_dkg_reveal_request(
            &public_config,
            &peer_private,
            local_member,
            1,
            dkg_reveal_request(&public_config, peer_private.member_id, 21),
        );

        let response = post_dkg_reveal(State(state.clone()), Json(reveal_request))
            .await
            .expect_err("reveal before commitment must fail");

        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        let sequences = state
            .mint
            .localstore()
            .get_federation_member_sequences()
            .await
            .expect("persisted sequences");
        let reveals = state
            .mint
            .localstore()
            .get_federation_dkg_reveals()
            .await
            .expect("persisted reveals");
        assert!(sequences.is_empty());
        assert!(reveals.is_empty());
    }

    #[tokio::test]
    async fn private_dkg_commitment_route_rejects_conflict_before_sequence_persistence() {
        let (state, public_config, peer_private) = federation_route_state().await;
        let local_member = state
            .mint
            .federation_config()
            .expect("federation config")
            .member_id();
        let first_payload = dkg_commitment_request(&public_config, peer_private.member_id, 21);
        let first_commitment = first_payload.commitment;
        let first_request = signed_dkg_commitment_request(
            &public_config,
            &peer_private,
            local_member,
            1,
            first_payload,
        );
        let conflicting_request = signed_dkg_commitment_request(
            &public_config,
            &peer_private,
            local_member,
            2,
            dkg_commitment_request(&public_config, peer_private.member_id, 31),
        );

        let _ = post_dkg_commitment(State(state.clone()), Json(first_request))
            .await
            .expect("first commitment accepted");
        let response = post_dkg_commitment(State(state.clone()), Json(conflicting_request))
            .await
            .expect_err("conflicting commitment must fail");

        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        let sequences = state
            .mint
            .localstore()
            .get_federation_member_sequences()
            .await
            .expect("persisted sequences");
        let commitments = state
            .mint
            .localstore()
            .get_federation_dkg_commitments()
            .await
            .expect("persisted commitments");
        assert_eq!(sequences.get(&peer_private.member_id), Some(&1));
        assert_eq!(commitments, vec![first_commitment]);
    }

    #[tokio::test]
    async fn private_operation_route_accepts_signed_peer_request() {
        let (state, public_config, peer_private) = federation_route_state().await;
        let envelope = restore_envelope(
            public_config.federation_id,
            "0204bbffa045f28ec836117a29ea0a00d77f1d692e38cf94f72a5145bfda6d8f41",
        );
        let request = signed_operation_request(
            &public_config,
            &peer_private,
            1,
            FederationRestMessageKind::SubmitOperation,
            envelope.clone(),
        );

        let response = post_operation(State(state.clone()), Json(request))
            .await
            .expect("operation response");

        assert_eq!(
            response.0.operation_id,
            envelope.operation_id().expect("operation id")
        );
        assert_eq!(consensus_operation_count(&state).await, 1);
        let persisted = state
            .mint
            .localstore()
            .get_federation_journal_entries()
            .await
            .expect("persisted journal");
        let sequences = state
            .mint
            .localstore()
            .get_federation_member_sequences()
            .await
            .expect("persisted sequences");
        assert_eq!(persisted.len(), 1);
        assert_eq!(persisted[0].operation_id, response.0.operation_id);
        assert_eq!(sequences.get(&peer_private.member_id), Some(&1));
    }

    #[tokio::test]
    async fn private_operation_route_rejects_oversized_request_before_mutation() {
        let (state, public_config, peer_private) = federation_route_state().await;
        let request = signed_operation_request(
            &public_config,
            &peer_private,
            1,
            FederationRestMessageKind::SubmitOperation,
            oversized_payment_observation_envelope(public_config.federation_id),
        );

        let response = post_operation(State(state.clone()), Json(request))
            .await
            .expect_err("oversized request must fail");

        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        assert_eq!(consensus_operation_count(&state).await, 0);
        let sequences = state
            .mint
            .localstore()
            .get_federation_member_sequences()
            .await
            .expect("persisted sequences");
        assert!(sequences.is_empty());
    }

    #[tokio::test]
    async fn private_operation_route_rejects_tampered_payload_before_mutation() {
        let (state, public_config, peer_private) = federation_route_state().await;
        let first = restore_envelope(
            public_config.federation_id,
            "0204bbffa045f28ec836117a29ea0a00d77f1d692e38cf94f72a5145bfda6d8f41",
        );
        let second = restore_envelope(
            public_config.federation_id,
            "025f0615ccba96f810582a6885ffdb04bd57c96dbc590f5aa560447b31258988d7",
        );
        let mut request = signed_operation_request(
            &public_config,
            &peer_private,
            1,
            FederationRestMessageKind::SubmitOperation,
            first,
        );
        request.payload = FederationSubmitOperationRequest::new(second);

        let response = post_operation(State(state.clone()), Json(request))
            .await
            .expect_err("tampered request must fail");

        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        assert_eq!(consensus_operation_count(&state).await, 0);
    }

    #[tokio::test]
    async fn private_operation_route_rejects_unknown_sender_before_mutation() {
        let (state, public_config, _) = federation_route_state().await;
        let unknown_private = private_member(3);
        let envelope = restore_envelope(
            public_config.federation_id,
            "0204bbffa045f28ec836117a29ea0a00d77f1d692e38cf94f72a5145bfda6d8f41",
        );
        let request = signed_operation_request(
            &public_config,
            &unknown_private,
            1,
            FederationRestMessageKind::SubmitOperation,
            envelope,
        );

        let response = post_operation(State(state.clone()), Json(request))
            .await
            .expect_err("unknown sender must fail");

        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        assert_eq!(consensus_operation_count(&state).await, 0);
    }

    #[tokio::test]
    async fn private_operation_route_rejects_kind_mismatch_before_mutation() {
        let (state, public_config, peer_private) = federation_route_state().await;
        let envelope = restore_envelope(
            public_config.federation_id,
            "0204bbffa045f28ec836117a29ea0a00d77f1d692e38cf94f72a5145bfda6d8f41",
        );
        let request = signed_operation_request(
            &public_config,
            &peer_private,
            1,
            FederationRestMessageKind::Status,
            envelope,
        );

        let response = post_operation(State(state.clone()), Json(request))
            .await
            .expect_err("wrong message kind must fail");

        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        assert_eq!(consensus_operation_count(&state).await, 0);
    }

    #[tokio::test]
    async fn private_operation_route_rejects_stale_sequence_before_second_mutation() {
        let (state, public_config, peer_private) = federation_route_state().await;
        let first = restore_envelope(
            public_config.federation_id,
            "0204bbffa045f28ec836117a29ea0a00d77f1d692e38cf94f72a5145bfda6d8f41",
        );
        let second = restore_envelope(
            public_config.federation_id,
            "025f0615ccba96f810582a6885ffdb04bd57c96dbc590f5aa560447b31258988d7",
        );
        let first_request = signed_operation_request(
            &public_config,
            &peer_private,
            1,
            FederationRestMessageKind::SubmitOperation,
            first,
        );
        let second_request = signed_operation_request(
            &public_config,
            &peer_private,
            1,
            FederationRestMessageKind::SubmitOperation,
            second,
        );

        let _ = post_operation(State(state.clone()), Json(first_request))
            .await
            .expect("first operation accepted");
        let response = post_operation(State(state.clone()), Json(second_request))
            .await
            .expect_err("stale sequence must fail");

        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        assert_eq!(consensus_operation_count(&state).await, 1);
    }
}
