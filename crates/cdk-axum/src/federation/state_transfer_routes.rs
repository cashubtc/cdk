//! Serve pages of a stored checkpoint state snapshot, and of the validity
//! window before that checkpoint, to a peer.
//!
//! The rows were frozen when this member captured the scheduled checkpoint;
//! the window is read from this member's immutable journal and finalized
//! items. Responses are not signed: the requester checks the rows against the
//! checkpoint's quorum-signed materialized-state digest and the window
//! against its signed cursors, so any peer that holds them can serve them.

use axum::extract::{Json, State};
use axum::response::Response;
use cdk::federation::{
    FederationCheckpointError, FederationRestMessageKind, FederationSignedRequest,
    FederationStateTransferRequest, FederationStateTransferResponse,
    FederationStateTransferWindowRequest, FederationStateTransferWindowResponse,
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

#[instrument(skip_all)]
pub(super) async fn post_state_transfer_window(
    State(state): State<MintState>,
    Json(payload): Json<FederationSignedRequest<FederationStateTransferWindowRequest>>,
) -> Result<Json<FederationStateTransferWindowResponse>, Response> {
    let verified =
        admit_private_request(&state, &payload, FederationRestMessageKind::StateTransfer).await?;
    state
        .record_federation_sequence(
            FederationRestMessageKind::StateTransfer,
            verified.sender(),
            verified.sequence(),
        )
        .await;
    let response = state_transfer_window_page(&state, &payload.payload).await?;
    Ok(Json(response))
}

fn window_invalid(reason: &'static str) -> Response {
    federation_error_response(FederationCheckpointError::StateTransferInvalid { reason }.into())
}

pub(super) async fn state_transfer_window_page(
    state: &MintState,
    request: &FederationStateTransferWindowRequest,
) -> Result<FederationStateTransferWindowResponse, Response> {
    let runtime_config = federation_runtime_config(state)?;
    let db = state.mint.localstore();
    let checkpoint = db
        .get_federation_checkpoint(request.checkpoint_id)
        .await
        .map_err(|err| anyhow_response(err.into()))?
        .ok_or_else(|| window_invalid("the requested checkpoint is not stored"))?;
    checkpoint
        .validate_against_validated_config(runtime_config.public_config())
        .map_err(federation_error_response)?;
    if let Some(floor) = db
        .get_federation_retention_floor()
        .await
        .map_err(|err| anyhow_response(err.into()))?
    {
        if !floor.retains_window_of(&checkpoint) {
            return Err(federation_error_response(
                FederationCheckpointError::BelowRetentionFloor {
                    requested: checkpoint.finalized_next_item_index,
                    floor: floor.item_start,
                }
                .into(),
            ));
        }
    }
    let bounds = db
        .get_federation_state_transfer_window_bounds(&checkpoint)
        .await
        .map_err(|err| anyhow_response(err.into()))?;
    let from_item_index = request.next_item_index.unwrap_or(bounds.item_start);
    let from_journal_index = request.next_journal_index.unwrap_or(bounds.journal_start);
    if from_item_index < bounds.item_start
        || from_item_index > checkpoint.finalized_next_item_index
        || from_journal_index < bounds.journal_start
        || from_journal_index > checkpoint.journal_next_index
    {
        return Err(window_invalid("window cursor is outside the window"));
    }
    let limit = request.effective_limit();
    let mut items = db
        .get_federation_aleph_bft_finalized_items_range(
            from_item_index,
            checkpoint.finalized_next_item_index,
            limit,
        )
        .await
        .map_err(|err| anyhow_response(err.into()))?;
    let mut entries = db
        .get_federation_journal_entries_range(
            from_journal_index,
            checkpoint.journal_next_index,
            limit,
        )
        .await
        .map_err(|err| anyhow_response(err.into()))?;
    loop {
        let response = FederationStateTransferWindowResponse {
            version: cdk::federation::FEDERATION_REST_API_VERSION,
            federation_id: runtime_config.federation_id(),
            checkpoint_id: request.checkpoint_id,
            bounds,
            from_item_index,
            from_journal_index,
            items,
            entries,
        };
        if response.ensure_within_page_limit().is_ok() {
            return Ok(response);
        }
        items = response.items;
        entries = response.entries;
        // Shrink the longer list until the page fits.
        if items.len() >= entries.len() && items.len() > 1 {
            items.truncate(items.len() / 2);
        } else if entries.len() > 1 {
            entries.truncate(entries.len() / 2);
        } else {
            return Err(window_invalid(
                "a single window item or entry exceeds the page byte limit",
            ));
        }
    }
}
