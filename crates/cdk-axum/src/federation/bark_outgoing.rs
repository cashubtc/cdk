//! Consensus-stepped outgoing Bark execution and local checkpoint projection.

use std::collections::BTreeSet;
use std::str::FromStr;

use cdk::federation::{
    FederationBarkActionKind, FederationBarkObservationStatus, FederationBarkOperation,
    FederationBarkSemanticPhase, FederationOperation, FederationOperationId, MemberId,
};
use cdk::mint::QuoteId;
use cdk::nuts::MeltQuoteState;

use crate::MintState;

use super::payment_schedule::FederationPaymentAdvancement;

/// Advance one accepted pending melt by at most one consensus-visible phase.
pub(super) async fn advance_federated_bark_melt_quote(
    state: &MintState,
    quote_id: &QuoteId,
) -> Result<FederationPaymentAdvancement, cdk::Error> {
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
        return Ok(FederationPaymentAdvancement::Complete);
    }
    let projection = crate::federation_bark_ready_projection(&state.mint, runtime)
        .await
        .map_err(|_| cdk::Error::Internal)?;
    let quote_label = quote_id.to_string();
    let existing_action = projection.actions.iter().find(|action| {
        action.quote.as_deref() == Some(quote_label.as_str())
            && matches!(
                action.phase,
                FederationBarkSemanticPhase::SendPlanned
                    | FederationBarkSemanticPhase::SendHtlcsCosigned
                    | FederationBarkSemanticPhase::PaymentInitiated
                    | FederationBarkSemanticPhase::PaymentSucceeded
                    | FederationBarkSemanticPhase::PaymentFailed
                    | FederationBarkSemanticPhase::RevocationPlanned
                    | FederationBarkSemanticPhase::RevocationCompleted
            )
    });

    if let Some(action) = existing_action {
        let plan_entry = send_transition_entry(
            state,
            &projection,
            action.action_id,
            FederationBarkSemanticPhase::SendPlanned,
        )
        .await?;
        let FederationOperation::Bark(FederationBarkOperation::ActionPlan(plan)) =
            &plan_entry.envelope.operation
        else {
            return Err(cdk::Error::Internal);
        };
        runtime
            .apply_lightning_send_plan(&quote, &projection, plan)
            .await
            .map_err(|_| cdk::Error::Internal)?;

        let htlc_entry = if matches!(
            action.phase,
            FederationBarkSemanticPhase::SendHtlcsCosigned
                | FederationBarkSemanticPhase::PaymentInitiated
                | FederationBarkSemanticPhase::PaymentSucceeded
                | FederationBarkSemanticPhase::PaymentFailed
                | FederationBarkSemanticPhase::RevocationPlanned
                | FederationBarkSemanticPhase::RevocationCompleted
        ) {
            Some(
                send_transition_entry(
                    state,
                    &projection,
                    action.action_id,
                    FederationBarkSemanticPhase::SendHtlcsCosigned,
                )
                .await?,
            )
        } else {
            None
        };
        if let Some(htlcs) = &htlc_entry {
            runtime
                .apply_lightning_send_htlcs(&quote, &projection, &plan_entry, htlcs)
                .await
                .map_err(|_| cdk::Error::Internal)?;
        }

        let initiated_entry = if matches!(
            action.phase,
            FederationBarkSemanticPhase::PaymentInitiated
                | FederationBarkSemanticPhase::PaymentSucceeded
                | FederationBarkSemanticPhase::PaymentFailed
                | FederationBarkSemanticPhase::RevocationPlanned
                | FederationBarkSemanticPhase::RevocationCompleted
        ) {
            Some(
                send_transition_entry(
                    state,
                    &projection,
                    action.action_id,
                    FederationBarkSemanticPhase::PaymentInitiated,
                )
                .await?,
            )
        } else {
            None
        };
        if let Some(initiated) = &initiated_entry {
            runtime
                .apply_lightning_send_initiated(&quote, &projection, &plan_entry, initiated)
                .await
                .map_err(|_| cdk::Error::Internal)?;
        }

        let failed_entry = if matches!(
            action.phase,
            FederationBarkSemanticPhase::PaymentFailed
                | FederationBarkSemanticPhase::RevocationPlanned
                | FederationBarkSemanticPhase::RevocationCompleted
        ) {
            Some(
                send_transition_entry(
                    state,
                    &projection,
                    action.action_id,
                    FederationBarkSemanticPhase::PaymentFailed,
                )
                .await?,
            )
        } else {
            None
        };
        if let Some(failed) = &failed_entry {
            // `PaymentFailed` is the input to the accepted revocation flow, not
            // itself a runtime failure. Degrading here would make the readiness
            // gate reject the projection needed to propose that revocation.
            runtime
                .apply_lightning_send_failed(&quote, &projection, &plan_entry, failed)
                .await
                .map_err(|_| cdk::Error::Internal)?;
        }

        let revocation_plan_entry = if matches!(
            action.phase,
            FederationBarkSemanticPhase::RevocationPlanned
                | FederationBarkSemanticPhase::RevocationCompleted
        ) {
            Some(
                send_transition_entry(
                    state,
                    &projection,
                    action.action_id,
                    FederationBarkSemanticPhase::RevocationPlanned,
                )
                .await?,
            )
        } else {
            None
        };

        return match action.phase {
            FederationBarkSemanticPhase::SendPlanned => {
                if plan.actor != federation.member_id() {
                    return Ok(FederationPaymentAdvancement::AwaitingFederationEvidence);
                }
                let transition = runtime
                    .execute_lightning_send_htlcs(&quote, &projection, &plan_entry)
                    .await
                    .map_err(|_| cdk::Error::Internal)?;
                submit_bark_operation(
                    state,
                    federation.operation_envelope(FederationOperation::Bark(
                        FederationBarkOperation::ActionTransition(transition),
                    )),
                )
                .await
            }
            FederationBarkSemanticPhase::SendHtlcsCosigned => {
                if plan.actor != federation.member_id() {
                    return Ok(FederationPaymentAdvancement::AwaitingFederationEvidence);
                }
                let transition = runtime
                    .initiate_lightning_send(
                        &quote,
                        &projection,
                        &plan_entry,
                        htlc_entry.as_ref().ok_or(cdk::Error::Internal)?,
                    )
                    .await
                    .map_err(|_| cdk::Error::Internal)?;
                submit_bark_operation(
                    state,
                    federation.operation_envelope(FederationOperation::Bark(
                        FederationBarkOperation::ActionTransition(transition),
                    )),
                )
                .await
            }
            FederationBarkSemanticPhase::PaymentInitiated => {
                let initiated = initiated_entry.as_ref().ok_or(cdk::Error::Internal)?;
                let already_reported_failure = projection.observations.iter().any(|record| {
                    matches!(
                        &record.operation,
                        FederationBarkOperation::MemberObservation(observation)
                            if observation.action_id == action.action_id
                                && observation.observer == federation.member_id()
                                && observation.status
                                    == FederationBarkObservationStatus::Failed
                    )
                });
                if !already_reported_failure {
                    let next_epoch = next_observation_epoch(
                        &projection,
                        action.action_id,
                        federation.member_id(),
                    )?;
                    if let Some(observation) = runtime
                        .observe_lightning_send_failure(
                            &quote,
                            &projection,
                            &plan_entry,
                            initiated,
                            next_epoch,
                        )
                        .await
                        .map_err(|_| cdk::Error::Internal)?
                    {
                        return submit_bark_operation(
                            state,
                            federation.operation_envelope(FederationOperation::Bark(
                                FederationBarkOperation::MemberObservation(observation),
                            )),
                        )
                        .await;
                    }
                }
                if plan.actor != federation.member_id() {
                    return Ok(FederationPaymentAdvancement::AwaitingExternalEvidence);
                }
                let failed_observers = projection
                    .observations
                    .iter()
                    .filter_map(|record| match &record.operation {
                        FederationBarkOperation::MemberObservation(observation)
                            if observation.action_id == action.action_id
                                && observation.status
                                    == FederationBarkObservationStatus::Failed =>
                        {
                            Some(observation.observer)
                        }
                        _ => None,
                    })
                    .collect::<BTreeSet<_>>();
                if failed_observers.len()
                    >= usize::from(
                        federation
                            .public_config()
                            .threshold_params
                            .signature_threshold,
                    )
                {
                    let transition = runtime
                        .finalize_lightning_send_failure(&plan_entry, initiated)
                        .map_err(|_| cdk::Error::Internal)?;
                    return submit_bark_operation(
                        state,
                        federation.operation_envelope(FederationOperation::Bark(
                            FederationBarkOperation::ActionTransition(transition),
                        )),
                    )
                    .await;
                }
                let Some(transition) = runtime
                    .observe_lightning_send_success(&quote, &projection, &plan_entry, initiated)
                    .await
                    .map_err(|_| cdk::Error::Internal)?
                else {
                    return Ok(FederationPaymentAdvancement::AwaitingExternalEvidence);
                };
                submit_bark_operation(
                    state,
                    federation.operation_envelope(FederationOperation::Bark(
                        FederationBarkOperation::ActionTransition(transition),
                    )),
                )
                .await
            }
            FederationBarkSemanticPhase::PaymentSucceeded => {
                let success_entry = send_transition_entry(
                    state,
                    &projection,
                    action.action_id,
                    FederationBarkSemanticPhase::PaymentSucceeded,
                )
                .await?;
                let already_observed = projection.observations.iter().any(|record| {
                    matches!(
                        &record.operation,
                        FederationBarkOperation::MemberObservation(observation)
                            if observation.action_id == action.action_id
                                && observation.observer == federation.member_id()
                                && observation.status
                                    == FederationBarkObservationStatus::Succeeded
                    )
                });
                if already_observed {
                    return Ok(FederationPaymentAdvancement::AwaitingFederationEvidence);
                }
                let next_epoch =
                    next_observation_epoch(&projection, action.action_id, federation.member_id())?;
                let observation = runtime
                    .apply_lightning_send_succeeded(
                        &quote,
                        &projection,
                        &plan_entry,
                        &success_entry,
                        next_epoch,
                    )
                    .await
                    .map_err(|_| cdk::Error::Internal)?;
                submit_bark_operation(
                    state,
                    federation.operation_envelope(FederationOperation::Bark(
                        FederationBarkOperation::MemberObservation(observation),
                    )),
                )
                .await
            }
            FederationBarkSemanticPhase::PaymentFailed => {
                if plan.actor != federation.member_id() {
                    return Ok(FederationPaymentAdvancement::AwaitingFederationEvidence);
                }
                let revocation = runtime
                    .propose_failed_send_revocation(
                        &projection,
                        &plan_entry,
                        failed_entry.as_ref().ok_or(cdk::Error::Internal)?,
                    )
                    .map_err(|_| cdk::Error::Internal)?;
                submit_bark_operation(
                    state,
                    federation.operation_envelope(FederationOperation::Bark(
                        FederationBarkOperation::ActionPlan(revocation),
                    )),
                )
                .await
            }
            FederationBarkSemanticPhase::RevocationPlanned => {
                if plan.actor != federation.member_id() {
                    return Ok(FederationPaymentAdvancement::AwaitingFederationEvidence);
                }
                let revocation_plan_entry =
                    revocation_plan_entry.as_ref().ok_or(cdk::Error::Internal)?;
                let transition = match runtime
                    .execute_failed_send_revocation(
                        &quote,
                        &projection,
                        &plan_entry,
                        revocation_plan_entry,
                    )
                    .await
                {
                    Ok(transition) => transition,
                    Err(_) => {
                        runtime
                            .mark_degraded(cdk_bark::FederatedBarkDegradedReason::RecoveryRequired)
                            .await;
                        tracing::warn!(
                            action_id = %action.action_id,
                            "accepted Bark revocation remains recovery-required"
                        );
                        return Ok(FederationPaymentAdvancement::AwaitingExternalEvidence);
                    }
                };
                submit_bark_operation(
                    state,
                    federation.operation_envelope(FederationOperation::Bark(
                        FederationBarkOperation::ActionTransition(transition),
                    )),
                )
                .await
            }
            FederationBarkSemanticPhase::RevocationCompleted => {
                let completed_entry = send_transition_entry(
                    state,
                    &projection,
                    action.action_id,
                    FederationBarkSemanticPhase::RevocationCompleted,
                )
                .await?;
                let FederationOperation::Bark(FederationBarkOperation::ActionTransition(completed)) =
                    &completed_entry.envelope.operation
                else {
                    return Err(cdk::Error::Internal);
                };
                let already_observed = projection.observations.iter().any(|record| {
                    matches!(
                        &record.operation,
                        FederationBarkOperation::MemberObservation(observation)
                            if observation.action_id == action.action_id
                                && observation.observer == federation.member_id()
                                && observation.status
                                    == FederationBarkObservationStatus::Failed
                                && observation.result_hash == completed.result_hash
                    )
                });
                let next_epoch =
                    next_observation_epoch(&projection, action.action_id, federation.member_id())?;
                let observation = match runtime
                    .apply_failed_send_revocation(
                        &quote,
                        &projection,
                        &plan_entry,
                        revocation_plan_entry.as_ref().ok_or(cdk::Error::Internal)?,
                        &completed_entry,
                        next_epoch,
                    )
                    .await
                {
                    Ok(observation) => observation,
                    Err(_) => {
                        runtime
                            .mark_degraded(cdk_bark::FederatedBarkDegradedReason::RecoveryRequired)
                            .await;
                        tracing::error!(
                            action_id = %action.action_id,
                            "accepted Bark revocation result could not be validated locally"
                        );
                        return Ok(FederationPaymentAdvancement::AwaitingExternalEvidence);
                    }
                };
                runtime
                    .clear_degraded(cdk_bark::FederatedBarkDegradedReason::RecoveryRequired)
                    .await;
                if already_observed {
                    return Ok(FederationPaymentAdvancement::AwaitingFederationEvidence);
                }
                submit_bark_operation(
                    state,
                    federation.operation_envelope(FederationOperation::Bark(
                        FederationBarkOperation::MemberObservation(observation),
                    )),
                )
                .await
            }
            _ => Err(cdk::Error::Internal),
        };
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
        return Ok(FederationPaymentAdvancement::AwaitingFederationEvidence);
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
    let expires_at = runtime
        .lightning_send_expiry_height()
        .await
        .map_err(|_| cdk::Error::InvalidPaymentRequest)?;
    let plan = runtime
        .propose_lightning_send(
            &quote,
            predecessor_operation_id,
            &projection,
            cdk_bark::FederatedBarkSendProposal {
                actor,
                signer_set,
                attempt: 1,
                expires_at,
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
    Ok(FederationPaymentAdvancement::Progressed)
}

async fn send_transition_entry(
    state: &MintState,
    projection: &cdk::federation::FederationBarkProjectionSnapshot,
    action_id: cdk::federation::FederationBarkActionId,
    phase: FederationBarkSemanticPhase,
) -> Result<cdk::federation::FederationJournalEntry, cdk::Error> {
    let operation_id = projection
        .transitions
        .iter()
        .find(|record| record.action_id == action_id && record.phase == phase)
        .map(|record| record.operation_id)
        .ok_or(cdk::Error::Internal)?;
    state
        .mint
        .localstore()
        .get_federation_journal_entry_by_operation_id(operation_id)
        .await?
        .ok_or(cdk::Error::Internal)
}

fn next_observation_epoch(
    projection: &cdk::federation::FederationBarkProjectionSnapshot,
    action_id: cdk::federation::FederationBarkActionId,
    member_id: MemberId,
) -> Result<u64, cdk::Error> {
    projection
        .observations
        .iter()
        .filter_map(|record| match &record.operation {
            FederationBarkOperation::MemberObservation(observation)
                if observation.action_id == action_id && observation.observer == member_id =>
            {
                Some(observation.observation_epoch)
            }
            _ => None,
        })
        .max()
        .unwrap_or(0)
        .checked_add(1)
        .ok_or(cdk::Error::Internal)
}

async fn submit_bark_operation(
    state: &MintState,
    envelope: cdk::federation::FederationOperationEnvelope,
) -> Result<FederationPaymentAdvancement, cdk::Error> {
    state
        .submit_federation_envelope(envelope)
        .await?
        .ensure_accepted()?;
    Ok(FederationPaymentAdvancement::Progressed)
}
