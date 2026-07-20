//! Deterministic materialized-state projection for federation convergence.

use std::collections::{BTreeSet, HashMap};
use std::str::FromStr;

use bitcoin::hashes::{sha256, Hash};
use cdk_common::error::Error;
use cdk_common::federation::{
    ark_server_commitment, FederatedKeyset, FederationBarkActionKind, FederationBarkActionPlan,
    FederationBarkActionRecord, FederationBarkActionTransition, FederationBarkArtifactKind,
    FederationBarkDigest, FederationBarkObservationStatus, FederationBarkOperation,
    FederationBarkProjectionSnapshot, FederationBarkReceiveIntent, FederationBarkSemanticPhase,
    FederationBarkVtxoInput, FederationBarkVtxoLocator, FederationBarkVtxoState,
    FederationMaterializedStateKey, FederationMaterializedStateRow,
    FederationMaterializedStateSnapshot, FederationOperation, FederationOperationId,
    FederationOperationStatus, FederationPaymentObservationRecord, FEDERATION_MAX_DERIVATION_INDEX,
};
use cdk_common::mint::MeltPaymentRequest;
use cdk_common::mint::Operation;
use cdk_common::{Amount, PublicKey};
use tracing::instrument;
use uuid::Uuid;

use crate::mint::{Mint, QuoteId};

const MATERIALIZED_STATE_QUERY_BATCH_SIZE: usize = 500;

impl Mint {
    /// Load the canonical Bark action/VTXO projection for route orchestration and workers.
    #[instrument(skip_all)]
    pub async fn federation_bark_projection_snapshot(
        &self,
    ) -> Result<FederationBarkProjectionSnapshot, Error> {
        Ok(self
            .localstore()
            .get_federation_bark_projection_snapshot()
            .await?)
    }

    /// Load one applied receive intent by its accepted operation ID.
    #[instrument(skip_all)]
    pub async fn federation_bark_receive_intent(
        &self,
        operation_id: FederationOperationId,
    ) -> Result<Option<FederationBarkReceiveIntent>, Error> {
        let Some(entry) = self
            .localstore()
            .get_federation_journal_entry_by_operation_id(operation_id)
            .await?
        else {
            return Ok(None);
        };
        let FederationOperation::Bark(FederationBarkOperation::ReceiveIntent(intent)) =
            entry.envelope.operation
        else {
            return Ok(None);
        };
        Ok(Some(intent))
    }

