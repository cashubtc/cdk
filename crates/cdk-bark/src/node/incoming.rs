//! Consensus-first orchestration for federated Bark incoming BOLT11 quotes.

use std::collections::BTreeMap;
use std::time::{SystemTime, UNIX_EPOCH};

use cdk::federation::{
    ark_server_commitment, federation_bark_description_hash, federation_bark_mailbox_scope,
    payment_hash_for_preimage, preimage_commitment, FederationBarkActionId,
    FederationBarkActionRecord, FederationBarkActionTransition, FederationBarkEvidencePhase,
    FederationBarkMemberObservation, FederationBarkObservationStatus, FederationBarkOperation,
    FederationBarkReceiveIntent, FederationBarkReceivePrepared, FederationBarkReceiveProof,
    FederationBarkReceiveSecretBinding, FederationBarkSemanticPhase, FederationBarkUnit,
    FederationBarkVtxoLocator, FederationBarkVtxoState, FederationError, FederationId,
    FederationJournalEntry, FederationOperationId, FederationPaymentAdvancement,
    FederationPaymentObservationContext, MemberConfigPublic, FEDERATION_BARK_OPERATION_VERSION,
    FEDERATION_MAX_DERIVATION_INDEX,
};
use cdk::mint::{MintQuoteRequest, MintQuoteResponse, QuoteId};
use cdk::nuts::CurrencyUnit;
use cdk::Amount;
use cdk_common::database::mint::FederationJournalEntryKey;
use cdk_frost::rand_core::{OsRng, RngCore};
use sha2::{Digest, Sha256};

#[cfg(test)]
use super::evidence::bark_evidence_retry_collector;
use super::evidence::{
    bark_evidence_collector, collect_bark_evidence_certificate,
    persist_local_bark_evidence_attestation, scheduled_bark_evidence_collector_round,
};
use super::observer::{
    is_selected_bark_observer, is_selected_bark_receive_observer, required_bark_result_observers,
    unselected_bark_observer_wait,
};
use super::receive_secret::FederationBarkReceiveSecretCoordinator;
use super::FederatedBarkNode;

const FEDERATION_BARK_RECEIVE_INTENT_LIFETIME_SECONDS: u64 = 7 * 24 * 60 * 60;
const FEDERATION_BARK_RECEIVE_EXPIRY_GRACE_SECONDS: u64 = 15 * 60;
const FEDERATION_BARK_QUOTE_ID_DOMAIN: &[u8] = b"cdk-federation-bark-quote-id-v1";

fn validate_atomic_receive_claim_group(
    disclosed: &FederationBarkActionTransition,
    disclosed_operation_id: FederationOperationId,
    claimed: &FederationBarkActionTransition,
) -> Result<(), cdk::Error> {
    let invalid = || FederationError::ApplicationOperationInvalid {
        reason: "atomic Bark receive claim is not the exact disclosure successor",
    };
    if disclosed.phase != FederationBarkSemanticPhase::ReceivePreimageDisclosed
        || claimed.phase != FederationBarkSemanticPhase::ReceiveClaimed
        || claimed.action_id != disclosed.action_id
        || claimed.quote != disclosed.quote
        || claimed.actor != disclosed.actor
        || claimed.plan_operation_id != disclosed.plan_operation_id
        || claimed.predecessor_operation_id != disclosed_operation_id
        || claimed.payment_hash != disclosed.payment_hash
        || claimed.attempt != disclosed.attempt
        || claimed.signer_set != disclosed.signer_set
        || claimed.request_hash != disclosed.result_hash
    {
        return Err(invalid().into());
    }
    let disclosed_inputs = disclosed
        .vtxo_transitions
        .iter()
        .map(|transition| (transition.vtxo_id, transition))
        .collect::<BTreeMap<_, _>>();
    let claimed_inputs = claimed
        .vtxo_transitions
        .iter()
        .filter(|transition| {
            transition.previous_state == Some(FederationBarkVtxoState::PendingSpend)
                && transition.new_state == FederationBarkVtxoState::Spent
        })
        .map(|transition| (transition.vtxo_id, transition))
        .collect::<BTreeMap<_, _>>();
    if disclosed_inputs.len() != disclosed.vtxo_transitions.len()
        || claimed_inputs.len() != disclosed_inputs.len()
    {
        return Err(invalid().into());
    }
    for (vtxo_id, disclosure) in disclosed_inputs {
        let Some(claim) = claimed_inputs.get(&vtxo_id) else {
            return Err(invalid().into());
        };
        if disclosure.previous_state != Some(FederationBarkVtxoState::Reserved)
            || disclosure.new_state != FederationBarkVtxoState::PendingSpend
            || claim.locator != disclosure.locator
            || claim.amount != disclosure.amount
            || claim.expiry_height != disclosure.expiry_height
            || claim.artifact_hash != disclosure.artifact_hash
        {
            return Err(invalid().into());
        }
    }
    Ok(())
}

async fn receive_transition_entry(
    state: &dyn FederatedBarkNode,
    projection: &cdk::federation::FederationBarkProjectionSnapshot,
    action_id: FederationBarkActionId,
    phase: FederationBarkSemanticPhase,
) -> Result<cdk::federation::FederationJournalEntry, cdk::Error> {
    let operation_id = projection
        .transitions
        .iter()
        .filter(|record| record.action_id == action_id && record.phase == phase)
        .max_by_key(|record| record.operation_index)
        .map(|record| record.operation_id)
        .ok_or(cdk::Error::Internal)?;
    state
        .mint()
        .localstore()
        .get_federation_journal_entry(FederationJournalEntryKey::Operation(operation_id), None)
        .await?
        .ok_or(cdk::Error::Internal)
}

fn receive_claim_proposal(
    config: &cdk::federation::FederationConfig,
    projection: &cdk::federation::FederationBarkProjectionSnapshot,
    actor: cdk::federation::MemberId,
) -> Result<crate::FederatedBarkReceiveClaimProposal, cdk::Error> {
    let application = super::bark_application(config)?;
    let signer_set = config
        .members
        .keys()
        .copied()
        .take(usize::from(application.threshold))
        .collect::<Vec<_>>();
    let next_index = projection
        .derivation_allocations
        .iter()
        .map(|allocation| allocation.locator.index)
        .max()
        .unwrap_or(0)
        .checked_add(1)
        .filter(|index| *index <= FEDERATION_MAX_DERIVATION_INDEX)
        .ok_or(cdk::Error::Internal)?;
    Ok(crate::FederatedBarkReceiveClaimProposal {
        actor,
        signer_set,
        attempt: 1,
        output_locator: FederationBarkVtxoLocator::vtxo_custody(next_index),
    })
}

/// How one incoming advancement waits for the Ark server to report a change.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ReceiveBackendWait {
    /// Poll once; an unchanged receive awaits external evidence.
    Poll,
    /// Stop before observing the receive and record what to wait for, so the
    /// caller can wait for an Ark update after releasing the incoming lock.
    Defer,
    /// The caller already waited without the incoming lock: poll once and
    /// report an unchanged receive as awaiting a backend notification.
    Waited,
}

/// The accepted receive a deferred advancement waits on.
#[derive(Debug)]
struct ReceiveWaitTarget {
    action_id: FederationBarkActionId,
    intent: FederationBarkReceiveIntent,
    prepared: FederationBarkReceivePrepared,
}

