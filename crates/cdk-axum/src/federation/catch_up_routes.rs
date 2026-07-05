use axum::extract::{Json, State};
use axum::response::Response;
use cdk::federation::{
    FederationError, FederationJournalCatchUpRequest, FederationJournalCatchUpResponse,
    FederationJournalEntry, FederationRestMessageKind, FederationSignedRequest,
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
    let entries = journal.entries().to_vec();

    Ok((response, entries))
}
