//! Consensus-stepped outgoing Bark execution and local checkpoint projection.

use std::collections::BTreeSet;
use std::str::FromStr;

use cdk::federation::{
    FederationBarkActionKind, FederationBarkDigest, FederationBarkMemberObservation,
    FederationBarkObservationStatus, FederationBarkOperation, FederationBarkSemanticPhase,
    FederationOperation, FederationOperationId, FederationPaymentObservationContext, MemberId,
};
use cdk::mint::QuoteId;
use cdk::nuts::MeltQuoteState;

use crate::MintState;

use super::bark_observer::{
    bark_outgoing_terminal_pending_wait, is_selected_bark_observer,
    is_selected_bark_outgoing_terminal_observer, required_bark_result_observers,
    unselected_bark_observer_wait,
};
use super::payment_schedule::FederationPaymentAdvancement;

fn matching_observers(
    projection: &cdk::federation::FederationBarkProjectionSnapshot,
    expected: &FederationBarkMemberObservation,
) -> BTreeSet<MemberId> {
    projection
        .observations
        .iter()
        .filter_map(|record| match &record.operation {
            FederationBarkOperation::MemberObservation(existing)
                if existing.action_id == expected.action_id
                    && existing.status == expected.status
                    && existing.payment_hash == expected.payment_hash
                    && existing.source_scope == expected.source_scope
                    && existing.amount == expected.amount
                    && existing.fee == expected.fee
                    && existing.result_hash == expected.result_hash =>
            {
                Some(existing.observer)
            }
            _ => None,
        })
        .collect()
}

fn matching_observer_count(
    projection: &cdk::federation::FederationBarkProjectionSnapshot,
    action_id: cdk::federation::FederationBarkActionId,
    status: FederationBarkObservationStatus,
    result_hash: Option<FederationBarkDigest>,
) -> usize {
    projection
        .observations
        .iter()
        .filter_map(|record| match &record.operation {
            FederationBarkOperation::MemberObservation(observation)
                if observation.action_id == action_id
                    && observation.status == status
                    && result_hash.is_none_or(|expected| observation.result_hash == expected) =>
            {
                Some(matching_observers(projection, observation).len())
            }
            _ => None,
        })
        .max()
        .unwrap_or(0)
}

fn fallback_may_initiate(
    is_actor: bool,
    status: &cdk_bark::FederatedBarkSendTerminalStatus,
) -> bool {
    !is_actor && matches!(status, cdk_bark::FederatedBarkSendTerminalStatus::Missing)
}