/// Poll and advance one already-created federated Bark quote. Mailbox events
/// call the same function; they never supply settlement truth themselves.
///
/// With `wait_for_backend_update`, the Ark long-poll runs after the member's
/// incoming lock is released. That lock serializes every incoming quote on the
/// member, including new quote creation, so holding it across a multi-second
/// server wait for each unpaid invoice starves everything else.
pub async fn advance_federated_bark_mint_quote(
    state: &dyn FederatedBarkNode,
    quote_id: &QuoteId,
    observer_fallback_round: u32,
    wait_for_backend_update: bool,
) -> Result<FederationPaymentAdvancement, cdk::Error> {
    advance_with_unlocked_backend_wait(
        state.bark_incoming_lock(),
        wait_for_backend_update,
        |backend_wait| async move {
            let mut wait_target = None;
            let advancement = advance_federated_bark_mint_quote_inner(
                state,
                quote_id,
                observer_fallback_round,
                None,
                backend_wait,
                &mut wait_target,
            )
            .await?;
            Ok((advancement, wait_target))
        },
        |wait_target: ReceiveWaitTarget| async move {
            let runtime = state.bark_runtime().ok_or_else(bark_unavailable)?;
            let wait_timer = super::latency::FederationLatencyTimer::start(
                super::latency::FederationLatencyCorrelation::Action,
                wait_target.action_id.to_string(),
                super::latency::FederationLatencyBackend::Bark,
                super::latency::FederationLatencyDirection::Incoming,
                super::latency::FederationLatencyPhase::BarkReceiveServerWait,
            );
            let wait_result = runtime
                .wait_for_receive_update(&wait_target.intent, &wait_target.prepared)
                .await;
            wait_timer.finish(super::latency::FederationLatencyOutcome::from_success(
                wait_result.is_ok(),
            ));
            wait_result.map_err(|error| {
                error
                    .into_federation_error(
                        "wait for incoming HTLCs",
                        Some(wait_target.prepared.intent_operation_id),
                    )
                    .into()
            })
        },
    )
    .await
}

/// Run one incoming advancement under `lock`, waiting for a backend update
/// between two locked passes instead of inside one.
///
/// The first pass runs with [`ReceiveBackendWait::Defer`] when a wait is
/// wanted. If it reports a receive to wait on, the lock is released for the
/// wait and a second pass observes the receive with
/// [`ReceiveBackendWait::Waited`], re-reading every state the wait may have
/// outdated.
async fn advance_with_unlocked_backend_wait<T, Pass, PassFuture, Wait, WaitFuture>(
    lock: &tokio::sync::Mutex<()>,
    wait_for_backend_update: bool,
    mut pass: Pass,
    wait: Wait,
) -> Result<FederationPaymentAdvancement, cdk::Error>
where
    Pass: FnMut(ReceiveBackendWait) -> PassFuture,
    PassFuture:
        std::future::Future<Output = Result<(FederationPaymentAdvancement, Option<T>), cdk::Error>>,
    Wait: FnOnce(T) -> WaitFuture,
    WaitFuture: std::future::Future<Output = Result<(), cdk::Error>>,
{
    let backend_wait = match wait_for_backend_update {
        true => ReceiveBackendWait::Defer,
        false => ReceiveBackendWait::Poll,
    };
    let (advancement, wait_target) = {
        let _guard = lock.lock().await;
        pass(backend_wait).await?
    };
    let Some(wait_target) = wait_target else {
        return Ok(advancement);
    };
    wait(wait_target).await?;
    let _guard = lock.lock().await;
    let (advancement, _) = pass(ReceiveBackendWait::Waited).await?;
    Ok(advancement)
}

/// Advance a mint quote after a peer delivered Bark evidence, without waiting
/// for a backend update.
pub async fn advance_federated_bark_mint_quote_from_evidence_delivery(
    state: &dyn FederatedBarkNode,
    quote_id: &QuoteId,
    collector_fallback_round: u32,
) -> Result<FederationPaymentAdvancement, cdk::Error> {
    let _guard = state.bark_incoming_lock().lock().await;
    advance_federated_bark_mint_quote_inner(
        state,
        quote_id,
        collector_fallback_round,
        Some(collector_fallback_round),
        ReceiveBackendWait::Poll,
        &mut None,
    )
    .await
}

