//! Checks of Bark transitions against accepted projection records.
//!
//! These functions are pure: they compare a proposed transition with the
//! accepted plan and the current projection, so every member reaches the same
//! verdict. The mint projection calls them while applying ordered operations.

use std::collections::BTreeSet;

use bitcoin::hashes::sha256::Hash as Sha256Hash;
use bitcoin::hashes::Hash;
use cdk_common::federation::{
    FederationError, FederationOperationId, MemberId, Result, FEDERATION_MAX_DERIVATION_INDEX,
};
use cdk_common::Amount;

use crate::{
    FederationBarkActionKind, FederationBarkActionPlan, FederationBarkActionRecord,
    FederationBarkActionTransition, FederationBarkArtifact, FederationBarkArtifactKind,
    FederationBarkDigest, FederationBarkObservationRecord, FederationBarkObservationStatus,
    FederationBarkOperation, FederationBarkSemanticPhase, FederationBarkVtxoRecord,
    FederationBarkVtxoState,
};

fn invalid_state(reason: &str) -> FederationError {
    FederationError::ApplicationOperationInvalid {
        reason: format!("invalid federated Bark state: {reason}"),
    }
}

/// Next unused VTXO locator index after every allocated derivation.
pub fn next_bark_locator_index(max_allocated: Option<u32>) -> Result<u32> {
    max_allocated.map_or(Ok(0), |index| {
        index
            .checked_add(1)
            .filter(|index| *index <= FEDERATION_MAX_DERIVATION_INDEX)
            .ok_or_else(|| invalid_state("Bark locator space is exhausted"))
    })
}

/// Check that a maintenance transition continues its accepted plan and custody attempt.
pub fn validate_maintenance_transition_header(
    action: &FederationBarkActionRecord,
    plan_operation_id: FederationOperationId,
    plan: &FederationBarkActionPlan,
    transition: &FederationBarkActionTransition,
    expected_kind: FederationBarkActionKind,
) -> Result<()> {
    let expected_signers = action_custody_signers(action, plan)?;
    if plan.kind != expected_kind
        || transition.action_id != plan.action_id
        || transition.quote.is_some()
        || transition.plan_operation_id != plan_operation_id
        || transition.payment_hash.is_some()
        || transition.actor != action.actor
        || transition.attempt != action.attempt
        || transition.signer_set != expected_signers
        || transition.external_id != plan.external_id
        || transition.request_hash
            != FederationBarkOperation::ActionPlan(plan.clone()).commitment()?
        || action.action_id != plan.action_id
        || action.quote.is_some()
        || action.payment_hash.is_some()
    {
        return Err(invalid_state(
            "maintenance transition differs from its accepted plan",
        ));
    }
    Ok(())
}

/// Check that at least `threshold` members accepted an observation matching the maintenance plan.
pub fn validate_maintenance_trigger_quorum(
    observations: &[FederationBarkObservationRecord],
    threshold: u16,
    plan: &FederationBarkActionPlan,
) -> Result<()> {
    let commitment = FederationBarkOperation::ActionPlan(plan.clone()).commitment()?;
    let amount = if plan.kind == FederationBarkActionKind::Refresh {
        plan.inputs.iter().try_fold(Amount::ZERO, |sum, input| {
            sum.checked_add(input.amount)
                .ok_or_else(|| invalid_state("maintenance quorum amount overflow"))
        })?
    } else {
        plan.external_amount
    };
    let observers = observations
        .iter()
        .filter_map(|record| match &record.operation {
            FederationBarkOperation::MemberObservation(observation)
                if observation.action_id == plan.action_id
                    && observation.status == FederationBarkObservationStatus::Accepted
                    && observation.amount == amount
                    && observation.fee == plan.fee_amount
                    && observation.result_hash == commitment =>
            {
                Some(observation.observer)
            }
            _ => None,
        })
        .collect::<BTreeSet<_>>();
    if observers.len() < usize::from(threshold) {
        return Err(invalid_state(
            "maintenance trigger has not reached the accepted member threshold",
        ));
    }
    Ok(())
}