/// Advance one accepted pending melt by at most one consensus-visible phase.
pub(super) async fn advance_federated_bark_melt_quote(
    state: &MintState,
    quote_id: &QuoteId,
    observer_fallback_round: u32,
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
                    | FederationBarkSemanticPhase::PaymentSucceeded
                    | FederationBarkSemanticPhase::PaymentFailed
                    | FederationBarkSemanticPhase::RevocationPlanned
                    | FederationBarkSemanticPhase::RevocationCompleted
                    | FederationBarkSemanticPhase::SigningAttemptAdvanced
            )
    });

    if let Some(action) = existing_action {
        let effective_phase = action.signing_retry_phase.unwrap_or(action.phase);
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
            effective_phase,
            FederationBarkSemanticPhase::SendHtlcsCosigned
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

        let failed_entry = if matches!(
            effective_phase,
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
            effective_phase,
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

        return match effective_phase {
            FederationBarkSemanticPhase::SendPlanned => {
                if plan.actor != federation.member_id() {
                    return Ok(FederationPaymentAdvancement::AwaitingFederationEvidence);
                }
                let authorization_entry =
                    super::bark_custody_attempt::accepted_bark_custody_authorization(
                        &state.mint,
                        &projection,
                        action,
                        &plan_entry,
                    )
                    .await?;
                let transition = match runtime
                    .execute_lightning_send_htlcs(
                        &quote,
                        &projection,
                        &plan_entry,
                        &authorization_entry,
                    )
                    .await
                {
                    Ok(transition) => transition,
                    Err(cdk_bark::Error::CustodySignerUnavailable) => {
                        let retry = runtime
                            .advance_signing_attempt(action, &plan_entry)
                            .map_err(|_| cdk::Error::Internal)?;
                        return submit_bark_operation(
                            state,
                            federation.operation_envelope(FederationOperation::Bark(
                                FederationBarkOperation::ActionTransition(retry),
                            )),
                        )
                        .await;
                    }
                    Err(_) => return Err(cdk::Error::Internal),
                };
                submit_bark_operation(
                    state,
                    federation.operation_envelope(FederationOperation::Bark(
                        FederationBarkOperation::ActionTransition(transition),
                    )),
                )
                .await
            }
            FederationBarkSemanticPhase::SendHtlcsCosigned => {
                let htlcs = htlc_entry.as_ref().ok_or(cdk::Error::Internal)?;
                let threshold = usize::from(
                    federation
                        .public_config()
                        .threshold_params
                        .signature_threshold,
                );
                let failed_observer_count = matching_observer_count(
                    &projection,
                    action.action_id,
                    FederationBarkObservationStatus::Failed,
                    None,
                );
                if failed_observer_count >= threshold {
                    if plan.actor != federation.member_id() {
                        return Ok(FederationPaymentAdvancement::AwaitingFederationEvidence);
                    }
                    let transition = runtime
                        .finalize_lightning_send_failure(&plan_entry, htlcs)
                        .map_err(|_| cdk::Error::Internal)?;
                    return submit_bark_operation(
                        state,
                        federation.operation_envelope(FederationOperation::Bark(
                            FederationBarkOperation::ActionTransition(transition),
                        )),
                    )
                    .await;
                }
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
                if already_reported_failure {
                    return Ok(FederationPaymentAdvancement::AwaitingFederationEvidence);
                }
                if !is_selected_bark_outgoing_terminal_observer(
                    federation.public_config().members.keys().copied(),
                    plan.actor,
                    federation.member_id(),
                    failed_observer_count,
                    threshold,
                    observer_fallback_round,
                ) {
                    return Ok(FederationPaymentAdvancement::AwaitingObserverFallback);
                }
                let next_epoch =
                    next_observation_epoch(&projection, action.action_id, federation.member_id())?;
                let is_actor = plan.actor == federation.member_id();
                if is_actor {
                    runtime
                        .initiate_lightning_send(&quote, &projection, &plan_entry, htlcs)
                        .await
                        .map_err(|_| cdk::Error::Internal)?;
                }
                let mut status = runtime
                    .observe_lightning_send_terminal(
                        &quote,
                        &projection,
                        &plan_entry,
                        htlcs,
                        next_epoch,
                    )
                    .await
                    .map_err(|_| cdk::Error::Internal)?;
                if fallback_may_initiate(is_actor, &status) {
                    runtime
                        .initiate_lightning_send(&quote, &projection, &plan_entry, htlcs)
                        .await
                        .map_err(|_| cdk::Error::Internal)?;
                    status = runtime
                        .observe_lightning_send_terminal(
                            &quote,
                            &projection,
                            &plan_entry,
                            htlcs,
                            next_epoch,
                        )
                        .await
                        .map_err(|_| cdk::Error::Internal)?;
                }
                match status {
                    cdk_bark::FederatedBarkSendTerminalStatus::Pending
                    | cdk_bark::FederatedBarkSendTerminalStatus::Missing => {
                        Ok(bark_outgoing_terminal_pending_wait(
                            plan.actor,
                            federation.member_id(),
                            failed_observer_count,
                            observer_fallback_round,
                        ))
                    }
                    cdk_bark::FederatedBarkSendTerminalStatus::Succeeded(transition) => {
                        submit_bark_operation(
                            state,
                            federation.operation_envelope(FederationOperation::Bark(
                                FederationBarkOperation::ActionTransition(*transition),
                            )),
                        )
                        .await
                    }
                    cdk_bark::FederatedBarkSendTerminalStatus::Failed(observation) => {
                        submit_bark_operation(
                            state,
                            federation.operation_envelope(FederationOperation::Bark(
                                FederationBarkOperation::MemberObservation(observation),
                            )),
                        )
                        .await
                    }
                }
            }
            FederationBarkSemanticPhase::PaymentSucceeded => {
                let success_entry = send_transition_entry(
                    state,
                    &projection,
                    action.action_id,
                    FederationBarkSemanticPhase::PaymentSucceeded,
                )
                .await?;
                let FederationOperation::Bark(FederationBarkOperation::ActionTransition(succeeded)) =
                    &success_entry.envelope.operation
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
                                    == FederationBarkObservationStatus::Succeeded
                    )
                });
                if already_observed {
                    return Ok(FederationPaymentAdvancement::AwaitingFederationEvidence);
                }
                let public_config = federation.public_config();
                let required = required_bark_result_observers(
                    public_config.threshold_params.signature_threshold,
                    public_config
                        .payment_verification_policy(FederationPaymentObservationContext::MeltQuote)
                        .required_matching_observations(),
                );
                let matching_observers = matching_observer_count(
                    &projection,
                    action.action_id,
                    FederationBarkObservationStatus::Succeeded,
                    Some(succeeded.result_hash),
                );
                if matching_observers >= required {
                    return Ok(FederationPaymentAdvancement::AwaitingFederationEvidence);
                }
                if !is_selected_bark_observer(
                    public_config.members.keys().copied(),
                    plan.actor,
                    federation.member_id(),
                    required,
                    observer_fallback_round,
                ) {
                    return Ok(unselected_bark_observer_wait(matching_observers > 0));
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
                let authorization_entry =
                    super::bark_custody_attempt::accepted_bark_custody_authorization(
                        &state.mint,
                        &projection,
                        action,
                        revocation_plan_entry,
                    )
                    .await?;
                let transition = match runtime
                    .execute_failed_send_revocation(
                        &quote,
                        &projection,
                        &plan_entry,
                        revocation_plan_entry,
                        &authorization_entry,
                    )
                    .await
                {
                    Ok(transition) => transition,
                    Err(cdk_bark::Error::CustodySignerUnavailable) => {
                        let retry = runtime
                            .advance_signing_attempt(action, revocation_plan_entry)
                            .map_err(|_| cdk::Error::Internal)?;
                        return submit_bark_operation(
                            state,
                            federation.operation_envelope(FederationOperation::Bark(
                                FederationBarkOperation::ActionTransition(retry),
                            )),
                        )
                        .await;
                    }
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
                if already_observed {
                    return Ok(FederationPaymentAdvancement::AwaitingFederationEvidence);
                }
                let public_config = federation.public_config();
                let required = required_bark_result_observers(
                    public_config.threshold_params.signature_threshold,
                    public_config
                        .payment_verification_policy(FederationPaymentObservationContext::MeltQuote)
                        .required_matching_observations(),
                );
                let matching_observers = matching_observer_count(
                    &projection,
                    action.action_id,
                    FederationBarkObservationStatus::Failed,
                    Some(completed.result_hash),
                );
                if matching_observers >= required {
                    return Ok(FederationPaymentAdvancement::AwaitingFederationEvidence);
                }
                if !is_selected_bark_observer(
                    public_config.members.keys().copied(),
                    plan.actor,
                    federation.member_id(),
                    required,
                    observer_fallback_round,
                ) {
                    return Ok(unselected_bark_observer_wait(matching_observers > 0));
                }
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

#[cfg(test)]
mod tests {
    use cdk::federation::{
        FederationBarkActionId, FederationBarkDigest, FederationBarkMemberObservation,
        FederationBarkObservationRecord, FederationBarkObservationStatus, FederationBarkOperation,
        FederationBarkProjectionSnapshot, FederationOperationId, MemberId,
        FEDERATION_BARK_OPERATION_VERSION,
    };
    use cdk::Amount;

    use super::{fallback_may_initiate, matching_observer_count};

    fn member(id: u16) -> MemberId {
        MemberId::new(id).expect("member ID")
    }

    fn observation(
        action_id: FederationBarkActionId,
        observer: MemberId,
        operation_index: u64,
        result_seed: u8,
    ) -> FederationBarkObservationRecord {
        FederationBarkObservationRecord {
            operation_id: FederationOperationId::from_bytes([operation_index as u8; 32]),
            operation_index,
            operation: FederationBarkOperation::MemberObservation(
                FederationBarkMemberObservation {
                    version: FEDERATION_BARK_OPERATION_VERSION,
                    action_id,
                    quote: Some("quote".to_string()),
                    observer,
                    observation_epoch: 1,
                    source_scope: FederationBarkDigest::from_bytes([3; 32]).expect("source scope"),
                    payment_hash: Some(
                        FederationBarkDigest::from_bytes([4; 32]).expect("payment hash"),
                    ),
                    status: FederationBarkObservationStatus::Failed,
                    amount: Amount::from(1_000),
                    fee: Amount::from(25),
                    result_hash: FederationBarkDigest::from_bytes([result_seed; 32])
                        .expect("result hash"),
                },
            ),
        }
    }

    #[test]
    fn outgoing_failure_quorum_requires_exact_matching_evidence() {
        let action_id = FederationBarkActionId::from_bytes([1; 32]).expect("action ID");
        let expected_result = FederationBarkDigest::from_bytes([9; 32]).expect("result hash");
        let mut projection = FederationBarkProjectionSnapshot {
            observations: vec![
                observation(action_id, member(1), 1, 9),
                observation(action_id, member(2), 2, 9),
                observation(action_id, member(3), 3, 8),
            ],
            ..FederationBarkProjectionSnapshot::default()
        };

        assert_eq!(
            matching_observer_count(
                &projection,
                action_id,
                FederationBarkObservationStatus::Failed,
                None,
            ),
            2,
        );
        assert_eq!(
            matching_observer_count(
                &projection,
                action_id,
                FederationBarkObservationStatus::Failed,
                Some(expected_result),
            ),
            2,
        );

        projection
            .observations
            .push(observation(action_id, member(3), 4, 9));
        assert_eq!(
            matching_observer_count(
                &projection,
                action_id,
                FederationBarkObservationStatus::Failed,
                Some(expected_result),
            ),
            3,
        );
    }

    #[test]
    fn only_a_typed_missing_status_authorizes_fallback_initiation() {
        assert!(!fallback_may_initiate(
            false,
            &cdk_bark::FederatedBarkSendTerminalStatus::Pending,
        ));
        assert!(fallback_may_initiate(
            false,
            &cdk_bark::FederatedBarkSendTerminalStatus::Missing,
        ));
        assert!(!fallback_may_initiate(
            true,
            &cdk_bark::FederatedBarkSendTerminalStatus::Missing,
        ));
    }
}