/// Advance one incoming quote. The caller holds the member's incoming Bark
/// lock.
///
/// With [`ReceiveBackendWait::Defer`], a receive that would be observed is
/// recorded in `wait_target` instead, and the returned advancement is only a
/// placeholder for the caller's second pass.
async fn advance_federated_bark_mint_quote_inner(
    state: &dyn FederatedBarkNode,
    quote_id: &QuoteId,
    observer_fallback_round: u32,
    collector_fallback_round_override: Option<u32>,
    backend_wait: ReceiveBackendWait,
    wait_target: &mut Option<ReceiveWaitTarget>,
) -> Result<FederationPaymentAdvancement, cdk::Error> {
    let federation = state
        .mint()
        .federation_config()
        .ok_or(cdk::Error::Internal)?;
    let runtime = state.bark_runtime().ok_or_else(bark_unavailable)?;
    let quote = quote_id.to_string();
    let projection_timer = super::latency::FederationLatencyTimer::start(
        super::latency::FederationLatencyCorrelation::Quote,
        quote_id.to_string(),
        super::latency::FederationLatencyBackend::Bark,
        super::latency::FederationLatencyDirection::Incoming,
        super::latency::FederationLatencyPhase::BarkProjectionReady,
    );
    let projection_result =
        super::projection::federation_bark_ready_projection(state.mint(), runtime).await;
    projection_timer.finish(super::latency::FederationLatencyOutcome::from_success(
        projection_result.is_ok(),
    ));
    let projection = projection_result.map_err(super::projection::projection_error)?;
    let Some(action) = projection
        .actions
        .iter()
        .find(|action| action.quote.as_deref() == Some(quote.as_str()))
        .cloned()
    else {
        return Ok(FederationPaymentAdvancement::AwaitingFederationEvidence);
    };
    let intent = state
        .mint()
        .federation_bark_receive_intent(action.origin_operation_id)
        .await?
        .ok_or(cdk::Error::Internal)?;
    let prepared_record = projection
        .transitions
        .iter()
        .find(|record| {
            record.action_id == action.action_id
                && record.phase == FederationBarkSemanticPhase::ReceivePrepared
        })
        .ok_or(cdk::Error::Internal)?;
    let FederationBarkOperation::ReceivePrepared(prepared) = &prepared_record.operation else {
        return Err(cdk::Error::Internal);
    };
    let accepted_evidence_present = projection.observations.iter().any(|record| {
        matches!(
            &record.operation,
            FederationBarkOperation::MemberObservation(observation)
                if observation.action_id == action.action_id
                    && observation.status == FederationBarkObservationStatus::Accepted
        )
    });
    if action.phase == FederationBarkSemanticPhase::ReceivePrepared
        && receive_is_automatically_terminal(
            prepared.expires_at,
            accepted_evidence_present,
            unix_time()?,
        )
    {
        return Ok(FederationPaymentAdvancement::Complete);
    }
    if action.phase != FederationBarkSemanticPhase::ReceiveClaimed {
        runtime
            .apply_receive_prepared(&intent, prepared)
            .await
            .map_err(|error| {
                error.into_federation_error(
                    "apply incoming preparation",
                    Some(action.origin_operation_id),
                )
            })?;
    }

    match action.phase {
        FederationBarkSemanticPhase::ReceivePrepared => {
            let receive_actor = state
                .mint()
                .federation_bark_receive_actor(action.origin_operation_id)
                .await?;
            let is_receive_actor = receive_actor == federation.member_id();
            let threshold = federation
                .public_config()
                .threshold_params
                .signature_threshold;
            let collector_fallback_round = collector_fallback_round_override.unwrap_or_else(|| {
                scheduled_bark_evidence_collector_round(observer_fallback_round, threshold)
            });
            let evidence_collector = bark_evidence_collector(
                federation.public_config().members.keys().copied(),
                intent.actor,
                collector_fallback_round,
            )
            .ok_or(cdk::Error::Internal)?;
            let is_evidence_collector = evidence_collector == federation.member_id();
            let cached_binding = runtime
                .receive_secret_store()
                .evidence_binding(
                    action.action_id,
                    FederationBarkEvidencePhase::ReceiveHtlcsReady,
                    federation.member_id(),
                    federation.public_config(),
                )
                .map_err(|error| {
                    error.into_federation_error(
                        "read incoming evidence binding",
                        Some(action.latest_operation_id),
                    )
                })?;
            if !is_selected_bark_receive_observer(
                federation.public_config().members.keys().copied(),
                intent.actor,
                receive_actor,
                federation.member_id(),
                usize::from(threshold),
                observer_fallback_round,
            ) {
                return Ok(unselected_bark_observer_wait(accepted_evidence_present));
            }
            let (observation, mut transition) = match cached_binding {
                Some(binding) => {
                    if binding.evidence_operation_id != prepared_record.operation_id
                        || binding.attempt != 1
                    {
                        return Err(bark_unavailable());
                    }
                    let observation = binding.observation_for(federation.member_id());
                    let transition = if is_evidence_collector {
                        Some(
                            runtime
                                .receive_htlc_transition_from_accepted_observation(
                                    &intent,
                                    prepared_record.operation_id,
                                    prepared,
                                    &observation,
                                )
                                .await
                                .map_err(|error| {
                                    error.into_federation_error(
                                        "reconstruct incoming HTLC transition",
                                        Some(prepared_record.operation_id),
                                    )
                                })?,
                        )
                    } else {
                        None
                    };
                    (observation, transition)
                }
                None => {
                    if backend_wait == ReceiveBackendWait::Defer {
                        *wait_target = Some(ReceiveWaitTarget {
                            action_id: action.action_id,
                            intent,
                            prepared: prepared.clone(),
                        });
                        return Ok(FederationPaymentAdvancement::AwaitingBackendNotification);
                    }
                    let observation_timer = super::latency::FederationLatencyTimer::start(
                        super::latency::FederationLatencyCorrelation::Action,
                        action.action_id.to_string(),
                        super::latency::FederationLatencyBackend::Bark,
                        super::latency::FederationLatencyDirection::Incoming,
                        super::latency::FederationLatencyPhase::BarkReceiveHtlcObservation,
                    );
                    let observation_result = runtime
                        .observe_receive_htlcs(
                            &intent,
                            prepared_record.operation_id,
                            prepared,
                            1,
                            is_receive_actor,
                        )
                        .await;
                    let observation_outcome = match &observation_result {
                        Ok(Some(_)) => super::latency::FederationLatencyOutcome::Success,
                        Ok(None) => super::latency::FederationLatencyOutcome::Pending,
                        Err(_) => super::latency::FederationLatencyOutcome::Error,
                    };
                    observation_timer.finish(observation_outcome);
                    let Some((observation, transition)) = observation_result.map_err(|error| {
                        error.into_federation_error(
                            "observe incoming HTLCs",
                            Some(prepared_record.operation_id),
                        )
                    })?
                    else {
                        return Ok(if backend_wait == ReceiveBackendWait::Waited {
                            FederationPaymentAdvancement::AwaitingBackendNotification
                        } else {
                            FederationPaymentAdvancement::AwaitingExternalEvidence
                        });
                    };
                    (observation, is_evidence_collector.then_some(transition))
                }
            };
            let certificate = collect_bark_evidence_certificate(
                state,
                intent.action_id,
                intent.actor,
                FederationBarkEvidencePhase::ReceiveHtlcsReady,
                prepared_record.operation_id,
                1,
                &observation,
                threshold,
                collector_fallback_round,
                super::latency::FederationLatencyDirection::Incoming,
            )
            .await?;
            if let (Some(certificate), Some(transition)) = (certificate, transition.as_mut()) {
                transition.actor = intent.actor;
                transition.evidence_certificate = Some(Box::new(certificate));
                let plan = runtime
                    .propose_certified_receive_claim(
                        &intent,
                        prepared,
                        transition.clone(),
                        receive_claim_proposal(
                            federation.public_config(),
                            &projection,
                            intent.actor,
                        )?,
                    )
                    .await
                    .map_err(|error| {
                        error.into_federation_error(
                            "plan certified incoming claim",
                            Some(prepared_record.operation_id),
                        )
                    })?;
                let envelope = federation.operation_envelope(super::bark_federation_operation(
                    FederationBarkOperation::ActionPlan(plan),
                )?);
                state.submit_envelope(envelope).await?.ensure_accepted()?;
                return Ok(FederationPaymentAdvancement::Progressed);
            }
            Ok(if is_evidence_collector {
                FederationPaymentAdvancement::AwaitingObserverFallback
            } else {
                FederationPaymentAdvancement::AwaitingFederationEvidence
            })
        }
        FederationBarkSemanticPhase::ClaimPlanned
        | FederationBarkSemanticPhase::ReceivePreimageDisclosed
        | FederationBarkSemanticPhase::SigningAttemptAdvanced
            if action.signing_retry_phase.is_none_or(|phase| {
                matches!(
                    phase,
                    FederationBarkSemanticPhase::ClaimPlanned
                        | FederationBarkSemanticPhase::ReceivePreimageDisclosed
                )
            }) =>
        {
            let plan_operation_id = match &action.operation {
                FederationBarkOperation::ActionPlan(_) => action.latest_operation_id,
                FederationBarkOperation::ActionTransition(transition) => {
                    transition.plan_operation_id
                }
                _ => return Err(cdk::Error::Internal),
            };
            let plan_entry = state
                .mint()
                .localstore()
                .get_federation_journal_entry(
                    FederationJournalEntryKey::Operation(plan_operation_id),
                    None,
                )
                .await?
                .ok_or(cdk::Error::Internal)?;
            let FederationBarkOperation::ActionPlan(plan) =
                super::bark_operation(&plan_entry.envelope.operation)?
            else {
                return Err(cdk::Error::Internal);
            };
            if plan.actor != federation.member_id() {
                return Ok(FederationPaymentAdvancement::AwaitingFederationEvidence);
            }
            let authorization_entry = super::custody_attempt::accepted_bark_custody_authorization(
                state.mint(),
                &projection,
                &action,
                &plan_entry,
            )
            .await?;
            let claim_timer = super::latency::FederationLatencyTimer::start(
                super::latency::FederationLatencyCorrelation::Action,
                action.action_id.to_string(),
                super::latency::FederationLatencyBackend::Bark,
                super::latency::FederationLatencyDirection::Incoming,
                super::latency::FederationLatencyPhase::BarkReceiveClaim,
            );
            let claim_result = runtime
                .execute_receive_claim(&intent, prepared, plan_entry.clone(), authorization_entry)
                .await;
            claim_timer.finish(super::latency::FederationLatencyOutcome::from_success(
                claim_result.is_ok(),
            ));
            let outcome = match claim_result {
                Ok(outcome) => outcome,
                Err(crate::Error::CustodySignerUnavailable) => {
                    return advance_receive_signing_attempt(state, &action, &plan_entry).await;
                }
                Err(error) => {
                    return Err(error
                        .into_federation_error(
                            "execute incoming claim",
                            Some(plan_entry.operation_id),
                        )
                        .into())
                }
            };
            let effective_phase = action.effective_phase();
            match outcome {
                crate::FederatedBarkReceiveClaimOutcome::RecoveryRequired {
                    preimage_disclosed,
                    signer_unavailable,
                } => {
                    if effective_phase == FederationBarkSemanticPhase::ClaimPlanned {
                        let envelope =
                            federation.operation_envelope(super::bark_federation_operation(
                                FederationBarkOperation::ActionTransition(preimage_disclosed),
                            )?);
                        state.submit_envelope(envelope).await?.ensure_accepted()?;
                        Ok(FederationPaymentAdvancement::Progressed)
                    } else if signer_unavailable {
                        advance_receive_signing_attempt(state, &action, &plan_entry).await
                    } else {
                        Ok(FederationPaymentAdvancement::AwaitingExternalEvidence)
                    }
                }
                crate::FederatedBarkReceiveClaimOutcome::Claimed {
                    preimage_disclosed,
                    result,
                } => {
                    let claimed_amount = result.amount();
                    let (claimed, response) = if effective_phase
                        == FederationBarkSemanticPhase::ClaimPlanned
                    {
                        let disclosed_envelope =
                            federation.operation_envelope(super::bark_federation_operation(
                                FederationBarkOperation::ActionTransition(
                                    preimage_disclosed.clone(),
                                ),
                            )?);
                        let disclosed_operation_id = disclosed_envelope.operation_id()?;
                        let claimed = runtime
                            .prepare_receive_claim_successor(
                                &preimage_disclosed,
                                disclosed_operation_id,
                                *result,
                            )
                            .map_err(|error| {
                                error.into_federation_error(
                                    "prepare incoming claim successor",
                                    Some(disclosed_operation_id),
                                )
                            })?;
                        validate_atomic_receive_claim_group(
                            &preimage_disclosed,
                            disclosed_operation_id,
                            &claimed,
                        )?;
                        let claimed_envelope =
                            federation.operation_envelope(super::bark_federation_operation(
                                FederationBarkOperation::ActionTransition(claimed.clone()),
                            )?);
                        let responses = state
                            .submit_ordered_successor_group(vec![
                                disclosed_envelope,
                                claimed_envelope,
                            ])
                            .await?;
                        let [disclosed_response, claimed_response] = responses.as_slice() else {
                            return Err(bark_unavailable());
                        };
                        disclosed_response.ensure_accepted()?;
                        claimed_response.ensure_accepted()?;
                        (claimed, claimed_response.clone())
                    } else {
                        let accepted_preimage = receive_transition_entry(
                            state,
                            &projection,
                            action.action_id,
                            FederationBarkSemanticPhase::ReceivePreimageDisclosed,
                        )
                        .await?;
                        let claimed = runtime
                            .finalize_receive_claim(&accepted_preimage, *result)
                            .map_err(|error| {
                                error.into_federation_error(
                                    "finalize incoming claim",
                                    Some(action.latest_operation_id),
                                )
                            })?;
                        let envelope =
                            federation.operation_envelope(super::bark_federation_operation(
                                FederationBarkOperation::ActionTransition(claimed.clone()),
                            )?);
                        let response = state.submit_envelope(envelope).await?;
                        response.ensure_accepted()?;
                        (claimed, response)
                    };
                    let observation = FederationBarkMemberObservation {
                        version: FEDERATION_BARK_OPERATION_VERSION,
                        action_id: claimed.action_id,
                        quote: Some(intent.quote.clone()),
                        observer: federation.member_id(),
                        observation_epoch: u64::from(claimed.attempt),
                        source_scope: intent.ark_server,
                        payment_hash: Some(intent.payment_hash),
                        status: FederationBarkObservationStatus::Succeeded,
                        amount: claimed_amount,
                        fee: Amount::ZERO,
                        result_hash: claimed.result_hash,
                    };
                    persist_local_bark_evidence_attestation(
                        state,
                        FederationBarkEvidencePhase::ReceiveClaimed,
                        response.operation_id,
                        claimed.attempt,
                        &observation,
                    )?;
                    let acknowledge_timer = super::latency::FederationLatencyTimer::start(
                        super::latency::FederationLatencyCorrelation::Action,
                        action.action_id.to_string(),
                        super::latency::FederationLatencyBackend::Bark,
                        super::latency::FederationLatencyDirection::Incoming,
                        super::latency::FederationLatencyPhase::BarkReceiveAcknowledge,
                    );
                    let acknowledge_result =
                        runtime.acknowledge_receive_claim(&intent, prepared).await;
                    acknowledge_timer.finish(
                        super::latency::FederationLatencyOutcome::from_success(
                            acknowledge_result.is_ok(),
                        ),
                    );
                    acknowledge_result.map_err(|error| {
                        error.into_federation_error(
                            "acknowledge incoming claim",
                            Some(response.operation_id),
                        )
                    })?;
                    Ok(FederationPaymentAdvancement::Progressed)
                }
            }
        }
        FederationBarkSemanticPhase::ReceiveClaimed => {
            advance_receive_claimed_evidence(
                state,
                &projection,
                &action,
                &intent,
                observer_fallback_round,
                collector_fallback_round_override,
            )
            .await
        }
        _ => Ok(FederationPaymentAdvancement::Complete),
    }
}

