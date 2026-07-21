//! Authenticated, redacted operator routes for the federated on-chain wallet.

use axum::extract::{Json, State};
use axum::response::Response;
#[cfg(feature = "federation-bark")]
use cdk::federation::{
    FederationBarkActionKind, FederationBarkActionRecord, FederationBarkReconcileRequest,
    FederationBarkReconcileResponse, FederationBarkRetryRequest, FederationBarkRetryResponse,
    FederationError,
};
use cdk::federation::{
    FederationRestMessageKind, FederationSignedRequest, FederationWalletRebroadcastResponse,
    FederationWalletTransactionRequest, FederationWalletTransactionStatus,
    FEDERATION_REST_API_VERSION,
};
#[cfg(feature = "federation-bark")]
use cdk::mint::QuoteId;
use tracing::instrument;

use crate::MintState;

#[cfg(feature = "federation-bark")]
use super::anyhow_response;
use super::{admit_private_request, cdk_error_response, federation_error_response};

#[instrument(skip_all)]
pub(super) async fn post_wallet_transaction_status(
    State(state): State<MintState>,
    Json(payload): Json<FederationSignedRequest<FederationWalletTransactionRequest>>,
) -> Result<Json<FederationWalletTransactionStatus>, Response> {
    let kind = FederationRestMessageKind::WalletTransactionStatus;
    let verified = admit_private_request(&state, &payload, kind).await?;
    payload
        .payload
        .validate()
        .map_err(federation_error_response)?;
    state
        .ensure_federation_ready_for_authoritative_read()
        .await
        .map_err(cdk_error_response)?;
    let status = transaction_status(&state, payload.payload.proposal_id).await?;
    state
        .record_federation_sequence(kind, verified.sender(), verified.sequence())
        .await;
    Ok(Json(status))
}

#[instrument(skip_all)]
pub(super) async fn post_wallet_rebroadcast(
    State(state): State<MintState>,
    Json(payload): Json<FederationSignedRequest<FederationWalletTransactionRequest>>,
) -> Result<Json<FederationWalletRebroadcastResponse>, Response> {
    let kind = FederationRestMessageKind::WalletRebroadcast;
    let verified = admit_private_request(&state, &payload, kind).await?;
    payload
        .payload
        .validate()
        .map_err(federation_error_response)?;
    state
        .ensure_federation_ready_for_authoritative_read()
        .await
        .map_err(cdk_error_response)?;
    // Resolve first so an unknown proposal never records a successful operator
    // request or wakes unrelated transactions.
    transaction_status(&state, payload.payload.proposal_id).await?;
    let advanced = state
        .federation
        .transaction_broadcaster
        .run_proposal_now(&state, payload.payload.proposal_id)
        .await
        .map_err(cdk_error_response)?
        > 0;
    let status = transaction_status(&state, payload.payload.proposal_id).await?;
    state
        .record_federation_sequence(kind, verified.sender(), verified.sequence())
        .await;
    Ok(Json(FederationWalletRebroadcastResponse {
        version: FEDERATION_REST_API_VERSION,
        advanced,
        status,
    }))
}

async fn transaction_status(
    state: &MintState,
    proposal_id: cdk::federation::FederationTransactionProposalId,
) -> Result<FederationWalletTransactionStatus, Response> {
    let transactions = state
        .mint
        .federation_transaction_state()
        .await
        .map_err(cdk_error_response)?;
    FederationWalletTransactionStatus::from_transactions(&transactions, proposal_id)
        .map_err(federation_error_response)
}

#[cfg(feature = "federation-bark")]
#[instrument(skip_all)]
pub(super) async fn post_bark_reconcile(
    State(state): State<MintState>,
    Json(payload): Json<FederationSignedRequest<FederationBarkReconcileRequest>>,
) -> Result<Json<FederationBarkReconcileResponse>, Response> {
    let kind = FederationRestMessageKind::BarkReconcile;
    let verified = admit_private_request(&state, &payload, kind).await?;
    payload
        .payload
        .validate()
        .map_err(federation_error_response)?;
    state
        .ensure_federation_ready_for_authoritative_read()
        .await
        .map_err(cdk_error_response)?;
    let runtime = state
        .federation
        .bark_runtime()
        .cloned()
        .ok_or_else(|| cdk_error_response(cdk::Error::Internal))?;
    let projection = state
        .mint
        .federation_bark_projection_snapshot()
        .await
        .map_err(cdk_error_response)?;
    let accepted_frontier = projection.frontier().unwrap_or(0);
    let local_frontier_before = runtime.status(accepted_frontier).await.local_frontier;
    let entries = state
        .mint
        .localstore()
        .get_federation_journal_entries()
        .await
        .map_err(|error| cdk_error_response(error.into()))?;
    crate::reconcile_federation_bark_runtime(&state.mint, &runtime, &entries)
        .await
        .map_err(anyhow_response)?;
    let runtime_status = runtime.status(accepted_frontier).await;
    state
        .record_federation_sequence(kind, verified.sender(), verified.sequence())
        .await;
    Ok(Json(FederationBarkReconcileResponse {
        version: FEDERATION_REST_API_VERSION,
        accepted_frontier,
        local_frontier_before,
        local_frontier_after: runtime_status.local_frontier,
        readiness: runtime_status.readiness.as_str().to_string(),
        degraded_reason: runtime_status
            .degraded_reason
            .map(cdk_bark::FederatedBarkDegradedReason::as_str)
            .map(str::to_string),
    }))
}

