//! Private federation member REST routes.

use axum::extract::{Json, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::Router;
use cdk::federation::{
    FederationJournalCatchUpRequest, FederationJournalCatchUpResponse, FederationJournalDigest,
    FederationPeerStatus, FederationSubmitOperationRequest, FederationSubmitOperationResponse,
};
use tracing::instrument;

use crate::MintState;

const FEDERATION_JOURNAL_NOT_WIRED: &str = "federation consensus journal is not wired yet";

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
    let config = state
        .mint
        .federation_config()
        .ok_or_else(federation_not_configured)?;
    Ok(Json(FederationPeerStatus::new(
        config.federation_id(),
        config.member_id(),
        0,
        None,
        FederationJournalDigest::genesis(),
    )))
}

#[instrument(skip_all)]
async fn post_operation(
    State(state): State<MintState>,
    Json(payload): Json<FederationSubmitOperationRequest>,
) -> Result<Json<FederationSubmitOperationResponse>, Response> {
    let config = state
        .mint
        .federation_config()
        .ok_or_else(federation_not_configured)?;
    if payload.envelope.federation_id != config.federation_id() {
        return Err(federation_mismatch());
    }

    let operation_id = payload.operation_id().map_err(|err| {
        tracing::warn!("Invalid federation operation payload: {}", err);
        (StatusCode::BAD_REQUEST, err.to_string()).into_response()
    })?;
    tracing::debug!("Received federation operation {}", operation_id);
    Err(not_implemented())
}

#[instrument(skip_all)]
async fn post_journal(
    State(state): State<MintState>,
    Json(payload): Json<FederationJournalCatchUpRequest>,
) -> Result<Json<FederationJournalCatchUpResponse>, Response> {
    let _config = state
        .mint
        .federation_config()
        .ok_or_else(federation_not_configured)?;
    tracing::debug!(
        next_index = payload.next_index,
        limit = ?payload.limit,
        "Received federation journal catch-up request"
    );
    Err(not_implemented())
}

fn federation_not_configured() -> Response {
    (
        StatusCode::NOT_FOUND,
        "mint is not configured as a federation member",
    )
        .into_response()
}

fn federation_mismatch() -> Response {
    (
        StatusCode::BAD_REQUEST,
        "request federation id does not match this member",
    )
        .into_response()
}

fn not_implemented() -> Response {
    (StatusCode::NOT_IMPLEMENTED, FEDERATION_JOURNAL_NOT_WIRED).into_response()
}
