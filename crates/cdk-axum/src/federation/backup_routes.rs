use axum::extract::{Json, State};
use axum::response::Response;
use cdk::federation::{
    FederationAlephBftBackupRequest, FederationAlephBftBackupResponse, FederationError,
    FederationRestMessageKind, FederationSignedRequest,
    FEDERATION_MAX_ALEPH_BFT_BACKUP_CHUNKS_PER_PAGE,
    FEDERATION_MAX_ALEPH_BFT_BACKUP_RAW_BYTES_PER_PAGE,
};
use tracing::instrument;

use crate::MintState;

use super::{
    acquire_private_rest_admission, anyhow_response, federation_error_response,
    federation_runtime_config, verify_federation_request,
};

#[instrument(skip_all)]
pub(super) async fn post_aleph_bft_backup(
    State(state): State<MintState>,
    Json(payload): Json<FederationSignedRequest<FederationAlephBftBackupRequest>>,
) -> Result<Json<FederationSignedRequest<FederationAlephBftBackupResponse>>, Response> {
    let kind = FederationRestMessageKind::AlephBftBackup;
    let _permit = acquire_private_rest_admission(&state, kind)?;
    let verified = verify_federation_request(&state, &payload, kind).await?;
    payload
        .payload
        .validate()
        .map_err(federation_error_response)?;
    state
        .record_federation_sequence(kind, verified.sender, verified.sequence)
        .await;

    let runtime_config = federation_runtime_config(&state)?;
    let request = payload.payload;
    let response = backup_page(&state, request).await?;
    response
        .validate(
            runtime_config.federation_id(),
            runtime_config.member_id(),
            request,
        )
        .map_err(federation_error_response)?;
    let response = FederationSignedRequest::new_signed(
        runtime_config.federation_id(),
        verified.sender,
        verified.sequence,
        kind,
        response,
        runtime_config.private_config(),
    )
    .map_err(federation_error_response)?;
    Ok(Json(response))
}

async fn backup_page(
    state: &MintState,
    request: FederationAlephBftBackupRequest,
) -> Result<FederationAlephBftBackupResponse, Response> {
    let runtime_config = federation_runtime_config(state)?;
    let query_limit = FEDERATION_MAX_ALEPH_BFT_BACKUP_CHUNKS_PER_PAGE.saturating_add(1);
    let available = state
        .mint
        .localstore()
        .get_federation_aleph_bft_backup_chunks_from(
            request.session,
            request.next_chunk,
            query_limit,
        )
        .await
        .map_err(|err| anyhow_response(err.into()))?;
    let available_len = available.len();
    let mut chunks = Vec::new();
    let mut raw_bytes = 0usize;
    for chunk in available
        .into_iter()
        .take(FEDERATION_MAX_ALEPH_BFT_BACKUP_CHUNKS_PER_PAGE)
    {
        let next_bytes = raw_bytes.checked_add(chunk.len()).ok_or_else(|| {
            federation_error_response(FederationError::CheckpointCatchUpTooLarge {
                resource: "AlephBFT backup raw bytes",
                actual: u64::MAX,
                max: FEDERATION_MAX_ALEPH_BFT_BACKUP_RAW_BYTES_PER_PAGE as u64,
            })
        })?;
        if next_bytes > FEDERATION_MAX_ALEPH_BFT_BACKUP_RAW_BYTES_PER_PAGE {
            if chunks.is_empty() {
                return Err(federation_error_response(
                    FederationError::CheckpointCatchUpTooLarge {
                        resource: "single AlephBFT backup chunk",
                        actual: u64::try_from(next_bytes).unwrap_or(u64::MAX),
                        max: FEDERATION_MAX_ALEPH_BFT_BACKUP_RAW_BYTES_PER_PAGE as u64,
                    },
                ));
            }
            break;
        }
        raw_bytes = next_bytes;
        chunks.push(chunk);
    }
    let complete = available_len <= chunks.len();
    Ok(FederationAlephBftBackupResponse::new(
        runtime_config.federation_id(),
        runtime_config.member_id(),
        request,
        chunks,
        complete,
    ))
}