#[cfg(feature = "federation-bark")]
#[instrument(skip_all)]
pub(super) async fn post_bark_action_retry(
    State(state): State<MintState>,
    Json(payload): Json<FederationSignedRequest<FederationBarkRetryRequest>>,
) -> Result<Json<FederationBarkRetryResponse>, Response> {
    let kind = FederationRestMessageKind::BarkActionRetry;
    let verified = admit_private_request(&state, &payload, kind).await?;
    payload
        .payload
        .validate()
        .map_err(federation_error_response)?;
    state
        .ensure_federation_ready_for_authoritative_read()
        .await
        .map_err(cdk_error_response)?;
    let projection = crate::federation_bark_ready_projection(
        &state.mint,
        state
            .federation
            .bark_runtime()
            .ok_or_else(|| cdk_error_response(cdk::Error::Internal))?,
    )
    .await
    .map_err(anyhow_response)?;
    let action = projection
        .actions
        .iter()
        .find(|action| action.action_id == payload.payload.action_id)
        .cloned()
        .ok_or_else(|| {
            federation_error_response(FederationError::BarkOperationInvalid {
                reason: "operator retry target is not an accepted Bark action".to_string(),
            })
        })?;
    let phase_before = action.phase;
    let advanced = retry_bark_action(&state, &action).await?;
    let refreshed = state
        .mint
        .federation_bark_projection_snapshot()
        .await
        .map_err(cdk_error_response)?;
    let phase_after = refreshed
        .actions
        .iter()
        .find(|current| current.action_id == action.action_id)
        .map(|current| current.phase)
        .ok_or_else(|| cdk_error_response(cdk::Error::Internal))?;
    state
        .record_federation_sequence(kind, verified.sender(), verified.sequence())
        .await;
    Ok(Json(FederationBarkRetryResponse {
        version: FEDERATION_REST_API_VERSION,
        action_id: action.action_id,
        phase_before,
        phase_after,
        advanced: u64::try_from(advanced).unwrap_or(u64::MAX),
    }))
}

#[cfg(feature = "federation-bark")]
async fn retry_bark_action(
    state: &MintState,
    action: &FederationBarkActionRecord,
) -> Result<usize, Response> {
    match action.action_kind {
        Some(FederationBarkActionKind::LightningSend)
        | Some(FederationBarkActionKind::FailedSendRevocation) => {
            let quote = action_quote_id(action).map_err(cdk_error_response)?;
            super::bark_outgoing::advance_federated_bark_melt_quote(state, &quote)
                .await
                .map(|outcome| {
                    usize::from(matches!(
                        outcome,
                        super::payment_schedule::FederationPaymentAdvancement::Progressed
                    ))
                })
                .map_err(cdk_error_response)
        }
        Some(FederationBarkActionKind::IncomingClaim) | None => {
            if action.phase == cdk::federation::FederationBarkSemanticPhase::ReceiveIntent {
                return super::bark_incoming::advance_federated_bark_receive_intent(
                    state,
                    action.action_id,
                )
                .await
                .map_err(cdk_error_response);
            }
            let quote = action_quote_id(action).map_err(cdk_error_response)?;
            super::bark_incoming::advance_federated_bark_mint_quote(state, &quote)
                .await
                .map(|outcome| {
                    usize::from(matches!(
                        outcome,
                        super::payment_schedule::FederationPaymentAdvancement::Progressed
                    ))
                })
                .map_err(cdk_error_response)
        }
        Some(FederationBarkActionKind::Refresh) | Some(FederationBarkActionKind::EmergencyExit) => {
            super::bark_maintenance::retry_federated_bark_maintenance_action(
                state.clone(),
                action.action_id,
            )
            .await
            .map_err(cdk_error_response)
        }
    }
}

#[cfg(feature = "federation-bark")]
fn action_quote_id(action: &FederationBarkActionRecord) -> Result<QuoteId, cdk::Error> {
    action
        .quote
        .as_deref()
        .ok_or(cdk::Error::Internal)?
        .parse::<QuoteId>()
        .map_err(|_| cdk::Error::Internal)
}