    /// Validate ordered-state Bark preconditions that cannot be checked from
    /// one operation in isolation.
    #[instrument(skip_all)]
    pub async fn validate_federation_bark_operation_submission(
        &self,
        operation: &FederationBarkOperation,
    ) -> Result<(), Error> {
        operation.validate()?;
        let config = self
            .effective_federation_config()
            .await?
            .ok_or(Error::Internal)?;
        let application = config
            .public_config()
            .frost_wallet
            .as_ref()
            .and_then(|wallet| wallet.bark_application.as_ref())
            .ok_or(Error::Internal)?;
        let snapshot = self.federation_bark_projection_snapshot().await?;
        let action = snapshot
            .actions
            .iter()
            .find(|action| action.action_id == operation.action_id());

        match operation {
            FederationBarkOperation::MemberObservation(observation) => {
                let action = action.ok_or_else(|| bark_state_error("observation has no action"))?;
                if action.quote.as_deref() != observation.quote.as_deref()
                    || action.payment_hash != observation.payment_hash
                    || observation.source_scope
                        != ark_server_commitment(&application.ark_server_public_key)?
                {
                    return Err(bark_state_error(
                        "observation differs from its action or Ark source",
                    ));
                }
                if observation.status == FederationBarkObservationStatus::Succeeded {
                    let FederationBarkOperation::ActionTransition(transition) = &action.operation
                    else {
                        return Err(bark_state_error(
                            "successful observation does not follow an action result",
                        ));
                    };
                    match action.phase {
                        FederationBarkSemanticPhase::ReceiveClaimed => {
                            let amount = transition
                                .vtxo_transitions
                                .iter()
                                .filter(|mutation| {
                                    mutation.previous_state.is_none()
                                        && mutation.new_state == FederationBarkVtxoState::Spendable
                                })
                                .try_fold(Amount::ZERO, |sum, mutation| {
                                    sum.checked_add(mutation.amount).ok_or_else(|| {
                                        bark_state_error(
                                            "claimed output observation amount overflow",
                                        )
                                    })
                                })?;
                            if observation.result_hash != transition.result_hash
                                || observation.amount != amount
                                || observation.fee != Amount::ZERO
                            {
                                return Err(bark_state_error(
                                    "successful observation differs from the accepted claim result",
                                ));
                            }
                        }
                        FederationBarkSemanticPhase::PaymentSucceeded => {
                            let plan_entry = self
                                .localstore()
                                .get_federation_journal_entry_by_operation_id(
                                    transition.plan_operation_id,
                                )
                                .await?
                                .ok_or_else(|| bark_state_error("send plan is missing"))?;
                            let FederationOperation::Bark(FederationBarkOperation::ActionPlan(
                                plan,
                            )) = &plan_entry.envelope.operation
                            else {
                                return Err(bark_state_error(
                                    "payment success references a non-plan",
                                ));
                            };
                            validate_payment_success_proof(plan, transition)?;
                            if plan.kind != FederationBarkActionKind::LightningSend
                                || observation.result_hash != transition.result_hash
                                || observation.amount != plan.external_amount
                                || observation.fee != plan.fee_amount
                            {
                                return Err(bark_state_error(
                                    "successful observation differs from the accepted payment result",
                                ));
                            }
                        }
                        _ => {
                            return Err(bark_state_error(
                                "successful observation follows a non-terminal action phase",
                            ));
                        }
                    }
                }
            }
            FederationBarkOperation::ActionTransition(transition)
                if transition.phase == FederationBarkSemanticPhase::ReceiveHtlcsReady =>
            {
                let action =
                    action.ok_or_else(|| bark_state_error("HTLC evidence has no action"))?;
                if action.phase != FederationBarkSemanticPhase::ReceivePrepared {
                    return Err(bark_state_error(
                        "HTLC evidence does not follow receive preparation",
                    ));
                }
                let intent = self
                    .federation_bark_receive_intent(action.origin_operation_id)
                    .await?
                    .ok_or_else(|| bark_state_error("receive intent is missing"))?;
                let amount = transition.vtxo_transitions.iter().try_fold(
                    Amount::ZERO,
                    |sum, mutation| {
                        sum.checked_add(mutation.amount)
                            .ok_or_else(|| bark_state_error("HTLC evidence amount overflow"))
                    },
                )?;
                let fee = intent
                    .gross_amount
                    .checked_sub(amount)
                    .ok_or_else(|| bark_state_error("HTLC evidence exceeds invoice amount"))?;
                if amount < intent.minimum_net_amount || fee > intent.receive_fee {
                    return Err(bark_state_error(
                        "HTLC evidence does not cover the accepted Cashu liability",
                    ));
                }
                let mut observers = BTreeSet::new();
                for record in &snapshot.observations {
                    let FederationBarkOperation::MemberObservation(observation) = &record.operation
                    else {
                        continue;
                    };
                    if observation.action_id == transition.action_id
                        && observation.quote == transition.quote
                        && observation.payment_hash == transition.payment_hash
                        && observation.source_scope
                            == ark_server_commitment(&application.ark_server_public_key)?
                        && observation.status == FederationBarkObservationStatus::Accepted
                        && observation.amount == amount
                        && observation.fee == fee
                        && observation.result_hash == transition.result_hash
                    {
                        observers.insert(observation.observer);
                    }
                }
                if observers.len()
                    < usize::from(config.public_config().threshold_params.signature_threshold)
                {
                    return Err(bark_state_error(
                        "HTLC evidence has not reached the accepted member threshold",
                    ));
                }
            }
            FederationBarkOperation::ActionPlan(plan)
                if plan.kind == FederationBarkActionKind::IncomingClaim =>
            {
                let action = action.ok_or_else(|| bark_state_error("claim plan has no action"))?;
                if action.phase != FederationBarkSemanticPhase::ReceiveHtlcsReady
                    || plan.signer_set.len() < usize::from(application.threshold)
                    || plan
                        .signer_set
                        .iter()
                        .any(|member| !config.public_config().members.contains_key(member))
                {
                    return Err(bark_state_error(
                        "claim plan is not backed by ready HTLCs and an eligible signer set",
                    ));
                }
                for input in &plan.inputs {
                    let vtxo = snapshot
                        .vtxos
                        .iter()
                        .find(|vtxo| vtxo.vtxo_id == input.vtxo_id)
                        .ok_or_else(|| bark_state_error("claim plan input is unknown"))?;
                    if vtxo.state != FederationBarkVtxoState::Observed
                        || vtxo.amount != input.amount
                        || vtxo.locator != input.locator
                        || vtxo.locator
                            != self
                                .federation_bark_receive_intent(action.origin_operation_id)
                                .await?
                                .ok_or_else(|| bark_state_error("receive intent is missing"))?
                                .destination
                    {
                        return Err(bark_state_error(
                            "claim plan input differs from accepted HTLC evidence",
                        ));
                    }
                }
            }
            FederationBarkOperation::ActionPlan(plan)
                if plan.kind == FederationBarkActionKind::LightningSend =>
            {
                if action.is_some() {
                    return Err(bark_state_error("Lightning send action already exists"));
                }
                let predecessor = self
                    .localstore()
                    .get_federation_journal_entry_by_operation_id(plan.predecessor_operation_id)
                    .await?
                    .ok_or_else(|| bark_state_error("Lightning send predecessor is missing"))?;
                let FederationOperation::Melt(melt) = &predecessor.envelope.operation else {
                    return Err(bark_state_error(
                        "Lightning send predecessor is not an accepted melt",
                    ));
                };
                if predecessor.status() != FederationOperationStatus::Applied {
                    return Err(bark_state_error(
                        "Lightning send predecessor is not applied",
                    ));
                }
                let quote_label = plan
                    .quote
                    .as_deref()
                    .ok_or_else(|| bark_state_error("Lightning send quote is missing"))?;
                if melt.request.quote() != quote_label {
                    return Err(bark_state_error(
                        "Lightning send quote differs from its melt",
                    ));
                }
                let quote_id = QuoteId::from_str(quote_label)
                    .map_err(|_| bark_state_error("Lightning send quote id is invalid"))?;
                let quote = self
                    .localstore()
                    .get_melt_quote(&quote_id)
                    .await?
                    .ok_or_else(|| bark_state_error("Lightning send quote is missing"))?;
                let MeltPaymentRequest::Bolt11 { bolt11 } = &quote.request else {
                    return Err(bark_state_error("Lightning send quote is not BOLT11"));
                };
                if quote.state != cdk_common::MeltQuoteState::Pending
                    || quote.unit != cdk_common::CurrencyUnit::Sat
                    || bolt11.network() != application.network
                    || plan.lightning_send_invoice()? != *bolt11
                    || u64::from(plan.external_amount) != quote.amount().value()
                    || u64::from(plan.fee_bound) != quote.fee_reserve().value()
                    || plan.fee_amount != plan.fee_bound
                    || application
                        .policy
                        .maximum_outgoing_fee(plan.external_amount)
                        .map_err(Error::from)?
                        != plan.fee_bound
                {
                    return Err(bark_state_error(
                        "Lightning send differs from its accepted quote or fee policy",
                    ));
                }
                let expected_action = cdk_common::federation::FederationBarkActionId::derive(
                    config.federation_id(),
                    plan.predecessor_operation_id,
                    FederationBarkSemanticPhase::SendPlanned,
                    0,
                )?;
                let author = predecessor
                    .envelope
                    .verify_authorization(config.public_config())?;
                let expected_actor = self
                    .federation_melt_payment_lease_actor(&config, author, predecessor.index)
                    .await?;
                let expected_signers = config
                    .public_config()
                    .members
                    .keys()
                    .copied()
                    .take(usize::from(application.threshold))
                    .collect::<Vec<_>>();
                if plan.action_id != expected_action
                    || plan.actor != expected_actor
                    || plan.signer_set != expected_signers
                    || plan.attempt != 1
                {
                    return Err(bark_state_error(
                        "Lightning send actor, action, signer set, or attempt is not deterministic",
                    ));
                }

                let target = plan
                    .external_amount
                    .checked_add(plan.fee_amount)
                    .ok_or_else(|| bark_state_error("Lightning send target amount overflow"))?;
                let mut expected_inputs = Vec::new();
                let mut selected_amount = Amount::ZERO;
                let mut minimum_expiry = u32::MAX;
                for vtxo in snapshot
                    .vtxos
                    .iter()
                    .filter(|vtxo| vtxo.state == FederationBarkVtxoState::Spendable)
                {
                    expected_inputs.push(FederationBarkVtxoInput {
                        vtxo_id: vtxo.vtxo_id,
                        locator: vtxo.locator,
                        amount: vtxo.amount,
                    });
                    selected_amount =
                        selected_amount.checked_add(vtxo.amount).ok_or_else(|| {
                            bark_state_error("Lightning send selected amount overflow")
                        })?;
                    minimum_expiry = minimum_expiry.min(vtxo.expiry_height);
                    if selected_amount >= target {
                        break;
                    }
                }
                expected_inputs.sort();
                if selected_amount < target || expected_inputs != plan.inputs {
                    return Err(bark_state_error(
                        "Lightning send inputs are not the deterministic accepted selection",
                    ));
                }
                let expected_expiry = minimum_expiry
                    .checked_sub(application.policy.emergency_exit_before_expiry_blocks)
                    .filter(|height| *height != 0)
                    .ok_or_else(|| bark_state_error("Lightning send expiry is unsafe"))?;
                let output_index = snapshot
                    .derivation_allocations
                    .iter()
                    .map(|allocation| allocation.locator.index)
                    .max()
                    .map_or(Ok(0), |index| {
                        index
                            .checked_add(1)
                            .ok_or_else(|| bark_state_error("Bark locator space is exhausted"))
                    })?;
                let revocation_index = output_index
                    .checked_add(1)
                    .filter(|index| *index <= FEDERATION_MAX_DERIVATION_INDEX)
                    .ok_or_else(|| bark_state_error("Bark locator space is exhausted"))?;
                let output = plan
                    .outputs
                    .first()
                    .ok_or_else(|| bark_state_error("Lightning send output is missing"))?;
                let expected_change = selected_amount
                    .checked_sub(target)
                    .ok_or_else(|| bark_state_error("Lightning send change underflow"))?;
                if output.locator != FederationBarkVtxoLocator::vtxo_custody(output_index)
                    || output.amount != expected_change
                    || plan.revocation_locator
                        != Some(FederationBarkVtxoLocator::vtxo_custody(revocation_index))
                    || plan.expires_at != u64::from(expected_expiry)
                {
                    return Err(bark_state_error(
                        "Lightning send change, locators, or expiry is not deterministic",
                    ));
                }
            }
            FederationBarkOperation::ActionTransition(transition)
                if transition.phase == FederationBarkSemanticPhase::SendHtlcsCosigned =>
            {
                let action =
                    action.ok_or_else(|| bark_state_error("outgoing HTLC result has no action"))?;
                let plan_entry = self
                    .localstore()
                    .get_federation_journal_entry_by_operation_id(transition.plan_operation_id)
                    .await?
                    .ok_or_else(|| bark_state_error("accepted send plan is missing"))?;
                let FederationOperation::Bark(FederationBarkOperation::ActionPlan(plan)) =
                    &plan_entry.envelope.operation
                else {
                    return Err(bark_state_error(
                        "outgoing HTLC result references a non-plan",
                    ));
                };
                validate_send_transition_header(action, plan_entry.operation_id, plan, transition)?;
                if action.phase != FederationBarkSemanticPhase::SendPlanned
                    || transition.predecessor_operation_id != action.latest_operation_id
                    || transition.request_hash
                        != FederationBarkOperation::ActionPlan(plan.clone()).commitment()?
                {
                    return Err(bark_state_error(
                        "outgoing HTLC result does not follow its accepted send plan",
                    ));
                }
                let expected_inputs = plan
                    .inputs
                    .iter()
                    .map(|input| input.vtxo_id)
                    .collect::<BTreeSet<_>>();
                let pending_inputs = transition
                    .vtxo_transitions
                    .iter()
                    .filter(|mutation| {
                        mutation.previous_state == Some(FederationBarkVtxoState::Reserved)
                            && mutation.new_state == FederationBarkVtxoState::PendingSpend
                    })
                    .map(|mutation| mutation.vtxo_id)
                    .collect::<BTreeSet<_>>();
                if pending_inputs != expected_inputs {
                    return Err(bark_state_error(
                        "outgoing HTLC result does not pend every accepted input",
                    ));
                }
                for input in &plan.inputs {
                    let record = snapshot
                        .vtxos
                        .iter()
                        .find(|record| record.vtxo_id == input.vtxo_id)
                        .ok_or_else(|| bark_state_error("reserved send input is missing"))?;
                    let mutation = transition
                        .vtxo_transitions
                        .iter()
                        .find(|mutation| mutation.vtxo_id == input.vtxo_id)
                        .ok_or_else(|| bark_state_error("send input mutation is missing"))?;
                    if record.state != FederationBarkVtxoState::Reserved
                        || record.reservation_action_id != Some(plan.action_id)
                        || record.locator != input.locator
                        || record.amount != input.amount
                        || mutation.locator != input.locator
                        || mutation.amount != input.amount
                        || mutation.expiry_height != record.expiry_height
                        || mutation.artifact_hash != record.artifact.hash
                    {
                        return Err(bark_state_error(
                            "outgoing HTLC input differs from its accepted reservation",
                        ));
                    }
                }
                let output = plan
                    .outputs
                    .first()
                    .ok_or_else(|| bark_state_error("send change allocation is missing"))?;
                let change_amount = transition
                    .vtxo_transitions
                    .iter()
                    .filter(|mutation| mutation.previous_state.is_none())
                    .try_fold(Amount::ZERO, |sum, mutation| {
                        if mutation.new_state != FederationBarkVtxoState::Spendable
                            || mutation.locator != output.locator
                        {
                            return Err(bark_state_error(
                                "outgoing result uses an unallocated change locator",
                            ));
                        }
                        sum.checked_add(mutation.amount)
                            .ok_or_else(|| bark_state_error("send change amount overflow"))
                    })?;
                if change_amount != output.amount
                    || transition
                        .artifacts
                        .iter()
                        .filter(|artifact| artifact.kind == FederationBarkArtifactKind::ArkResult)
                        .all(|artifact| artifact.hash != transition.result_hash)
                {
                    return Err(bark_state_error(
                        "outgoing HTLC result differs from accepted change or result evidence",
                    ));
                }
            }
            FederationBarkOperation::ActionTransition(transition)
                if transition.phase == FederationBarkSemanticPhase::PaymentInitiated =>
            {
                let action =
                    action.ok_or_else(|| bark_state_error("payment initiation has no action"))?;
                let plan_entry = self
                    .localstore()
                    .get_federation_journal_entry_by_operation_id(transition.plan_operation_id)
                    .await?
                    .ok_or_else(|| bark_state_error("accepted send plan is missing"))?;
                let FederationOperation::Bark(FederationBarkOperation::ActionPlan(plan)) =
                    &plan_entry.envelope.operation
                else {
                    return Err(bark_state_error("payment initiation references a non-plan"));
                };
                validate_send_transition_header(action, plan_entry.operation_id, plan, transition)?;
                let FederationBarkOperation::ActionTransition(previous) = &action.operation else {
                    return Err(bark_state_error(
                        "payment initiation does not follow an HTLC result",
                    ));
                };
                if action.phase != FederationBarkSemanticPhase::SendHtlcsCosigned
                    || transition.predecessor_operation_id != action.latest_operation_id
                    || transition.request_hash != previous.result_hash
                {
                    return Err(bark_state_error(
                        "payment initiation differs from its accepted HTLC predecessor",
                    ));
                }
                validate_payment_initiated_result(plan, transition)?;
            }
            FederationBarkOperation::ActionTransition(transition)
                if transition.phase == FederationBarkSemanticPhase::PaymentSucceeded =>
            {
                let action =
                    action.ok_or_else(|| bark_state_error("payment success has no action"))?;
                let plan_entry = self
                    .localstore()
                    .get_federation_journal_entry_by_operation_id(transition.plan_operation_id)
                    .await?
                    .ok_or_else(|| bark_state_error("accepted send plan is missing"))?;
                let FederationOperation::Bark(FederationBarkOperation::ActionPlan(plan)) =
                    &plan_entry.envelope.operation
                else {
                    return Err(bark_state_error("payment success references a non-plan"));
                };
                validate_send_transition_header(action, plan_entry.operation_id, plan, transition)?;
                let FederationBarkOperation::ActionTransition(previous) = &action.operation else {
                    return Err(bark_state_error(
                        "payment success does not follow payment initiation",
                    ));
                };
                if action.phase != FederationBarkSemanticPhase::PaymentInitiated
                    || transition.predecessor_operation_id != action.latest_operation_id
                    || transition.request_hash != previous.result_hash
                {
                    return Err(bark_state_error(
                        "payment success differs from its accepted initiation predecessor",
                    ));
                }
                validate_payment_success_proof(plan, transition)?;
                let expected_inputs = plan
                    .inputs
                    .iter()
                    .map(|input| input.vtxo_id)
                    .collect::<BTreeSet<_>>();
                let spent_inputs = transition
                    .vtxo_transitions
                    .iter()
                    .map(|mutation| mutation.vtxo_id)
                    .collect::<BTreeSet<_>>();
                if spent_inputs != expected_inputs {
                    return Err(bark_state_error(
                        "payment success does not spend every accepted send input",
                    ));
                }
                for input in &plan.inputs {
                    let record = snapshot
                        .vtxos
                        .iter()
                        .find(|record| record.vtxo_id == input.vtxo_id)
                        .ok_or_else(|| bark_state_error("pending send input is missing"))?;
                    let mutation = transition
                        .vtxo_transitions
                        .iter()
                        .find(|mutation| mutation.vtxo_id == input.vtxo_id)
                        .ok_or_else(|| bark_state_error("spent send mutation is missing"))?;
                    if record.state != FederationBarkVtxoState::PendingSpend
                        || record.reservation_action_id != Some(plan.action_id)
                        || record.locator != input.locator
                        || record.amount != input.amount
                        || mutation.locator != input.locator
                        || mutation.amount != input.amount
                        || mutation.expiry_height != record.expiry_height
                        || mutation.artifact_hash != record.artifact.hash
                    {
                        return Err(bark_state_error(
                            "payment success input differs from its pending spend",
                        ));
                    }
                }
            }
            FederationBarkOperation::ActionTransition(transition)
                if transition.phase == FederationBarkSemanticPhase::ReceivePreimageDisclosed =>
            {
                let action =
                    action.ok_or_else(|| bark_state_error("preimage disclosure has no action"))?;
                let plan_entry = self
                    .localstore()
                    .get_federation_journal_entry_by_operation_id(transition.plan_operation_id)
                    .await?
                    .ok_or_else(|| bark_state_error("accepted claim plan is missing"))?;
                let FederationOperation::Bark(FederationBarkOperation::ActionPlan(plan)) =
                    &plan_entry.envelope.operation
                else {
                    return Err(bark_state_error("claim transition references a non-plan"));
                };
                let transitioned = transition
                    .vtxo_transitions
                    .iter()
                    .map(|mutation| mutation.vtxo_id)
                    .collect::<BTreeSet<_>>();
                let planned = plan
                    .inputs
                    .iter()
                    .map(|input| input.vtxo_id)
                    .collect::<BTreeSet<_>>();
                if action.phase != FederationBarkSemanticPhase::ClaimPlanned
                    || transition.predecessor_operation_id != action.latest_operation_id
                    || transitioned != planned
                {
                    return Err(bark_state_error(
                        "preimage disclosure differs from the accepted claim plan",
                    ));
                }
                for input in &plan.inputs {
                    let vtxo = snapshot
                        .vtxos
                        .iter()
                        .find(|vtxo| vtxo.vtxo_id == input.vtxo_id)
                        .ok_or_else(|| bark_state_error("reserved claim input is missing"))?;
                    if vtxo.state != FederationBarkVtxoState::Reserved
                        || vtxo.reservation_action_id != Some(plan.action_id)
                        || vtxo.amount != input.amount
                    {
                        return Err(bark_state_error(
                            "preimage disclosure input is not reserved by its claim",
                        ));
                    }
                }
            }
            FederationBarkOperation::ActionTransition(transition)
                if transition.phase == FederationBarkSemanticPhase::ReceiveClaimed =>
            {
                let action =
                    action.ok_or_else(|| bark_state_error("claimed receive has no action"))?;
                let plan_entry = self
                    .localstore()
                    .get_federation_journal_entry_by_operation_id(transition.plan_operation_id)
                    .await?
                    .ok_or_else(|| bark_state_error("accepted claim plan is missing"))?;
                let FederationOperation::Bark(FederationBarkOperation::ActionPlan(plan)) =
                    &plan_entry.envelope.operation
                else {
                    return Err(bark_state_error("claim result references a non-plan"));
                };
                let intent = self
                    .federation_bark_receive_intent(action.origin_operation_id)
                    .await?
                    .ok_or_else(|| bark_state_error("receive intent is missing"))?;
                let spent = transition
                    .vtxo_transitions
                    .iter()
                    .filter(|mutation| {
                        mutation.previous_state == Some(FederationBarkVtxoState::PendingSpend)
                            && mutation.new_state == FederationBarkVtxoState::Spent
                    })
                    .map(|mutation| mutation.vtxo_id)
                    .collect::<BTreeSet<_>>();
                let planned = plan
                    .inputs
                    .iter()
                    .map(|input| input.vtxo_id)
                    .collect::<BTreeSet<_>>();
                let output = plan
                    .outputs
                    .first()
                    .ok_or_else(|| bark_state_error("claim plan has no output allocation"))?;
                let mut spendable_sat = 0_u64;
                for mutation in transition.vtxo_transitions.iter().filter(|mutation| {
                    mutation.previous_state.is_none()
                        && mutation.new_state == FederationBarkVtxoState::Spendable
                }) {
                    if mutation.locator != output.locator {
                        return Err(bark_state_error(
                            "claim result uses an unallocated output locator",
                        ));
                    }
                    spendable_sat = spendable_sat
                        .checked_add(u64::from(mutation.amount))
                        .ok_or_else(|| bark_state_error("claim result amount overflow"))?;
                }
                if action.phase != FederationBarkSemanticPhase::ReceivePreimageDisclosed
                    || transition.predecessor_operation_id != action.latest_operation_id
                    || spent != planned
                    || spendable_sat < u64::from(intent.cashu_amount)
                    || spendable_sat > u64::from(output.amount)
                {
                    return Err(bark_state_error(
                        "claim result does not prove sufficient accepted spendable value",
                    ));
                }
            }
            FederationBarkOperation::ReceiveIntent(_)
            | FederationBarkOperation::ReceivePrepared(_)
            | FederationBarkOperation::ActionPlan(_)
            | FederationBarkOperation::ActionTransition(_) => {}
        }
        Ok(())
    }

