//! Consensus-first orchestration for federated Bark incoming BOLT11 quotes.

use std::collections::BTreeSet;
use std::time::{SystemTime, UNIX_EPOCH};

use cdk::federation::{
    ark_server_commitment, federation_bark_description_hash, federation_bark_mailbox_scope,
    payment_hash_for_preimage, preimage_commitment, FederationBarkActionId,
    FederationBarkMemberObservation, FederationBarkObservationStatus, FederationBarkOperation,
    FederationBarkReceiveIntent, FederationBarkReceivePrepared, FederationBarkReceiveProof,
    FederationBarkReceiveSecretBinding, FederationBarkSemanticPhase, FederationBarkUnit,
    FederationBarkVtxoLocator, FederationId, FederationOperation, FederationOperationId,
    FederationPaymentObservationContext, MemberConfigPublic, FEDERATION_BARK_OPERATION_VERSION,
    FEDERATION_MAX_DERIVATION_INDEX,
};
use cdk::mint::{MintQuoteRequest, MintQuoteResponse, QuoteId};
use cdk::nuts::CurrencyUnit;
use cdk::Amount;
use cdk_frost::rand_core::{OsRng, RngCore};
use cdk_http_client::Async;
use sha2::{Digest, Sha256};

use crate::MintState;

use super::bark_observer::{
    is_selected_bark_observer, required_bark_result_observers, unselected_bark_observer_wait,
};
use super::bark_secret::FederationBarkReceiveSecretCoordinator;
use super::payment_schedule::FederationPaymentAdvancement;
use super::peer_client::FederationRestPeerClient;

const FEDERATION_BARK_RECEIVE_INTENT_LIFETIME_SECONDS: u64 = 7 * 24 * 60 * 60;
const FEDERATION_BARK_QUOTE_ID_DOMAIN: &[u8] = b"cdk-federation-bark-quote-id-v1";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FederatedBarkReceiveObservationStep<'a> {
    PollArk,
    WaitForQuorum,
    AdvanceFromCache(&'a FederationBarkMemberObservation),
}

fn matching_receive_observers(
    projection: &cdk::federation::FederationBarkProjectionSnapshot,
    expected: &FederationBarkMemberObservation,
) -> BTreeSet<cdk::federation::MemberId> {
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

fn receive_observation_step<'a>(
    projection: &'a cdk::federation::FederationBarkProjectionSnapshot,
    action_id: FederationBarkActionId,
    member_id: cdk::federation::MemberId,
    is_receive_actor: bool,
    threshold: usize,
) -> FederatedBarkReceiveObservationStep<'a> {
    let accepted = projection
        .observations
        .iter()
        .filter_map(|record| match &record.operation {
            FederationBarkOperation::MemberObservation(observation)
                if observation.action_id == action_id
                    && observation.observer == member_id
                    && observation.status == FederationBarkObservationStatus::Accepted =>
            {
                Some((record.operation_index, observation))
            }
            _ => None,
        })
        .min_by_key(|(operation_index, _)| *operation_index)
        .map(|(_, observation)| observation);
    if is_receive_actor {
        let quorum_observation = accepted
            .filter(|observation| {
                matching_receive_observers(projection, observation).len() >= threshold
            })
            .or_else(|| {
                projection
                    .observations
                    .iter()
                    .filter_map(|record| match &record.operation {
                        FederationBarkOperation::MemberObservation(observation)
                            if observation.action_id == action_id
                                && observation.status
                                    == FederationBarkObservationStatus::Accepted
                                && matching_receive_observers(projection, observation).len()
                                    >= threshold =>
                        {
                            Some((record.operation_index, observation))
                        }
                        _ => None,
                    })
                    .min_by_key(|(operation_index, _)| *operation_index)
                    .map(|(_, observation)| observation)
            });
        if let Some(observation) = quorum_observation {
            return FederatedBarkReceiveObservationStep::AdvanceFromCache(observation);
        }
    }
    if accepted.is_some()
        || projection.observations.iter().any(|record| {
            matches!(
                &record.operation,
                FederationBarkOperation::MemberObservation(observation)
                    if observation.action_id == action_id
                        && observation.status == FederationBarkObservationStatus::Accepted
                        && matching_receive_observers(projection, observation).len() >= threshold
            )
        })
    {
        FederatedBarkReceiveObservationStep::WaitForQuorum
    } else {
        FederatedBarkReceiveObservationStep::PollArk
    }
}