async fn advance_receive_signing_attempt(
    state: &dyn FederatedBarkNode,
    action: &FederationBarkActionRecord,
    plan_entry: &FederationJournalEntry,
) -> Result<FederationPaymentAdvancement, cdk::Error> {
    let federation = state
        .mint()
        .federation_config()
        .ok_or(cdk::Error::Internal)?;
    let runtime = state.bark_runtime().ok_or_else(bark_unavailable)?;
    let retry = runtime
        .advance_signing_attempt(action, plan_entry)
        .map_err(|error| {
            error.into_federation_error(
                "advance incoming signing attempt",
                Some(plan_entry.operation_id),
            )
        })?;
    let envelope = federation.operation_envelope(super::bark_federation_operation(
        FederationBarkOperation::ActionTransition(retry),
    )?);
    state.submit_envelope(envelope).await?.ensure_accepted()?;
    Ok(FederationPaymentAdvancement::Progressed)
}

async fn advance_receive_claimed_evidence(
    state: &dyn FederatedBarkNode,
    projection: &cdk::federation::FederationBarkProjectionSnapshot,
    action: &cdk::federation::FederationBarkActionRecord,
    intent: &FederationBarkReceiveIntent,
    observer_fallback_round: u32,
    collector_fallback_round_override: Option<u32>,
) -> Result<FederationPaymentAdvancement, cdk::Error> {
    let federation = state
        .mint()
        .federation_config()
        .ok_or(cdk::Error::Internal)?;
    let runtime = state.bark_runtime().ok_or_else(bark_unavailable)?;
    let FederationBarkOperation::ActionTransition(claimed) = &action.operation else {
        return Err(cdk::Error::Internal);
    };
    let already_certified = projection.observations.iter().any(|record| {
        matches!(
            &record.operation,
            FederationBarkOperation::EvidenceCertificate(certificate)
                if certificate.binding.action_id == action.action_id
                    && certificate.binding.phase == FederationBarkEvidencePhase::ReceiveClaimed
        )
    });
    if already_certified {
        return Ok(FederationPaymentAdvancement::Complete);
    }
    let public_config = federation.public_config();
    let required = required_bark_result_observers(
        public_config.threshold_params.signature_threshold,
        public_config
            .payment_verification_policy(FederationPaymentObservationContext::MintQuote)
            .required_matching_observations(),
    );
    let required = u16::try_from(required).map_err(|_| cdk::Error::Internal)?;
    let collector_fallback_round = collector_fallback_round_override.unwrap_or_else(|| {
        scheduled_bark_evidence_collector_round(observer_fallback_round, required)
    });
    let is_evidence_collector = bark_evidence_collector(
        public_config.members.keys().copied(),
        action.actor,
        collector_fallback_round,
    )
    .is_some_and(|collector| collector == federation.member_id());
    let cached_binding = runtime
        .receive_secret_store()
        .evidence_binding(
            action.action_id,
            FederationBarkEvidencePhase::ReceiveClaimed,
            federation.member_id(),
            public_config,
        )
        .map_err(|error| {
            error.into_federation_error(
                "read incoming evidence binding",
                Some(action.latest_operation_id),
            )
        })?;
    if !is_selected_bark_observer(
        public_config.members.keys().copied(),
        action.actor,
        federation.member_id(),
        usize::from(required),
        observer_fallback_round,
    ) {
        return Ok(unselected_bark_observer_wait(cached_binding.is_some()));
    }
    let observation = match cached_binding {
        Some(binding) => {
            if binding.evidence_operation_id != action.latest_operation_id
                || binding.attempt != claimed.attempt
            {
                return Err(bark_unavailable());
            }
            binding.observation_for(federation.member_id())
        }
        None => {
            let amount = runtime
                .validate_receive_claim_result(claimed)
                .await
                .map_err(|error| {
                    error.into_federation_error(
                        "validate incoming claim result",
                        Some(action.latest_operation_id),
                    )
                })?;
            FederationBarkMemberObservation {
                version: FEDERATION_BARK_OPERATION_VERSION,
                action_id: action.action_id,
                quote: Some(intent.quote.clone()),
                observer: federation.member_id(),
                observation_epoch: u64::from(claimed.attempt),
                source_scope: intent.ark_server,
                payment_hash: Some(intent.payment_hash),
                status: FederationBarkObservationStatus::Succeeded,
                amount,
                fee: Amount::ZERO,
                result_hash: claimed.result_hash,
            }
        }
    };
    let certificate = collect_bark_evidence_certificate(
        state,
        intent.action_id,
        intent.actor,
        FederationBarkEvidencePhase::ReceiveClaimed,
        action.latest_operation_id,
        claimed.attempt,
        &observation,
        required,
        collector_fallback_round,
        super::latency::FederationLatencyDirection::Incoming,
    )
    .await?;
    if let Some(certificate) = certificate {
        let envelope = federation.operation_envelope(super::bark_federation_operation(
            FederationBarkOperation::EvidenceCertificate(certificate),
        )?);
        state.submit_envelope(envelope).await?.ensure_accepted()?;
        return Ok(FederationPaymentAdvancement::Progressed);
    }
    // Only the active collector starts the bounded standby timer. Letting
    // every attester rotate independently can race multiple publications for
    // the same logical certificate.
    Ok(if is_evidence_collector {
        FederationPaymentAdvancement::AwaitingObserverFallback
    } else {
        FederationPaymentAdvancement::AwaitingFederationEvidence
    })
}

