use axum::extract::{Json, State};
use axum::response::Response;
use cdk::federation::{
    FederationAlephBftBackupResponse, FederationAlephBftFinalizedItem, FederationError,
    FederationJournalCatchUpRequest, FederationJournalCatchUpResponse, FederationRestMessageKind,
    FederationSignedRequest,
};
use tracing::instrument;

use crate::MintState;

use super::{
    acquire_private_rest_admission, anyhow_response, durable_federation_journal,
    federation_error_response, federation_runtime_config, materialized_state_digest,
    verify_federation_request,
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
    let private_config = federation_runtime_config(&state)?.private_config().clone();
    tracing::debug!(
        next_index = payload.payload.next_index,
        limit = ?payload.payload.limit,
        "Received federation journal catch-up request"
    );
    let response = durable_catch_up_response(&state, &payload.payload).await?;
    let response = response
        .with_signed_catch_up_certificate(&private_config)
        .map_err(federation_error_response)?;
    Ok(Json(response))
}

async fn durable_catch_up_response(
    state: &MintState,
    request: &FederationJournalCatchUpRequest,
) -> Result<FederationJournalCatchUpResponse, Response> {
    let runtime_config = federation_runtime_config(state)?;
    let journal = durable_federation_journal(state)
        .await
        .map_err(anyhow_response)?;

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

    let entries = journal.entries().to_vec();
    let state_digest = materialized_state_digest(state, &entries)
        .await
        .map_err(anyhow_response)?;
    let response = FederationJournalCatchUpResponse::from_journal(
        runtime_config.federation_id(),
        &journal,
        state_digest,
        request,
        DEFAULT_FEDERATION_CATCH_UP_LIMIT,
        MAX_FEDERATION_CATCH_UP_LIMIT,
    );
    let response = maybe_attach_historical_catch_up_certificate_quorum(state, request, response)
        .await
        .map_err(anyhow_response)?;
    let response = maybe_attach_aleph_bft_finalized_items(state, request, response)
        .await
        .map_err(anyhow_response)?;
    let response = maybe_attach_aleph_bft_backup(state, request, response)
        .await
        .map_err(anyhow_response)?;
    Ok(response)
}

async fn maybe_attach_historical_catch_up_certificate_quorum(
    state: &MintState,
    request: &FederationJournalCatchUpRequest,
    mut response: FederationJournalCatchUpResponse,
) -> anyhow::Result<FederationJournalCatchUpResponse> {
    if !request.allow_historical_certificate_page || response.entries.is_empty() {
        return Ok(response);
    }
    let runtime_config =
        state
            .mint
            .federation_config()
            .ok_or_else(|| FederationError::RuntimeUnavailable {
                component: "mint configuration",
            })?;
    let public_config = runtime_config.public_config();
    let required = usize::from(public_config.threshold_params.consensus_threshold);
    let mut certificates = state
        .mint
        .localstore()
        .get_federation_catch_up_certificates_at(response.from_index)
        .await?;
    if certificates.len() < required {
        return Ok(response);
    }
    certificates.sort_by_key(|certificate| std::cmp::Reverse(certificate.operation_count()));

    for candidate in &certificates {
        if candidate.operation_count() == 0
            || candidate.latest_order_digest != candidate.range_order_digest()
        {
            continue;
        }
        let Ok(operation_count) = usize::try_from(candidate.operation_count()) else {
            continue;
        };
        let Some(entries) = response.entries.get(..operation_count) else {
            continue;
        };
        let mut signers = std::collections::BTreeSet::new();
        let mut relayed = Vec::with_capacity(required);
        for certificate in &certificates {
            let Some(member) = public_config.member(certificate.signer) else {
                continue;
            };
            if certificate
                .verify(
                    member,
                    response.federation_id,
                    response.from_index,
                    entries,
                    candidate.latest_order_digest,
                    candidate.materialized_state_digest,
                )
                .is_ok()
                && signers.insert(certificate.signer)
            {
                relayed.push(certificate.clone());
            }
        }
        if signers.len() < required {
            continue;
        }

        let original_entry_count = response.entries.len();
        response.entries.truncate(operation_count);
        response.latest_index = Some(candidate.latest_index());
        response.latest_order_digest = candidate.latest_order_digest;
        response.materialized_state_digest = candidate.materialized_state_digest;
        response.complete = response.complete && operation_count == original_entry_count;
        response.catch_up_certificates = relayed;
        return Ok(response);
    }

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
    response: FederationJournalCatchUpResponse,
) -> anyhow::Result<FederationJournalCatchUpResponse> {
    let finalized_items = state
        .mint
        .localstore()
        .get_federation_aleph_bft_finalized_items_from(request.aleph_bft_next_item_index)
        .await?;
    let requested_limit = request.effective_limit(
        DEFAULT_FEDERATION_CATCH_UP_LIMIT,
        MAX_FEDERATION_CATCH_UP_LIMIT,
    );
    let limit = if response.catch_up_certificates.is_empty() {
        requested_limit
    } else {
        requested_limit.min(response.entries.len())
    };
    let (finalized_items, complete) = truncate_to_complete_aleph_bft_units(finalized_items, limit)?;
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