    /// Build the deterministic federation materialized-state snapshot.
    #[instrument(skip_all)]
    pub async fn federation_materialized_state_snapshot(
        &self,
    ) -> Result<FederationMaterializedStateSnapshot, Error> {
        let db = self.localstore();
        let mut snapshot = FederationMaterializedStateSnapshot::new();
        for root in db.get_federation_frost_root_public_configs().await? {
            snapshot.push(
                FederationMaterializedStateRow::new(
                    "federation_frost_root_public_config",
                    root.root_key_epoch.to_string(),
                )
                .with_field("payload_json", serde_json::to_string(&root)?),
            );
        }
        for application in db.get_federation_frost_application_configs().await? {
            snapshot.push(
                FederationMaterializedStateRow::new(
                    "federation_frost_application_config",
                    format!(
                        "{}:{}",
                        application.application_id, application.application_key_epoch
                    ),
                )
                .with_field("payload_json", serde_json::to_string(&application)?),
            );
        }
        for row in self
            .federation_address_allocation_state()
            .await?
            .materialized_rows()?
        {
            snapshot.push(row);
        }
        for row in self
            .federation_chain_observation_state()
            .await?
            .materialized_rows()?
        {
            snapshot.push(row);
        }
        for row in self
            .federation_transaction_state()
            .await?
            .materialized_rows()?
        {
            snapshot.push(row);
        }
        for row in db
            .get_federation_bark_projection_snapshot()
            .await?
            .materialized_rows()?
        {
            snapshot.push(row);
        }
        let mut mint_quotes = BTreeSet::new();
        let mut melt_quotes = BTreeSet::new();
        let mut completed_operations = BTreeSet::new();
        let mut proof_ys = BTreeSet::new();
        let mut blinded_messages = BTreeSet::new();
        let mut keyset_rotations = Vec::new();

        for record in db.get_federation_materialized_state_keys().await? {
            match record.key {
                FederationMaterializedStateKey::MintQuote { quote } => {
                    mint_quotes.insert(quote);
                }
                FederationMaterializedStateKey::MeltQuote { quote } => {
                    melt_quotes.insert(quote);
                }
                FederationMaterializedStateKey::CompletedOperation => {
                    completed_operations.insert(record.operation_id);
                }
                FederationMaterializedStateKey::ProofState { y } => {
                    proof_ys.insert(y);
                }
                FederationMaterializedStateKey::BlindSignature { blinded_message } => {
                    blinded_messages.insert(blinded_message);
                }
                FederationMaterializedStateKey::KeysetRotation { keyset } => {
                    keyset_rotations.push((record.operation_index, record.operation_id, keyset));
                }
                FederationMaterializedStateKey::BarkAction { .. }
                | FederationMaterializedStateKey::BarkVtxo { .. }
                | FederationMaterializedStateKey::BarkDerivationAllocation { .. }
                | FederationMaterializedStateKey::BarkReservation { .. }
                | FederationMaterializedStateKey::BarkTransition { .. }
                | FederationMaterializedStateKey::BarkObservation { .. } => {}
            }
        }

        let completed_operation_ids = completed_operations
            .into_iter()
            .map(|operation_id| (operation_id, federation_operation_uuid(operation_id)))
            .collect::<Vec<_>>();
        let completed_operation_uuids = completed_operation_ids
            .iter()
            .map(|(_, operation_uuid)| *operation_uuid)
            .collect::<Vec<_>>();
        let mut completed_by_id = HashMap::new();
        for operation_ids in completed_operation_uuids.chunks(MATERIALIZED_STATE_QUERY_BATCH_SIZE) {
            completed_by_id.extend(
                db.get_federation_completed_operations(operation_ids)
                    .await?
                    .into_iter()
                    .map(|operation| (*operation.id(), operation)),
            );
        }
        for (operation_id, operation_uuid) in completed_operation_ids {
            match completed_by_id.remove(&operation_uuid) {
                Some(operation) => snapshot.push(completed_operation_materialized_row(
                    operation_id,
                    operation_uuid,
                    &operation,
                )),
                None => snapshot.push(
                    FederationMaterializedStateRow::new(
                        "completed_operations",
                        operation_uuid.to_string(),
                    )
                    .with_field("federation_operation_id", operation_id.to_string())
                    .with_field("missing", "true"),
                ),
            }
        }

        for (operation_index, operation_id, keyset) in keyset_rotations {
            snapshot.push(federation_keyset_materialized_row(
                operation_index,
                operation_id,
                &keyset,
            )?);
        }

        let mint_quote_ids = mint_quotes
            .iter()
            .map(|quote| QuoteId::from_str(quote))
            .collect::<Result<Vec<_>, _>>()?;
        let mut stored_mint_quotes = Vec::with_capacity(mint_quote_ids.len());
        for quote_ids in mint_quote_ids.chunks(MATERIALIZED_STATE_QUERY_BATCH_SIZE) {
            stored_mint_quotes.extend(db.get_federation_mint_quote_states(quote_ids).await?);
        }
        for (quote, stored_quote) in mint_quotes.into_iter().zip(stored_mint_quotes) {
            match stored_quote {
                Some(quote) => snapshot.push(
                    FederationMaterializedStateRow::new("mint_quotes", quote.id.to_string())
                        .with_field("amount", optional_amount_value(quote.amount.as_ref()))
                        .with_field("unit", quote.unit.to_string())
                        .with_field("request", quote.request.clone())
                        .with_field("expiry", quote.expiry.to_string())
                        .with_field("amount_paid", quote.amount_paid.value().to_string())
                        .with_field("amount_issued", quote.amount_issued.value().to_string())
                        .with_field("state", quote.state.to_string())
                        .with_field("payment_method", quote.payment_method.to_string()),
                ),
                None => snapshot.push(
                    FederationMaterializedStateRow::new("mint_quotes", quote)
                        .with_field("missing", "true"),
                ),
            }
        }

        let melt_quote_ids = melt_quotes
            .iter()
            .map(|quote| QuoteId::from_str(quote))
            .collect::<Result<Vec<_>, _>>()?;
        let mut stored_melt_quotes = HashMap::new();
        for quote_ids in melt_quote_ids.chunks(MATERIALIZED_STATE_QUERY_BATCH_SIZE) {
            stored_melt_quotes.extend(
                db.get_federation_melt_quotes(quote_ids)
                    .await?
                    .into_iter()
                    .map(|quote| (quote.id.to_string(), quote)),
            );
        }
        for quote in melt_quotes {
            match stored_melt_quotes.remove(&quote) {
                Some(quote) => snapshot.push(
                    FederationMaterializedStateRow::new("melt_quotes", quote.id.to_string())
                        .with_field("amount", quote.amount().value().to_string())
                        .with_field("fee_reserve", quote.fee_reserve().value().to_string())
                        .with_field("unit", quote.unit.to_string())
                        .with_field("request", quote.request.to_string())
                        .with_field("state", quote.state.to_string())
                        .with_field("payment_method", quote.payment_method.to_string())
                        .with_field("payment_proof", quote.payment_proof.unwrap_or_default()),
                ),
                None => snapshot.push(
                    FederationMaterializedStateRow::new("melt_quotes", quote)
                        .with_field("missing", "true"),
                ),
            }
        }

        for record in db.get_all_federation_payment_observations().await? {
            snapshot.push(payment_observation_materialized_row(&record)?);
        }

        if !proof_ys.is_empty() {
            append_proof_state_rows(&mut snapshot, db.as_ref(), proof_ys).await?;
        }

        if !blinded_messages.is_empty() {
            append_blind_signature_rows(&mut snapshot, db.as_ref(), blinded_messages).await?;
        }

        Ok(snapshot)
    }
}

