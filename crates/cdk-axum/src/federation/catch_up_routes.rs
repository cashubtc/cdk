use axum::extract::{Json, State};
use axum::response::Response;
use cdk::federation::{
    FederationAlephBftFinalizedItem, FederationError, FederationJournalCatchUpRequest,
    FederationJournalCatchUpResponse, FederationRestMessageKind, FederationSignedRequest,
    InMemoryFederationJournal, FEDERATION_MAX_ALEPH_BFT_UNIT_ITEMS,
    FEDERATION_MAX_CHECKPOINT_CATCH_UP_PAGE_BYTES,
};
use tracing::instrument;

use crate::MintState;

use super::{
    acquire_private_rest_admission, anyhow_response, federation_error_response,
    federation_runtime_config, verify_federation_request,
};

const DEFAULT_FEDERATION_CATCH_UP_LIMIT: u16 = 100;
const MAX_FEDERATION_CATCH_UP_LIMIT: u16 = 1000;

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
                .validate_against_validated_config(runtime_config.public_config())
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
    let fetch_limit = limit.max(FEDERATION_MAX_ALEPH_BFT_UNIT_ITEMS);
    let entries = state
        .mint
        .localstore()
        .get_federation_journal_entries_range(request.next_index, end_index, fetch_limit)
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

    let mut response_request = request.clone();
    response_request.limit = Some(u16::try_from(fetch_limit).map_err(|_| {
        federation_error_response(FederationError::CheckpointInvalid {
            reason: "catch-up fetch limit exceeds u16".to_string(),
        })
    })?);
    let mut response = FederationJournalCatchUpResponse::from_journal(
        runtime_config.federation_id(),
        &journal,
        state_digest,
        &response_request,
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
    let response = align_catch_up_page_to_finalized_prefix(response, end_index)
        .map_err(federation_error_response)?;
    let response =
        fit_checkpoint_page_to_byte_limit(response).map_err(federation_error_response)?;
    if let Some(checkpoint) = target_checkpoint.as_ref() {
        response
            .validate_checkpoint_binding(checkpoint)
            .map_err(federation_error_response)?;
    }
    Ok(response)
}

pub(super) fn fit_checkpoint_page_to_byte_limit(
    mut response: FederationJournalCatchUpResponse,
) -> Result<FederationJournalCatchUpResponse, FederationError> {
    loop {
        let encoded_size = response.encoded_size()?;
        if encoded_size <= FEDERATION_MAX_CHECKPOINT_CATCH_UP_PAGE_BYTES {
            return Ok(response);
        }

        if let Some(last) = response.aleph_bft_finalized_items.last() {
            let last_unit_start = response
                .aleph_bft_finalized_items
                .iter()
                .rposition(|item| {
                    item.session != last.session
                        || item.unit_finalization_index != last.unit_finalization_index
                        || item.unit_submitting_member != last.unit_submitting_member
                        || item.unit_index != last.unit_index
                        || item.unit_body_hash != last.unit_body_hash
                })
                .map_or(0, |index| index.saturating_add(1));
            if last_unit_start == 0 {
                return Err(FederationError::CheckpointCatchUpTooLarge {
                    resource: "single finalized unit page",
                    actual: u64::try_from(encoded_size).unwrap_or(u64::MAX),
                    max: FEDERATION_MAX_CHECKPOINT_CATCH_UP_PAGE_BYTES as u64,
                });
            }
            let retain_at_most = response
                .aleph_bft_finalized_items
                .len()
                .saturating_mul(FEDERATION_MAX_CHECKPOINT_CATCH_UP_PAGE_BYTES)
                .checked_div(encoded_size)
                .unwrap_or_default()
                .min(response.aleph_bft_finalized_items.len().saturating_sub(1));
            let mut retained_items = 0;
            let mut cursor = 0;
            while cursor < response.aleph_bft_finalized_items.len() {
                let unit = &response.aleph_bft_finalized_items[cursor];
                let mut next = cursor.saturating_add(1);
                while next < response.aleph_bft_finalized_items.len() {
                    let next_item = &response.aleph_bft_finalized_items[next];
                    if next_item.session != unit.session
                        || next_item.unit_finalization_index != unit.unit_finalization_index
                        || next_item.unit_submitting_member != unit.unit_submitting_member
                        || next_item.unit_index != unit.unit_index
                        || next_item.unit_body_hash != unit.unit_body_hash
                    {
                        break;
                    }
                    next = next.saturating_add(1);
                }
                if retained_items != 0 && next > retain_at_most {
                    break;
                }
                retained_items = next;
                cursor = next;
            }
            retained_items = retained_items.min(last_unit_start);
            let removed_accepted_operations = response.aleph_bft_finalized_items[retained_items..]
                .iter()
                .filter(|item| {
                    item.status == cdk::federation::FederationAlephBftFinalizedItemStatus::Accepted
                        && item.operation_id.is_some()
                })
                .count();
            let retained_entries = response
                .entries
                .len()
                .checked_sub(removed_accepted_operations)
                .ok_or_else(|| FederationError::CheckpointInvalid {
                    reason: "catch-up page finalized prefix has too few journal claims".to_string(),
                })?;
            response.entries.truncate(retained_entries);
            response.complete = false;
            response.aleph_bft_finalized_items.truncate(retained_items);
            response.aleph_bft_finalized_items_complete = false;
            continue;
        }

        return Err(FederationError::CheckpointCatchUpTooLarge {
            resource: "page metadata and backup",
            actual: u64::try_from(encoded_size).unwrap_or(u64::MAX),
            max: FEDERATION_MAX_CHECKPOINT_CATCH_UP_PAGE_BYTES as u64,
        });
    }
}

fn align_catch_up_page_to_finalized_prefix(
    mut response: FederationJournalCatchUpResponse,
    journal_end_index: u64,
) -> Result<FederationJournalCatchUpResponse, FederationError> {
    let accepted_operations = response
        .aleph_bft_finalized_items
        .iter()
        .filter(|item| {
            item.status == cdk::federation::FederationAlephBftFinalizedItemStatus::Accepted
                && item.operation_id.is_some()
        })
        .count();
    if response.entries.len() < accepted_operations {
        return Err(FederationError::CheckpointInvalid {
            reason: "catch-up finalized prefix exceeds the available journal claims".to_string(),
        });
    }
    response.entries.truncate(accepted_operations);
    let returned_next_index = response
        .from_index
        .checked_add(u64::try_from(response.entries.len()).map_err(|_| {
            FederationError::CheckpointInvalid {
                reason: "catch-up journal page length exceeds u64".to_string(),
            }
        })?)
        .ok_or_else(|| FederationError::CheckpointInvalid {
            reason: "catch-up journal cursor overflow".to_string(),
        })?;
    response.complete = returned_next_index == journal_end_index;
    Ok(response)
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
    let fetch_limit = requested_limit.max(FEDERATION_MAX_ALEPH_BFT_UNIT_ITEMS);
    let finalized_items = state
        .mint
        .localstore()
        .get_federation_aleph_bft_finalized_items_range(
            request.aleph_bft_next_item_index,
            finalized_end_item_index,
            fetch_limit.saturating_add(1),
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
        if next > limit && keep != 0 {
            break;
        }
        keep = next;
        cursor = next;
    }

    let complete = keep == finalized_items.len();
    Ok((finalized_items[..keep].to_vec(), complete))
}