/// Check that a refresh result spends exactly the reserved inputs into the allocated outputs.
pub fn validate_refresh_completed_result(
    vtxos: &[FederationBarkVtxoRecord],
    plan: &FederationBarkActionPlan,
    transition: &FederationBarkActionTransition,
) -> Result<()> {
    let expected_inputs = plan
        .inputs
        .iter()
        .map(|input| input.vtxo_id)
        .collect::<BTreeSet<_>>();
    let spent_inputs = transition
        .vtxo_transitions
        .iter()
        .filter(|mutation| {
            mutation.previous_state == Some(FederationBarkVtxoState::Reserved)
                && mutation.new_state == FederationBarkVtxoState::Spent
        })
        .map(|mutation| mutation.vtxo_id)
        .collect::<BTreeSet<_>>();
    if expected_inputs != spent_inputs {
        return Err(invalid_state(
            "refresh result does not spend every accepted input",
        ));
    }
    for input in &plan.inputs {
        let record = vtxos
            .iter()
            .find(|record| record.vtxo_id == input.vtxo_id)
            .ok_or_else(|| invalid_state("refresh reserved input is missing"))?;
        let mutation = transition
            .vtxo_transitions
            .iter()
            .find(|mutation| mutation.vtxo_id == input.vtxo_id)
            .ok_or_else(|| invalid_state("refresh input mutation is missing"))?;
        if record.state != FederationBarkVtxoState::Reserved
            || record.reservation_action_id != Some(plan.action_id)
            || record.locator != input.locator
            || record.amount != input.amount
            || mutation.locator != input.locator
            || mutation.amount != input.amount
            || mutation.expiry_height != record.expiry_height
            || mutation.artifact_hash != record.artifact.hash
        {
            return Err(invalid_state(
                "refresh result differs from its accepted reservation",
            ));
        }
    }
    let output = plan
        .outputs
        .first()
        .ok_or_else(|| invalid_state("refresh output allocation is missing"))?;
    let output_amount = transition
        .vtxo_transitions
        .iter()
        .filter(|mutation| {
            mutation.previous_state.is_none()
                && mutation.new_state == FederationBarkVtxoState::Spendable
        })
        .try_fold(Amount::ZERO, |sum, mutation| {
            if mutation.locator != output.locator {
                return Err(invalid_state(
                    "refresh result uses an unallocated output locator",
                ));
            }
            sum.checked_add(mutation.amount)
                .ok_or_else(|| invalid_state("refresh output amount overflow"))
        })?;
    if output_amount != output.amount
        || transition
            .artifacts
            .iter()
            .filter(|artifact| artifact.kind == FederationBarkArtifactKind::ArkResult)
            .all(|artifact| artifact.hash != transition.result_hash)
    {
        return Err(invalid_state(
            "refresh result differs from its accepted output or evidence",
        ));
    }
    Ok(())
}

/// Check that an exit registration moves exactly the reserved inputs into the exiting state.
pub fn validate_exit_registered_result(
    vtxos: &[FederationBarkVtxoRecord],
    plan: &FederationBarkActionPlan,
    transition: &FederationBarkActionTransition,
) -> Result<()> {
    let expected_inputs = plan
        .inputs
        .iter()
        .map(|input| input.vtxo_id)
        .collect::<BTreeSet<_>>();
    let exiting = transition
        .vtxo_transitions
        .iter()
        .map(|mutation| mutation.vtxo_id)
        .collect::<BTreeSet<_>>();
    if expected_inputs != exiting {
        return Err(invalid_state(
            "exit registration does not transition every accepted input",
        ));
    }
    for input in &plan.inputs {
        let record = vtxos
            .iter()
            .find(|record| record.vtxo_id == input.vtxo_id)
            .ok_or_else(|| invalid_state("exit reserved input is missing"))?;
        let mutation = transition
            .vtxo_transitions
            .iter()
            .find(|mutation| mutation.vtxo_id == input.vtxo_id)
            .ok_or_else(|| invalid_state("exit input mutation is missing"))?;
        if record.state != FederationBarkVtxoState::Reserved
            || record.reservation_action_id != Some(plan.action_id)
            || mutation.locator != record.locator
            || mutation.amount != record.amount
            || mutation.expiry_height != record.expiry_height
            || mutation.artifact_hash != record.artifact.hash
        {
            return Err(invalid_state(
                "exit registration differs from its accepted reservation",
            ));
        }
    }
    validate_exit_transaction_artifacts(plan, transition, false)
}

