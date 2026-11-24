//! Serve pages of a stored checkpoint state snapshot to a peer.
//!
//! The rows were frozen when this member captured the scheduled checkpoint.
//! The response is not signed: the requester checks the rows against the
//! checkpoint's quorum-signed materialized-state digest, so any peer that
//! holds the snapshot can serve it.

use axum::extract::{Json, State};
use axum::response::Response;
use cdk::federation::{
    FederationCheckpointError, FederationRestMessageKind, FederationSignedRequest,
    FederationStateTransferRequest, FederationStateTransferResponse,
};
use tracing::instrument;

use super::{
    admit_private_request, anyhow_response, federation_error_response, federation_runtime_config,
};
use crate::MintState;

#[instrument(skip_all)]
pub(super) async fn post_state_transfer(
    State(state): State<MintState>,
    Json(payload): Json<FederationSignedRequest<FederationStateTransferRequest>>,
) -> Result<Json<FederationStateTransferResponse>, Response> {
    let verified =
        admit_private_request(&state, &payload, FederationRestMessageKind::StateTransfer).await?;
    state
        .record_federation_sequence(
            FederationRestMessageKind::StateTransfer,
            verified.sender(),
            verified.sequence(),
        )
        .await;
    tracing::debug!(
        checkpoint = %payload.payload.checkpoint_id,
        next_row_index = payload.payload.next_row_index,
        "Received federation state transfer request"
    );
    let response = state_transfer_page(&state, &payload.payload).await?;
    Ok(Json(response))
}

pub(super) async fn state_transfer_page(
    state: &MintState,
    request: &FederationStateTransferRequest,
) -> Result<FederationStateTransferResponse, Response> {
    let runtime_config = federation_runtime_config(state)?;
    let db = state.mint.localstore();
    let snapshot = db
        .get_federation_state_snapshots()
        .await
        .map_err(|err| anyhow_response(err.into()))?
        .into_iter()
        .find(|snapshot| snapshot.checkpoint_id == request.checkpoint_id)
        .ok_or_else(|| {
            federation_error_response(
                FederationCheckpointError::StateTransferInvalid {
                    reason: "no snapshot is stored for the requested checkpoint",
                }
                .into(),
            )
        })?;
    if request.next_row_index > snapshot.row_count {
        return Err(federation_error_response(
            FederationCheckpointError::StateTransferInvalid {
                reason: "requested row is past the end of the snapshot",
            }
            .into(),
        ));
    }
    let mut rows = db
        .get_federation_state_snapshot_rows(
            request.checkpoint_id,
            request.next_row_index,
            request.effective_limit(),
        )
        .await
        .map_err(|err| anyhow_response(err.into()))?;
    let mut response = FederationStateTransferResponse::new(
        runtime_config.federation_id(),
        request.checkpoint_id,
        request.next_row_index,
        snapshot.row_count,
        Vec::new(),
    );
    // Keep the page under the protocol byte limit by returning fewer rows.
    loop {
        response.rows = rows;
        if response.ensure_within_page_limit().is_ok() {
            return Ok(response);
        }
        rows = std::mem::take(&mut response.rows);
        if rows.len() <= 1 {
            return Err(federation_error_response(
                FederationCheckpointError::StateTransferInvalid {
                    reason: "a single snapshot row exceeds the page byte limit",
                }
                .into(),
            ));
        }
        rows.truncate(rows.len() / 2);
    }
}