pub fn receive_is_automatically_terminal(
    expires_at: u64,
    accepted_evidence_present: bool,
    now: u64,
) -> bool {
    !accepted_evidence_present
        && now >= expires_at.saturating_add(FEDERATION_BARK_RECEIVE_EXPIRY_GRACE_SECONDS)
}

/// Retry invoice preparation for one exact accepted receive intent.
pub async fn advance_federated_bark_receive_intent(
    state: &dyn FederatedBarkNode,
    action_id: FederationBarkActionId,
) -> Result<usize, cdk::Error> {
    let _guard = state.bark_incoming_lock().lock().await;
    let runtime = state.bark_runtime().ok_or_else(bark_unavailable)?;
    let projection_timer = super::latency::FederationLatencyTimer::start(
        super::latency::FederationLatencyCorrelation::Action,
        action_id.to_string(),
        super::latency::FederationLatencyBackend::Bark,
        super::latency::FederationLatencyDirection::Incoming,
        super::latency::FederationLatencyPhase::BarkProjectionReady,
    );
    let projection_result =
        super::projection::federation_bark_ready_projection(state.mint(), runtime).await;
    projection_timer.finish(super::latency::FederationLatencyOutcome::from_success(
        projection_result.is_ok(),
    ));
    let projection = projection_result.map_err(super::projection::projection_error)?;
    let action = projection
        .actions
        .iter()
        .find(|action| action.action_id == action_id)
        .ok_or(cdk::Error::Internal)?;
    let FederationBarkOperation::ReceiveIntent(intent) = &action.operation else {
        return Ok(0);
    };
    prepare_and_accept_invoice(
        state,
        action.latest_operation_id,
        action.latest_operation_index,
        intent,
    )
    .await?;
    Ok(1)
}

/// Derive the stable quote ID used to recover an exact federated Bark request.
///
/// NUT-19's quote public key is required only for this backend. Besides locking
/// minting to the requester, it gives retries a protocol-native identity even
/// when the original HTTP response (and its quote ID) was lost.
pub fn federated_bark_quote_id(
    federation_id: FederationId,
    request: &MintQuoteRequest,
) -> Result<QuoteId, cdk::Error> {
    let MintQuoteRequest::Bolt11(request) = request else {
        return Err(cdk::Error::InvalidPaymentRequest);
    };
    let pubkey = request.pubkey.ok_or(cdk::Error::PubkeyRequired)?;
    let mut hasher = Sha256::new();
    hasher.update(FEDERATION_BARK_QUOTE_ID_DOMAIN);
    hasher.update(federation_id.to_bytes());
    hasher.update(pubkey.to_bytes());
    let digest = hasher.finalize();
    let mut bytes = [0_u8; 16];
    bytes.copy_from_slice(&digest[..16]);
    // Use an RFC 9562 UUIDv8 container for the domain-specific hash prefix.
    bytes[6] = (bytes[6] & 0x0f) | 0x80;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    Ok(uuid::Uuid::from_bytes(bytes).into())
}

/// Prepare the ordinary CDK quote from a consensus-accepted Bark invoice.
pub async fn prepare_federated_bark_mint_quote(
    state: &dyn FederatedBarkNode,
    quote_id: QuoteId,
    request: MintQuoteRequest,
) -> Result<
    (
        MintQuoteResponse<QuoteId>,
        cdk::cdk_payment::PaymentIdentifier,
        Option<cdk::federation::FederationOperationEnvelope>,
    ),
    cdk::Error,
> {
    let _guard = state.bark_incoming_lock().lock().await;
    if let Some(quote) = state.mint().localstore().get_mint_quote(&quote_id).await? {
        let request_lookup_id = quote.request_lookup_id.clone();
        return Ok((quote.try_into()?, request_lookup_id, None));
    }
    let (invoice, receive_prepared) = accepted_invoice(state, &quote_id, &request).await?;
    let (response, request_lookup_id) = state
        .mint()
        .prepare_bolt11_mint_quote_with_invoice(quote_id, request, invoice)
        .await?;
    Ok((response, request_lookup_id, receive_prepared))
}

async fn accepted_invoice(
    state: &dyn FederatedBarkNode,
    quote_id: &QuoteId,
    request: &MintQuoteRequest,
) -> Result<
    (
        cdk::Bolt11Invoice,
        Option<cdk::federation::FederationOperationEnvelope>,
    ),
    cdk::Error,