/// Check that a published exit carries the planned transaction artifacts.
pub fn validate_exit_published_result(
    plan: &FederationBarkActionPlan,
    transition: &FederationBarkActionTransition,
) -> Result<()> {
    validate_exit_transaction_artifacts(plan, transition, true)
}

/// Check that a confirmed exit spends the exiting inputs and keeps the published transactions.
pub fn validate_exit_confirmed_result(
    vtxos: &[FederationBarkVtxoRecord],
    plan: &FederationBarkActionPlan,
    published: &FederationBarkActionTransition,
    transition: &FederationBarkActionTransition,
) -> Result<()> {
    let expected_inputs = plan
        .inputs
        .iter()
        .map(|input| input.vtxo_id)
        .collect::<BTreeSet<_>>();
    let exited = transition
        .vtxo_transitions
        .iter()
        .map(|mutation| mutation.vtxo_id)
        .collect::<BTreeSet<_>>();
    if expected_inputs != exited {
        return Err(invalid_state(
            "exit confirmation does not finalize every accepted input",
        ));
    }
    for input in &plan.inputs {
        let record = vtxos
            .iter()
            .find(|record| record.vtxo_id == input.vtxo_id)
            .ok_or_else(|| invalid_state("exiting input is missing"))?;
        let mutation = transition
            .vtxo_transitions
            .iter()
            .find(|mutation| mutation.vtxo_id == input.vtxo_id)
            .ok_or_else(|| invalid_state("exit confirmation mutation is missing"))?;
        if record.state != FederationBarkVtxoState::Exiting
            || record.reservation_action_id != Some(plan.action_id)
            || mutation.locator != record.locator
            || mutation.amount != record.amount
            || mutation.expiry_height != record.expiry_height
            || mutation.artifact_hash != record.artifact.hash
        {
            return Err(invalid_state(
                "exit confirmation differs from its exiting reservation",
            ));
        }
    }
    let published_transactions = published
        .artifacts
        .iter()
        .filter(|artifact| artifact.kind == FederationBarkArtifactKind::Transaction)
        .collect::<BTreeSet<_>>();
    let confirmed_transactions = transition
        .artifacts
        .iter()
        .filter(|artifact| artifact.kind == FederationBarkArtifactKind::Transaction)
        .collect::<BTreeSet<_>>();
    if !published_transactions.is_subset(&confirmed_transactions)
        || transition
            .artifacts
            .iter()
            .filter(|artifact| artifact.kind == FederationBarkArtifactKind::ArkResult)
            .all(|artifact| artifact.hash != transition.result_hash)
    {
        return Err(invalid_state(
            "exit confirmation does not retain the published transaction package",
        ));
    }
    Ok(())
}

/// Check an exit transition's transaction artifacts against its plan.
pub fn validate_exit_transaction_artifacts(
    plan: &FederationBarkActionPlan,
    transition: &FederationBarkActionTransition,
    require_drain: bool,
) -> Result<()> {
    let planned = plan
        .artifacts
        .iter()
        .filter(|artifact| artifact.kind == FederationBarkArtifactKind::Transaction)
        .collect::<BTreeSet<_>>();
    let actual = transition
        .artifacts
        .iter()
        .filter(|artifact| artifact.kind == FederationBarkArtifactKind::Transaction)
        .collect::<BTreeSet<_>>();
    let result_count = transition
        .artifacts
        .iter()
        .filter(|artifact| {
            artifact.kind == FederationBarkArtifactKind::ArkResult
                && artifact.hash == transition.result_hash
        })
        .count();
    if planned.is_empty()
        || !planned.is_subset(&actual)
        || (require_drain && actual.len() <= planned.len())
        || result_count != 1
    {
        return Err(invalid_state(
            "exit evidence differs from its accepted transaction package",
        ));
    }
    Ok(())
}

