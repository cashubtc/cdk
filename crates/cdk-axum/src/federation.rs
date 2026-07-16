//! Federation REST routes.

use axum::extract::{DefaultBodyLimit, Json, State};
use axum::http::{header, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::Router;
#[cfg(test)]
use cdk::federation::FederationLocalStatusResponse;
use cdk::federation::{
    redact_federation_operator_message, FederationError, FederationMaterializedStateRowFingerprint,
    FederationMaterializedStateSnapshot, FederationOperationStatus, FederationPeerStatus,
    FederationRestMessageKind, FederationRestSequenceTracker, FederationSignedRequest,
    FederationSubmitOperationRequest, FederationSubmitOperationResponse,
    FederationVerifiedOperationAdmission, FEDERATION_MAX_DKG_REQUEST_BODY_BYTES,
};
#[cfg(test)]
use cdk::federation::{
    FederationJournalCatchUpResponse, FederationJournalEntry, FederationMaterializedStateDigest,
    FederationOperationId, FEDERATION_REST_API_VERSION,
};
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
mod checkpoint_routes;
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
use self::checkpoint_routes::{
    post_checkpoint_audit, post_checkpoint_fetch, post_checkpoint_quorum,
    post_checkpoint_signature, post_checkpoint_summary,
};
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
    run_federation_peer_sync_round, run_federation_peer_sync_round_with_backoff,
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
        .route("/checkpoints/summary", post(post_checkpoint_summary))
        .route("/checkpoints/fetch", post(post_checkpoint_fetch))
        .route("/checkpoints/signature", post(post_checkpoint_signature))
        .route("/checkpoints/quorum", post(post_checkpoint_quorum))
        .route("/checkpoints/audit", post(post_checkpoint_audit))
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
    let mut status = local_peer_order_status(state).await?;
    let frontier = state
        .mint
        .localstore()
        .get_federation_checkpoint_frontier()
        .await?;
    status.application_digest = Some(frontier.application_digest);
    status.latest_audited_checkpoint_id = frontier.latest_audited_checkpoint_id;
    status.materialized_state_digest = frontier.latest_materialized_state_digest;
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
    let mut status = local_peer_status(state).await?;
    let snapshot = materialized_state_snapshot(state).await?;
    status.materialized_state_digest = Some(snapshot.digest()?);
    let materialized_state_rows = snapshot.row_fingerprints()?;
    Ok((status, materialized_state_rows))
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

#[cfg(test)]
pub(crate) async fn materialized_state_digest(
    state: &MintState,
    _entries: &[FederationJournalEntry],
) -> anyhow::Result<FederationMaterializedStateDigest> {
    Ok(materialized_state_snapshot(state).await?.digest()?)
}

async fn materialized_state_snapshot(
    state: &MintState,
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

#[cfg(test)]
mod tests;