fn bark_state_error(reason: &str) -> Error {
    Error::Custom(format!("invalid federated Bark state: {reason}"))
}

fn validate_send_transition_header(
    action: &FederationBarkActionRecord,
    plan_operation_id: FederationOperationId,
    plan: &FederationBarkActionPlan,
    transition: &FederationBarkActionTransition,
) -> Result<(), Error> {
    if plan.kind != FederationBarkActionKind::LightningSend
        || transition.action_id != plan.action_id
        || transition.quote != plan.quote
        || transition.plan_operation_id != plan_operation_id
        || transition.payment_hash != plan.payment_hash
        || transition.actor != plan.actor
        || transition.attempt != plan.attempt
        || transition.signer_set != plan.signer_set
        || transition.external_id != plan.external_id
        || action.action_id != plan.action_id
        || action.quote != plan.quote
        || action.payment_hash != plan.payment_hash
    {
        return Err(bark_state_error(
            "outgoing transition differs from its accepted send bindings",
        ));
    }
    Ok(())
}

pub(super) fn validate_payment_success_proof(
    plan: &FederationBarkActionPlan,
    transition: &FederationBarkActionTransition,
) -> Result<(), Error> {
    let mut proofs = transition
        .artifacts
        .iter()
        .filter(|artifact| artifact.kind == FederationBarkArtifactKind::Proof);
    let proof = proofs
        .next()
        .ok_or_else(|| bark_state_error("payment success proof is missing"))?;
    let preimage: [u8; 32] = proof
        .bytes
        .as_slice()
        .try_into()
        .map_err(|_| bark_state_error("payment success proof is not a 32-byte preimage"))?;
    let payment_hash = plan
        .payment_hash
        .ok_or_else(|| bark_state_error("send plan payment hash is missing"))?;
    let mut result_bytes = b"cdk/bark/send-payment-succeeded/v1".to_vec();
    result_bytes.extend_from_slice(&plan.action_id.to_bytes());
    result_bytes.extend_from_slice(&transition.request_hash.to_bytes());
    result_bytes.extend_from_slice(&preimage);
    let result_hash =
        FederationBarkDigest::from_bytes(sha256::Hash::hash(&result_bytes).to_byte_array())?;
    if proofs.next().is_some()
        || sha256::Hash::hash(&preimage).to_byte_array() != payment_hash.to_bytes()
        || transition.result_hash != result_hash
    {
        return Err(bark_state_error(
            "payment success proof differs from the accepted invoice or result",
        ));
    }
    Ok(())
}