> {
    let MintQuoteRequest::Bolt11(bolt11) = request else {
        return Err(cdk::Error::InvalidPaymentRequest);
    };
    if bolt11.unit != CurrencyUnit::Sat
        || bolt11.amount == Amount::ZERO
        || federation_bark_description_hash(bolt11.description.as_deref()).is_err()
    {
        return Err(cdk::Error::InvalidPaymentRequest);
    }
    let federation = state
        .mint()
        .federation_config()
        .ok_or(cdk::Error::Internal)?;
    let runtime = state.bark_runtime().ok_or_else(bark_unavailable)?;
    let limits = runtime.limits();
    let amount = u64::from(bolt11.amount);
    if amount < limits.minimum_sat() || amount > limits.maximum_sat() {
        return Err(cdk::Error::InvalidPaymentRequest);
    }
    let projection = super::projection::federation_bark_ready_projection(state.mint(), runtime)
        .await
        .map_err(super::projection::projection_error)?;
    let quote = quote_id.to_string();
    let action = projection
        .actions
        .iter()
        .find(|action| action.quote.as_deref() == Some(quote.as_str()));

    if let Some(action) = action {
        return match &action.operation {
            FederationBarkOperation::ReceivePrepared(prepared) => {
                let intent = state
                    .mint()
                    .federation_bark_receive_intent(prepared.intent_operation_id)
                    .await?
                    .ok_or(cdk::Error::InvalidPaymentRequest)?;
                validate_request_against_intent(bolt11, &intent)?;
                prepared
                    .validate_against_intent(&intent)
                    .map_err(cdk::Error::from)?;
                validate_request_against_prepared(bolt11, prepared)?;
                let invoice = prepared
                    .invoice
                    .parse()
                    .map_err(|_| cdk::Error::InvalidPaymentRequest)?;
                Ok((invoice, None))
            }
            FederationBarkOperation::ReceiveIntent(intent) => {
                validate_request_against_intent(bolt11, intent)?;
                let (invoice, _, envelope) = prepare_invoice_candidate(
                    state,
                    action.latest_operation_id,
                    action.latest_operation_index,
                    intent,
                )
                .await?;
                Ok((invoice, Some(envelope)))
            }
            FederationBarkOperation::ActionPlan(_)
            | FederationBarkOperation::ActionTransition(_)
            | FederationBarkOperation::MemberObservation(_)
            | FederationBarkOperation::EvidenceCertificate(_) => {
                Err(cdk::Error::InvalidPaymentRequest)
            }
        };
    }

    let store = runtime.receive_secret_store();
    let resumable = store
        .find_by_quote(federation.federation_id(), &quote)
        .map_err(|error| error.into_federation_error("find protected receive secret", None))?;
    let now = unix_time()?;
    let (mut intent, secret) = match resumable {
        Some(resumable) => {
            validate_binding_against_request(federation.member_id(), bolt11, &resumable.binding)?;
            (
                intent_from_binding(&resumable.binding, bolt11.description.clone(), Vec::new()),
                resumable.secret,
            )
        }
        None => fresh_intent(state, quote_id, bolt11, &projection, now).await?,
    };
    let binding =
        FederationBarkReceiveSecretBinding::from_intent(federation.federation_id(), &intent);
    let local_request = cdk::federation::FederationBarkPrepareReceiveSecretRequest::new(
        binding.clone(),
        federation.member_id(),
        *secret.as_bytes(),
    );
    let local_acknowledgement = store
        .prepare_receive_secret(
            &local_request,
            federation.member_id(),
            federation.public_config(),
            federation.private_config(),
            now,
        )
        .map_err(|error| error.into_federation_error("prepare protected receive secret", None))?;
    let remote_members = federation
        .public_config()
        .members
        .values()
        .filter(|member| member.member_id != federation.member_id())
        .cloned()
        .collect::<Vec<MemberConfigPublic>>();
    let peer_client = state.bark_peers().map_err(|_| bark_unavailable())?;
    let secret_timer = super::latency::FederationLatencyTimer::start(
        super::latency::FederationLatencyCorrelation::Action,
        intent.action_id.to_string(),
        super::latency::FederationLatencyBackend::Bark,
        super::latency::FederationLatencyDirection::Incoming,
        super::latency::FederationLatencyPhase::BarkSecretReplication,
    );
    let secret_result = FederationBarkReceiveSecretCoordinator::new(peer_client.as_ref())
        .distribute(
            federation.public_config(),
            &binding,
            &secret,
            &remote_members,
            vec![local_acknowledgement],
        )
        .await;
    secret_timer.finish(super::latency::FederationLatencyOutcome::from_success(
        secret_result.is_ok(),
    ));
    intent.secret_acknowledgements = secret_result.map_err(|_| bark_unavailable())?;
    intent
        .validate_secret_acknowledgements(federation.public_config())
        .map_err(cdk::Error::from)?;
    let envelope = federation.operation_envelope(super::bark_federation_operation(
        FederationBarkOperation::ReceiveIntent(intent.clone()),
    )?);
    let accepted = state.submit_envelope(envelope).await?;
    let entry = accepted.outcome.entry();
    store
        .mark_intent_accepted(
            intent.action_id,
            crate::FederatedBarkReceiveSecretAcceptedRef {
                operation_id: entry.operation_id,
                operation_index: entry.index,
            },
        )
        .map_err(|error| {
            error
                .into_federation_error("persist accepted incoming intent", Some(entry.operation_id))
        })?;
    let (invoice, _, envelope) =
        prepare_invoice_candidate(state, entry.operation_id, entry.index, &intent).await?;
    Ok((invoice, Some(envelope)))
}

async fn fresh_intent(
    state: &dyn FederatedBarkNode,
    quote_id: &QuoteId,
    request: &cdk::nuts::MintQuoteBolt11Request,
    projection: &cdk::federation::FederationBarkProjectionSnapshot,
    now: u64,
) -> Result<
    (
        FederationBarkReceiveIntent,
        cdk::federation::FederationBarkReceiveSecret,
    ),
    cdk::Error,
> {
    let federation = state
        .mint()
        .federation_config()
        .ok_or(cdk::Error::Internal)?;
    let runtime = state.bark_runtime().ok_or_else(bark_unavailable)?;
    let application =
        super::bark_application(federation.public_config()).map_err(|_| bark_unavailable())?;
    let minimum_cltv_delta = runtime
        .minimum_receive_cltv_delta()
        .await
        .map_err(|error| error.into_federation_error("read incoming CLTV requirement", None))?;
    let next_index = projection
        .derivation_allocations
        .iter()
        .map(|allocation| allocation.locator.index)
        .max()
        .map_or(Ok(0), |index| {
            index.checked_add(1).ok_or_else(bark_unavailable)
        })?;
    if next_index > FEDERATION_MAX_DERIVATION_INDEX {
        return Err(bark_unavailable());
    }
    let mailbox_id = runtime
        .derive_public_key(next_index)
        .map_err(|error| error.into_federation_error("derive incoming mailbox", None))?
        .serialize();
    let mut preimage = [0; 32];
    OsRng.fill_bytes(&mut preimage);
    let quote = quote_id.to_string();
    // The Ark server owns the exact invoice expiry (its default may be 48h).
    // This is only the consensus preparation deadline; the ordinary MintQuote
    // below retains the exact returned BOLT11 expiry.
    let expires_at = now
        .checked_add(FEDERATION_BARK_RECEIVE_INTENT_LIFETIME_SECONDS)
        .ok_or_else(bark_unavailable)?;
    let intent = FederationBarkReceiveIntent {
        version: FEDERATION_BARK_OPERATION_VERSION,
        action_id: FederationBarkActionId::derive_receive_intent(
            federation.federation_id(),
            &quote,
        )
        .map_err(cdk::Error::from)?,
        quote,
        actor: federation.member_id(),
        cashu_amount: request.amount,
        gross_amount: request.amount,
        receive_fee: Amount::ZERO,
        minimum_net_amount: request.amount,
        unit: FederationBarkUnit::Sat,
        description_hash: federation_bark_description_hash(request.description.as_deref())
            .map_err(cdk::Error::from)?,
        description: request.description.clone(),
        payment_hash: payment_hash_for_preimage(&preimage).map_err(cdk::Error::from)?,
        preimage_commitment: preimage_commitment(&preimage).map_err(cdk::Error::from)?,
        secret_acknowledgements: Vec::new(),
        destination: FederationBarkVtxoLocator::vtxo_custody(next_index),
        mailbox_scope: federation_bark_mailbox_scope(&mailbox_id).map_err(cdk::Error::from)?,
        mailbox_id,
        ark_server: ark_server_commitment(&application.ark_server_public_key)
            .map_err(cdk::Error::from)?,
        network: application.network,
        minimum_cltv_delta,
        fee_policy_version: 1,
        anti_dos_policy_version: 1,
        receive_proof: FederationBarkReceiveProof::NoProof,
        expires_at,
    };
    Ok((
        intent,
        cdk::federation::FederationBarkReceiveSecret::from_bytes(preimage),
    ))
}

