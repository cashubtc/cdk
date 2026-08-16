//! Authenticated, redacted operator routes for the federated on-chain wallet.

#[cfg(any(feature = "federation-bdk", feature = "federation-bark"))]
use axum::extract::{Json, State};
#[cfg(any(feature = "federation-bdk", feature = "federation-bark"))]
use axum::response::Response;
#[cfg(feature = "federation-bark")]
#[cfg(any(feature = "federation-bdk", feature = "federation-bark"))]
use cdk::federation::{
    FederationAddressAllocationScope, FederationAddressKeychain, FederationBarkActionKind,
    FederationBarkActionRecord, FederationBarkReconcileRequest, FederationBarkReconcileResponse,
    FederationBarkRecoveryAddressRequest, FederationBarkRecoveryAddressResponse,
    FederationBarkRetryRequest, FederationBarkRetryResponse, FederationError,
};
#[cfg(any(feature = "federation-bdk", feature = "federation-bark"))]
use cdk::federation::{
    FederationRestMessageKind, FederationSignedRequest, FEDERATION_REST_API_VERSION,
};
#[cfg(feature = "federation-bdk")]
use cdk::federation::{
    FederationWalletRebroadcastResponse, FederationWalletTransactionRequest,
    FederationWalletTransactionStatus,
};
#[cfg(feature = "federation-bark")]
use cdk::mint::QuoteId;
#[cfg(any(feature = "federation-bdk", feature = "federation-bark"))]
use tracing::instrument;

#[cfg(any(feature = "federation-bdk", feature = "federation-bark"))]
use crate::MintState;

#[cfg(feature = "federation-bark")]
use super::anyhow_response;
#[cfg(any(feature = "federation-bdk", feature = "federation-bark"))]
use super::{admit_private_request, cdk_error_response, federation_error_response};

#[cfg(feature = "federation-bdk")]
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

#[cfg(feature = "federation-bdk")]
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

#[cfg(feature = "federation-bdk")]
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
    let kind = FederationRestMessageKind::ApplicationReconcile;
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
    let kind = FederationRestMessageKind::ApplicationActionRetry;
    let verified = admit_private_request(&state, &payload, kind).await?;
    payload
        .payload
        .validate()
        .map_err(federation_error_response)?;
    state
        .ensure_federation_ready_for_authoritative_read()
        .await
        .map_err(cdk_error_response)?;
    let projection = super::bark_maintenance::recovery_projection(
        &state,
        state
            .federation
            .bark_runtime()
            .ok_or_else(|| cdk_error_response(cdk::Error::Internal))?,
    )
    .await
    .map_err(cdk_error_response)?;
    let action = projection
        .actions
        .iter()
        .find(|action| action.action_id == payload.payload.action_id)
        .cloned()
        .ok_or_else(|| {
            federation_error_response(FederationError::ApplicationOperationInvalid {
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
#[instrument(skip_all)]
pub(super) async fn post_bark_recovery_address(
    State(state): State<MintState>,
    Json(payload): Json<FederationSignedRequest<FederationBarkRecoveryAddressRequest>>,
) -> Result<Json<FederationBarkRecoveryAddressResponse>, Response> {
    let kind = FederationRestMessageKind::ApplicationWalletAddress;
    let verified = admit_private_request(&state, &payload, kind).await?;
    payload
        .payload
        .validate()
        .map_err(federation_error_response)?;
    state
        .ensure_federation_ready_for_authoritative_read()
        .await
        .map_err(cdk_error_response)?;
    let federation = state
        .mint
        .federation_config()
        .cloned()
        .ok_or_else(|| cdk_error_response(cdk::Error::Internal))?;
    let wallet = federation
        .public_config()
        .frost_wallet
        .as_ref()
        .ok_or_else(|| cdk_error_response(cdk::Error::Internal))?;
    let recovery =
        cdk::federation::federation_bark_recovery_application(federation.public_config())
            .map_err(federation_error_response)?
            .ok_or_else(|| cdk_error_response(cdk::Error::Internal))?;
    let quote = payload
        .payload
        .allocation_quote()
        .map_err(federation_error_response)?;
    let scope = FederationAddressAllocationScope {
        application_id: recovery.application_id(),
        application_key_epoch: recovery.application_key_epoch(),
    };
    let allocation = match state
        .mint
        .federation_address_allocation(scope, &quote)
        .await
        .map_err(cdk_error_response)?
    {
        Some(allocation) => allocation,
        None => {
            let envelope = federation
                .address_allocation_operation(
                    &quote,
                    recovery.application_id(),
                    recovery.application_key_epoch(),
                    FederationAddressKeychain::External,
                )
                .map_err(cdk_error_response)?;
            state
                .submit_federation_envelope(envelope)
                .await
                .map_err(cdk_error_response)?
                .ensure_accepted()
                .map_err(federation_error_response)?;
            state
                .mint
                .federation_address_allocation(scope, &quote)
                .await
                .map_err(cdk_error_response)?
                .ok_or_else(|| cdk_error_response(cdk::Error::Internal))?
        }
    };
    let script = recovery
        .derive_script(
            federation.public_config(),
            &wallet.root,
            false,
            allocation.derivation_index,
        )
        .map_err(federation_error_response)?;
    let address = cdk_common::bitcoin::Address::from_script(&script, recovery.network())
        .map_err(|_| cdk_error_response(cdk::Error::Internal))?;
    state
        .record_federation_sequence(kind, verified.sender(), verified.sequence())
        .await;
    Ok(Json(FederationBarkRecoveryAddressResponse {
        version: FEDERATION_REST_API_VERSION,
        reserve_id: payload.payload.reserve_id,
        application_id: recovery.application_id(),
        application_key_epoch: recovery.application_key_epoch(),
        derivation_index: allocation.derivation_index,
        address: address.to_string(),
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
            super::bark_outgoing::advance_federated_bark_melt_quote(state, &quote, 0)
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
            super::bark_incoming::advance_federated_bark_mint_quote(state, &quote, 0)
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
