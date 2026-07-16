use axum::extract::{Json, State};
use axum::response::Response;
use cdk::federation::{
    FederationAlephBftBackupResponse, FederationAlephBftFinalizedItem, FederationError,
    FederationJournalCatchUpRequest, FederationJournalCatchUpResponse, FederationRestMessageKind,
    FederationSignedRequest, InMemoryFederationJournal,
};
use tracing::instrument;

use crate::MintState;

use super::{
    acquire_private_rest_admission, anyhow_response, federation_error_response,
    federation_runtime_config, verify_federation_request,
};

const DEFAULT_FEDERATION_CATCH_UP_LIMIT: u16 = 100;
const MAX_FEDERATION_CATCH_UP_LIMIT: u16 = 1000;
const MAX_ALEPH_BFT_BACKUP_CHUNKS_PER_RESPONSE: usize = 512;
const MAX_ALEPH_BFT_BACKUP_BYTES_PER_RESPONSE: usize = 1024 * 1024;

#[instrument(skip_all)]
pub(super) async fn post_journal(
    State(state): State<MintState>,
    Json(payload): Json<FederationSignedRequest<FederationJournalCatchUpRequest>>,
) -> Result<Json<FederationJournalCatchUpResponse>, Response> {
    let _permit = acquire_private_rest_admission(&state, FederationRestMessageKind::CatchUp)?;
    let verified =
        verify_federation_request(&state, &payload, FederationRestMessageKind::CatchUp).await?;
    state
        .record_federation_sequence(
            FederationRestMessageKind::CatchUp,
            verified.sender,
            verified.sequence,
        )
        .await;
    tracing::debug!(
        next_index = payload.payload.next_index,
        limit = ?payload.payload.limit,
        "Received federation journal catch-up request"
    );
    let response = durable_catch_up_response(&state, &payload.payload).await?;
    Ok(Json(response))
}

async fn durable_catch_up_response(
    state: &MintState,
    request: &FederationJournalCatchUpRequest,
) -> Result<FederationJournalCatchUpResponse, Response> {
    let runtime_config = federation_runtime_config(state)?;
    let target_checkpoint = match request.target_checkpoint_id {
        Some(checkpoint_id) => {
            let checkpoint = state
                .mint
                .localstore()
                .get_federation_checkpoint(checkpoint_id)
                .await
                .map_err(|err| anyhow_response(err.into()))?
                .ok_or_else(|| {
                    federation_error_response(FederationError::CheckpointInvalid {
                        reason: "requested catch-up checkpoint is not stored".to_string(),
                    })
                })?;
            checkpoint
                .validate_against_config(runtime_config.public_config())
                .map_err(federation_error_response)?;
            if checkpoint.id().map_err(federation_error_response)? != checkpoint_id {
                return Err(federation_error_response(
                    FederationError::CheckpointInvalid {
                        reason: "requested catch-up checkpoint lookup binding mismatch".to_string(),
                    },
                ));
            }
            Some(checkpoint)
        }
        None => None,
    };
    let frontier = state
        .mint
        .localstore()
        .get_federation_checkpoint_frontier()
        .await
        .map_err(|err| anyhow_response(err.into()))?;
    let (end_index, end_order_digest, state_digest) = match target_checkpoint.as_ref() {
        Some(checkpoint) => (
            checkpoint.journal_next_index,
            checkpoint.journal_order_digest,
            checkpoint.materialized_state_digest,
        ),
        None => (
            frontier.journal_next_index,
            frontier.journal_order_digest,
            frontier
                .latest_materialized_state_digest
                .unwrap_or_else(cdk::federation::FederationMaterializedStateDigest::genesis),
        ),
    };
    if request.next_index > end_index {
        return Err(federation_error_response(
            FederationError::CheckpointInvalid {
                reason: "catch-up journal cursor is ahead of the available frontier".to_string(),
            },
        ));
    }
    let limit = request.effective_limit(
        DEFAULT_FEDERATION_CATCH_UP_LIMIT,
        MAX_FEDERATION_CATCH_UP_LIMIT,
    );
    let entries = state
        .mint
        .localstore()
        .get_federation_journal_entries_range(request.next_index, end_index, limit)
        .await
        .map_err(|err| anyhow_response(err.into()))?;
    let base_order_digest = match entries.first() {
        Some(entry) => entry.previous_order_digest,
        None if request.next_index == end_index => end_order_digest,
        None => {
            return Err(federation_error_response(
                FederationError::CheckpointInvalid {
                    reason: "journal bodies are unavailable at the requested cursor".to_string(),
                },
            ));
        }
    };
    let journal = InMemoryFederationJournal::from_base_and_entries(
        request.next_index,
        base_order_digest,
        entries,
    )
    .map_err(federation_error_response)?;

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

    let mut response = FederationJournalCatchUpResponse::from_journal(
        runtime_config.federation_id(),
        &journal,
        state_digest,
        request,
        DEFAULT_FEDERATION_CATCH_UP_LIMIT,
        MAX_FEDERATION_CATCH_UP_LIMIT,
    );
    if let Some(checkpoint) = target_checkpoint.as_ref() {
        response = response
            .bound_to_checkpoint(checkpoint)
            .map_err(federation_error_response)?;
    } else {
        response.latest_index = end_index.checked_sub(1);
        response.latest_order_digest = end_order_digest;
        response.complete = response
            .from_index
            .saturating_add(response.entries.len() as u64)
            == end_index;
    }
    let finalized_end_item_index = target_checkpoint
        .as_ref()
        .map_or(frontier.finalized_next_item_index, |checkpoint| {
            checkpoint.finalized_next_item_index
        });
    let response =
        maybe_attach_aleph_bft_finalized_items(state, request, finalized_end_item_index, response)
            .await
            .map_err(anyhow_response)?;
    if let Some(checkpoint) = target_checkpoint.as_ref() {
        response
            .validate_checkpoint_binding(checkpoint)
            .map_err(federation_error_response)?;
    }
    let response = maybe_attach_aleph_bft_backup(state, request, response)
        .await
        .map_err(anyhow_response)?;
    Ok(response)
}

