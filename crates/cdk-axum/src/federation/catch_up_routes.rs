use axum::extract::{Json, State};
use axum::response::Response;
use cdk::federation::{
    FederationAlephBftFinalizedItem, FederationConsensusBackendKind, FederationError,
    FederationJournalCatchUpRequest, FederationJournalCatchUpResponse, FederationJournalEntry,
    FederationRestMessageKind, FederationSignedRequest,
};
use tracing::instrument;

use crate::MintState;

use super::{
    acquire_private_rest_admission, anyhow_response, cdk_error_response,
    durable_federation_journal, federation_error_response, federation_runtime_config,
    materialized_state_digest, verify_federation_request,
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
        .persist_federation_sequence(verified.sender, verified.sequence)
        .await
        .map_err(cdk_error_response)?;
    let private_config = federation_runtime_config(&state)?.private_config().clone();
    tracing::debug!(
        next_index = payload.payload.next_index,
        limit = ?payload.payload.limit,
        "Received federation journal catch-up request"
    );
    let (mut response, entries) = durable_catch_up_response(&state, &payload.payload).await?;
    response.state_digest = materialized_state_digest(&state, &entries)
        .await
        .map_err(anyhow_response)?;
    let response = response
        .with_signed_session_outcome(&private_config)
        .map_err(federation_error_response)?;
    Ok(Json(response))
}

async fn durable_catch_up_response(
    state: &MintState,
    request: &FederationJournalCatchUpRequest,
) -> Result<
    (
        FederationJournalCatchUpResponse,
        Vec<FederationJournalEntry>,
    ),
    Response,
> {
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

    let response = FederationJournalCatchUpResponse::from_journal(
        runtime_config.federation_id(),
        &journal,
        request,
        DEFAULT_FEDERATION_CATCH_UP_LIMIT,
        MAX_FEDERATION_CATCH_UP_LIMIT,
    );
    let response = maybe_attach_aleph_bft_finalized_items(
        state,
        request,
        runtime_config.consensus_backend(),
        response,
    )
    .await
    .map_err(anyhow_response)?;
    let entries = journal.entries().to_vec();

    Ok((response, entries))
}

async fn maybe_attach_aleph_bft_finalized_items(
    state: &MintState,
    request: &FederationJournalCatchUpRequest,
    consensus_backend: FederationConsensusBackendKind,
    response: FederationJournalCatchUpResponse,
) -> anyhow::Result<FederationJournalCatchUpResponse> {
    if consensus_backend != FederationConsensusBackendKind::AlephBft {
        return Ok(response.with_aleph_bft_finalized_items(Vec::new(), true));
    }

    let finalized_items = state
        .mint
        .localstore()
        .get_federation_aleph_bft_finalized_items_from(request.aleph_bft_next_item_index)
        .await?;
    let limit = request.effective_limit(
        DEFAULT_FEDERATION_CATCH_UP_LIMIT,
        MAX_FEDERATION_CATCH_UP_LIMIT,
    );
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