/// Poll and advance one already-created federated Bark quote. Mailbox events
/// call the same function; they never supply settlement truth themselves.
pub(super) async fn advance_federated_bark_mint_quote(
    state: &MintState,
    quote_id: &QuoteId,
    observer_fallback_round: u32,
) -> Result<FederationPaymentAdvancement, cdk::Error> {
    let _guard = state.federation.bark_incoming_lock().lock().await;
    let federation = state.mint.federation_config().ok_or(cdk::Error::Internal)?;
    let runtime = state
        .federation
        .bark_runtime()
        .ok_or_else(bark_unavailable)?;
    let quote = quote_id.to_string();
    let projection = crate::federation_bark_ready_projection(&state.mint, runtime)
        .await
        .map_err(|_| bark_unavailable())?;
    let Some(action) = projection
        .actions
        .iter()
        .find(|action| action.quote.as_deref() == Some(quote.as_str()))
        .cloned()
    else {
        return Ok(FederationPaymentAdvancement::AwaitingFederationEvidence);
    };
    let intent = state
        .mint
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
    if action.phase != FederationBarkSemanticPhase::ReceiveClaimed {
        runtime
            .apply_receive_prepared(&intent, prepared)
            .await
            .map_err(|_| bark_unavailable())?;
    }

    match action.phase {
        FederationBarkSemanticPhase::ReceivePrepared => {
            let receive_actor = state
                .mint
                .federation_bark_receive_actor(action.origin_operation_id)
                .await?;
            let is_receive_actor = receive_actor == federation.member_id();
            let threshold = usize::from(
                federation
                    .public_config()
                    .threshold_params
                    .signature_threshold,
            );
            match receive_observation_step(
                &projection,
                action.action_id,
                federation.member_id(),
                is_receive_actor,
                threshold,
            ) {
                FederatedBarkReceiveObservationStep::WaitForQuorum => {
                    return Ok(FederationPaymentAdvancement::AwaitingFederationEvidence);
                }
                FederatedBarkReceiveObservationStep::AdvanceFromCache(observation) => {
                    let transition = runtime
                        .receive_htlc_transition_from_accepted_observation(
                            &intent,
                            prepared_record.operation_id,
                            prepared,
                            observation,
                        )
                        .await
                        .map_err(|_| bark_unavailable())?;
                    let envelope = federation.operation_envelope(FederationOperation::Bark(
                        FederationBarkOperation::ActionTransition(transition),
                    ));
                    state
                        .submit_federation_envelope(envelope)
                        .await?
                        .ensure_accepted()?;
                    return Ok(FederationPaymentAdvancement::Progressed);
                }
                FederatedBarkReceiveObservationStep::PollArk => {}
            }
            let accepted_evidence_present = projection.observations.iter().any(|record| {
                matches!(
                    &record.operation,
                    FederationBarkOperation::MemberObservation(observation)
                        if observation.action_id == action.action_id
                            && observation.status == FederationBarkObservationStatus::Accepted
                )
            });
            if !is_selected_bark_observer(
                federation.public_config().members.keys().copied(),
                receive_actor,
                federation.member_id(),
                threshold,
                observer_fallback_round,
            ) {
                return Ok(unselected_bark_observer_wait(accepted_evidence_present));
            }
            let next_epoch = projection
                .observations
                .iter()
                .filter_map(|record| match &record.operation {
                    FederationBarkOperation::MemberObservation(observation)
                        if observation.action_id == action.action_id
                            && observation.observer == federation.member_id() =>
                    {
                        Some(observation.observation_epoch)
                    }
                    _ => None,
                })
                .max()
                .unwrap_or(0)
                .checked_add(1)
                .ok_or(cdk::Error::Internal)?;
            let Some((observation, transition)) = runtime
                .observe_receive_htlcs(
                    &intent,
                    prepared_record.operation_id,
                    prepared,
                    next_epoch,
                    is_receive_actor,
                )
                .await
                .map_err(|_| bark_unavailable())?
            else {
                return Ok(FederationPaymentAdvancement::AwaitingExternalEvidence);
            };
            let already_observed = projection.observations.iter().any(|record| {
                matches!(
                    &record.operation,
                    FederationBarkOperation::MemberObservation(existing)
                        if existing.action_id == observation.action_id
                            && existing.observer == observation.observer
                            && existing.status == observation.status
                            && existing.result_hash == observation.result_hash
                            && existing.amount == observation.amount
                            && existing.fee == observation.fee
                )
            });
            let mut submitted = 0_usize;
            if !already_observed {
                let envelope = federation.operation_envelope(FederationOperation::Bark(
                    FederationBarkOperation::MemberObservation(observation.clone()),
                ));
                state
                    .submit_federation_envelope(envelope)
                    .await?
                    .ensure_accepted()?;
                submitted = submitted.saturating_add(1);
            }
            let refreshed = state.mint.federation_bark_projection_snapshot().await?;
            let matching_observers = matching_receive_observers(&refreshed, &observation);
            if is_receive_actor && matching_observers.len() >= threshold {
                let envelope = federation.operation_envelope(FederationOperation::Bark(
                    FederationBarkOperation::ActionTransition(transition),
                ));
                state
                    .submit_federation_envelope(envelope)
                    .await?
                    .ensure_accepted()?;
                submitted = submitted.saturating_add(1);
            }
            Ok(if submitted > 0 {
                FederationPaymentAdvancement::Progressed
            } else {
                FederationPaymentAdvancement::AwaitingFederationEvidence
            })
        }
        FederationBarkSemanticPhase::ReceiveHtlcsReady => {
            if action.actor != federation.member_id() {
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
            let next_index = projection
                .derivation_allocations
                .iter()
                .map(|allocation| allocation.locator.index)
                .max()
                .unwrap_or(0)
                .checked_add(1)
                .ok_or(cdk::Error::Internal)?;
            if next_index > FEDERATION_MAX_DERIVATION_INDEX {
                return Err(bark_unavailable());
            }
            let plan = runtime
                .propose_receive_claim(
                    &intent,
                    prepared,
                    action.latest_operation_id,
                    cdk_bark::FederatedBarkReceiveClaimProposal {
                        actor: federation.member_id(),
                        signer_set,
                        attempt: 1,
                        output_locator: FederationBarkVtxoLocator::vtxo_custody(next_index),
                    },
                )
                .await
                .map_err(|_| bark_unavailable())?;
            let envelope = federation.operation_envelope(FederationOperation::Bark(
                FederationBarkOperation::ActionPlan(plan),
            ));
            state
                .submit_federation_envelope(envelope)
                .await?
                .ensure_accepted()?;
            Ok(FederationPaymentAdvancement::Progressed)
        }
        FederationBarkSemanticPhase::ClaimPlanned
        | FederationBarkSemanticPhase::ReceivePreimageDisclosed => {
            let plan_operation_id = match &action.operation {
                FederationBarkOperation::ActionPlan(_) => action.latest_operation_id,
                FederationBarkOperation::ActionTransition(transition) => {
                    transition.plan_operation_id
                }
                _ => return Err(cdk::Error::Internal),
            };
            let plan_entry = state
                .mint
                .localstore()
                .get_federation_journal_entry_by_operation_id(plan_operation_id)
                .await?
                .ok_or(cdk::Error::Internal)?;
            let FederationOperation::Bark(FederationBarkOperation::ActionPlan(plan)) =
                &plan_entry.envelope.operation
            else {
                return Err(cdk::Error::Internal);
            };
            if plan.actor != federation.member_id() {
                return Ok(FederationPaymentAdvancement::AwaitingFederationEvidence);
            }
            let outcome = runtime
                .execute_receive_claim(&intent, prepared, plan_entry)
                .await
                .map_err(|_| bark_unavailable())?;
            match outcome {
                cdk_bark::FederatedBarkReceiveClaimOutcome::RecoveryRequired {
                    preimage_disclosed,
                } => {
                    if action.phase == FederationBarkSemanticPhase::ClaimPlanned {
                        let envelope = federation.operation_envelope(FederationOperation::Bark(
                            FederationBarkOperation::ActionTransition(preimage_disclosed),
                        ));
                        state
                            .submit_federation_envelope(envelope)
                            .await?
                            .ensure_accepted()?;
                        Ok(FederationPaymentAdvancement::Progressed)
                    } else {
                        Ok(FederationPaymentAdvancement::AwaitingExternalEvidence)
                    }
                }
                cdk_bark::FederatedBarkReceiveClaimOutcome::Claimed {
                    preimage_disclosed,
                    result,
                } => {
                    let accepted_preimage = if action.phase
                        == FederationBarkSemanticPhase::ClaimPlanned
                    {
                        let envelope = federation.operation_envelope(FederationOperation::Bark(
                            FederationBarkOperation::ActionTransition(preimage_disclosed),
                        ));
                        let response = state.submit_federation_envelope(envelope).await?;
                        response.ensure_accepted()?;
                        response.outcome.entry().clone()
                    } else {
                        state
                            .mint
                            .localstore()
                            .get_federation_journal_entry_by_operation_id(
                                action.latest_operation_id,
                            )
                            .await?
                            .ok_or(cdk::Error::Internal)?
                    };
                    let claimed = runtime
                        .finalize_receive_claim(&accepted_preimage, *result)
                        .map_err(|_| bark_unavailable())?;
                    let envelope = federation.operation_envelope(FederationOperation::Bark(
                        FederationBarkOperation::ActionTransition(claimed),
                    ));
                    state
                        .submit_federation_envelope(envelope)
                        .await?
                        .ensure_accepted()?;
                    runtime
                        .acknowledge_receive_claim(&intent, prepared)
                        .await
                        .map_err(|_| bark_unavailable())?;
                    Ok(FederationPaymentAdvancement::Progressed)
                }
            }
        }
        FederationBarkSemanticPhase::ReceiveClaimed => {
            let FederationBarkOperation::ActionTransition(claimed) = &action.operation else {
                return Err(cdk::Error::Internal);
            };
            let already_observed = projection.observations.iter().any(|record| {
                matches!(
                    &record.operation,
                    FederationBarkOperation::MemberObservation(existing)
                        if existing.action_id == action.action_id
                            && existing.observer == federation.member_id()
                            && existing.status == FederationBarkObservationStatus::Succeeded
                            && existing.result_hash == claimed.result_hash
                )
            });
            if already_observed {
                if action.actor == federation.member_id() {
                    runtime
                        .acknowledge_receive_claim(&intent, prepared)
                        .await
                        .map_err(|_| bark_unavailable())?;
                }
                return Ok(FederationPaymentAdvancement::AwaitingFederationEvidence);
            }
            let amount = runtime
                .validate_receive_claim_result(claimed)
                .await
                .map_err(|_| bark_unavailable())?;
            let next_epoch = projection
                .observations
                .iter()
                .filter_map(|record| match &record.operation {
                    FederationBarkOperation::MemberObservation(observation)
                        if observation.action_id == action.action_id
                            && observation.observer == federation.member_id() =>
                    {
                        Some(observation.observation_epoch)
                    }
                    _ => None,
                })
                .max()
                .unwrap_or(0)
                .checked_add(1)
                .ok_or(cdk::Error::Internal)?;
            let observation = FederationBarkMemberObservation {
                version: FEDERATION_BARK_OPERATION_VERSION,
                action_id: action.action_id,
                quote: Some(intent.quote.clone()),
                observer: federation.member_id(),
                observation_epoch: next_epoch,
                source_scope: intent.ark_server,
                payment_hash: Some(intent.payment_hash),
                status: FederationBarkObservationStatus::Succeeded,
                amount,
                fee: Amount::ZERO,
                result_hash: claimed.result_hash,
            };
            let public_config = federation.public_config();
            let required = required_bark_result_observers(
                public_config.threshold_params.signature_threshold,
                public_config
                    .payment_verification_policy(FederationPaymentObservationContext::MintQuote)
                    .required_matching_observations(),
            );
            let matching_observers = matching_receive_observers(&projection, &observation).len();
            if matching_observers >= required {
                return Ok(FederationPaymentAdvancement::AwaitingFederationEvidence);
            }
            if !is_selected_bark_observer(
                public_config.members.keys().copied(),
                action.actor,
                federation.member_id(),
                required,
                observer_fallback_round,
            ) {
                return Ok(unselected_bark_observer_wait(matching_observers > 0));
            }
            let envelope = federation.operation_envelope(FederationOperation::Bark(
                FederationBarkOperation::MemberObservation(observation),
            ));
            state
                .submit_federation_envelope(envelope)
                .await?
                .ensure_accepted()?;
            if action.actor == federation.member_id() {
                runtime
                    .acknowledge_receive_claim(&intent, prepared)
                    .await
                    .map_err(|_| bark_unavailable())?;
            }
            Ok(FederationPaymentAdvancement::Progressed)
        }
        _ => Ok(FederationPaymentAdvancement::Complete),
    }
}

/// Retry invoice preparation for one exact accepted receive intent.
pub(crate) async fn advance_federated_bark_receive_intent(
    state: &MintState,
    action_id: FederationBarkActionId,
) -> Result<usize, cdk::Error> {
    let _guard = state.federation.bark_incoming_lock().lock().await;
    let runtime = state
        .federation
        .bark_runtime()
        .ok_or_else(bark_unavailable)?;
    let projection = crate::federation_bark_ready_projection(&state.mint, runtime)
        .await
        .map_err(|_| bark_unavailable())?;
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
pub(crate) fn federated_bark_quote_id(
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
pub(crate) async fn prepare_federated_bark_mint_quote(
    state: &MintState,
    quote_id: QuoteId,
    request: MintQuoteRequest,
) -> Result<
    (
        MintQuoteResponse<QuoteId>,
        cdk::cdk_payment::PaymentIdentifier,
    ),
    cdk::Error,
> {
    let _guard = state.federation.bark_incoming_lock().lock().await;
    if let Some(quote) = state.mint.localstore().get_mint_quote(&quote_id).await? {
        let request_lookup_id = quote.request_lookup_id.clone();
        return Ok((quote.try_into()?, request_lookup_id));
    }
    let invoice = accepted_invoice(state, &quote_id, &request).await?;
    state
        .mint
        .prepare_bolt11_mint_quote_with_invoice(quote_id, request, invoice)
        .await
}

async fn accepted_invoice(
    state: &MintState,
    quote_id: &QuoteId,
    request: &MintQuoteRequest,
) -> Result<cdk::Bolt11Invoice, cdk::Error> {
    let MintQuoteRequest::Bolt11(bolt11) = request else {
        return Err(cdk::Error::InvalidPaymentRequest);
    };
    if bolt11.unit != CurrencyUnit::Sat
        || bolt11.amount == Amount::ZERO
        || federation_bark_description_hash(bolt11.description.as_deref()).is_err()
    {
        return Err(cdk::Error::InvalidPaymentRequest);
    }
    let federation = state.mint.federation_config().ok_or(cdk::Error::Internal)?;
    let runtime = state
        .federation
        .bark_runtime()
        .ok_or_else(bark_unavailable)?;
    let limits = runtime.limits();
    let amount = u64::from(bolt11.amount);
    if amount < limits.minimum_sat() || amount > limits.maximum_sat() {
        return Err(cdk::Error::InvalidPaymentRequest);
    }
    let projection = crate::federation_bark_ready_projection(&state.mint, runtime)
        .await
        .map_err(|_| bark_unavailable())?;
    let quote = quote_id.to_string();
    let action = projection
        .actions
        .iter()
        .find(|action| action.quote.as_deref() == Some(quote.as_str()));

    if let Some(action) = action {
        return match &action.operation {
            FederationBarkOperation::ReceivePrepared(prepared) => {
                let intent = state
                    .mint
                    .federation_bark_receive_intent(prepared.intent_operation_id)
                    .await?
                    .ok_or(cdk::Error::InvalidPaymentRequest)?;
                validate_request_against_intent(bolt11, &intent)?;
                prepared
                    .validate_against_intent(&intent)
                    .map_err(cdk::Error::from)?;
                validate_request_against_prepared(bolt11, prepared)?;
                prepared
                    .invoice
                    .parse()
                    .map_err(|_| cdk::Error::InvalidPaymentRequest)
            }
            FederationBarkOperation::ReceiveIntent(intent) => {
                validate_request_against_intent(bolt11, intent)?;
                prepare_and_accept_invoice(
                    state,
                    action.latest_operation_id,
                    action.latest_operation_index,
                    intent,
                )
                .await
            }
            FederationBarkOperation::ActionPlan(_)
            | FederationBarkOperation::ActionTransition(_)
            | FederationBarkOperation::MemberObservation(_) => {
                Err(cdk::Error::InvalidPaymentRequest)
            }
        };
    }

    let store = runtime.receive_secret_store();
    let resumable = store
        .find_by_quote(federation.federation_id(), &quote)
        .map_err(|_| bark_unavailable())?;
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
        .map_err(|_| bark_unavailable())?;
    let remote_members = federation
        .public_config()
        .members
        .values()
        .filter(|member| member.member_id != federation.member_id())
        .cloned()
        .collect::<Vec<MemberConfigPublic>>();
    let peer_client = FederationRestPeerClient::new_with_validated_config(
        Async::default(),
        federation.public_config(),
        federation.private_config().clone(),
        state
            .federation
            .peer_sync_config
            .ok_or_else(bark_unavailable)?,
    )
    .map_err(|_| bark_unavailable())?;
    intent.secret_acknowledgements = FederationBarkReceiveSecretCoordinator::new(&peer_client)
        .distribute(
            federation.public_config(),
            &binding,
            &secret,
            &remote_members,
            vec![local_acknowledgement],
        )
        .await
        .map_err(|_| bark_unavailable())?;
    intent
        .validate_secret_acknowledgements(federation.public_config())
        .map_err(cdk::Error::from)?;
    let envelope = federation.operation_envelope(FederationOperation::Bark(
        FederationBarkOperation::ReceiveIntent(intent.clone()),
    ));
    let accepted = state.submit_federation_envelope(envelope).await?;
    let entry = accepted.outcome.entry();
    store
        .mark_intent_accepted(
            intent.action_id,
            cdk_bark::FederatedBarkReceiveSecretAcceptedRef {
                operation_id: entry.operation_id,
                operation_index: entry.index,
            },
        )
        .map_err(|_| bark_unavailable())?;
    prepare_and_accept_invoice(state, entry.operation_id, entry.index, &intent).await
}

async fn fresh_intent(
    state: &MintState,
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
    let federation = state.mint.federation_config().ok_or(cdk::Error::Internal)?;
    let runtime = state
        .federation
        .bark_runtime()
        .ok_or_else(bark_unavailable)?;
    let application = federation
        .public_config()
        .frost_wallet
        .as_ref()
        .and_then(|wallet| wallet.bark_application.as_ref())
        .ok_or_else(bark_unavailable)?;
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
        .map_err(|_| bark_unavailable())?
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
        minimum_cltv_delta: application.policy.minimum_incoming_cltv_delta,
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
    state: &MintState,
    intent_operation_id: FederationOperationId,
    intent_operation_index: u64,
    intent: &FederationBarkReceiveIntent,
) -> Result<cdk::Bolt11Invoice, cdk::Error> {
    let federation = state.mint.federation_config().ok_or(cdk::Error::Internal)?;
    let execution_actor = federation.member_id();
    if !intent
        .secret_acknowledgements
        .iter()
        .any(|acknowledgement| acknowledgement.member == execution_actor)
        || unix_time()? >= intent.expires_at
    {
        return Err(bark_unavailable());
    }
    let runtime = state
        .federation
        .bark_runtime()
        .ok_or_else(bark_unavailable)?;
    runtime
        .receive_secret_store()
        .mark_intent_accepted(
            intent.action_id,
            cdk_bark::FederatedBarkReceiveSecretAcceptedRef {
                operation_id: intent_operation_id,
                operation_index: intent_operation_index,
            },
        )
        .map_err(|_| bark_unavailable())?;
    let prepared = runtime
        .prepare_receive_invoice(execution_actor, intent_operation_id, intent)
        .await
        .map_err(|_| bark_unavailable())?;
    let envelope = federation.operation_envelope(FederationOperation::Bark(
        FederationBarkOperation::ReceivePrepared(prepared.clone()),
    ));
    state.submit_federation_envelope(envelope).await?;
    if runtime
        .apply_receive_prepared(intent, &prepared)
        .await
        .is_err()
    {
        runtime
            .mark_degraded(cdk_bark::FederatedBarkDegradedReason::ProjectionInvalid)
            .await;
        tracing::error!(
            action_id = %intent.action_id,
            "accepted Bark receive plan could not be materialized locally"
        );
    }
    prepared
        .invoice
        .parse()
        .map_err(|_| cdk::Error::InvalidPaymentRequest)
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
    use cdk::federation::{
        FederationBarkDigest, FederationBarkMemberObservation, FederationBarkObservationRecord,
        FederationBarkObservationStatus, FederationBarkOperation, FederationBarkProjectionSnapshot,
        FederationOperationId, MemberId, FEDERATION_BARK_OPERATION_VERSION,
    };
    use cdk::mint::MintQuoteRequest;
    use cdk::nuts::{CurrencyUnit, MintQuoteBolt11Request, PublicKey};
    use cdk::{Amount, Error};

    use super::{
        federated_bark_quote_id, receive_observation_step, FederatedBarkReceiveObservationStep,
        FederationBarkActionId, FederationId,
    };

    fn request(pubkey: Option<PublicKey>, amount: u64) -> MintQuoteRequest {
        MintQuoteRequest::Bolt11(MintQuoteBolt11Request {
            amount: Amount::from(amount),
            unit: CurrencyUnit::Sat,
            description: Some("stable retry".to_string()),
            pubkey,
        })
    }

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
                    status: FederationBarkObservationStatus::Accepted,
                    amount: Amount::from(10_000),
                    fee: Amount::ZERO,
                    result_hash: FederationBarkDigest::from_bytes([result_seed; 32])
                        .expect("result hash"),
                },
            ),
        }
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
    fn receive_observer_without_accepted_evidence_polls_ark() {
        let action_id = FederationBarkActionId::from_bytes([1; 32]).expect("action ID");
        let projection = FederationBarkProjectionSnapshot {
            observations: vec![observation(action_id, member(2), 1, 9)],
            ..FederationBarkProjectionSnapshot::default()
        };

        assert_eq!(
            receive_observation_step(&projection, action_id, member(1), false, 3),
            FederatedBarkReceiveObservationStep::PollArk,
        );
    }

    #[test]
    fn accepted_receive_observer_waits_without_polling_ark() {
        let action_id = FederationBarkActionId::from_bytes([1; 32]).expect("action ID");
        let projection = FederationBarkProjectionSnapshot {
            observations: vec![observation(action_id, member(1), 1, 9)],
            ..FederationBarkProjectionSnapshot::default()
        };

        assert_eq!(
            receive_observation_step(&projection, action_id, member(1), false, 3),
            FederatedBarkReceiveObservationStep::WaitForQuorum,
        );
        assert_eq!(
            receive_observation_step(&projection, action_id, member(1), true, 3),
            FederatedBarkReceiveObservationStep::WaitForQuorum,
        );
    }

    #[test]
    fn receive_quorum_stops_members_without_their_own_evidence_from_polling() {
        let action_id = FederationBarkActionId::from_bytes([1; 32]).expect("action ID");
        let projection = FederationBarkProjectionSnapshot {
            observations: vec![
                observation(action_id, member(1), 1, 9),
                observation(action_id, member(2), 2, 9),
                observation(action_id, member(3), 3, 9),
            ],
            ..FederationBarkProjectionSnapshot::default()
        };

        assert_eq!(
            receive_observation_step(&projection, action_id, member(5), false, 3),
            FederatedBarkReceiveObservationStep::WaitForQuorum,
        );
        assert!(matches!(
            receive_observation_step(&projection, action_id, member(5), true, 3),
            FederatedBarkReceiveObservationStep::AdvanceFromCache(observation)
                if observation.observer == member(1)
        ));
    }

    #[test]
    fn receive_actor_advances_from_cache_only_after_matching_quorum() {
        let action_id = FederationBarkActionId::from_bytes([1; 32]).expect("action ID");
        let mut projection = FederationBarkProjectionSnapshot {
            observations: vec![
                observation(action_id, member(1), 1, 9),
                observation(action_id, member(2), 2, 9),
                observation(action_id, member(3), 3, 8),
            ],
            ..FederationBarkProjectionSnapshot::default()
        };

        assert_eq!(
            receive_observation_step(&projection, action_id, member(1), true, 3),
            FederatedBarkReceiveObservationStep::WaitForQuorum,
            "conflicting evidence must not complete the quorum",
        );

        projection
            .observations
            .push(observation(action_id, member(3), 4, 9));
        assert!(matches!(
            receive_observation_step(&projection, action_id, member(1), true, 3),
            FederatedBarkReceiveObservationStep::AdvanceFromCache(observation)
                if observation.observer == member(1)
        ));
    }
}
