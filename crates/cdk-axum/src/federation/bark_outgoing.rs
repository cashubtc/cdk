//! Pure outgoing Bark planning and accepted local checkpoint projection.

use std::str::FromStr;

use cdk::federation::{
    FederationBarkActionKind, FederationBarkOperation, FederationBarkSemanticPhase,
    FederationOperation, FederationOperationId,
};
use cdk::mint::QuoteId;
use cdk::nuts::MeltQuoteState;

use crate::MintState;

/// Advance one accepted pending melt through plan acceptance only.
///
/// This function never starts Bark's executor. Milestone 13 owns the first
/// Ark request after the exact plan has been ordered and projected locally.
pub(super) async fn advance_federated_bark_melt_quote(
    state: &MintState,
    quote_id: &QuoteId,
) -> Result<usize, cdk::Error> {
    let federation = state.mint.federation_config().ok_or(cdk::Error::Internal)?;
    let runtime = state
        .federation
        .bark_runtime()
        .ok_or(cdk::Error::Internal)?;
    let quote = state
        .mint
        .localstore()
        .get_melt_quote(quote_id)
        .await?
        .ok_or(cdk::Error::UnknownQuote)?;
    if quote.state != MeltQuoteState::Pending {
        return Ok(0);
    }
    let projection = state.mint.federation_bark_projection_snapshot().await?;
    let quote_label = quote_id.to_string();
    if let Some(action) = projection.actions.iter().find(|action| {
        action.quote.as_deref() == Some(quote_label.as_str())
            && matches!(
                action.phase,
                FederationBarkSemanticPhase::SendPlanned
                    | FederationBarkSemanticPhase::SendHtlcsCosigned
                    | FederationBarkSemanticPhase::PaymentInitiated
                    | FederationBarkSemanticPhase::PaymentSucceeded
                    | FederationBarkSemanticPhase::PaymentFailed
            )
    }) {
        if let FederationBarkOperation::ActionPlan(plan) = &action.operation {
            runtime
                .apply_lightning_send_plan(&quote, &projection, plan)
                .await
                .map_err(|_| cdk::Error::Internal)?;
        }
        return Ok(0);
    }

    let saga = state
        .mint
        .localstore()
        .get_melt_saga_by_quote_id(quote_id)
        .await?
        .ok_or(cdk::Error::Internal)?;
    let predecessor_operation_id = FederationOperationId::from_str(
        saga.federation_operation_id
            .as_deref()
            .ok_or(cdk::Error::Internal)?,
    )?;
    let actor = state
        .mint
        .federation_melt_payment_actor(predecessor_operation_id)
        .await?;
    if actor != federation.member_id() {
        return Ok(0);
    }
    let application = federation
        .public_config()
        .frost_wallet
        .as_ref()
        .and_then(|wallet| wallet.bark_application.as_ref())
        .ok_or(cdk::Error::Internal)?;
    let signer_set = federation
        .public_config()
        .members
        .keys()
        .copied()
        .take(usize::from(application.threshold))
        .collect::<Vec<_>>();
    let plan = runtime
        .propose_lightning_send(
            &quote,
            predecessor_operation_id,
            &projection,
            cdk_bark::FederatedBarkSendProposal {
                actor,
                signer_set,
                attempt: 1,
            },
        )
        .map_err(|_| cdk::Error::InvalidPaymentRequest)?;
    if plan.kind != FederationBarkActionKind::LightningSend {
        return Err(cdk::Error::Internal);
    }
    let envelope = federation.operation_envelope(FederationOperation::Bark(
        FederationBarkOperation::ActionPlan(plan.clone()),
    ));
    state
        .submit_federation_envelope(envelope)
        .await?
        .ensure_accepted()?;

    let accepted_projection = state.mint.federation_bark_projection_snapshot().await?;
    if runtime
        .apply_lightning_send_plan(&quote, &accepted_projection, &plan)
        .await
        .is_err()
    {
        runtime
            .mark_degraded(cdk_bark::FederatedBarkDegradedReason::ProjectionInvalid)
            .await;
        tracing::error!(
            action_id = %plan.action_id,
            "accepted Bark send plan could not be materialized locally"
        );
    }
    Ok(1)
}