fn validate_payment_initiated_result(
    plan: &FederationBarkActionPlan,
    transition: &FederationBarkActionTransition,
) -> Result<(), Error> {
    let payment_hash = plan
        .payment_hash
        .ok_or_else(|| bark_state_error("send plan payment hash is missing"))?;
    let mut result_bytes = b"cdk/bark/send-payment-initiated/v1".to_vec();
    result_bytes.extend_from_slice(&plan.action_id.to_bytes());
    result_bytes.extend_from_slice(&payment_hash.to_bytes());
    result_bytes.extend_from_slice(&transition.request_hash.to_bytes());
    result_bytes.extend_from_slice(&plan.external_id.to_bytes());
    let mut artifacts = transition
        .artifacts
        .iter()
        .filter(|artifact| artifact.kind == FederationBarkArtifactKind::ArkResult);
    let artifact = artifacts
        .next()
        .ok_or_else(|| bark_state_error("payment initiation result is missing"))?;
    let expected =
        FederationBarkDigest::from_bytes(sha256::Hash::hash(&result_bytes).to_byte_array())?;
    if artifacts.next().is_some()
        || artifact.bytes != result_bytes
        || artifact.hash != expected
        || transition.result_hash != expected
    {
        return Err(bark_state_error(
            "payment initiation result differs from its accepted request",
        ));
    }
    Ok(())
}

