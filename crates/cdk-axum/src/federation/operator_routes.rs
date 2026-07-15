//! Authenticated, redacted operator routes for the federated on-chain wallet.

use axum::extract::{Json, State};
use axum::response::Response;
use cdk::federation::{
    FederationRestMessageKind, FederationSignedRequest, FederationWalletRebroadcastResponse,
    FederationWalletTransactionRequest, FederationWalletTransactionStatus,
    FEDERATION_REST_API_VERSION,
};
use tracing::instrument;

use crate::MintState;

use super::{
    acquire_private_rest_admission, cdk_error_response, federation_error_response,
    verify_federation_request,
};

#[instrument(skip_all)]
pub(super) async fn post_wallet_transaction_status(
    State(state): State<MintState>,
    Json(payload): Json<FederationSignedRequest<FederationWalletTransactionRequest>>,
) -> Result<Json<FederationWalletTransactionStatus>, Response> {
    let kind = FederationRestMessageKind::WalletTransactionStatus;
    let _permit = acquire_private_rest_admission(&state, kind)?;
    let verified = verify_federation_request(&state, &payload, kind).await?;
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
        .record_federation_sequence(kind, verified.sender, verified.sequence)
        .await;
    Ok(Json(status))
}

#[instrument(skip_all)]
pub(super) async fn post_wallet_rebroadcast(
    State(state): State<MintState>,
    Json(payload): Json<FederationSignedRequest<FederationWalletTransactionRequest>>,
) -> Result<Json<FederationWalletRebroadcastResponse>, Response> {
    let kind = FederationRestMessageKind::WalletRebroadcast;
    let _permit = acquire_private_rest_admission(&state, kind)?;
    let verified = verify_federation_request(&state, &payload, kind).await?;
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
        .record_federation_sequence(kind, verified.sender, verified.sequence)
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