/// Check that a receive-claim transition continues its accepted plan and custody attempt.
pub fn validate_receive_claim_transition_header(
    action: &FederationBarkActionRecord,
    plan_operation_id: FederationOperationId,
    plan: &FederationBarkActionPlan,
    transition: &FederationBarkActionTransition,
) -> Result<()> {
    let expected_signers = action_custody_signers(action, plan)?;
    if plan.kind != FederationBarkActionKind::IncomingClaim
        || action.effective_phase() != FederationBarkSemanticPhase::ReceivePreimageDisclosed
        || action.plan_operation_id != Some(plan_operation_id)
        || action.action_id != plan.action_id
        || action.quote != plan.quote
        || action.payment_hash != plan.payment_hash
        || transition.phase != FederationBarkSemanticPhase::ReceiveClaimed
        || transition.plan_operation_id != plan_operation_id
        || transition.predecessor_operation_id != action.latest_operation_id
        || transition.action_id != action.action_id
        || transition.quote != action.quote
        || transition.payment_hash != action.payment_hash
        || transition.actor != action.actor
        || transition.attempt != action.attempt
        || transition.signer_set != expected_signers
    {
        return Err(invalid_state(
            "claim result differs from its accepted custody attempt",
        ));
    }
    Ok(())
}

/// Check that a Lightning-send transition continues its accepted plan and custody attempt.
pub fn validate_send_transition_header(
    action: &FederationBarkActionRecord,
    plan_operation_id: FederationOperationId,
    plan: &FederationBarkActionPlan,
    transition: &FederationBarkActionTransition,
) -> Result<()> {
    let expected_signers = action_custody_signers(action, plan)?;
    if plan.kind != FederationBarkActionKind::LightningSend
        || transition.action_id != plan.action_id
        || transition.quote != plan.quote
        || transition.plan_operation_id != plan_operation_id
        || transition.payment_hash != plan.payment_hash
        || transition.actor != plan.actor
        || transition.attempt != action.attempt
        || transition.signer_set != expected_signers
        || transition.external_id != plan.external_id
        || action.action_id != plan.action_id
        || action.quote != plan.quote
        || action.payment_hash != plan.payment_hash
    {
        return Err(invalid_state(
            "outgoing transition differs from its accepted send bindings",
        ));
    }
    Ok(())
}

/// Signer set bound to the action's current custody attempt.
pub fn action_custody_signers<'a>(
    action: &'a FederationBarkActionRecord,
    plan: &'a FederationBarkActionPlan,
) -> Result<&'a [MemberId]> {
    match &action.operation {
        FederationBarkOperation::ActionTransition(transition)
            if transition.attempt == action.attempt && !transition.signer_set.is_empty() =>
        {
            Ok(&transition.signer_set)
        }
        _ if action.attempt == plan.attempt => Ok(&plan.signer_set),
        _ => Err(invalid_state(
            "action custody attempt has no accepted signer binding",
        )),
    }
}

/// Artifact of the given kind whose hash commits to its bytes.
pub fn federation_bark_artifact(
    kind: FederationBarkArtifactKind,
    bytes: Vec<u8>,
) -> Result<FederationBarkArtifact> {
    Ok(FederationBarkArtifact {
        kind,
        hash: FederationBarkDigest::from_bytes(Sha256Hash::hash(&bytes).to_byte_array())?,
        bytes,
    })
}

