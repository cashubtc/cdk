//! Private federation member REST routes.

use std::collections::{BTreeMap, BTreeSet};
use std::str::FromStr;

use axum::extract::{DefaultBodyLimit, Json, State};
use axum::http::{header, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use axum::Router;
use cdk::federation::{
    FederatedKeyset, FederationError, FederationJournalCatchUpResponse, FederationJournalDigest,
    FederationJournalEntry, FederationMaterializedStateRow,
    FederationMaterializedStateRowFingerprint, FederationMaterializedStateSnapshot,
    FederationOperation, FederationOperationId, FederationOperationStatus,
    FederationPaymentObservationRecord, FederationPeerStatus, FederationRestMessageKind,
    FederationRestSequenceTracker, FederationSessionOutcome, FederationSessionOutcomeQuorum,
    FederationSignedRequest, FederationStateDigest, FederationSubmitOperationRequest,
    FederationSubmitOperationResponse, FederationVerifiedOperationAdmission,
    InMemoryFederationJournal, FEDERATION_MAX_DKG_REQUEST_BODY_BYTES, FEDERATION_REST_API_VERSION,
};
use serde::Serialize;
use sha2::{Digest, Sha256};
use tracing::instrument;
use uuid::Uuid;

use crate::MintState;

pub(crate) mod aleph_bft_latency;
mod aleph_bft_network;
mod aleph_bft_routes;
mod aleph_bft_runtime;
mod aleph_bft_session;
mod catch_up_routes;
mod dkg_driver;
mod dkg_routes;
mod node_runtime;
mod payment_observation;
mod peer_client;
mod status_routes;
mod sync_config;
mod sync_loop;

pub(crate) use self::aleph_bft_network::CdkAlephBftNetworkInboundQueue;
use self::aleph_bft_routes::{post_aleph_bft_activity, post_aleph_bft_network_message};
#[cfg(test)]
use self::aleph_bft_session::{
    federation_aleph_bft_live_session_startup_decision,
    rotate_federation_aleph_bft_live_session_to_next_empty_with_transport,
    start_federation_aleph_bft_live_session_with_config,
    start_federation_aleph_bft_live_session_with_options_and_transport,
    start_federation_aleph_bft_live_session_with_transport, FederationAlephBftLiveSessionOptions,
    FederationAlephBftLiveSessionStartupDecision,
};
pub(crate) use self::aleph_bft_session::{
    start_federation_aleph_bft_live_session, FederationAlephBftLiveSessionHandle,
};
use self::catch_up_routes::post_journal;
pub(crate) use self::dkg_driver::spawn_federation_dkg_driver;
#[cfg(test)]
use self::dkg_driver::{
    complete_federation_dkg_ceremony, federation_dkg_readiness_required,
    prepare_federation_dkg_participant_state, run_federation_dkg_driver_tick,
    run_federation_dkg_readiness_round, FederationDkgDeliveryTracker,
};
pub use self::dkg_driver::{
    run_federation_dkg_driver, FederationDkgAutoStartConfig, FederationDkgDriverRunConfig,
    FederationDkgDriverRunOutcome, FederationDkgDriverTickOutcome,
    FederationDkgSecretShareTransportPolicy,
};
use self::dkg_routes::{
    post_dkg_activation, post_dkg_commitment, post_dkg_readiness, post_dkg_reveal,
    post_dkg_secret_share, post_dkg_transcript_signature,
};
pub(crate) use self::node_runtime::FederationNodeRuntime;
use self::payment_observation::payment_observation_report_summaries;
#[cfg(test)]
use self::peer_client::FederationRestPeerClient;
#[cfg(test)]
pub(crate) use self::status_routes::FederationLocalStatusResponse;
use self::status_routes::{post_local_status, post_status};
#[cfg(test)]
use self::sync_config::FederationPeerReconnectBackoff;
pub use self::sync_config::FederationPeerSyncConfig;
#[cfg(test)]
use self::sync_config::{
    federation_peer_sync_disabled_value, federation_peer_sync_initial_delay,
    federation_peer_sync_interval, federation_reconnect_backoff_delay,
};
pub(crate) use self::sync_loop::spawn_federation_peer_sync_loop_with_config;
#[cfg(test)]
use self::sync_loop::{
    persist_local_session_outcome_evidence_chunks, run_federation_peer_sync_round,
    run_federation_peer_sync_round_with_backoff,
};
#[cfg(test)]
use cdk::federation::{
    FederationConfig, FederationDkgCommitmentRequest, FederationDkgReadinessRequest,
    FederationDkgRevealRequest, FederationDkgSecretShareRequest, FederationDkgSetupResponse,
    FederationDkgTranscriptSignatureRequest, FederationId, FederationJournalCatchUpRequest,
    FederationPeerStatusRequest,
};
#[cfg(test)]
use cdk_http_client::Transport;
#[cfg(test)]
use tokio::time::{self, Duration};

type SequenceHandle = std::sync::Arc<tokio::sync::Mutex<FederationRestSequenceTracker>>;

const FEDERATION_DISABLE_PEER_SYNC_ENV_VAR: &str = "CDK_FEDERATION_DISABLE_PEER_SYNC";

pub(crate) fn create_federation_router() -> Router<MintState> {
    let dkg_router = Router::new()
        .route("/dkg/readiness", post(post_dkg_readiness))
        .route("/dkg/commitments", post(post_dkg_commitment))
        .route("/dkg/reveals", post(post_dkg_reveal))
        .route("/dkg/secret-shares", post(post_dkg_secret_share))
        .route(
            "/dkg/transcript-signatures",
            post(post_dkg_transcript_signature),
        )
        .route("/dkg/activations", post(post_dkg_activation))
        .layer(DefaultBodyLimit::max(FEDERATION_MAX_DKG_REQUEST_BODY_BYTES));

    Router::new()
        .merge(dkg_router)
        .route("/status", post(post_status))
        .route("/local-status", post(post_local_status))
        .route("/operations", post(post_operation))
        .route("/aleph-bft/activity", post(post_aleph_bft_activity))
        .route("/aleph-bft/messages", post(post_aleph_bft_network_message))
        .route("/journal", post(post_journal))
}

pub(crate) async fn get_public_config(
    State(state): State<MintState>,
) -> Result<Response, Response> {
    let runtime_config = state.mint.federation_config().ok_or_else(|| {
        (
            StatusCode::NOT_FOUND,
            "mint is not configured as a federation member",
        )
            .into_response()
    })?;
    let public_config = runtime_config.public_config();

    if let Err(err) = public_config.validate() {
        tracing::error!(%err, "Refusing to advertise an invalid federation config");
        return Err((
            StatusCode::SERVICE_UNAVAILABLE,
            "federation public config is unavailable",
        )
            .into_response());
    }

    if public_config.setup_transcript.is_none()
        || !public_config.keysets.iter().any(|keyset| keyset.active)
    {
        return Err((
            StatusCode::SERVICE_UNAVAILABLE,
            "federation setup is not finalized",
        )
            .into_response());
    }

    let mut response = Json(public_config.clone()).into_response();
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    Ok(response)
}

fn acquire_private_rest_admission(
    state: &MintState,
    kind: FederationRestMessageKind,
) -> Result<Option<tokio::sync::OwnedSemaphorePermit>, Response> {
    let Some(admission) = state.federation.private_rest_admission.as_ref() else {
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

fn acquire_private_dkg_member_admission(
    state: &MintState,
    sender: cdk::federation::MemberId,
    kind: FederationRestMessageKind,
) -> Result<Option<tokio::sync::OwnedSemaphorePermit>, Response> {
    let Some(admissions) = state.federation.dkg_member_admission.as_ref() else {
        if state.federation.dkg_auto_start_config.is_none() {
            return Ok(None);
        }
        tracing::error!(
            member_id = %sender,
            request_kind = ?kind,
            "Federation DKG member admission is unavailable for an active ceremony"
        );
        return Err((
            StatusCode::SERVICE_UNAVAILABLE,
            "federation DKG member admission is unavailable",
        )
            .into_response());
    };
    let Some(admission) = admissions.get(&sender) else {
        tracing::error!(
            member_id = %sender,
            request_kind = ?kind,
            "Federation DKG member admission is missing"
        );
        return Err((
            StatusCode::SERVICE_UNAVAILABLE,
            "federation DKG member admission is unavailable",
        )
            .into_response());
    };
    let permit = match admission.in_flight.clone().try_acquire_owned() {
        Ok(permit) => Ok(Some(permit)),
        Err(tokio::sync::TryAcquireError::NoPermits) => {
            tracing::warn!(
                member_id = %sender,
                request_kind = ?kind,
                "Federation DKG member request is already in progress"
            );
            Err((
                StatusCode::TOO_MANY_REQUESTS,
                "federation DKG member request is already in progress",
            )
                .into_response())
        }
        Err(tokio::sync::TryAcquireError::Closed) => {
            tracing::error!(
                member_id = %sender,
                request_kind = ?kind,
                "Federation DKG member admission is closed"
            );
            Err((
                StatusCode::SERVICE_UNAVAILABLE,
                "federation DKG member admission is closed",
            )
                .into_response())
        }
    }?;
    let allowed = match admission.rate_limit.lock() {
        Ok(mut rate_limit) => rate_limit.try_acquire_at(std::time::Instant::now()),
        Err(_) => {
            tracing::error!(
                member_id = %sender,
                request_kind = ?kind,
                "Federation DKG member rate admission is unavailable"
            );
            return Err((
                StatusCode::SERVICE_UNAVAILABLE,
                "federation DKG member admission is unavailable",
            )
                .into_response());
        }
    };
    if !allowed {
        tracing::warn!(
            member_id = %sender,
            request_kind = ?kind,
            "Federation DKG member request rate limit exceeded"
        );
        return Err((
            StatusCode::TOO_MANY_REQUESTS,
            "federation DKG member request rate limit exceeded",
        )
            .into_response());
    }

    Ok(permit)
}

#[instrument(skip_all)]
async fn post_operation(
    State(state): State<MintState>,
    Json(payload): Json<FederationSignedRequest<FederationSubmitOperationRequest>>,
) -> Result<Json<FederationSubmitOperationResponse>, Response> {
    let _permit =
        acquire_private_rest_admission(&state, FederationRestMessageKind::SubmitOperation)?;
    let admission = verify_operation_admission(&state, &payload).await?;
    let sequence = Some(admission.sender_sequence());
    let envelope = admission.operation_envelope().clone();
    let response = state
        .submit_federation_envelope_with_sequence(envelope, sequence)
        .await
        .map_err(cdk_error_response)?;
    tracing::debug!("Accepted federation operation {}", response.operation_id);
    Ok(Json(response))
}

async fn verify_operation_admission(
    state: &MintState,
    request: &FederationSignedRequest<FederationSubmitOperationRequest>,
) -> Result<FederationVerifiedOperationAdmission, Response> {
    let runtime_config = federation_runtime_config(state)?;
    let admission = FederationVerifiedOperationAdmission::from_signed_submit_request(
        request,
        runtime_config.public_config(),
        runtime_config.member_id(),
    )
    .map_err(federation_error_response)?;

    let sequences = federation_sequences(state)?;
    let sequences = sequences.lock().await;
    sequences
        .verify_next_sequence(admission.sender, admission.sequence)
        .map_err(federation_error_response)?;

    Ok(admission)
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
            aleph_bft_finalized_items: Vec::new(),
            aleph_bft_finalized_items_complete: true,
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

pub(crate) async fn session_outcome_quorum_operation_count(
    state: &MintState,
) -> Result<Option<u64>, cdk::Error> {
    Ok(session_outcome_quorum_status(state)
        .await?
        .map(|quorum| quorum.operation_count))
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
        .federation
        .sequences
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

async fn local_peer_status(state: &MintState) -> anyhow::Result<FederationPeerStatus> {
    let (mut status, entries) = local_peer_status_and_entries(state).await?;
    status.state_digest = materialized_state_digest(state, &entries).await?;
    Ok(status)
}

async fn local_peer_order_status(state: &MintState) -> anyhow::Result<FederationPeerStatus> {
    let runtime_config = state
        .mint
        .federation_config()
        .ok_or_else(|| anyhow::anyhow!("mint is not configured as a federation member"))?;
    let tip = state.mint.localstore().get_federation_journal_tip().await?;
    let mut status = match tip {
        Some(tip) => {
            let operation_count = tip
                .index
                .checked_add(1)
                .ok_or_else(|| anyhow::anyhow!("federation journal index overflow"))?;
            FederationPeerStatus::new_with_state_digest(
                runtime_config.federation_id(),
                runtime_config.member_id(),
                operation_count,
                Some(tip.index),
                tip.order_digest,
                tip.state_digest,
            )
        }
        None => FederationPeerStatus::new_with_state_digest(
            runtime_config.federation_id(),
            runtime_config.member_id(),
            0,
            None,
            cdk::federation::FederationJournalDigest::genesis(),
            FederationStateDigest::genesis(),
        ),
    };
    let next_item_index = local_finalized_item_cursor(state).await?;
    status = status.with_aleph_bft_next_item_index(next_item_index);
    Ok(status)
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
    let mut status = FederationPeerStatus::from_journal(
        runtime_config.federation_id(),
        runtime_config.member_id(),
        &journal,
    );
    let next_item_index = local_finalized_item_cursor(state).await?;
    status = status.with_aleph_bft_next_item_index(next_item_index);
    let entries = journal.entries().to_vec();
    Ok((status, entries))
}

async fn local_finalized_item_cursor(state: &MintState) -> anyhow::Result<u64> {
    if let Some(status) = state.federation_aleph_bft_status().await {
        return Ok(status.next_item_index);
    }

    #[cfg(test)]
    return Ok(0);

    #[cfg(not(test))]
    anyhow::bail!("AlephBFT consensus status is unavailable")
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
    let mut completed_operations = BTreeSet::new();
    let mut proof_ys = BTreeSet::new();
    let mut blinded_messages = BTreeSet::new();
    let mut keyset_rotations = Vec::new();

    for entry in entries {
        if let Some(rejection) = &entry.rejection {
            snapshot.push(
                FederationMaterializedStateRow::new(
                    "federation_operation_rejections",
                    entry.operation_id.to_string(),
                )
                .with_field("operation_index", entry.index.to_string())
                .with_field("error_code", rejection.code.to_string())
                .with_field("detail", rejection.detail.clone()),
            );
        }
        if entry.status == FederationOperationStatus::Rejected {
            continue;
        }

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
                completed_operations.insert(entry.operation_id);
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
                completed_operations.insert(entry.operation_id);
                melt_quotes.insert(operation.request.quote().clone());
                for proof in operation.request.inputs() {
                    proof_ys.insert(proof.y()?);
                }
                if let Some(outputs) = operation.request.outputs() {
                    blinded_messages.extend(outputs.iter().map(|output| output.blinded_secret));
                }
            }
            FederationOperation::Swap(operation) => {
                completed_operations.insert(entry.operation_id);
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
            FederationOperation::KeysetRotation(operation) => {
                keyset_rotations.push((entry.index, entry.operation_id, operation.keyset.clone()));
            }
        }
    }

    for operation_id in completed_operations {
        let operation_uuid = federation_completed_operation_uuid(operation_id);
        match db.get_completed_operation(&operation_uuid).await? {
            Some(operation) => snapshot.push(completed_operation_materialized_row(
                operation_id,
                operation_uuid,
                &operation,
            )),
            None => snapshot.push(
                FederationMaterializedStateRow::new(
                    "completed_operations",
                    operation_uuid.to_string(),
                )
                .with_field("federation_operation_id", operation_id.to_string())
                .with_field("missing", "true"),
            ),
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

fn federation_completed_operation_uuid(operation_id: FederationOperationId) -> Uuid {
    let bytes = operation_id.to_bytes();
    let mut uuid = [0_u8; 16];
    uuid.copy_from_slice(&bytes[..16]);
    Uuid::from_bytes(uuid)
}

fn completed_operation_materialized_row(
    operation_id: FederationOperationId,
    operation_uuid: Uuid,
    operation: &cdk_common::mint::Operation,
) -> FederationMaterializedStateRow {
    FederationMaterializedStateRow::new("completed_operations", operation_uuid.to_string())
        .with_field("federation_operation_id", operation_id.to_string())
        .with_field("kind", operation.kind().to_string())
        .with_field(
            "total_issued",
            operation.total_issued().to_u64().to_string(),
        )
        .with_field(
            "total_redeemed",
            operation.total_redeemed().to_u64().to_string(),
        )
        .with_field(
            "fee_collected",
            operation.fee_collected().to_u64().to_string(),
        )
        .with_field(
            "completed_at",
            operation
                .completed_at()
                .map(|completed_at| completed_at.to_string())
                .unwrap_or_default(),
        )
        .with_field(
            "payment_method",
            operation
                .payment_method()
                .map(|method| method.to_string())
                .unwrap_or_default(),
        )
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