async fn maybe_attach_aleph_bft_backup(
    state: &MintState,
    request: &FederationJournalCatchUpRequest,
    response: FederationJournalCatchUpResponse,
) -> anyhow::Result<FederationJournalCatchUpResponse> {
    let Some(backup_request) = request.aleph_bft_backup else {
        return Ok(response);
    };
    let query_limit = MAX_ALEPH_BFT_BACKUP_CHUNKS_PER_RESPONSE.saturating_add(1);
    let available = state
        .mint
        .localstore()
        .get_federation_aleph_bft_backup_chunks_from(
            backup_request.session,
            backup_request.next_chunk,
            query_limit,
        )
        .await?;
    let available_len = available.len();
    let mut chunks = Vec::new();
    let mut encoded_bytes = 0usize;
    for chunk in available
        .into_iter()
        .take(MAX_ALEPH_BFT_BACKUP_CHUNKS_PER_RESPONSE)
    {
        let next_bytes = encoded_bytes.saturating_add(chunk.len());
        if !chunks.is_empty() && next_bytes > MAX_ALEPH_BFT_BACKUP_BYTES_PER_RESPONSE {
            break;
        }
        encoded_bytes = next_bytes;
        chunks.push(chunk);
    }
    let complete = available_len <= chunks.len();
    Ok(
        response.with_aleph_bft_backup(FederationAlephBftBackupResponse {
            session: backup_request.session,
            from_chunk: backup_request.next_chunk,
            chunks,
            complete,
        }),
    )
}

async fn maybe_attach_aleph_bft_finalized_items(
    state: &MintState,
    request: &FederationJournalCatchUpRequest,
    finalized_end_item_index: u64,
    response: FederationJournalCatchUpResponse,
) -> anyhow::Result<FederationJournalCatchUpResponse> {
    let requested_limit = request.effective_limit(
        DEFAULT_FEDERATION_CATCH_UP_LIMIT,
        MAX_FEDERATION_CATCH_UP_LIMIT,
    );
    if request.aleph_bft_next_item_index > finalized_end_item_index {
        return Err(FederationError::CheckpointInvalid {
            reason: "catch-up finalized cursor is ahead of the available frontier".to_string(),
        }
        .into());
    }
    let finalized_items = state
        .mint
        .localstore()
        .get_federation_aleph_bft_finalized_items_range(
            request.aleph_bft_next_item_index,
            finalized_end_item_index,
            requested_limit.saturating_add(1),
        )
        .await?;
    let (finalized_items, fetched_page_complete) =
        truncate_to_complete_aleph_bft_units(finalized_items, requested_limit)?;
    let next_item_index = request
        .aleph_bft_next_item_index
        .checked_add(finalized_items.len() as u64)
        .ok_or_else(|| anyhow::anyhow!("catch-up finalized cursor overflow"))?;
    let complete = fetched_page_complete && next_item_index == finalized_end_item_index;
    Ok(response.with_aleph_bft_finalized_items(finalized_items, complete))
}

fn truncate_to_complete_aleph_bft_units(
    finalized_items: Vec<FederationAlephBftFinalizedItem>,
    limit: usize,
) -> anyhow::Result<(Vec<FederationAlephBftFinalizedItem>, bool)> {
    if finalized_items.is_empty() {
        return Ok((Vec::new(), true));
    }
    if finalized_items[0].item_index_in_unit != 0 {
        anyhow::bail!(
            "AlephBFT catch-up requested finalized item {} inside unit {} from member {}",
            finalized_items[0].accepted_item_index,
            finalized_items[0].unit_index,
            finalized_items[0].unit_submitting_member
        );
    }
    if limit == 0 {
        return Ok((Vec::new(), false));
    }

    let mut keep = 0;
    let mut cursor = 0;
    while cursor < finalized_items.len() {
        let item = &finalized_items[cursor];
        let session = item.session;
        let unit_finalization_index = item.unit_finalization_index;
        let unit_submitting_member = item.unit_submitting_member;
        let unit_index = item.unit_index;
        let unit_body_hash = item.unit_body_hash;
        let mut next = cursor;
        while next < finalized_items.len() {
            let next_item = &finalized_items[next];
            if next_item.session != session
                || next_item.unit_finalization_index != unit_finalization_index
                || next_item.unit_submitting_member != unit_submitting_member
                || next_item.unit_index != unit_index
                || next_item.unit_body_hash != unit_body_hash
            {
                break;
            }
            next += 1;
        }
        if next > limit {
            break;
        }
        keep = next;
        cursor = next;
    }

    let complete = keep == finalized_items.len();
    Ok((finalized_items[..keep].to_vec(), complete))
}