async fn prepare_and_accept_invoice(
    state: &dyn FederatedBarkNode,
    intent_operation_id: FederationOperationId,
    intent_operation_index: u64,
    intent: &FederationBarkReceiveIntent,
) -> Result<cdk::Bolt11Invoice, cdk::Error> {
    let (invoice, prepared, envelope) =
        prepare_invoice_candidate(state, intent_operation_id, intent_operation_index, intent)
            .await?;
    state.submit_envelope(envelope).await?;
    apply_accepted_receive_prepared(state, intent, &prepared).await;
    Ok(invoice)
}

async fn prepare_invoice_candidate(
    state: &dyn FederatedBarkNode,
    intent_operation_id: FederationOperationId,
    intent_operation_index: u64,
    intent: &FederationBarkReceiveIntent,
) -> Result<
    (
        cdk::Bolt11Invoice,
        FederationBarkReceivePrepared,
        cdk::federation::FederationOperationEnvelope,
    ),
    cdk::Error,
> {
    let federation = state
        .mint()
        .federation_config()
        .ok_or(cdk::Error::Internal)?;
    let execution_actor = federation.member_id();
    if !intent
        .secret_acknowledgements
        .iter()
        .any(|acknowledgement| acknowledgement.member == execution_actor)
        || unix_time()? >= intent.expires_at
    {
        return Err(bark_unavailable());
    }
    let runtime = state.bark_runtime().ok_or_else(bark_unavailable)?;
    runtime
        .receive_secret_store()
        .mark_intent_accepted(
            intent.action_id,
            crate::FederatedBarkReceiveSecretAcceptedRef {
                operation_id: intent_operation_id,
                operation_index: intent_operation_index,
            },
        )
        .map_err(|error| {
            error.into_federation_error(
                "persist accepted incoming intent",
                Some(intent_operation_id),
            )
        })?;
    let invoice_timer = super::latency::FederationLatencyTimer::start(
        super::latency::FederationLatencyCorrelation::Action,
        intent.action_id.to_string(),
        super::latency::FederationLatencyBackend::Bark,
        super::latency::FederationLatencyDirection::Incoming,
        super::latency::FederationLatencyPhase::BarkInvoicePreparation,
    );
    let prepared_result = runtime
        .prepare_receive_invoice(execution_actor, intent_operation_id, intent)
        .await;
    invoice_timer.finish(super::latency::FederationLatencyOutcome::from_success(
        prepared_result.is_ok(),
    ));
    let prepared = prepared_result.map_err(|error| {
        error.into_federation_error("prepare incoming invoice", Some(intent_operation_id))
    })?;
    let envelope = federation.operation_envelope(super::bark_federation_operation(
        FederationBarkOperation::ReceivePrepared(prepared.clone()),
    )?);
    let invoice = prepared
        .invoice
        .parse()
        .map_err(|_| cdk::Error::InvalidPaymentRequest)?;
    Ok((invoice, prepared, envelope))
}

async fn apply_accepted_receive_prepared(
    state: &dyn FederatedBarkNode,
    intent: &FederationBarkReceiveIntent,
    prepared: &FederationBarkReceivePrepared,
) {
    let Some(runtime) = state.bark_runtime() else {
        return;
    };
    if let Err(error) = runtime.apply_receive_prepared(intent, prepared).await {
        runtime
            .mark_degraded(crate::FederatedBarkDegradedReason::ProjectionInvalid)
            .await;
        tracing::error!(
            action_id = %intent.action_id,
            error = %error,
            "accepted Bark receive plan could not be materialized locally"
        );
    }
}

fn intent_from_binding(
    binding: &FederationBarkReceiveSecretBinding,
    description: Option<String>,
    secret_acknowledgements: Vec<cdk::federation::FederationBarkReceiveSecretAcknowledgement>,
) -> FederationBarkReceiveIntent {
    FederationBarkReceiveIntent {
        version: FEDERATION_BARK_OPERATION_VERSION,
        action_id: binding.action_id,
        quote: binding.quote.clone(),
        actor: binding.actor,
        cashu_amount: binding.cashu_amount,
        gross_amount: binding.gross_amount,
        receive_fee: binding.receive_fee,
        minimum_net_amount: binding.minimum_net_amount,
        unit: binding.unit,
        description_hash: binding.description_hash,
        description,
        payment_hash: binding.payment_hash,
        preimage_commitment: binding.preimage_commitment,
        secret_acknowledgements,
        destination: binding.destination,
        mailbox_scope: binding.mailbox_scope,
        mailbox_id: binding.mailbox_id,
        ark_server: binding.ark_server,
        network: binding.network,
        minimum_cltv_delta: binding.minimum_cltv_delta,
        fee_policy_version: binding.fee_policy_version,
        anti_dos_policy_version: binding.anti_dos_policy_version,
        receive_proof: binding.receive_proof,
        expires_at: binding.expires_at,
    }
}

fn validate_binding_against_request(
    member_id: cdk::federation::MemberId,
    request: &cdk::nuts::MintQuoteBolt11Request,
    binding: &FederationBarkReceiveSecretBinding,
) -> Result<(), cdk::Error> {
    if binding.actor != member_id
        || binding.cashu_amount != request.amount
        || binding.gross_amount != request.amount
        || binding.description_hash
            != federation_bark_description_hash(request.description.as_deref())
                .map_err(cdk::Error::from)?
    {
        return Err(cdk::Error::InvalidPaymentRequest);
    }
    Ok(())
}

fn validate_request_against_intent(
    request: &cdk::nuts::MintQuoteBolt11Request,
    intent: &FederationBarkReceiveIntent,
) -> Result<(), cdk::Error> {
    if intent.cashu_amount != request.amount
        || intent.unit != FederationBarkUnit::Sat
        || intent.description != request.description
    {
        return Err(cdk::Error::InvalidPaymentRequest);
    }
    Ok(())
}

fn validate_request_against_prepared(
    request: &cdk::nuts::MintQuoteBolt11Request,
    prepared: &FederationBarkReceivePrepared,
) -> Result<(), cdk::Error> {
    let invoice = prepared
        .invoice
        .parse::<cdk::Bolt11Invoice>()
        .map_err(|_| cdk::Error::InvalidPaymentRequest)?;
    let expected_msat = u64::from(request.amount)
        .checked_mul(1_000)
        .ok_or(cdk::Error::InvalidPaymentRequest)?;
    if prepared.gross_amount != request.amount
        || invoice.amount_milli_satoshis() != Some(expected_msat)
    {
        return Err(cdk::Error::InvalidPaymentRequest);
    }
    Ok(())
}

fn unix_time() -> Result<u64, cdk::Error> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .map_err(|_| bark_unavailable())
}

