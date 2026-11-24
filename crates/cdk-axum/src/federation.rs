//! Federation REST routes over the `cdk-federation` node.

use axum::extract::{DefaultBodyLimit, Json, State};
use axum::http::{header, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::Router;
#[cfg(test)]
use cdk::federation::FederationLocalStatusResponse;
use cdk::federation::{
    redact_federation_operator_message, FederationAlephBftError, FederationCheckpointError,
    FederationError, FederationRestMessageKind, FederationRestSequenceTracker,
    FederationSignedRequest, FederationSubmitOperationRequest, FederationSubmitOperationResponse,
    FederationVerifiedOperationAdmission, FEDERATION_MAX_DKG_REQUEST_BODY_BYTES,
};
#[cfg(test)]
use cdk::federation::{
    FederationJournalCatchUpResponse, FederationMaterializedStateDigest,
    FEDERATION_REST_API_VERSION,
};
use serde::Serialize;
use tracing::instrument;

use crate::MintState;
#[cfg(test)]
use cdk::federation::{
    FederationConfig, FederationMaterializedStateSnapshot, FederationPeerStatus,
};

#[allow(unused_imports)]
pub(crate) use cdk_federation::*;

mod aleph_bft_routes;
mod backup_routes;
#[cfg(feature = "federation-bark")]
mod bark_custody_routes;
#[cfg(feature = "federation-bark")]
mod bark_secret_routes;
mod catch_up_routes;
mod checkpoint_routes;
mod dkg_routes;
mod frost_signing_routes;
mod operator_routes;
mod private_request;
mod setup;
mod state_transfer_routes;
mod status_routes;

use self::aleph_bft_routes::{post_aleph_bft_activity, post_aleph_bft_network_message};
use self::backup_routes::post_aleph_bft_backup;
#[cfg(feature = "federation-bark")]
use self::bark_custody_routes::{
    post_bark_custody_abort, post_bark_threshold_dh, post_bark_virtual_musig_commitment,
    post_bark_virtual_musig_partial,
};
#[cfg(feature = "federation-bark")]
use self::bark_secret_routes::{post_bark_evidence_attestation, post_bark_receive_secret};
use self::catch_up_routes::post_journal;
use self::checkpoint_routes::{
    post_checkpoint_audit, post_checkpoint_fetch, post_checkpoint_quorum,
    post_checkpoint_signature, post_checkpoint_summary,
};
use self::dkg_routes::{
    post_dkg_activation, post_dkg_commitment, post_dkg_readiness, post_dkg_reveal,
    post_dkg_secret_share, post_dkg_transcript_signature, post_frost_dkg_confirmation,
    post_frost_dkg_round1, post_frost_dkg_round2,
};
use self::frost_signing_routes::{
    post_frost_aggregate_result, post_frost_signing_abort, post_frost_signing_commitment,
    post_frost_signing_package,
};
#[cfg(feature = "federation-bark")]
use self::operator_routes::{
    post_bark_action_retry, post_bark_reconcile, post_bark_recovery_address,
};
#[cfg(feature = "federation-bdk")]
use self::operator_routes::{post_wallet_rebroadcast, post_wallet_transaction_status};
pub(super) use self::private_request::admit_private_request;
pub use self::setup::{
    create_federation_setup_router, FederationSetupCoordinatorService,
    FederationSetupCoordinatorServiceConfig, FederationSetupHttpClient, FederationSetupHttpError,
    FederationSetupHttpErrorCode,
};
use self::state_transfer_routes::{post_state_transfer, post_state_transfer_window};
use self::status_routes::{post_local_status, post_status};

#[cfg(test)]
use cdk::federation::{
    FederationDkgCommitmentRequest, FederationDkgReadinessRequest, FederationDkgRevealRequest,
    FederationDkgSecretShareRequest, FederationDkgSetupResponse,
    FederationDkgTranscriptSignatureRequest, FederationId, FederationJournalCatchUpRequest,
    FederationMaterializedStateRow, FederationPeerStatusRequest,
};
#[cfg(test)]
use tokio::time::{self, Duration};

type SequenceHandle = std::sync::Arc<tokio::sync::Mutex<FederationRestSequenceTracker>>;

pub(crate) fn create_federation_dkg_router() -> Router<MintState> {
    Router::new()
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
        .layer(DefaultBodyLimit::max(FEDERATION_MAX_DKG_REQUEST_BODY_BYTES))
}

pub(crate) fn create_federation_router() -> Router<MintState> {
    let dkg_router = create_federation_dkg_router();
    let frost_signing_router = Router::new()
        .route(
            "/frost/signing/commitments",
            post(post_frost_signing_commitment),
        )
        .route("/frost/signing/packages", post(post_frost_signing_package))
        .route("/frost/signing/aborts", post(post_frost_signing_abort))
        .route("/frost/signing/results", post(post_frost_aggregate_result));
    #[cfg(feature = "federation-bark")]
    let frost_signing_router = frost_signing_router
        .route(
            "/frost/bark/musig2/commitments",
            post(post_bark_virtual_musig_commitment),
        )
        .route(
            "/frost/bark/musig2/partials",
            post(post_bark_virtual_musig_partial),
        )
        .route("/frost/bark/threshold-dh", post(post_bark_threshold_dh))
        .route("/frost/bark/aborts", post(post_bark_custody_abort));
    let frost_signing_router = frost_signing_router.layer(DefaultBodyLimit::max(
        cdk::federation::FEDERATION_MAX_FROST_SIGNING_REQUEST_BODY_BYTES,
    ));
    #[cfg(feature = "federation-bark")]
    let bark_secret_router = Router::new()
        .route(
            "/bark/receive-secrets",
            post(post_bark_receive_secret).layer(DefaultBodyLimit::max(
                cdk::federation::FEDERATION_MAX_BARK_RECEIVE_SECRET_REQUEST_BYTES,
            )),
        )
        .route(
            "/bark/evidence-attestations",
            post(post_bark_evidence_attestation).layer(DefaultBodyLimit::max(
                cdk::federation::FEDERATION_MAX_BARK_EVIDENCE_ATTESTATION_REQUEST_BYTES,
            )),
        );

    let router = Router::new()
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
        .route("/operations", post(post_operation))
        .route("/aleph-bft/activity", post(post_aleph_bft_activity))
        .route("/aleph-bft/messages", post(post_aleph_bft_network_message))
        .route(
            "/aleph-bft/backup",
            post(post_aleph_bft_backup).layer(DefaultBodyLimit::max(
                cdk::federation::FEDERATION_MAX_ALEPH_BFT_BACKUP_REQUEST_BYTES,
            )),
        )
        .route(
            "/journal",
            post(post_journal).layer(DefaultBodyLimit::max(
                cdk::federation::FEDERATION_MAX_CHECKPOINT_CATCH_UP_REQUEST_BYTES,
            )),
        )
        .route(
            "/state-transfer",
            post(post_state_transfer).layer(DefaultBodyLimit::max(
                cdk::federation::FEDERATION_MAX_CHECKPOINT_CATCH_UP_REQUEST_BYTES,
            )),
        )
        .route(
            "/state-transfer/window",
            post(post_state_transfer_window).layer(DefaultBodyLimit::max(
                cdk::federation::FEDERATION_MAX_CHECKPOINT_CATCH_UP_REQUEST_BYTES,
            )),
        );
    #[cfg(feature = "federation-bdk")]
    let router = router
        .route(
            "/wallet/transactions/status",
            post(post_wallet_transaction_status),
        )
        .route(
            "/wallet/transactions/rebroadcast",
            post(post_wallet_rebroadcast),
        );
    #[cfg(feature = "federation-bark")]
    let router = router
        .merge(bark_secret_router)
        .route("/bark/reconcile", post(post_bark_reconcile))
        .route("/bark/recovery/address", post(post_bark_recovery_address))
        .route("/bark/actions/retry", post(post_bark_action_retry));
    router
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
    // Runtime construction is the full config-validation trust boundary. Serving the
    // immutable config must not repeat aggregate-key interpolation for every wallet.
    let public_config = runtime_config.public_config();

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
    let Some(admission) = state.federation.private_rest_admission() else {
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
    let Some(admissions) = state.federation.dkg_member_admission() else {
        if state.federation.dkg_auto_start_config().is_none() {
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
pub(super) struct VerifiedFederationRequest {
    pub(super) sender: cdk::federation::MemberId,
    pub(super) sequence: u64,
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
        .sequences()
        .cloned()
        .ok_or_else(federation_not_configured)
}

fn federation_not_configured() -> Response {
    (
        StatusCode::NOT_FOUND,
        "mint is not configured as a federation member",
    )
        .into_response()
}

/// Federation error messages carry fixed reasons and public protocol values;
/// the two variants that hold foreign text (`BackendFailure`, `NodeHalted`)
/// are redacted when they are built, so the message needs no redaction here.
fn federation_error_response(err: FederationError) -> Response {
    let status = federation_error_status(&err);
    let diagnostic = err.to_string();
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
        FederationError::Checkpoint(FederationCheckpointError::AuditNotLocal { .. }) => {
            StatusCode::FORBIDDEN
        }
        FederationError::PendingConsensusMempoolFull { .. }
        | FederationError::ResponseWaiterLimitExceeded { .. } => StatusCode::TOO_MANY_REQUESTS,
        FederationError::ConsensusQuorumUnavailable { .. }
        | FederationError::RuntimeUnavailable { .. }
        | FederationError::NodeAwaitingQuorum { .. }
        | FederationError::NodeLagging { .. }
        | FederationError::NodeCatchingUp { .. }
        | FederationError::NodeHalted { .. }
        | FederationError::OperationPendingFinalization { .. } => StatusCode::SERVICE_UNAVAILABLE,
        FederationError::AlephBft(err) => match err {
            FederationAlephBftError::Backpressure { .. }
            | FederationAlephBftError::InboundByteBudgetExceeded { .. } => {
                StatusCode::TOO_MANY_REQUESTS
            }
            FederationAlephBftError::PendingFinalization { .. }
            | FederationAlephBftError::SessionRotationBlocked { .. } => {
                StatusCode::SERVICE_UNAVAILABLE
            }
            err if err.is_recovery_mismatch() => StatusCode::INTERNAL_SERVER_ERROR,
            _ => StatusCode::BAD_REQUEST,
        },
        FederationError::BackendFailure { .. } => StatusCode::INTERNAL_SERVER_ERROR,
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

pub(crate) async fn federation_invalid_operation_alert_middleware(
    axum::extract::State(state): axum::extract::State<MintState>,
    req: axum::http::Request<axum::body::Body>,
    next: axum::middleware::Next,
) -> axum::response::Response {
    let monitor = federation_invalid_operation_alert_path(req.method(), req.uri().path());
    let source = monitor.then(|| invalid_operation_source_from_request(&req));
    let response = next.run(req).await;
    if monitor && federation_invalid_operation_alert_status(response.status()) {
        if let Some(source) = source {
            state
                .record_federation_invalid_operation_alert(source)
                .await;
        }
    }

    response
}

pub(crate) fn federation_invalid_operation_alert_path(
    method: &axum::http::Method,
    path: &str,
) -> bool {
    if *method != axum::http::Method::POST {
        return false;
    }

    path == "/v1/swap"
        || path == "/v1/restore"
        || path == "/v1/checkstate"
        || path == "/v1/auth/blind/mint"
        || path == "/federation/v1/operations"
        || path.starts_with("/v1/mint/")
        || path.starts_with("/v1/melt/")
}

pub(crate) fn federation_invalid_operation_alert_status(status: axum::http::StatusCode) -> bool {
    status.is_client_error()
        && status != axum::http::StatusCode::NOT_FOUND
        && status != axum::http::StatusCode::METHOD_NOT_ALLOWED
}

/// Identifies who sent a request, for invalid-operation alerting.
fn invalid_operation_source_from_request(
    req: &axum::http::Request<axum::body::Body>,
) -> FederationInvalidOperationSource {
    if let Some(token) = req.headers().get(crate::auth::CLEAR_AUTH_KEY) {
        return FederationInvalidOperationSource::auth_token("clear", token.as_bytes());
    }
    if let Some(token) = req.headers().get(crate::auth::BLIND_AUTH_KEY) {
        return FederationInvalidOperationSource::auth_token("blind", token.as_bytes());
    }
    if let Some(axum::extract::ConnectInfo(address)) = req
        .extensions()
        .get::<axum::extract::ConnectInfo<std::net::SocketAddr>>()
    {
        return FederationInvalidOperationSource::client_ip(address.ip());
    }

    FederationInvalidOperationSource::anonymous()
}

#[cfg(test)]
mod request_source_tests {
    use super::*;

    #[test]
    fn invalid_operation_source_prefers_auth_token_then_connect_info() {
        let mut auth_req = axum::http::Request::builder()
            .uri("/v1/swap")
            .header(crate::auth::CLEAR_AUTH_KEY, "clear-token")
            .body(axum::body::Body::empty())
            .expect("request");
        auth_req.extensions_mut().insert(axum::extract::ConnectInfo(
            "127.0.0.1:8080"
                .parse::<std::net::SocketAddr>()
                .expect("socket address"),
        ));
        let auth_source = invalid_operation_source_from_request(&auth_req);
        assert_eq!(
            auth_source.kind,
            FederationInvalidOperationSourceKind::AuthToken
        );

        let mut ip_req = axum::http::Request::builder()
            .uri("/v1/swap")
            .body(axum::body::Body::empty())
            .expect("request");
        ip_req.extensions_mut().insert(axum::extract::ConnectInfo(
            "127.0.0.1:8080"
                .parse::<std::net::SocketAddr>()
                .expect("socket address"),
        ));
        let ip_source = invalid_operation_source_from_request(&ip_req);
        assert_eq!(
            ip_source.kind,
            FederationInvalidOperationSourceKind::ClientIp
        );
    }
}

#[cfg(test)]
mod tests;
