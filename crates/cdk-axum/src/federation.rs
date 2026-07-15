//! Federation REST routes.

use std::collections::{BTreeMap, BTreeSet};

use axum::extract::{DefaultBodyLimit, Json, State};
use axum::http::{header, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::Router;
#[cfg(test)]
use cdk::federation::FederationLocalStatusResponse;
use cdk::federation::{
    redact_federation_operator_message, FederationCatchUpCertificate,
    FederationCatchUpCertificateQuorum, FederationCatchUpOperation, FederationError,
    FederationJournalCatchUpResponse, FederationJournalDigest, FederationJournalEntry,
    FederationMaterializedStateDigest, FederationMaterializedStateRowFingerprint,
    FederationMaterializedStateSnapshot, FederationOperationId, FederationOperationStatus,
    FederationPeerStatus, FederationRestMessageKind, FederationRestSequenceTracker,
    FederationSignedRequest, FederationSubmitOperationRequest, FederationSubmitOperationResponse,
    FederationVerifiedOperationAdmission, InMemoryFederationJournal,
    FEDERATION_MAX_DKG_REQUEST_BODY_BYTES, FEDERATION_REST_API_VERSION,
};
use cdk::mint::Mint;
use serde::Serialize;
use tracing::instrument;
#[cfg(test)]
use uuid::Uuid;

use crate::MintState;

pub(crate) mod aleph_bft_latency;
mod aleph_bft_network;
mod aleph_bft_routes;
mod aleph_bft_runtime;
mod aleph_bft_session;
mod broadcast;
mod catch_up_routes;
mod dkg_driver;
mod dkg_routes;
pub(crate) mod frost_signing;
mod node_runtime;
mod operator_routes;
mod payment_observation;
mod peer_client;
mod status_routes;
mod sync_config;
mod sync_loop;
mod wallet_liveness;

pub(crate) use self::aleph_bft_network::CdkAlephBftNetworkInboundQueue;
use self::aleph_bft_routes::{post_aleph_bft_activity, post_aleph_bft_network_message};
#[cfg(test)]
use self::aleph_bft_session::{
    federation_aleph_bft_live_session_startup_decision,
    halt_federation_aleph_bft_generation_for_test,
    rotate_federation_aleph_bft_live_session_to_next_empty_with_transport,
    start_federation_aleph_bft_live_session_with_config,
    start_federation_aleph_bft_live_session_with_options_and_transport,
    start_federation_aleph_bft_live_session_with_transport, FederationAlephBftLiveSessionOptions,
    FederationAlephBftLiveSessionStartupDecision,
};
pub(crate) use self::aleph_bft_session::{
    halt_current_federation_aleph_bft_session, start_federation_aleph_bft_live_session,
    AlephBftSessionSupervisor,
};
pub(crate) use self::broadcast::FederationTransactionBroadcaster;
use self::catch_up_routes::post_journal;
#[cfg(test)]
use self::dkg_driver::{
    complete_federation_dkg_ceremony, federation_dkg_readiness_required,
    prepare_federation_dkg_participant_state, run_federation_dkg_driver_tick,
    run_federation_dkg_readiness_round, FederationDkgDeliveryTracker,
};
pub(crate) use self::dkg_driver::{frost_storage_key, spawn_federation_dkg_driver};
pub use self::dkg_driver::{
    run_federation_dkg_driver, FederationDkgAutoStartConfig, FederationDkgDriverRunConfig,
    FederationDkgDriverRunOutcome, FederationDkgDriverTickOutcome,
    FederationDkgSecretShareTransportPolicy,
};
use self::dkg_routes::{
    post_dkg_activation, post_dkg_commitment, post_dkg_readiness, post_dkg_reveal,
    post_dkg_secret_share, post_dkg_transcript_signature, post_frost_dkg_confirmation,
    post_frost_dkg_round1, post_frost_dkg_round2,
};
use self::frost_signing::{
    post_frost_aggregate_result, post_frost_signing_abort, post_frost_signing_commitment,
    post_frost_signing_package,
};
pub(crate) use self::node_runtime::FederationNodeRuntime;
use self::operator_routes::{post_wallet_rebroadcast, post_wallet_transaction_status};
use self::payment_observation::payment_observation_report_summaries;
pub(crate) use self::payment_observation::FederationPaymentObserver;
#[cfg(test)]
use self::peer_client::FederationRestPeerClient;
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
    persist_local_catch_up_certificate_chunks, run_federation_peer_sync_round,
    run_federation_peer_sync_round_with_backoff,
};
pub(crate) use self::wallet_liveness::FederationWalletLivenessWorker;
#[cfg(test)]
use cdk::federation::{
    FederationConfig, FederationDkgCommitmentRequest, FederationDkgReadinessRequest,
    FederationDkgRevealRequest, FederationDkgSecretShareRequest, FederationDkgSetupResponse,
    FederationDkgTranscriptSignatureRequest, FederationId, FederationJournalCatchUpRequest,
    FederationMaterializedStateRow, FederationPeerStatusRequest,
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
        .route("/dkg/frost/round1", post(post_frost_dkg_round1))
        .route("/dkg/frost/round2", post(post_frost_dkg_round2))
        .route(
            "/dkg/frost/confirmations",
            post(post_frost_dkg_confirmation),
        )
        .route(
            "/dkg/transcript-signatures",
            post(post_dkg_transcript_signature),
        )
        .route("/dkg/activations", post(post_dkg_activation))
        .layer(DefaultBodyLimit::max(FEDERATION_MAX_DKG_REQUEST_BODY_BYTES));
    let frost_signing_router = Router::new()
        .route(
            "/frost/signing/commitments",
            post(post_frost_signing_commitment),
        )
        .route("/frost/signing/packages", post(post_frost_signing_package))
        .route("/frost/signing/aborts", post(post_frost_signing_abort))
        .route("/frost/signing/results", post(post_frost_aggregate_result))
        .layer(DefaultBodyLimit::max(
            cdk::federation::FEDERATION_MAX_FROST_SIGNING_REQUEST_BODY_BYTES,
        ));

    Router::new()
        .merge(dkg_router)
        .merge(frost_signing_router)
        .route("/config", get(get_public_config))
        .route("/status", post(post_status))
        .route("/local-status", post(post_local_status))
        .route(
            "/wallet/transactions/status",
            post(post_wallet_transaction_status),
        )
        .route(
            "/wallet/transactions/rebroadcast",
            post(post_wallet_rebroadcast),
        )
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
    let envelope = admission.operation_envelope().clone();
    let response = state
        .submit_federation_envelope_to_consensus(envelope)
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
    FederationVerifiedOperationAdmission::from_signed_submit_request(
        request,
        runtime_config.public_config(),
        runtime_config.member_id(),
    )
    .map_err(federation_error_response)
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
pub(crate) struct CatchUpCertificateQuorumStatus {
    signers: BTreeSet<cdk::federation::MemberId>,
    from_index: u64,
    latest_index: u64,
    operation_count: u64,
    materialized_state_digest: FederationMaterializedStateDigest,
}

impl From<FederationCatchUpCertificateQuorum> for CatchUpCertificateQuorumStatus {
    fn from(quorum: FederationCatchUpCertificateQuorum) -> Self {
        Self {
            signers: quorum.signers,
            from_index: quorum.from_index,
            latest_index: quorum.latest_index,
            operation_count: quorum.operation_count,
            materialized_state_digest: quorum.materialized_state_digest,
        }
    }
}

impl CatchUpCertificateQuorumStatus {
    fn append(&mut self, quorum: &FederationCatchUpCertificateQuorum) {
        self.signers.extend(quorum.signers.iter().copied());
        self.latest_index = quorum.latest_index;
        self.operation_count = self
            .latest_index
            .checked_sub(self.from_index)
            .and_then(|span| span.checked_add(1))
            .unwrap_or(u64::MAX);
        self.materialized_state_digest = quorum.materialized_state_digest;
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct CatchUpCertificateRangeKey {
    from_index: u64,
    operation_count: u64,
    latest_index: u64,
    previous_order_digest: FederationJournalDigest,
    range_order_digest: FederationJournalDigest,
    latest_order_digest: FederationJournalDigest,
    materialized_state_digest: FederationMaterializedStateDigest,
    operation_ids: Vec<FederationOperationId>,
}

impl From<&FederationCatchUpCertificate> for CatchUpCertificateRangeKey {
    fn from(certificate: &FederationCatchUpCertificate) -> Self {
        Self {
            from_index: certificate.from_index,
            operation_count: certificate.operation_count(),
            latest_index: certificate.latest_index(),
            previous_order_digest: certificate.previous_order_digest,
            range_order_digest: certificate.range_order_digest(),
            latest_order_digest: certificate.latest_order_digest,
            materialized_state_digest: certificate.materialized_state_digest,
            operation_ids: certificate.operation_ids.clone(),
        }
    }
}

async fn catch_up_certificate_quorum_status(
    mint: &Mint,
) -> Result<Option<CatchUpCertificateQuorumStatus>, cdk::Error> {
    let Some(runtime_config) = mint.federation_config() else {
        return Ok(None);
    };
    let db = mint.localstore();
    let entries = db.get_federation_journal_entries().await?;
    let certificates = db.get_federation_catch_up_certificates().await?;
    if entries.is_empty() || certificates.is_empty() {
        return Ok(None);
    }

    let entries_by_index = entries
        .into_iter()
        .map(|entry| (entry.index, entry))
        .collect::<BTreeMap<_, _>>();
    let mut grouped_outcomes: BTreeMap<
        CatchUpCertificateRangeKey,
        Vec<FederationCatchUpCertificate>,
    > = BTreeMap::new();
    for certificate in certificates {
        grouped_outcomes
            .entry(CatchUpCertificateRangeKey::from(&certificate))
            .or_default()
            .push(certificate);
    }

    let mut quorums_by_start: BTreeMap<u64, Vec<FederationCatchUpCertificateQuorum>> =
        BTreeMap::new();
    for (range, certificates) in grouped_outcomes {
        let Some(entries) = entries_for_catch_up_certificate_range(&entries_by_index, &range)
        else {
            continue;
        };
        let response = FederationJournalCatchUpResponse {
            version: FEDERATION_REST_API_VERSION,
            federation_id: runtime_config.public_config().federation_id,
            from_index: range.from_index,
            entries: entries
                .iter()
                .map(FederationCatchUpOperation::from)
                .collect(),
            latest_index: Some(range.latest_index),
            latest_order_digest: range.latest_order_digest,
            materialized_state_digest: range.materialized_state_digest,
            catch_up_certificate: None,
            catch_up_certificates: Vec::new(),
            aleph_bft_finalized_items: Vec::new(),
            aleph_bft_finalized_items_complete: true,
            aleph_bft_backup: None,
            complete: true,
        };

        let quorum = match response
            .verify_catch_up_certificate_quorum(runtime_config.public_config(), &certificates)
        {
            Ok(quorum) => quorum,
            Err(FederationError::CatchUpCertificateQuorumNotReached { .. }) => continue,
            Err(_) => continue,
        };
        quorums_by_start
            .entry(quorum.from_index)
            .or_default()
            .push(quorum);
    }

    Ok(contiguous_catch_up_certificate_quorum_prefix(
        &quorums_by_start,
    ))
}

fn contiguous_catch_up_certificate_quorum_prefix(
    quorums_by_start: &BTreeMap<u64, Vec<FederationCatchUpCertificateQuorum>>,
) -> Option<CatchUpCertificateQuorumStatus> {
    let mut next_index = 0;
    let mut status: Option<CatchUpCertificateQuorumStatus> = None;
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

fn entries_for_catch_up_certificate_range(
    entries_by_index: &BTreeMap<u64, FederationJournalEntry>,
    range: &CatchUpCertificateRangeKey,
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
    let status = federation_error_status(&err);
    let diagnostic = redact_federation_operator_message(&err.to_string());
    tracing::warn!(
        status = status.as_u16(),
        "Federation request failed: {diagnostic}"
    );
    let message = match status {
        StatusCode::INTERNAL_SERVER_ERROR => "federation internal error".to_string(),
        _ => diagnostic,
    };
    (status, message).into_response()
}

fn federation_error_status(err: &FederationError) -> StatusCode {
    match err {
        FederationError::ConflictingOperation { .. } => StatusCode::CONFLICT,
        FederationError::PendingConsensusMempoolFull { .. }
        | FederationError::ResponseWaiterLimitExceeded { .. } => StatusCode::TOO_MANY_REQUESTS,
        FederationError::ConsensusQuorumUnavailable { .. }
        | FederationError::RuntimeUnavailable { .. }
        | FederationError::NodeAwaitingQuorum { .. }
        | FederationError::NodeLagging { .. }
        | FederationError::NodeCatchingUp { .. }
        | FederationError::NodeVerifyingCatchUpCertificate { .. }
        | FederationError::NodeMaterializedStateMismatch { .. }
        | FederationError::NodeHalted { .. }
        | FederationError::OperationPendingFinalization { .. }
        | FederationError::PendingAlephBftFinalization { .. }
        | FederationError::AlephBftSessionRotationBlocked { .. } => StatusCode::SERVICE_UNAVAILABLE,
        FederationError::AlephBftFinalizedItemRecoveryMismatch { .. } => {
            StatusCode::INTERNAL_SERVER_ERROR
        }
        _ => StatusCode::BAD_REQUEST,
    }
}

fn cdk_error_response(err: cdk::Error) -> Response {
    match err {
        cdk::Error::Federation(err) => federation_error_response(err),
        err => {
            let diagnostic = redact_federation_operator_message(&err.to_string());
            tracing::error!("Federation storage mutation failed: {diagnostic}");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                "federation internal error",
            )
                .into_response()
        }
    }
}

fn anyhow_response(err: anyhow::Error) -> Response {
    let diagnostic = redact_federation_operator_message(&err.to_string());
    tracing::error!("Federation status failed: {diagnostic}");
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        "federation internal error",
    )
        .into_response()
}

async fn local_peer_status(state: &MintState) -> anyhow::Result<FederationPeerStatus> {
    let (order_status, latest_operation_status) =
        local_peer_order_status_with_latest_operation_status(state).await?;
    let mut cache = state
        .federation
        .materialized_state_digest_cache
        .lock()
        .await;
    if let Some(cached) = cache.as_ref() {
        if cached.operation_count == order_status.operation_count
            && cached.latest_order_digest == order_status.latest_order_digest
            && cached.latest_operation_status == latest_operation_status
        {
            let mut status = order_status;
            status.materialized_state_digest = Some(cached.materialized_state_digest);
            return Ok(status);
        }
    }

    #[cfg(test)]
    state
        .federation
        .materialized_state_digest_computation_count
        .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    let (mut status, entries) = local_peer_status_and_entries(state).await?;
    let materialized_state_digest = materialized_state_digest(state, &entries).await?;
    *cache = Some(node_runtime::MaterializedStateDigestCache {
        operation_count: status.operation_count,
        latest_order_digest: status.latest_order_digest,
        latest_operation_status,
        materialized_state_digest,
    });
    status.materialized_state_digest = Some(materialized_state_digest);
    Ok(status)
}

async fn local_peer_order_status(state: &MintState) -> anyhow::Result<FederationPeerStatus> {
    Ok(local_peer_order_status_with_latest_operation_status(state)
        .await?
        .0)
}

async fn local_peer_order_status_with_latest_operation_status(
    state: &MintState,
) -> anyhow::Result<(FederationPeerStatus, Option<FederationOperationStatus>)> {
    let runtime_config =
        state
            .mint
            .federation_config()
            .ok_or(FederationError::RuntimeUnavailable {
                component: "mint configuration",
            })?;
    let tip = state.mint.localstore().get_federation_journal_tip().await?;
    let (mut status, latest_operation_status) = match tip {
        Some(tip) => {
            let operation_count = tip
                .index
                .checked_add(1)
                .ok_or_else(|| anyhow::anyhow!("federation journal index overflow"))?;
            (
                FederationPeerStatus::new(
                    runtime_config.federation_id(),
                    runtime_config.member_id(),
                    operation_count,
                    Some(tip.index),
                    tip.order_digest,
                ),
                Some(tip.status),
            )
        }
        None => (
            FederationPeerStatus::new(
                runtime_config.federation_id(),
                runtime_config.member_id(),
                0,
                None,
                cdk::federation::FederationJournalDigest::genesis(),
            ),
            None,
        ),
    };
    let next_item_index = local_finalized_item_cursor(state).await?;
    let session = state
        .federation_aleph_bft_status()
        .await
        .ok_or(FederationError::RuntimeUnavailable {
            component: "AlephBFT consensus status",
        })?
        .session;
    status = status
        .with_aleph_bft_next_item_index(next_item_index)
        .with_aleph_bft_session(session);
    Ok((status, latest_operation_status))
}

async fn local_peer_status_with_materialized_rows(
    state: &MintState,
) -> anyhow::Result<(
    FederationPeerStatus,
    Vec<FederationMaterializedStateRowFingerprint>,
)> {
    let (mut status, entries) = local_peer_status_and_entries(state).await?;
    let snapshot = materialized_state_snapshot(state, &entries).await?;
    status.materialized_state_digest = Some(snapshot.digest()?);
    let materialized_state_rows = snapshot.row_fingerprints()?;
    Ok((status, materialized_state_rows))
}

async fn local_peer_status_and_entries(
    state: &MintState,
) -> anyhow::Result<(FederationPeerStatus, Vec<FederationJournalEntry>)> {
    let runtime_config =
        state
            .mint
            .federation_config()
            .ok_or(FederationError::RuntimeUnavailable {
                component: "mint configuration",
            })?;
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
    return Err(FederationError::RuntimeUnavailable {
        component: "AlephBFT consensus status",
    }
    .into());
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
) -> anyhow::Result<FederationMaterializedStateDigest> {
    Ok(materialized_state_snapshot(state, entries)
        .await?
        .digest()?)
}

async fn materialized_state_snapshot(
    state: &MintState,
    _entries: &[FederationJournalEntry],
) -> anyhow::Result<FederationMaterializedStateSnapshot> {
    Ok(state.mint.federation_materialized_state_snapshot().await?)
}

#[cfg(test)]
fn federation_completed_operation_uuid(operation_id: FederationOperationId) -> Uuid {
    let bytes = operation_id.to_bytes();
    let mut uuid = [0_u8; 16];
    uuid.copy_from_slice(&bytes[..16]);
    Uuid::from_bytes(uuid)
}

fn verify_catch_up_certificate_entries(
    local_entries: &[FederationJournalEntry],
    response: &FederationJournalCatchUpResponse,
) -> anyhow::Result<()> {
    let expected = local_entries
        .iter()
        .map(FederationCatchUpOperation::from)
        .collect::<Vec<_>>();
    verify_catch_up_operation_evidence(&expected, response)
}

fn verify_catch_up_operation_evidence(
    expected: &[FederationCatchUpOperation],
    response: &FederationJournalCatchUpResponse,
) -> anyhow::Result<()> {
    let first = expected
        .first()
        .ok_or_else(|| FederationError::CatchUpCertificateMismatch {
            reason: "missing local entries for evidence response".to_string(),
        })?;
    if response.from_index != first.index {
        return Err(FederationError::CatchUpIndexMismatch {
            expected: first.index,
            found: response.from_index,
        }
        .into());
    }
    if response.entries.len() != expected.len() {
        return Err(FederationError::CatchUpCertificateMismatch {
            reason: format!(
                "evidence response entry count mismatch: expected={}, found={}",
                expected.len(),
                response.entries.len()
            ),
        }
        .into());
    }

    for (local, peer) in expected.iter().zip(&response.entries) {
        if peer.index != local.index
            || peer.operation_id != local.operation_id
            || peer.previous_order_digest != local.previous_order_digest
            || peer.order_digest != local.order_digest
        {
            return Err(FederationError::CatchUpCertificateMismatch {
                reason: format!(
                    "evidence entry mismatch at local index {} and peer index {}",
                    local.index, peer.index
                ),
            }
            .into());
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests;