async fn append_proof_state_rows(
    snapshot: &mut FederationMaterializedStateSnapshot,
    db: &(dyn cdk_common::database::MintDatabase<cdk_common::database::Error> + Send + Sync),
    proof_ys: BTreeSet<PublicKey>,
) -> Result<(), Error> {
    let proof_ys = proof_ys.into_iter().collect::<Vec<_>>();
    let proof_states = db.get_proofs_states(&proof_ys).await?;
    for (y, state) in proof_ys.into_iter().zip(proof_states) {
        let mut row = FederationMaterializedStateRow::new("proof_states", y.to_hex());
        match state {
            Some(state) => row.insert_field("state", state.to_string()),
            None => row.insert_field("missing", "true"),
        }
        snapshot.push(row);
    }
    Ok(())
}

async fn append_blind_signature_rows(
    snapshot: &mut FederationMaterializedStateSnapshot,
    db: &(dyn cdk_common::database::MintDatabase<cdk_common::database::Error> + Send + Sync),
    blinded_messages: BTreeSet<PublicKey>,
) -> Result<(), Error> {
    let blinded_messages = blinded_messages.into_iter().collect::<Vec<_>>();
    let blind_signatures = db.get_blind_signatures(&blinded_messages).await?;
    for (blinded_message, signature) in blinded_messages.into_iter().zip(blind_signatures) {
        let mut row =
            FederationMaterializedStateRow::new("blind_signatures", blinded_message.to_hex());
        match signature {
            Some(signature) => {
                row.insert_field("amount", signature.amount.to_string());
                row.insert_field("keyset_id", signature.keyset_id.to_string());
            }
            None => row.insert_field("missing", "true"),
        }
        snapshot.push(row);
    }
    Ok(())
}