/// Check that a revocation result returns every accepted HTLC input to custody.
pub fn validate_revocation_completed_result(
    vtxos: &[FederationBarkVtxoRecord],
    plan: &FederationBarkActionPlan,
    transition: &FederationBarkActionTransition,
) -> Result<()> {
    let expected_inputs = plan
        .inputs
        .iter()
        .map(|input| input.vtxo_id)
        .collect::<BTreeSet<_>>();
    let spent_inputs = transition
        .vtxo_transitions
        .iter()
        .filter(|mutation| {
            mutation.previous_state == Some(FederationBarkVtxoState::Reserved)
                && mutation.new_state == FederationBarkVtxoState::Spent
        })
        .map(|mutation| mutation.vtxo_id)
        .collect::<BTreeSet<_>>();
    if expected_inputs.is_empty() || spent_inputs != expected_inputs {
        return Err(invalid_state(
            "revocation result does not spend every accepted HTLC",
        ));
    }
    let mut expected_artifacts = Vec::new();
    for input in &plan.inputs {
        let record = vtxos
            .iter()
            .find(|record| record.vtxo_id == input.vtxo_id)
            .ok_or_else(|| invalid_state("reserved revocation input is missing"))?;
        let mutation = transition
            .vtxo_transitions
            .iter()
            .find(|mutation| mutation.vtxo_id == input.vtxo_id)
            .ok_or_else(|| invalid_state("revocation input mutation is missing"))?;
        if record.state != FederationBarkVtxoState::Reserved
            || record.reservation_action_id != Some(plan.action_id)
            || record.locator != input.locator
            || record.amount != input.amount
            || mutation.locator != input.locator
            || mutation.amount != input.amount
            || mutation.expiry_height != record.expiry_height
            || mutation.artifact_hash != record.artifact.hash
        {
            return Err(invalid_state(
                "revocation input differs from its accepted reservation",
            ));
        }
        expected_artifacts.push(record.artifact.clone());
    }
    let output = plan
        .outputs
        .first()
        .ok_or_else(|| invalid_state("revocation output allocation is missing"))?;
    let outputs = transition
        .vtxo_transitions
        .iter()
        .filter(|mutation| {
            mutation.previous_state.is_none()
                && mutation.new_state == FederationBarkVtxoState::Spendable
        })
        .collect::<Vec<_>>();
    if outputs.is_empty() || transition.vtxo_transitions.len() != plan.inputs.len() + outputs.len()
    {
        return Err(invalid_state(
            "revocation result has incomplete or unexpected VTXO mutations",
        ));
    }
    let mut recovered = Amount::ZERO;
    let mut result_bytes = b"cdk/bark/send-revocation-result/v1".to_vec();
    result_bytes.extend_from_slice(&plan.action_id.to_bytes());
    result_bytes.extend_from_slice(
        &FederationBarkOperation::ActionPlan(plan.clone())
            .commitment()?
            .to_bytes(),
    );
    result_bytes.extend_from_slice(
        &plan
            .payment_hash
            .ok_or_else(|| invalid_state("revocation payment hash is missing"))?
            .to_bytes(),
    );
    result_bytes.extend_from_slice(
        &u32::try_from(plan.inputs.len())
            .map_err(|_| invalid_state("too many revocation inputs"))?
            .to_be_bytes(),
    );
    for input in &plan.inputs {
        result_bytes.extend_from_slice(&input.vtxo_id.to_bytes());
    }
    result_bytes.extend_from_slice(
        &u32::try_from(outputs.len())
            .map_err(|_| invalid_state("too many revocation outputs"))?
            .to_be_bytes(),
    );
    for mutation in outputs {
        if mutation.locator != output.locator {
            return Err(invalid_state(
                "revocation result uses an unallocated output locator",
            ));
        }
        let mut artifacts = transition.artifacts.iter().filter(|artifact| {
            artifact.kind == FederationBarkArtifactKind::Vtxo
                && artifact.hash == mutation.artifact_hash
        });
        let artifact = artifacts
            .next()
            .ok_or_else(|| invalid_state("revocation output artifact is missing"))?;
        if artifacts.next().is_some() {
            return Err(invalid_state("revocation output artifact is ambiguous"));
        }
        result_bytes.extend_from_slice(
            &u64::try_from(artifact.bytes.len())
                .map_err(|_| invalid_state("revocation output artifact is oversized"))?
                .to_be_bytes(),
        );
        result_bytes.extend_from_slice(&artifact.bytes);
        recovered = recovered
            .checked_add(mutation.amount)
            .ok_or_else(|| invalid_state("revocation output amount overflow"))?;
        expected_artifacts.push(artifact.clone());
    }
    if recovered != output.amount {
        return Err(invalid_state(
            "revocation result does not conserve the accepted HTLC value",
        ));
    }
    let result = federation_bark_artifact(FederationBarkArtifactKind::ArkResult, result_bytes)?;
    if result.hash != transition.result_hash {
        return Err(invalid_state(
            "revocation result commitment differs from its exact outputs",
        ));
    }
    expected_artifacts.push(result);
    expected_artifacts.sort();
    expected_artifacts.dedup();
    if transition.artifacts != expected_artifacts {
        return Err(invalid_state(
            "revocation result artifacts differ from its exact recovery",
        ));
    }
    Ok(())
}