fn bark_unavailable() -> cdk::Error {
    cdk::Error::Custom("federated Bark incoming backend is unavailable".to_string())
}

#[cfg(test)]
mod tests {
    use cdk::mint::MintQuoteRequest;
    use cdk::nuts::{CurrencyUnit, MintQuoteBolt11Request, PublicKey};
    use cdk::{Amount, Error};

    use super::{
        advance_with_unlocked_backend_wait, bark_evidence_collector, bark_evidence_retry_collector,
        federated_bark_quote_id, receive_is_automatically_terminal,
        scheduled_bark_evidence_collector_round, FederationId, FederationPaymentAdvancement,
        ReceiveBackendWait, FEDERATION_BARK_RECEIVE_EXPIRY_GRACE_SECONDS,
    };

    fn request(pubkey: Option<PublicKey>, amount: u64) -> MintQuoteRequest {
        MintQuoteRequest::Bolt11(MintQuoteBolt11Request {
            amount: Amount::from(amount),
            unit: CurrencyUnit::Sat,
            description: Some("stable retry".to_string()),
            pubkey,
        })
    }

    #[test]
    fn federated_bark_quote_id_is_stable_and_scoped_to_requester() {
        let first_key = PublicKey::from_hex(
            "0279be667ef9dcbbac55a06295ce870b07029bfcdb2dce28d959f2815b16f81798",
        )
        .expect("generator public key");
        let second_key = PublicKey::from_hex(
            "0379be667ef9dcbbac55a06295ce870b07029bfcdb2dce28d959f2815b16f81798",
        )
        .expect("second public key");
        let federation = FederationId::from_bytes([7; 32]);
        let quote = federated_bark_quote_id(federation, &request(Some(first_key), 42))
            .expect("valid quote key");

        assert_eq!(
            quote,
            federated_bark_quote_id(federation, &request(Some(first_key), 42))
                .expect("identical retry")
        );
        assert_eq!(
            quote,
            federated_bark_quote_id(federation, &request(Some(first_key), 43))
                .expect("the key, not mutable request fields, owns the retry identity")
        );
        assert_ne!(
            quote,
            federated_bark_quote_id(federation, &request(Some(second_key), 42))
                .expect("different requester")
        );
        assert_ne!(
            quote,
            federated_bark_quote_id(
                FederationId::from_bytes([8; 32]),
                &request(Some(first_key), 42)
            )
            .expect("different federation")
        );
    }

    #[test]
    fn federated_bark_quote_id_requires_nut19_key() {
        assert!(matches!(
            federated_bark_quote_id(FederationId::from_bytes([7; 32]), &request(None, 42)),
            Err(Error::PubkeyRequired)
        ));
    }

    #[test]
    fn evidence_collector_rotates_once_per_bounded_fallback_round() {
        let members = || (1..=5).map(|id| cdk::federation::MemberId::new(id).expect("member"));
        let actor = cdk::federation::MemberId::new(4).expect("actor");
        let selected = |round| {
            bark_evidence_collector(members(), actor, round)
                .expect("collector")
                .get()
        };

        assert_eq!(selected(0), 4);
        assert_eq!(selected(1), 5);
        assert_eq!(selected(2), 1);
        assert_eq!(selected(4), 3);
        assert_eq!(selected(u32::MAX), 3);
    }

    #[test]
    fn failed_primary_delivery_skips_to_the_first_standby_observer() {
        let members = || (1..=5).map(|id| cdk::federation::MemberId::new(id).expect("member"));
        let actor = cdk::federation::MemberId::new(4).expect("actor");
        let retry = |round, required| {
            bark_evidence_retry_collector(members(), actor, round, required)
                .map(cdk::federation::MemberId::get)
        };

        assert_eq!(retry(0, 3), Some(2));
        assert_eq!(retry(3, 3), Some(3));
        assert_eq!(retry(4, 3), None);
        assert_eq!(retry(0, 4), Some(3));
    }

    #[test]
    fn scheduled_fallback_starts_after_the_required_observer_set() {
        assert_eq!(scheduled_bark_evidence_collector_round(0, 3), 0);
        assert_eq!(scheduled_bark_evidence_collector_round(1, 3), 3);
        assert_eq!(scheduled_bark_evidence_collector_round(2, 3), 4);
        assert_eq!(scheduled_bark_evidence_collector_round(1, 4), 4);
    }

    #[test]
    fn expired_receive_stops_only_after_grace_without_accepted_evidence() {
        let expires_at = 1_000;
        let terminal_at = expires_at + FEDERATION_BARK_RECEIVE_EXPIRY_GRACE_SECONDS;

        assert!(!receive_is_automatically_terminal(
            expires_at,
            false,
            terminal_at - 1,
        ));
        assert!(receive_is_automatically_terminal(
            expires_at,
            false,
            terminal_at,
        ));
        assert!(!receive_is_automatically_terminal(
            expires_at,
            true,
            terminal_at,
        ));
    }

    /// Every unpaid invoice is polled with a multi-second Ark wait. Quote
    /// creation and every other incoming quote need the same member-wide
    /// lock, so the wait must run while that lock is free.
    #[tokio::test]
    async fn backend_wait_runs_without_the_incoming_lock() {
        let lock = tokio::sync::Mutex::new(());
        let passes = std::sync::Mutex::new(Vec::new());

        let advancement = advance_with_unlocked_backend_wait(
            &lock,
            true,
            |backend_wait| {
                passes
                    .lock()
                    .expect("pass log")
                    .push((backend_wait, lock.try_lock().is_err()));
                async move {
                    Ok(match backend_wait {
                        ReceiveBackendWait::Defer => (
                            FederationPaymentAdvancement::AwaitingBackendNotification,
                            Some("receive"),
                        ),
                        ReceiveBackendWait::Waited => (
                            FederationPaymentAdvancement::AwaitingBackendNotification,
                            None,
                        ),
                        ReceiveBackendWait::Poll => {
                            panic!("a waiting advancement never polls first")
                        }
                    })
                }
            },
            |target| {
                assert_eq!(target, "receive");
                let lock_free = lock.try_lock().is_ok();
                async move {
                    assert!(lock_free, "the Ark wait must not hold the incoming lock");
                    Ok(())
                }
            },
        )
        .await
        .expect("advancement");

        assert_eq!(
            advancement,
            FederationPaymentAdvancement::AwaitingBackendNotification
        );
        assert_eq!(
            *passes.lock().expect("pass log"),
            vec![
                (ReceiveBackendWait::Defer, true),
                (ReceiveBackendWait::Waited, true),
            ],
            "both passes run under the lock, the second after the wait"
        );
    }

    #[tokio::test]
    async fn advancement_without_a_receive_to_wait_on_returns_after_one_pass() {
        let lock = tokio::sync::Mutex::new(());
        let passes = std::sync::Mutex::new(Vec::new());

        for (wait_for_backend_update, expected_mode) in [
            (true, ReceiveBackendWait::Defer),
            (false, ReceiveBackendWait::Poll),
        ] {
            let advancement =
                advance_with_unlocked_backend_wait(
                    &lock,
                    wait_for_backend_update,
                    |backend_wait| {
                        passes.lock().expect("pass log").push(backend_wait);
                        async move {
                            Ok::<_, Error>((FederationPaymentAdvancement::Progressed, None::<()>))
                        }
                    },
                    |()| async move { panic!("nothing to wait on") },
                )
                .await
                .expect("advancement");
            assert_eq!(advancement, FederationPaymentAdvancement::Progressed);
            assert_eq!(passes.lock().expect("pass log").pop(), Some(expected_mode));
        }
        assert!(passes.lock().expect("pass log").is_empty());
    }
}