fn federation_operation_uuid(operation_id: FederationOperationId) -> Uuid {
    let bytes = operation_id.to_bytes();
    let mut uuid = [0_u8; 16];
    uuid.copy_from_slice(&bytes[..16]);
    Uuid::from_bytes(uuid)
}

fn completed_operation_materialized_row(
    operation_id: FederationOperationId,
    operation_uuid: Uuid,
    operation: &Operation,
) -> FederationMaterializedStateRow {
    FederationMaterializedStateRow::new("completed_operations", operation_uuid.to_string())
        .with_field("federation_operation_id", operation_id.to_string())
        .with_field("kind", operation.kind().to_string())
        .with_field(
            "total_issued",
            operation.total_issued().to_u64().to_string(),
        )
        .with_field(
            "total_redeemed",
            operation.total_redeemed().to_u64().to_string(),
        )
        .with_field(
            "fee_collected",
            operation.fee_collected().to_u64().to_string(),
        )
        .with_field(
            "completed_at",
            operation
                .completed_at()
                .map(|completed_at| completed_at.to_string())
                .unwrap_or_default(),
        )
        .with_field(
            "payment_method",
            operation
                .payment_method()
                .map(|method| method.to_string())
                .unwrap_or_default(),
        )
}

fn federation_keyset_materialized_row(
    operation_index: u64,
    operation_id: FederationOperationId,
    keyset: &FederatedKeyset,
) -> Result<FederationMaterializedStateRow, Error> {
    Ok(
        FederationMaterializedStateRow::new("federation_keysets", keyset.id.to_string())
            .with_field("operation_index", operation_index.to_string())
            .with_field("operation_id", operation_id.to_string())
            .with_field("unit", keyset.unit.to_string())
            .with_field("active", keyset.active.to_string())
            .with_field("input_fee_ppk", keyset.input_fee_ppk.to_string())
            .with_field(
                "final_expiry",
                keyset
                    .final_expiry
                    .map(|expiry| expiry.to_string())
                    .unwrap_or_default(),
            )
            .with_field(
                "aggregate_keys",
                serde_json::to_string(&keyset.aggregate_keys)?,
            )
            .with_field(
                "public_shares",
                serde_json::to_string(&keyset.public_shares)?,
            ),
    )
}

