//! Private federation member REST routes.

use axum::extract::{Json, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::Router;
use cdk::federation::{
    FederationError, FederationJournalCatchUpRequest, FederationJournalCatchUpResponse,
    FederationPeerStatus, FederationSubmitOperationRequest, FederationSubmitOperationResponse,
    InMemoryFederationConsensus,
};
use tracing::instrument;

use crate::MintState;

type ConsensusHandle = std::sync::Arc<tokio::sync::Mutex<InMemoryFederationConsensus>>;

pub(crate) fn create_federation_router() -> Router<MintState> {
    Router::new()
        .route("/status", get(get_status))
        .route("/operations", post(post_operation))
        .route("/journal", post(post_journal))
}

#[instrument(skip_all)]
async fn get_status(
    State(state): State<MintState>,
) -> Result<Json<FederationPeerStatus>, Response> {
    let consensus = federation_consensus(&state)?;
    let consensus = consensus.lock().await;
    Ok(Json(consensus.peer_status()))
}

#[instrument(skip_all)]
async fn post_operation(
    State(state): State<MintState>,
    Json(payload): Json<FederationSubmitOperationRequest>,
) -> Result<Json<FederationSubmitOperationResponse>, Response> {
    let consensus = federation_consensus(&state)?;
    let mut consensus = consensus.lock().await;
    let response = consensus
        .submit_operation(payload)
        .map_err(federation_error_response)?;
    tracing::debug!("Accepted federation operation {}", response.operation_id);
    Ok(Json(response))
}

#[instrument(skip_all)]
async fn post_journal(
    State(state): State<MintState>,
    Json(payload): Json<FederationJournalCatchUpRequest>,
) -> Result<Json<FederationJournalCatchUpResponse>, Response> {
    let consensus = federation_consensus(&state)?;
    tracing::debug!(
        next_index = payload.next_index,
        limit = ?payload.limit,
        "Received federation journal catch-up request"
    );
    let consensus = consensus.lock().await;
    Ok(Json(consensus.catch_up(&payload)))
}

fn federation_consensus(state: &MintState) -> Result<ConsensusHandle, Response> {
    state
        .federation_consensus
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
        FederationError::OperationFederationMismatch { .. } => StatusCode::BAD_REQUEST,
        _ => StatusCode::BAD_REQUEST,
    };
    tracing::warn!("Federation request failed: {}", err);
    (status, err.to_string()).into_response()
}
