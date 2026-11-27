//! Consensus-stepped outgoing Bark execution and local checkpoint projection.

use std::collections::BTreeSet;
use std::str::FromStr;

use cdk::federation::{
    FederationBarkActionKind, FederationBarkDigest, FederationBarkEvidencePhase,
    FederationBarkMemberObservation, FederationBarkObservationStatus, FederationBarkOperation,
    FederationBarkSemanticPhase, FederationOperationEnvelope, FederationOperationId,
    FederationPaymentAdvancement, FederationPaymentObservationContext,
    FederationSubmitOperationResponse, MemberId,
};
use cdk::mint::QuoteId;
use cdk::nuts::MeltQuoteState;
use cdk_common::database::mint::FederationJournalEntryKey;

use super::evidence::{collect_bark_evidence_certificate, scheduled_bark_evidence_collector_round};
use super::observer::{
    bark_outgoing_terminal_pending_wait, is_selected_bark_observer, required_bark_result_observers,
    unselected_bark_observer_wait,
};
use super::FederatedBarkNode;
use crate::IndexedBarkProjection;

fn matching_observers(
    projection: &IndexedBarkProjection,
    expected: &FederationBarkMemberObservation,
) -> BTreeSet<MemberId> {
    projection
        .observations_for_action(expected.action_id)
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
    projection: &IndexedBarkProjection,
    action_id: cdk::federation::FederationBarkActionId,
    status: FederationBarkObservationStatus,
    result_hash: Option<FederationBarkDigest>,
) -> usize {
    projection
        .observations_for_action(action_id)
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

fn send_needs_initiation(status: &crate::FederatedBarkSendTerminalStatus) -> bool {
    matches!(status, crate::FederatedBarkSendTerminalStatus::Missing)
}

/// Whether the actor may skip the initial `observe_lightning_send_terminal`
/// probe and call `initiate_lightning_send` directly.
///
/// Only permitted on the first pass (`observer_fallback_round == 0`,
/// `next_epoch == 1`) by the accepted actor immediately after this process
/// finalized `SendHtlcsCosigned`. After any restart, on any retry, or on any
/// fallback observer, `freshly_cosigned_locally` is `false` and the observer
/// still probes first and requires a typed `Missing` status before initiation
/// (`docs/federation/payment-trust-model.md`).
fn should_skip_initial_send_probe(
    observer_fallback_round: u32,
    plan_actor: MemberId,
    local_member: MemberId,
    next_epoch: u64,
    freshly_cosigned_locally: bool,
) -> bool {
    observer_fallback_round == 0
        && plan_actor == local_member
        && next_epoch == 1
        && freshly_cosigned_locally
}

/// Advance one accepted pending melt by at most one consensus-visible phase.
pub async fn advance_federated_bark_melt_quote(
    state: &dyn FederatedBarkNode,
    quote_id: &QuoteId,
    observer_fallback_round: u32,
    wait_for_backend_update: bool,
) -> Result<FederationPaymentAdvancement, cdk::Error> {
    let federation = state
        .mint()
        .federation_config()
        .ok_or(cdk::Error::Internal)?;
    let runtime = state.bark_runtime().ok_or(cdk::Error::Internal)?;
    let quote = state
        .mint()
        .localstore()
        .get_melt_quote(quote_id)
        .await?
        .ok_or(cdk::Error::UnknownQuote)?;
    if quote.state != MeltQuoteState::Pending {
        return Ok(FederationPaymentAdvancement::Complete);
    }
    let projection_timer = super::latency::FederationLatencyTimer::start(
        super::latency::FederationLatencyCorrelation::Quote,
        quote_id.to_string(),
        super::latency::FederationLatencyBackend::Bark,
        super::latency::FederationLatencyDirection::Outgoing,
        super::latency::FederationLatencyPhase::BarkProjectionReady,
    );
    // Accepted actions must remain recoverable when an earlier external action
    // requires recovery. The recovery gate still requires complete local replay.
    let projection_result =
        super::projection::federation_bark_recovery_indexed_projection(state, runtime).await;
    projection_timer.finish(super::latency::FederationLatencyOutcome::from_success(
        projection_result.is_ok(),
    ));
    let projection = projection_result.map_err(super::projection::projection_error)?;
    let snapshot = projection.snapshot();
    let quote_label = quote_id.to_string();
    let existing_action = projection.actions_for_quote(&quote_label).find(|action| {
        matches!(
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
        let effective_phase = action.effective_phase();
        let plan_entry = send_transition_entry(
            state,
            &projection,
            action.action_id,
            FederationBarkSemanticPhase::SendPlanned,
        )
        .await?;
        let FederationBarkOperation::ActionPlan(plan) =
            super::bark_operation(&plan_entry.envelope.operation)?
        else {
            return Err(cdk::Error::Internal);
        };
        runtime
            .apply_lightning_send_plan(&quote, snapshot, &plan)
            .await
            .map_err(|error| {
                error.into_federation_error("apply outgoing plan", Some(plan_entry.operation_id))
            })?;

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
                .apply_lightning_send_htlcs(&quote, snapshot, &plan_entry, htlcs)
                .await
                .map_err(|error| {
                    error.into_federation_error(
                        "apply outgoing HTLCs",
                        Some(plan_entry.operation_id),
                    )
                })?;
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
                .apply_lightning_send_failed(&quote, snapshot, &plan_entry, failed)
                .await
                .map_err(|error| {
                    error.into_federation_error(
                        "apply failed outgoing payment",
                        Some(plan_entry.operation_id),
                    )
                })?;
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
                    super::custody_attempt::accepted_bark_custody_authorization(
                        state.mint(),
                        &projection,
                        action,
                        &plan_entry,
                    )
                    .await?;
                let htlc_timer = super::latency::FederationLatencyTimer::start(
                    super::latency::FederationLatencyCorrelation::Action,
                    action.action_id.to_string(),
                    super::latency::FederationLatencyBackend::Bark,
                    super::latency::FederationLatencyDirection::Outgoing,
                    super::latency::FederationLatencyPhase::BarkHtlcCosigning,
                );
                let htlc_result = runtime
                    .execute_lightning_send_htlcs(
                        &quote,
                        snapshot,
                        &plan_entry,
                        &authorization_entry,
                    )
                    .await;
                htlc_timer.finish(super::latency::FederationLatencyOutcome::from_success(
                    htlc_result.is_ok(),
                ));
                let transition = match htlc_result {
                    Ok(transition) => transition,
                    Err(crate::Error::CustodySignerUnavailable) => {
                        let retry = runtime
                            .advance_signing_attempt(action, &plan_entry)
                            .map_err(|error| {
                                error.into_federation_error(
                                    "advance outgoing signing attempt",
                                    Some(plan_entry.operation_id),
                                )
                            })?;
                        return submit_bark_operation(
                            state,
                            federation.operation_envelope(super::bark_federation_operation(
                                FederationBarkOperation::ActionTransition(retry),
                            )?),
                        )
                        .await;
                    }
                    Err(error) => {
                        return Err(error
                            .into_federation_error(
                                "execute outgoing HTLCs",
                                Some(plan_entry.operation_id),
                            )
                            .into())
                    }
                };
                let advancement = submit_bark_operation(
                    state,
                    federation.operation_envelope(super::bark_federation_operation(
                        FederationBarkOperation::ActionTransition(transition),
                    )?),
                )
                .await?;
                runtime.mark_send_htlcs_freshly_cosigned(action.action_id);
                Ok(advancement)
            }
            FederationBarkSemanticPhase::SendHtlcsCosigned => {
                let htlcs = htlc_entry.as_ref().ok_or(cdk::Error::Internal)?;
                let threshold = usize::from(
                    federation
                        .public_config()
                        .threshold_params
                        .signature_threshold,
                );
                let public_config = federation.public_config();
                let success_required = required_bark_result_observers(
                    public_config.threshold_params.signature_threshold,
                    public_config
                        .payment_verification_policy(FederationPaymentObservationContext::MeltQuote)
                        .required_matching_observations(),
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
                        .map_err(|error| {
                            error.into_federation_error(
                                "finalize outgoing failure",
                                Some(plan_entry.operation_id),
                            )
                        })?;
                    return submit_bark_operation(
                        state,
                        federation.operation_envelope(super::bark_federation_operation(
                            FederationBarkOperation::ActionTransition(transition),
                        )?),
                    )
                    .await;
                }
                let already_reported_failure = projection
                    .observations_for_action(action.action_id)
                    .any(|record| {
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
                if !is_selected_bark_observer(
                    federation.public_config().members.keys().copied(),
                    plan.actor,
                    federation.member_id(),
                    success_required,
                    observer_fallback_round,
                ) {
                    return Ok(FederationPaymentAdvancement::AwaitingObserverFallback);
                }
                let next_epoch =
                    next_observation_epoch(&projection, action.action_id, federation.member_id())?;
                let skip_initial_probe = should_skip_initial_send_probe(
                    observer_fallback_round,
                    plan.actor,
                    federation.member_id(),
                    next_epoch,
                    runtime.take_send_htlcs_freshly_cosigned(action.action_id),
                );
                let mut status = if skip_initial_probe {
                    crate::FederatedBarkSendTerminalStatus::Missing
                } else {
                    // A peer may have initiated this exact accepted payment while
                    // the actor still has an HTLC-only local checkpoint. Observe
                    // terminal state before retrying initiation, including on actor
                    // after a restart or fallback.
                    let terminal_timer = super::latency::FederationLatencyTimer::start(
                        super::latency::FederationLatencyCorrelation::Action,
                        action.action_id.to_string(),
                        super::latency::FederationLatencyBackend::Bark,
                        super::latency::FederationLatencyDirection::Outgoing,
                        if wait_for_backend_update {
                            super::latency::FederationLatencyPhase::BarkLightningServerWait
                        } else {
                            super::latency::FederationLatencyPhase::BarkLightningTerminalPoll
                        },
                    );
                    let status_result = runtime
                        .observe_lightning_send_terminal(
                            &quote,
                            snapshot,
                            &plan_entry,
                            htlcs,
                            next_epoch,
                            wait_for_backend_update,
                        )
                        .await;
                    let terminal_outcome = match &status_result {
                        Ok(crate::FederatedBarkSendTerminalStatus::Pending)
                        | Ok(crate::FederatedBarkSendTerminalStatus::Missing) => {
                            super::latency::FederationLatencyOutcome::Pending
                        }
                        Ok(_) => super::latency::FederationLatencyOutcome::Success,
                        Err(_) => super::latency::FederationLatencyOutcome::Error,
                    };
                    terminal_timer.finish(terminal_outcome);
                    status_result.map_err(|error| {
                        error.into_federation_error(
                            "observe outgoing payment",
                            Some(plan_entry.operation_id),
                        )
                    })?
                };
                if send_needs_initiation(&status) {
                    let initiation_timer = super::latency::FederationLatencyTimer::start(
                        super::latency::FederationLatencyCorrelation::Action,
                        action.action_id.to_string(),
                        super::latency::FederationLatencyBackend::Bark,
                        super::latency::FederationLatencyDirection::Outgoing,
                        super::latency::FederationLatencyPhase::BarkLightningInitiation,
                    );
                    let initiation_result = runtime
                        .initiate_lightning_send(&quote, snapshot, &plan_entry, htlcs)
                        .await;
                    initiation_timer.finish(
                        super::latency::FederationLatencyOutcome::from_success(
                            initiation_result.is_ok(),
                        ),
                    );
                    initiation_result.map_err(|error| {
                        error.into_federation_error(
                            "initiate outgoing payment",
                            Some(plan_entry.operation_id),
                        )
                    })?;
                    let terminal_timer = super::latency::FederationLatencyTimer::start(
                        super::latency::FederationLatencyCorrelation::Action,
                        action.action_id.to_string(),
                        super::latency::FederationLatencyBackend::Bark,
                        super::latency::FederationLatencyDirection::Outgoing,
                        if wait_for_backend_update {
                            super::latency::FederationLatencyPhase::BarkLightningServerWait
                        } else {
                            super::latency::FederationLatencyPhase::BarkLightningTerminalPoll
                        },
                    );
                    let status_result = runtime
                        .observe_lightning_send_terminal(
                            &quote,
                            snapshot,
                            &plan_entry,
                            htlcs,
                            next_epoch,
                            wait_for_backend_update,
                        )
                        .await;
                    let terminal_outcome = match &status_result {
                        Ok(crate::FederatedBarkSendTerminalStatus::Pending)
                        | Ok(crate::FederatedBarkSendTerminalStatus::Missing) => {
                            super::latency::FederationLatencyOutcome::Pending
                        }
                        Ok(_) => super::latency::FederationLatencyOutcome::Success,
                        Err(_) => super::latency::FederationLatencyOutcome::Error,
                    };
                    terminal_timer.finish(terminal_outcome);
                    status = status_result.map_err(|error| {
                        error.into_federation_error(
                            "observe outgoing payment",
                            Some(plan_entry.operation_id),
                        )
                    })?;
                }
                match status {
                    crate::FederatedBarkSendTerminalStatus::Pending
                    | crate::FederatedBarkSendTerminalStatus::Missing => {
                        Ok(if wait_for_backend_update {
                            FederationPaymentAdvancement::AwaitingBackendNotification
                        } else {
                            bark_outgoing_terminal_pending_wait(
                                plan.actor,
                                federation.member_id(),
                                failed_observer_count,
                                observer_fallback_round,
                            )
                        })
                    }
                    crate::FederatedBarkSendTerminalStatus::Succeeded(mut transition) => {
                        let validation_timer = super::latency::FederationLatencyTimer::start(
                            super::latency::FederationLatencyCorrelation::Action,
                            action.action_id.to_string(),
                            super::latency::FederationLatencyBackend::Bark,
                            super::latency::FederationLatencyDirection::Outgoing,
                            super::latency::FederationLatencyPhase::BarkResultValidation,
                        );
                        let observation_result = runtime
                            .attest_lightning_send_succeeded_candidate(
                                &quote,
                                snapshot,
                                &plan_entry,
                                htlcs,
                                &transition,
                                next_epoch,
                            )
                            .await;
                        validation_timer.finish(
                            super::latency::FederationLatencyOutcome::from_success(
                                observation_result.is_ok(),
                            ),
                        );
                        let observation = observation_result.map_err(|error| {
                            error.into_federation_error(
                                "validate outgoing success",
                                Some(plan_entry.operation_id),
                            )
                        })?;
                        let required =
                            u16::try_from(success_required).map_err(|_| cdk::Error::Internal)?;
                        let collector_fallback_round = scheduled_bark_evidence_collector_round(
                            observer_fallback_round,
                            required,
                        );
                        let certificate = collect_bark_evidence_certificate(
                            state,
                            action.action_id,
                            plan.actor,
                            FederationBarkEvidencePhase::SendSucceeded,
                            htlcs.operation_id,
                            transition.attempt,
                            &observation,
                            required,
                            collector_fallback_round,
                            super::latency::FederationLatencyDirection::Outgoing,
                        )
                        .await?;
                        let Some(certificate) = certificate else {
                            return Ok(FederationPaymentAdvancement::AwaitingFederationEvidence);
                        };
                        transition.evidence_certificate = Some(Box::new(certificate));
                        submit_bark_operation(
                            state,
                            federation.operation_envelope(super::bark_federation_operation(
                                FederationBarkOperation::ActionTransition(*transition),
                            )?),
                        )
                        .await
                    }
                    crate::FederatedBarkSendTerminalStatus::Failed(observation) => {
                        submit_bark_operation(
                            state,
                            federation.operation_envelope(super::bark_federation_operation(
                                FederationBarkOperation::MemberObservation(observation),
                            )?),
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
                let FederationBarkOperation::ActionTransition(_) =
                    super::bark_operation(&success_entry.envelope.operation)?
                else {
                    return Err(cdk::Error::Internal);
                };
                let next_epoch =
                    next_observation_epoch(&projection, action.action_id, federation.member_id())?;
                let validation_timer = super::latency::FederationLatencyTimer::start(
                    super::latency::FederationLatencyCorrelation::Action,
                    action.action_id.to_string(),
                    super::latency::FederationLatencyBackend::Bark,
                    super::latency::FederationLatencyDirection::Outgoing,
                    super::latency::FederationLatencyPhase::BarkResultValidation,
                );
                let apply_result = runtime
                    .apply_lightning_send_succeeded(
                        &quote,
                        snapshot,
                        &plan_entry,
                        &success_entry,
                        next_epoch,
                    )
                    .await;
                validation_timer.finish(super::latency::FederationLatencyOutcome::from_success(
                    apply_result.is_ok(),
                ));
                apply_result.map_err(|error| {
                    error.into_federation_error(
                        "apply outgoing success",
                        Some(success_entry.operation_id),
                    )
                })?;
                Ok(FederationPaymentAdvancement::Complete)
            }
            FederationBarkSemanticPhase::PaymentFailed => {
                if plan.actor != federation.member_id() {
                    return Ok(FederationPaymentAdvancement::AwaitingFederationEvidence);
                }
                let revocation = runtime
                    .propose_failed_send_revocation(
                        snapshot,
                        &plan_entry,
                        failed_entry.as_ref().ok_or(cdk::Error::Internal)?,
                    )
                    .map_err(|error| {
                        error.into_federation_error(
                            "plan outgoing revocation",
                            Some(plan_entry.operation_id),
                        )
                    })?;
                submit_bark_operation(
                    state,
                    federation.operation_envelope(super::bark_federation_operation(
                        FederationBarkOperation::ActionPlan(revocation),
                    )?),
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
                    super::custody_attempt::accepted_bark_custody_authorization(
                        state.mint(),
                        &projection,
                        action,
                        revocation_plan_entry,
                    )
                    .await?;
                let transition = match runtime
                    .execute_failed_send_revocation(
                        &quote,
                        snapshot,
                        &plan_entry,
                        revocation_plan_entry,
                        &authorization_entry,
                    )
                    .await
                {
                    Ok(transition) => transition,
                    Err(crate::Error::CustodySignerUnavailable) => {
                        let retry = runtime
                            .advance_signing_attempt(action, revocation_plan_entry)
                            .map_err(|error| {
                                error.into_federation_error(
                                    "advance revocation signing attempt",
                                    Some(revocation_plan_entry.operation_id),
                                )
                            })?;
                        return submit_bark_operation(
                            state,
                            federation.operation_envelope(super::bark_federation_operation(
                                FederationBarkOperation::ActionTransition(retry),
                            )?),
                        )
                        .await;
                    }
                    Err(error) => {
                        runtime
                            .mark_degraded(crate::FederatedBarkDegradedReason::RecoveryRequired)
                            .await;
                        tracing::warn!(
                            action_id = %action.action_id,
                            error = %error,
                            "accepted Bark revocation remains recovery-required"
                        );
                        return Ok(FederationPaymentAdvancement::AwaitingExternalEvidence);
                    }
                };
                submit_bark_operation(
                    state,
                    federation.operation_envelope(super::bark_federation_operation(
                        FederationBarkOperation::ActionTransition(transition),
                    )?),
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
                let FederationBarkOperation::ActionTransition(completed) =
                    super::bark_operation(&completed_entry.envelope.operation)?
                else {
                    return Err(cdk::Error::Internal);
                };
                let already_observed =
                    projection
                        .observations_for_action(action.action_id)
                        .any(|record| {
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
                        snapshot,
                        &plan_entry,
                        revocation_plan_entry.as_ref().ok_or(cdk::Error::Internal)?,
                        &completed_entry,
                        next_epoch,
                    )
                    .await
                {
                    Ok(observation) => observation,
                    Err(error) => {
                        runtime
                            .mark_degraded(crate::FederatedBarkDegradedReason::RecoveryRequired)
                            .await;
                        tracing::error!(
                            action_id = %action.action_id,
                            error = %error,
                            "accepted Bark revocation result could not be validated locally"
                        );
                        return Ok(FederationPaymentAdvancement::AwaitingExternalEvidence);
                    }
                };
                runtime
                    .clear_degraded(crate::FederatedBarkDegradedReason::RecoveryRequired)
                    .await;
                submit_bark_operation(
                    state,
                    federation.operation_envelope(super::bark_federation_operation(
                        FederationBarkOperation::MemberObservation(observation),
                    )?),
                )
                .await
            }
            _ => Err(cdk::Error::Internal),
        };
    }

    // Recovery permission never authorizes planning a new external payment.
    runtime
        .ensure_ready(projection.frontier().unwrap_or(0))
        .await
        .map_err(|error| error.into_federation_error("verify outgoing readiness", None))?;

    let saga = state
        .mint()
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
        .mint()
        .federation_melt_payment_actor(predecessor_operation_id)
        .await?;
    if actor != federation.member_id() {
        return Ok(FederationPaymentAdvancement::AwaitingFederationEvidence);
    }
    let application = super::bark_application(federation.public_config())?;
    let signer_set = federation
        .public_config()
        .members
        .keys()
        .copied()
        .take(usize::from(application.threshold))
        .collect::<Vec<_>>();
    let plan_timer = super::latency::FederationLatencyTimer::start(
        super::latency::FederationLatencyCorrelation::Quote,
        quote_id.to_string(),
        super::latency::FederationLatencyBackend::Bark,
        super::latency::FederationLatencyDirection::Outgoing,
        super::latency::FederationLatencyPhase::BarkSendPlanPreparation,
    );
    let expires_at_result = runtime.lightning_send_expiry_height().await;
    let expires_at = match expires_at_result {
        Ok(expires_at) => expires_at,
        Err(_) => {
            plan_timer.finish(super::latency::FederationLatencyOutcome::Error);
            return Err(cdk::Error::InvalidPaymentRequest);
        }
    };
    let plan_result = runtime
        .propose_lightning_send(
            &quote,
            predecessor_operation_id,
            snapshot,
            crate::FederatedBarkSendProposal {
                actor,
                signer_set,
                attempt: 1,
                expires_at,
            },
        )
        .map_err(|_| cdk::Error::InvalidPaymentRequest);
    plan_timer.finish(super::latency::FederationLatencyOutcome::from_success(
        plan_result.is_ok(),
    ));
    let plan = plan_result?;
    if plan.kind != FederationBarkActionKind::LightningSend {
        return Err(cdk::Error::Internal);
    }
    let envelope = federation.operation_envelope(super::bark_federation_operation(
        FederationBarkOperation::ActionPlan(plan.clone()),
    )?);
    state.submit_envelope(envelope).await?.ensure_accepted()?;

    let accepted_projection =
        super::projection::federation_bark_recovery_indexed_projection(state, runtime)
            .await
            .map_err(super::projection::projection_error)?;
    if let Err(error) = runtime
        .apply_lightning_send_plan(&quote, accepted_projection.snapshot(), &plan)
        .await
    {
        runtime
            .mark_degraded(crate::FederatedBarkDegradedReason::ProjectionInvalid)
            .await;
        tracing::error!(
            action_id = %plan.action_id,
            error = %error,
            "accepted Bark send plan could not be materialized locally"
        );
    }
    Ok(FederationPaymentAdvancement::Progressed)
}

/// Advance a melt quote after a peer delivered Bark evidence, without waiting
/// for a backend update.
pub async fn advance_federated_bark_melt_quote_from_evidence_delivery(
    state: &dyn FederatedBarkNode,
    quote_id: &QuoteId,
    collector_fallback_round: u32,
) -> Result<FederationPaymentAdvancement, cdk::Error> {
    advance_federated_bark_melt_quote(state, quote_id, collector_fallback_round, false).await
}

/// Publish the wallet-authored melt and its deterministic Bark send plan in
/// one ordered Aleph unit. The existing rotating wallet publisher is retained:
/// non-primary members first wait for the identical melt operation to appear.
pub async fn submit_federated_bark_melt_with_plan(
    state: &dyn FederatedBarkNode,
    melt_envelope: FederationOperationEnvelope,
    quote_id: &QuoteId,
) -> Result<FederationSubmitOperationResponse, cdk::Error> {
    let federation = state
        .mint()
        .federation_config()
        .ok_or(cdk::Error::Internal)?;
    state.validate_envelope_admission(&melt_envelope).await?;
    let melt_operation_id = melt_envelope.operation_id()?;
    state.ensure_ready_for_public_submit().await?;
    if let Some(response) = state
        .wait_for_prior_wallet_publisher(melt_operation_id)
        .await?
    {
        response.validate(melt_operation_id)?;
        response.ensure_accepted()?;
        return Ok(response);
    }

    let runtime = state.bark_runtime().ok_or(cdk::Error::Internal)?;
    let mut quote = state
        .mint()
        .localstore()
        .get_melt_quote(quote_id)
        .await?
        .ok_or(cdk::Error::UnknownQuote)?;
    quote.state = MeltQuoteState::Pending;
    let projection =
        super::projection::federation_bark_ready_indexed_projection(state.mint(), runtime)
            .await
            .map_err(super::projection::projection_error)?;
    let application = super::bark_application(federation.public_config())?;
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
            melt_operation_id,
            projection.snapshot(),
            crate::FederatedBarkSendProposal {
                actor: federation.member_id(),
                signer_set,
                attempt: 1,
                expires_at,
            },
        )
        .map_err(|_| cdk::Error::InvalidPaymentRequest)?;
    let plan_envelope = federation.operation_envelope(super::bark_federation_operation(
        FederationBarkOperation::ActionPlan(plan),
    )?);
    let responses = state
        .submit_ordered_successor_group(vec![melt_envelope, plan_envelope])
        .await?;
    let [melt_response, plan_response] = responses.as_slice() else {
        return Err(cdk::Error::Internal);
    };
    melt_response.ensure_accepted()?;
    plan_response.ensure_accepted()?;
    Ok(melt_response.clone())
}

async fn send_transition_entry(
    state: &dyn FederatedBarkNode,
    projection: &IndexedBarkProjection,
    action_id: cdk::federation::FederationBarkActionId,
    phase: FederationBarkSemanticPhase,
) -> Result<cdk::federation::FederationJournalEntry, cdk::Error> {
    let operation_id = projection
        .first_transition_for_phase(action_id, phase)
        .map(|record| record.operation_id)
        .ok_or(cdk::Error::Internal)?;
    state
        .mint()
        .localstore()
        .get_federation_journal_entry(FederationJournalEntryKey::Operation(operation_id), None)
        .await?
        .ok_or(cdk::Error::Internal)
}

fn next_observation_epoch(
    projection: &IndexedBarkProjection,
    action_id: cdk::federation::FederationBarkActionId,
    member_id: MemberId,
) -> Result<u64, cdk::Error> {
    projection
        .observations_for_action(action_id)
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
    state: &dyn FederatedBarkNode,
    envelope: cdk::federation::FederationOperationEnvelope,
) -> Result<FederationPaymentAdvancement, cdk::Error> {
    state.submit_envelope(envelope).await?.ensure_accepted()?;
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

    use super::{matching_observer_count, send_needs_initiation, should_skip_initial_send_probe};
    use crate::IndexedBarkProjection;

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
        let indexed = IndexedBarkProjection::from_snapshot(projection.clone());

        assert_eq!(
            matching_observer_count(
                &indexed,
                action_id,
                FederationBarkObservationStatus::Failed,
                None,
            ),
            2,
        );
        assert_eq!(
            matching_observer_count(
                &indexed,
                action_id,
                FederationBarkObservationStatus::Failed,
                Some(expected_result),
            ),
            2,
        );

        projection
            .observations
            .push(observation(action_id, member(3), 4, 9));
        let indexed = IndexedBarkProjection::from_snapshot(projection);
        assert_eq!(
            matching_observer_count(
                &indexed,
                action_id,
                FederationBarkObservationStatus::Failed,
                Some(expected_result),
            ),
            3,
        );
    }

    #[test]
    fn only_a_typed_missing_status_authorizes_initiation() {
        assert!(!send_needs_initiation(
            &crate::FederatedBarkSendTerminalStatus::Pending,
        ));
        assert!(send_needs_initiation(
            &crate::FederatedBarkSendTerminalStatus::Missing,
        ));
    }

    #[test]
    fn attempt_zero_probe_skip_requires_freshly_cosigned_primary_actor_first_pass() {
        let actor = member(1);
        let peer = member(2);
        assert!(should_skip_initial_send_probe(0, actor, actor, 1, true));
        // Restart / already-consumed marker must still probe before initiating.
        assert!(!should_skip_initial_send_probe(0, actor, actor, 1, false));
        // Fallback rounds must probe first.
        assert!(!should_skip_initial_send_probe(1, actor, actor, 1, true));
        // Non-actor observers must probe first.
        assert!(!should_skip_initial_send_probe(0, actor, peer, 1, true));
        // Later observation epochs (retries) must probe first.
        assert!(!should_skip_initial_send_probe(0, actor, actor, 2, true));
    }
}