fn optional_amount_value(amount: Option<&Amount<cdk_common::CurrencyUnit>>) -> String {
    amount
        .map(|amount| amount.value().to_string())
        .unwrap_or_default()
}

fn payment_observation_materialized_row(
    record: &FederationPaymentObservationRecord,
) -> Result<FederationMaterializedStateRow, Error> {
    let key = format!(
        "{}:{}:{}:{}",
        record.observation.context(),
        record.observation.quote(),
        record.operation_index,
        record.operation_id
    );
    let canonical = serde_json::to_vec(&record.observation)?;
    Ok(
        FederationMaterializedStateRow::new("payment_observations", key)
            .with_field("context", record.observation.context().to_string())
            .with_field("quote", record.observation.quote().to_string())
            .with_field("operation_index", record.operation_index.to_string())
            .with_field("operation_id", record.operation_id.to_string())
            .with_field("observer", record.observation.observer().to_string())
            .with_field("state", record.observation.state_label())
            .with_field("evidence_hash", sha256::Hash::hash(&canonical).to_string()),
    )
}

#[cfg(test)]
mod outgoing_tests {
    use super::*;
    use cdk_common::federation::{
        FederationBarkActionId, FederationBarkArtifact, FederationBarkExternalId,
        FederationBarkVtxoOutput, MemberId, FEDERATION_BARK_OPERATION_VERSION,
    };

    fn digest(seed: u8) -> FederationBarkDigest {
        FederationBarkDigest::from_bytes([seed; 32]).expect("non-zero digest")
    }

    fn send_plan(payment_hash: FederationBarkDigest) -> FederationBarkActionPlan {
        FederationBarkActionPlan {
            version: FEDERATION_BARK_OPERATION_VERSION,
            action_id: FederationBarkActionId::from_bytes([1; 32]).expect("action id"),
            quote: Some("outgoing-proof-test".to_string()),
            kind: FederationBarkActionKind::LightningSend,
            actor: MemberId::new(1).expect("member"),
            predecessor_operation_id: FederationOperationId::from_bytes([2; 32]),
            payment_hash: Some(payment_hash),
            inputs: Vec::new(),
            outputs: vec![FederationBarkVtxoOutput {
                locator: FederationBarkVtxoLocator::vtxo_custody(1),
                amount: Amount::ZERO,
                artifact_hash: digest(3),
            }],
            revocation_locator: Some(FederationBarkVtxoLocator::vtxo_custody(2)),
            external_amount: Amount::from(1),
            fee_amount: Amount::from(2),
            fee_bound: Amount::from(2),
            artifacts: Vec::new(),
            external_id: FederationBarkExternalId::from_bytes([4; 32]).expect("external id"),
            signer_set: vec![MemberId::new(1).expect("member")],
            attempt: 1,
            expires_at: 1,
        }
    }

    #[test]
    fn outgoing_success_proof_binds_preimage_hash_and_predecessor() {
        let preimage = [7; 32];
        let payment_hash =
            FederationBarkDigest::from_bytes(sha256::Hash::hash(&preimage).to_byte_array())
                .expect("payment hash");
        let plan = send_plan(payment_hash);
        let request_hash = digest(8);
        let mut result_bytes = b"cdk/bark/send-payment-succeeded/v1".to_vec();
        result_bytes.extend_from_slice(&plan.action_id.to_bytes());
        result_bytes.extend_from_slice(&request_hash.to_bytes());
        result_bytes.extend_from_slice(&preimage);
        let result_hash =
            FederationBarkDigest::from_bytes(sha256::Hash::hash(&result_bytes).to_byte_array())
                .expect("result hash");
        let proof_hash =
            FederationBarkDigest::from_bytes(sha256::Hash::hash(&preimage).to_byte_array())
                .expect("proof hash");
        let transition = FederationBarkActionTransition {
            version: FEDERATION_BARK_OPERATION_VERSION,
            action_id: plan.action_id,
            quote: plan.quote.clone(),
            phase: FederationBarkSemanticPhase::PaymentSucceeded,
            actor: plan.actor,
            plan_operation_id: FederationOperationId::from_bytes([9; 32]),
            predecessor_operation_id: FederationOperationId::from_bytes([10; 32]),
            payment_hash: plan.payment_hash,
            attempt: plan.attempt,
            signer_set: plan.signer_set.clone(),
            request_hash,
            result_hash,
            external_id: plan.external_id,
            artifacts: vec![FederationBarkArtifact {
                kind: FederationBarkArtifactKind::Proof,
                hash: proof_hash,
                bytes: preimage.to_vec(),
            }],
            vtxo_transitions: Vec::new(),
        };
        validate_payment_success_proof(&plan, &transition).expect("valid success proof");

        let mut changed_proof = transition.clone();
        changed_proof.artifacts[0].bytes[0] ^= 1;
        assert!(validate_payment_success_proof(&plan, &changed_proof).is_err());
        let mut changed_predecessor = transition;
        changed_predecessor.request_hash = digest(11);
        assert!(validate_payment_success_proof(&plan, &changed_predecessor).is_err());
    }
}
