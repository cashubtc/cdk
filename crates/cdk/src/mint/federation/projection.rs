//! Deterministic materialized-state projection for federation convergence.

use std::collections::{BTreeSet, HashMap};
use std::str::FromStr;

use bitcoin::hashes::{sha256, Hash};
use cdk_common::error::Error;
use cdk_common::federation::{
    FederatedKeyset, FederationMaterializedStateKey, FederationMaterializedStateRow,
    FederationMaterializedStateSnapshot, FederationOperationId, FederationPaymentObservationRecord,
};
#[cfg(feature = "federation-bark")]
use cdk_common::federation::{
    FederationAddressAllocationScope, FederationAddressKeychain, FederationOperation,
    FederationOperationStatus, FederationPaymentObservationContext,
    FEDERATION_MAX_DERIVATION_INDEX,
};
#[cfg(feature = "federation-bark")]
use cdk_common::mint::MeltPaymentRequest;
use cdk_common::mint::Operation;
use cdk_common::{Amount, PublicKey};
#[cfg(feature = "federation-bark")]
use cdk_federation_bark::{
    ark_server_commitment, federation_bark_application, federation_bark_application_id,
    federation_bark_cpfp_allocation_quote, federation_bark_exit_allocation_quote,
    federation_bark_next_actor, federation_bark_recovery_application,
    federation_bark_signer_set_for_attempt, validate_federation_bark_operation,
    FederationBarkActionKind, FederationBarkActionPlan, FederationBarkActionRecord,
    FederationBarkActionTransition, FederationBarkArtifact, FederationBarkArtifactKind,
    FederationBarkCpfpPackageSet, FederationBarkCpfpPublication, FederationBarkDigest,
    FederationBarkEvidencePhase, FederationBarkObservationStatus, FederationBarkOperation,
    FederationBarkProjectionSnapshot, FederationBarkReceiveIntent, FederationBarkRecoveryState,
    FederationBarkSemanticPhase, FederationBarkVtxoInput, FederationBarkVtxoLocator,
    FederationBarkVtxoState, FEDERATION_BARK_EXIT_FEE_RATE_SAT_PER_KWU,
};
use tracing::instrument;
use uuid::Uuid;

use crate::mint::{Mint, QuoteId};

#[cfg(feature = "federation-bark")]
use super::{
    CHAIN_OBSERVATION_PRIMARY_NAMESPACE, FEDERATION_BARK_RECOVERY_SECONDARY_NAMESPACE,
    FEDERATION_BARK_RECOVERY_STATE_KEY,
};

#[cfg(feature = "federation-bark")]
pub(super) use self::bark_validation::*;
use super::federation_operation_uuid;

const MATERIALIZED_STATE_QUERY_BATCH_SIZE: usize = 500;

#[cfg(feature = "federation-bark")]
pub(super) fn bark_operation(
    operation: &FederationOperation,
) -> Result<FederationBarkOperation, Error> {
    validate_federation_bark_operation(operation).map_err(Error::from)
}

#[cfg(feature = "federation-bark")]
impl Mint {
    /// Load the canonical Bark action/VTXO projection for route orchestration and workers.
    #[instrument(skip_all)]
    pub async fn federation_bark_projection_snapshot(
        &self,
    ) -> Result<FederationBarkProjectionSnapshot, Error> {
        let application_id = federation_bark_application_id()?;
        let snapshot = self
            .localstore()
            .get_federation_application_projection_snapshot(application_id)
            .await?;
        serde_json::from_value(snapshot).map_err(|_| Error::Internal)
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
        let FederationBarkOperation::ReceiveIntent(intent) =
            bark_operation(&entry.envelope.operation)?
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
        accepted_operation_index: Option<u64>,
    ) -> Result<(), Error> {
        operation.validate()?;
        let config = self
            .effective_federation_config()
            .await?
            .ok_or(Error::Internal)?;
        let application =
            federation_bark_application(config.public_config())?.ok_or(Error::Internal)?;
        let snapshot = self.federation_bark_projection_snapshot().await?;
        let action = snapshot
            .actions
            .iter()
            .find(|action| action.action_id == operation.action_id());

        if let FederationBarkOperation::ActionPlan(plan) = operation {
            let expected =
                federation_bark_signer_set_for_attempt(config.public_config(), plan.attempt)?;
            if plan.signer_set != expected {
                return Err(bark_state_error(
                    "action plan signer set is not the deterministic custody roster",
                ));
            }
        }

        if let FederationBarkOperation::ActionTransition(transition) = operation {
            if transition.phase == FederationBarkSemanticPhase::SigningAttemptAdvanced {
                let action = action
                    .ok_or_else(|| bark_state_error("signing retry has no accepted action"))?;
                let retry_phase = action.signing_retry_phase.unwrap_or(action.phase);
                if !matches!(
                    retry_phase,
                    FederationBarkSemanticPhase::ClaimPlanned
                        | FederationBarkSemanticPhase::ReceivePreimageDisclosed
                        | FederationBarkSemanticPhase::SendPlanned
                        | FederationBarkSemanticPhase::RevocationPlanned
                        | FederationBarkSemanticPhase::RefreshPlanned
                        | FederationBarkSemanticPhase::ExitPlanned
                        | FederationBarkSemanticPhase::ExitRegistered
                        | FederationBarkSemanticPhase::ExitPublished
                ) {
                    return Err(bark_state_error(
                        "signing retry does not follow a custody phase",
                    ));
                }
                let plan_operation_id = action
                    .plan_operation_id
                    .ok_or_else(|| bark_state_error("signing retry has no accepted plan"))?;
                let plan_entry = self
                    .localstore()
                    .get_federation_journal_entry_by_operation_id(plan_operation_id)
                    .await?
                    .ok_or_else(|| bark_state_error("signing retry plan is missing"))?;
                let FederationBarkOperation::ActionPlan(plan) =
                    bark_operation(&plan_entry.envelope.operation)?
                else {
                    return Err(bark_state_error("signing retry references a non-plan"));
                };
                let attempt = action
                    .attempt
                    .checked_add(1)
                    .ok_or_else(|| bark_state_error("signing retry attempt overflow"))?;
                let signer_set =
                    federation_bark_signer_set_for_attempt(config.public_config(), attempt)?;
                let fallback_actor =
                    federation_bark_next_actor(config.public_config(), action.actor)?;
                if transition.actor != action.actor && transition.actor != fallback_actor {
                    return Err(bark_state_error(
                        "signing retry actor is not the current or next canonical actor",
                    ));
                }
                let expected = FederationBarkActionTransition::signing_attempt_advanced(
                    plan_operation_id,
                    action.latest_operation_id,
                    &plan,
                    transition.actor,
                    attempt,
                    signer_set,
                )?;
                if transition != &expected {
                    return Err(bark_state_error(
                        "signing retry differs from the deterministic next attempt",
                    ));
                }
            }
        }

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
                            let FederationBarkOperation::ActionPlan(plan) =
                                bark_operation(&plan_entry.envelope.operation)?
                            else {
                                return Err(bark_state_error(
                                    "payment success references a non-plan",
                                ));
                            };
                            validate_payment_success_proof(&plan, transition)?;
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
                        FederationBarkSemanticPhase::ExitPublished => {
                            let plan_entry = self
                                .localstore()
                                .get_federation_journal_entry_by_operation_id(
                                    transition.plan_operation_id,
                                )
                                .await?
                                .ok_or_else(|| bark_state_error("exit plan is missing"))?;
                            let FederationBarkOperation::ActionPlan(plan) =
                                bark_operation(&plan_entry.envelope.operation)?
                            else {
                                return Err(bark_state_error(
                                    "exit publication references a non-plan",
                                ));
                            };
                            if plan.kind != FederationBarkActionKind::EmergencyExit
                                || observation.amount != plan.external_amount
                                || observation.fee != plan.fee_amount
                            {
                                return Err(bark_state_error(
                                    "exit confirmation observation differs from its accepted plan",
                                ));
                            }
                        }
                        _ => {
                            return Err(bark_state_error(
                                "successful observation follows a non-terminal action phase",
                            ));
                        }
                    }
                } else if observation.status == FederationBarkObservationStatus::Failed {
                    let FederationBarkOperation::ActionTransition(transition) = &action.operation
                    else {
                        return Err(bark_state_error(
                            "failed observation does not follow an action result",
                        ));
                    };
                    let (amount, fee, expected_result_hash) = match action.phase {
                        FederationBarkSemanticPhase::SendHtlcsCosigned => {
                            let plan_entry = self
                                .localstore()
                                .get_federation_journal_entry_by_operation_id(
                                    transition.plan_operation_id,
                                )
                                .await?
                                .ok_or_else(|| bark_state_error("send plan is missing"))?;
                            let FederationBarkOperation::ActionPlan(plan) =
                                bark_operation(&plan_entry.envelope.operation)?
                            else {
                                return Err(bark_state_error(
                                    "outgoing HTLC result references a non-plan",
                                ));
                            };
                            if plan.kind != FederationBarkActionKind::LightningSend {
                                return Err(bark_state_error(
                                    "outgoing HTLC result references a non-send plan",
                                ));
                            }
                            (
                                plan.external_amount,
                                plan.fee_amount,
                                payment_failed_result_hash(&plan, transition.result_hash)?,
                            )
                        }
                        FederationBarkSemanticPhase::RevocationCompleted => {
                            let revocation_entry = self
                                .localstore()
                                .get_federation_journal_entry_by_operation_id(
                                    transition.plan_operation_id,
                                )
                                .await?
                                .ok_or_else(|| bark_state_error("revocation plan is missing"))?;
                            let FederationBarkOperation::ActionPlan(revocation) =
                                bark_operation(&revocation_entry.envelope.operation)?
                            else {
                                return Err(bark_state_error(
                                    "revocation result references a non-plan",
                                ));
                            };
                            let failed_entry = self
                                .localstore()
                                .get_federation_journal_entry_by_operation_id(
                                    revocation.predecessor_operation_id,
                                )
                                .await?
                                .ok_or_else(|| bark_state_error("payment failure is missing"))?;
                            let FederationBarkOperation::ActionTransition(failed) =
                                bark_operation(&failed_entry.envelope.operation)?
                            else {
                                return Err(bark_state_error(
                                    "revocation predecessor is not a payment failure",
                                ));
                            };
                            let send_entry = self
                                .localstore()
                                .get_federation_journal_entry_by_operation_id(
                                    failed.plan_operation_id,
                                )
                                .await?
                                .ok_or_else(|| bark_state_error("send plan is missing"))?;
                            let FederationBarkOperation::ActionPlan(send) =
                                bark_operation(&send_entry.envelope.operation)?
                            else {
                                return Err(bark_state_error(
                                    "payment failure references a non-plan",
                                ));
                            };
                            if send.kind != FederationBarkActionKind::LightningSend {
                                return Err(bark_state_error(
                                    "payment failure references a non-send plan",
                                ));
                            }
                            (
                                send.external_amount,
                                send.fee_amount,
                                transition.result_hash,
                            )
                        }
                        _ => {
                            return Err(bark_state_error(
                                "failed observation follows an unsafe action phase",
                            ));
                        }
                    };
                    if observation.result_hash != expected_result_hash
                        || observation.amount != amount
                        || observation.fee != fee
                    {
                        return Err(bark_state_error(
                            "failed observation differs from its accepted recovery phase",
                        ));
                    }
                } else if observation.status == FederationBarkObservationStatus::Accepted
                    && matches!(
                        action.phase,
                        FederationBarkSemanticPhase::RefreshPlanned
                            | FederationBarkSemanticPhase::ExitPlanned
                    )
                {
                    let FederationBarkOperation::ActionPlan(plan) = &action.operation else {
                        return Err(bark_state_error(
                            "maintenance trigger observation has no accepted plan",
                        ));
                    };
                    let amount = if plan.kind == FederationBarkActionKind::Refresh {
                        plan.inputs.iter().try_fold(Amount::ZERO, |sum, input| {
                            sum.checked_add(input.amount).ok_or_else(|| {
                                bark_state_error("maintenance observation amount overflow")
                            })
                        })?
                    } else {
                        plan.external_amount
                    };
                    if !matches!(
                        plan.kind,
                        FederationBarkActionKind::Refresh | FederationBarkActionKind::EmergencyExit
                    ) || observation.amount != amount
                        || observation.fee != plan.fee_amount
                        || observation.result_hash
                            != FederationBarkOperation::ActionPlan(plan.clone()).commitment()?
                    {
                        return Err(bark_state_error(
                            "maintenance trigger observation differs from its accepted plan",
                        ));
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
                let required = config.public_config().threshold_params.signature_threshold;
                match &transition.evidence_certificate {
                    Some(certificate) => {
                        certificate.verify(config.public_config(), required)?;
                        let binding = &certificate.binding;
                        if binding.phase != FederationBarkEvidencePhase::ReceiveHtlcsReady
                            || binding.action_id != transition.action_id
                            || transition.actor != action.actor
                            || transition.quote.as_deref() != Some(binding.quote.as_str())
                            || binding.evidence_operation_id != transition.predecessor_operation_id
                            || binding.payment_hash
                                != transition.payment_hash.ok_or_else(|| {
                                    bark_state_error("HTLC certificate has no payment hash")
                                })?
                            || binding.source_scope
                                != ark_server_commitment(&application.ark_server_public_key)?
                            || binding.amount != amount
                            || binding.fee != fee
                            || binding.evidence_digest != transition.result_hash
                            || binding.attempt != transition.attempt
                        {
                            return Err(bark_state_error(
                                "HTLC certificate differs from the accepted transition",
                            ));
                        }
                    }
                    None => {
                        let mut observers = BTreeSet::new();
                        for record in &snapshot.observations {
                            let FederationBarkOperation::MemberObservation(observation) =
                                &record.operation
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
                        if observers.len() < usize::from(required) {
                            return Err(bark_state_error(
                                "HTLC evidence has not reached the accepted member threshold",
                            ));
                        }
                    }
                }
            }
            FederationBarkOperation::EvidenceCertificate(certificate) => {
                let required = config
                    .public_config()
                    .threshold_params
                    .signature_threshold
                    .max(
                        config
                            .public_config()
                            .payment_verification_policy(
                                FederationPaymentObservationContext::MintQuote,
                            )
                            .required_matching_observations(),
                    );
                certificate.verify(config.public_config(), required)?;
                let action =
                    action.ok_or_else(|| bark_state_error("evidence certificate has no action"))?;
                let FederationBarkOperation::ActionTransition(transition) = &action.operation
                else {
                    return Err(bark_state_error(
                        "result certificate does not follow an action result",
                    ));
                };
                let binding = &certificate.binding;
                let amount = transition
                    .vtxo_transitions
                    .iter()
                    .filter(|mutation| {
                        mutation.previous_state.is_none()
                            && mutation.new_state == FederationBarkVtxoState::Spendable
                    })
                    .try_fold(Amount::ZERO, |sum, mutation| {
                        sum.checked_add(mutation.amount)
                            .ok_or_else(|| bark_state_error("claim certificate amount overflow"))
                    })?;
                if binding.phase != FederationBarkEvidencePhase::ReceiveClaimed
                    || action.phase != FederationBarkSemanticPhase::ReceiveClaimed
                    || binding.action_id != action.action_id
                    || action.quote.as_deref() != Some(binding.quote.as_str())
                    || binding.evidence_operation_id != action.latest_operation_id
                    || binding.payment_hash
                        != action.payment_hash.ok_or_else(|| {
                            bark_state_error("claim certificate has no payment hash")
                        })?
                    || binding.source_scope
                        != ark_server_commitment(&application.ark_server_public_key)?
                    || binding.amount != amount
                    || binding.fee != Amount::ZERO
                    || binding.evidence_digest != transition.result_hash
                    || binding.attempt != transition.attempt
                {
                    return Err(bark_state_error(
                        "claim certificate differs from the accepted result",
                    ));
                }
            }
            FederationBarkOperation::ActionPlan(plan)
                if plan.kind == FederationBarkActionKind::Refresh =>
            {
                self.validate_federation_bark_refresh_plan_submission(
                    &config,
                    &application,
                    &snapshot,
                    action,
                    plan,
                )?;
            }
            FederationBarkOperation::ActionPlan(plan)
                if plan.kind == FederationBarkActionKind::EmergencyExit =>
            {
                let allocations = self.federation_address_allocation_state().await?;
                self.validate_federation_bark_exit_plan_submission(
                    &config,
                    &application,
                    &snapshot,
                    action,
                    plan,
                    &allocations,
                )
                .await?;
            }
            FederationBarkOperation::ActionPlan(plan)
                if plan.kind == FederationBarkActionKind::IncomingClaim =>
            {
                let action = action.ok_or_else(|| bark_state_error("claim plan has no action"))?;
                let intent = self
                    .federation_bark_receive_intent(action.origin_operation_id)
                    .await?
                    .ok_or_else(|| bark_state_error("receive intent is missing"))?;
                let combined = plan.certified_receive_htlcs.as_deref();
                let expected_phase = if combined.is_some() {
                    FederationBarkSemanticPhase::ReceivePrepared
                } else {
                    FederationBarkSemanticPhase::ReceiveHtlcsReady
                };
                if action.phase != expected_phase
                    || plan.predecessor_operation_id != action.latest_operation_id
                    || plan.actor != action.actor
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
                if let Some(certified) = combined {
                    let amount = certified.vtxo_transitions.iter().try_fold(
                        Amount::ZERO,
                        |sum, mutation| {
                            sum.checked_add(mutation.amount).ok_or_else(|| {
                                bark_state_error("certified HTLC evidence amount overflow")
                            })
                        },
                    )?;
                    let fee = intent.gross_amount.checked_sub(amount).ok_or_else(|| {
                        bark_state_error("certified HTLC evidence exceeds invoice amount")
                    })?;
                    if amount < intent.minimum_net_amount || fee > intent.receive_fee {
                        return Err(bark_state_error(
                            "certified HTLC evidence does not cover the accepted Cashu liability",
                        ));
                    }
                    let certificate = &certified.certificate;
                    certificate.verify(
                        config.public_config(),
                        config.public_config().threshold_params.signature_threshold,
                    )?;
                    let binding = &certificate.binding;
                    if binding.phase != FederationBarkEvidencePhase::ReceiveHtlcsReady
                        || binding.action_id != plan.action_id
                        || plan.quote.as_deref() != Some(binding.quote.as_str())
                        || binding.evidence_operation_id != plan.predecessor_operation_id
                        || Some(binding.payment_hash) != plan.payment_hash
                        || binding.source_scope
                            != ark_server_commitment(&application.ark_server_public_key)?
                        || binding.amount != amount
                        || binding.fee != fee
                        || binding.status != FederationBarkObservationStatus::Accepted
                        || binding.attempt != plan.attempt
                    {
                        return Err(bark_state_error(
                            "certified HTLC evidence differs from the claim plan",
                        ));
                    }
                } else {
                    for input in &plan.inputs {
                        let vtxo = snapshot
                            .vtxos
                            .iter()
                            .find(|vtxo| vtxo.vtxo_id == input.vtxo_id)
                            .ok_or_else(|| bark_state_error("claim plan input is unknown"))?;
                        if vtxo.state != FederationBarkVtxoState::Observed
                            || vtxo.amount != input.amount
                            || vtxo.locator != input.locator
                            || vtxo.locator != intent.destination
                        {
                            return Err(bark_state_error(
                                "claim plan input differs from accepted HTLC evidence",
                            ));
                        }
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
                let expected_action = cdk_federation_bark::FederationBarkActionId::derive(
                    config.federation_id(),
                    plan.predecessor_operation_id,
                    FederationBarkSemanticPhase::SendPlanned,
                    0,
                )?;
                let author = predecessor
                    .envelope
                    .verify_authorization(config.public_config())?;
                let expected_actor = match accepted_operation_index {
                    Some(operation_index) => Self::federation_external_action_lease_actor_at_index(
                        &config,
                        author,
                        predecessor.index,
                        operation_index,
                    )?,
                    None => {
                        self.federation_external_action_lease_actor(
                            &config,
                            author,
                            predecessor.index,
                        )
                        .await?
                    }
                };
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
                let plan_expiry = u32::try_from(plan.expires_at)
                    .map_err(|_| bark_state_error("Lightning send expiry exceeds u32"))?;
                cdk_federation_bark::validate_federation_bark_lightning_send_expiry(
                    minimum_expiry,
                    plan_expiry,
                )?;
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
                {
                    return Err(bark_state_error(
                        "Lightning send change or locators are not deterministic",
                    ));
                }
            }
            FederationBarkOperation::ActionPlan(plan)
                if plan.kind == FederationBarkActionKind::FailedSendRevocation =>
            {
                let action =
                    action.ok_or_else(|| bark_state_error("revocation plan has no action"))?;
                if action.phase != FederationBarkSemanticPhase::PaymentFailed
                    || plan.predecessor_operation_id != action.latest_operation_id
                {
                    return Err(bark_state_error(
                        "revocation plan does not follow terminal payment failure",
                    ));
                }
                let FederationBarkOperation::ActionTransition(failed) = &action.operation else {
                    return Err(bark_state_error(
                        "revocation plan predecessor is not a payment failure",
                    ));
                };
                let send_entry = self
                    .localstore()
                    .get_federation_journal_entry_by_operation_id(failed.plan_operation_id)
                    .await?
                    .ok_or_else(|| bark_state_error("accepted send plan is missing"))?;
                let FederationBarkOperation::ActionPlan(send) =
                    bark_operation(&send_entry.envelope.operation)?
                else {
                    return Err(bark_state_error("payment failure references a non-plan"));
                };
                validate_payment_failed_result(&send, failed)?;
                let mut records = snapshot
                    .vtxos
                    .iter()
                    .filter(|record| {
                        record.state == FederationBarkVtxoState::PendingSpend
                            && record.reservation_action_id == Some(send.action_id)
                    })
                    .collect::<Vec<_>>();
                records.sort_by_key(|record| record.vtxo_id);
                let expected_inputs = records
                    .iter()
                    .map(|record| FederationBarkVtxoInput {
                        vtxo_id: record.vtxo_id,
                        locator: record.locator,
                        amount: record.amount,
                    })
                    .collect::<Vec<_>>();
                let expected_amount =
                    expected_inputs
                        .iter()
                        .try_fold(Amount::ZERO, |sum, input| {
                            sum.checked_add(input.amount)
                                .ok_or_else(|| bark_state_error("revocation input amount overflow"))
                        })?;
                let output = plan
                    .outputs
                    .first()
                    .ok_or_else(|| bark_state_error("revocation output is missing"))?;
                let expected_locator = send
                    .revocation_locator
                    .ok_or_else(|| bark_state_error("send revocation locator is missing"))?;
                let request_bytes = failed_send_revocation_request_bytes(
                    plan.action_id,
                    plan.predecessor_operation_id,
                    plan.payment_hash
                        .ok_or_else(|| bark_state_error("revocation payment hash is missing"))?,
                    output.locator,
                    &plan.inputs,
                )?;
                let request_artifact = bark_projection_artifact(
                    FederationBarkArtifactKind::ArkRequest,
                    request_bytes,
                )?;
                let mut expected_artifacts = records
                    .iter()
                    .map(|record| record.artifact.clone())
                    .collect::<Vec<_>>();
                expected_artifacts.push(request_artifact.clone());
                expected_artifacts.sort();
                expected_artifacts.dedup();
                if send.kind != FederationBarkActionKind::LightningSend
                    || plan.action_id != send.action_id
                    || plan.quote != send.quote
                    || plan.actor != send.actor
                    || plan.payment_hash != send.payment_hash
                    || plan.signer_set != failed.signer_set
                    || plan.attempt != failed.attempt
                    || plan.expires_at != send.expires_at
                    || plan.inputs != expected_inputs
                    || expected_inputs.is_empty()
                    || expected_amount
                        != send
                            .external_amount
                            .checked_add(send.fee_amount)
                            .ok_or_else(|| bark_state_error("revocation amount overflow"))?
                    || output.locator != expected_locator
                    || output.amount != expected_amount
                    || output.artifact_hash != request_artifact.hash
                    || plan.artifacts != expected_artifacts
                {
                    return Err(bark_state_error(
                        "revocation plan differs from the exact failed HTLC recovery",
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
                let FederationBarkOperation::ActionPlan(plan) =
                    bark_operation(&plan_entry.envelope.operation)?
                else {
                    return Err(bark_state_error(
                        "outgoing HTLC result references a non-plan",
                    ));
                };
                validate_send_transition_header(
                    action,
                    plan_entry.operation_id,
                    &plan,
                    transition,
                )?;
                if action_effective_phase(action) != FederationBarkSemanticPhase::SendPlanned
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
                let spent_inputs = transition
                    .vtxo_transitions
                    .iter()
                    .filter(|mutation| {
                        mutation.previous_state == Some(FederationBarkVtxoState::Reserved)
                            && mutation.new_state == FederationBarkVtxoState::Spent
                    })
                    .map(|mutation| mutation.vtxo_id)
                    .collect::<BTreeSet<_>>();
                if spent_inputs != expected_inputs {
                    return Err(bark_state_error(
                        "outgoing HTLC result does not spend every accepted input",
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
                    .filter(|mutation| {
                        mutation.previous_state.is_none()
                            && mutation.new_state == FederationBarkVtxoState::Spendable
                    })
                    .try_fold(Amount::ZERO, |sum, mutation| {
                        if mutation.locator != output.locator {
                            return Err(bark_state_error(
                                "outgoing result uses an unallocated change locator",
                            ));
                        }
                        sum.checked_add(mutation.amount)
                            .ok_or_else(|| bark_state_error("send change amount overflow"))
                    })?;
                let htlc_amount = transition
                    .vtxo_transitions
                    .iter()
                    .filter(|mutation| {
                        mutation.previous_state.is_none()
                            && mutation.new_state == FederationBarkVtxoState::PendingSpend
                    })
                    .try_fold(Amount::ZERO, |sum, mutation| {
                        if mutation.locator != output.locator {
                            return Err(bark_state_error(
                                "outgoing HTLC uses an unallocated locator",
                            ));
                        }
                        sum.checked_add(mutation.amount)
                            .ok_or_else(|| bark_state_error("send HTLC amount overflow"))
                    })?;
                let expected_htlc = plan
                    .external_amount
                    .checked_add(plan.fee_amount)
                    .ok_or_else(|| bark_state_error("send HTLC amount overflow"))?;
                if change_amount != output.amount
                    || htlc_amount != expected_htlc
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
                if transition.phase == FederationBarkSemanticPhase::PaymentSucceeded =>
            {
                let action =
                    action.ok_or_else(|| bark_state_error("payment success has no action"))?;
                let plan_entry = self
                    .localstore()
                    .get_federation_journal_entry_by_operation_id(transition.plan_operation_id)
                    .await?
                    .ok_or_else(|| bark_state_error("accepted send plan is missing"))?;
                let FederationBarkOperation::ActionPlan(plan) =
                    bark_operation(&plan_entry.envelope.operation)?
                else {
                    return Err(bark_state_error("payment success references a non-plan"));
                };
                validate_send_transition_header(
                    action,
                    plan_entry.operation_id,
                    &plan,
                    transition,
                )?;
                let FederationBarkOperation::ActionTransition(previous) = &action.operation else {
                    return Err(bark_state_error(
                        "payment success does not follow an HTLC result",
                    ));
                };
                if action.phase != FederationBarkSemanticPhase::SendHtlcsCosigned
                    || transition.predecessor_operation_id != action.latest_operation_id
                    || transition.request_hash != previous.result_hash
                {
                    return Err(bark_state_error(
                        "payment success differs from its accepted HTLC predecessor",
                    ));
                }
                validate_payment_success_proof(&plan, transition)?;
                let expected_inputs = snapshot
                    .vtxos
                    .iter()
                    .filter(|record| {
                        record.state == FederationBarkVtxoState::PendingSpend
                            && record.reservation_action_id == Some(plan.action_id)
                    })
                    .map(|record| record.vtxo_id)
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
                if expected_inputs.is_empty() {
                    return Err(bark_state_error(
                        "payment success has no pending HTLC value",
                    ));
                }
                let mut pending_amount = Amount::ZERO;
                for input in snapshot.vtxos.iter().filter(|record| {
                    record.state == FederationBarkVtxoState::PendingSpend
                        && record.reservation_action_id == Some(plan.action_id)
                }) {
                    let mutation = transition
                        .vtxo_transitions
                        .iter()
                        .find(|mutation| mutation.vtxo_id == input.vtxo_id)
                        .ok_or_else(|| bark_state_error("spent send mutation is missing"))?;
                    if mutation.locator != input.locator
                        || mutation.amount != input.amount
                        || mutation.expiry_height != input.expiry_height
                        || mutation.artifact_hash != input.artifact.hash
                    {
                        return Err(bark_state_error(
                            "payment success input differs from its pending spend",
                        ));
                    }
                    pending_amount = pending_amount
                        .checked_add(input.amount)
                        .ok_or_else(|| bark_state_error("payment success amount overflow"))?;
                }
                if pending_amount
                    != plan
                        .external_amount
                        .checked_add(plan.fee_amount)
                        .ok_or_else(|| bark_state_error("payment success amount overflow"))?
                {
                    return Err(bark_state_error(
                        "payment success does not consume the exact HTLC value",
                    ));
                }
                let certificate = transition.evidence_certificate.as_deref().ok_or_else(|| {
                    bark_state_error("payment success has no evidence certificate")
                })?;
                let binding = &certificate.binding;
                if binding.phase != FederationBarkEvidencePhase::SendSucceeded
                    || binding.action_id != transition.action_id
                    || binding.quote != plan.quote.as_deref().unwrap_or_default()
                    || binding.evidence_operation_id != action.latest_operation_id
                    || binding.payment_hash
                        != plan.payment_hash.ok_or_else(|| {
                            bark_state_error("payment success plan has no payment hash")
                        })?
                    || binding.source_scope
                        != ark_server_commitment(&application.ark_server_public_key)?
                    || binding.status != FederationBarkObservationStatus::Succeeded
                    || binding.amount != plan.external_amount
                    || binding.fee != plan.fee_amount
                    || binding.evidence_digest != transition.result_hash
                    || binding.attempt != transition.attempt
                {
                    return Err(bark_state_error(
                        "payment success certificate differs from its exact send result",
                    ));
                }
            }
            FederationBarkOperation::ActionTransition(transition)
                if transition.phase == FederationBarkSemanticPhase::PaymentFailed =>
            {
                let action =
                    action.ok_or_else(|| bark_state_error("payment failure has no action"))?;
                let plan_entry = self
                    .localstore()
                    .get_federation_journal_entry_by_operation_id(transition.plan_operation_id)
                    .await?
                    .ok_or_else(|| bark_state_error("accepted send plan is missing"))?;
                let FederationBarkOperation::ActionPlan(plan) =
                    bark_operation(&plan_entry.envelope.operation)?
                else {
                    return Err(bark_state_error("payment failure references a non-plan"));
                };
                validate_send_transition_header(
                    action,
                    plan_entry.operation_id,
                    &plan,
                    transition,
                )?;
                let FederationBarkOperation::ActionTransition(previous) = &action.operation else {
                    return Err(bark_state_error(
                        "payment failure does not follow an HTLC result",
                    ));
                };
                if action.phase != FederationBarkSemanticPhase::SendHtlcsCosigned
                    || transition.predecessor_operation_id != action.latest_operation_id
                    || transition.request_hash != previous.result_hash
                {
                    return Err(bark_state_error(
                        "payment failure differs from its accepted HTLC predecessor",
                    ));
                }
                validate_payment_failed_result(&plan, transition)?;
                let source_scope = ark_server_commitment(&application.ark_server_public_key)?;
                let mut observers = BTreeSet::new();
                for record in &snapshot.observations {
                    let FederationBarkOperation::MemberObservation(observation) = &record.operation
                    else {
                        continue;
                    };
                    if observation.action_id == transition.action_id
                        && observation.quote == transition.quote
                        && observation.payment_hash == transition.payment_hash
                        && observation.source_scope == source_scope
                        && observation.status == FederationBarkObservationStatus::Failed
                        && observation.amount == plan.external_amount
                        && observation.fee == plan.fee_amount
                        && observation.result_hash == transition.result_hash
                    {
                        observers.insert(observation.observer);
                    }
                }
                if observers.len()
                    < usize::from(config.public_config().threshold_params.signature_threshold)
                {
                    return Err(bark_state_error(
                        "payment failure has not reached the accepted member threshold",
                    ));
                }
            }
            FederationBarkOperation::ActionTransition(transition)
                if transition.phase == FederationBarkSemanticPhase::RevocationCompleted =>
            {
                let action =
                    action.ok_or_else(|| bark_state_error("revocation result has no action"))?;
                let plan_entry = self
                    .localstore()
                    .get_federation_journal_entry_by_operation_id(transition.plan_operation_id)
                    .await?
                    .ok_or_else(|| bark_state_error("accepted revocation plan is missing"))?;
                let FederationBarkOperation::ActionPlan(plan) =
                    bark_operation(&plan_entry.envelope.operation)?
                else {
                    return Err(bark_state_error("revocation result references a non-plan"));
                };
                let expected_signers = action_custody_signers(action, &plan)?;
                if plan.kind != FederationBarkActionKind::FailedSendRevocation
                    || action_effective_phase(action)
                        != FederationBarkSemanticPhase::RevocationPlanned
                    || transition.action_id != plan.action_id
                    || transition.quote != plan.quote
                    || transition.plan_operation_id != plan_entry.operation_id
                    || transition.predecessor_operation_id != action.latest_operation_id
                    || transition.payment_hash != plan.payment_hash
                    || transition.actor != plan.actor
                    || transition.attempt != action.attempt
                    || transition.signer_set != expected_signers
                    || transition.external_id != plan.external_id
                    || transition.request_hash
                        != FederationBarkOperation::ActionPlan(plan.clone()).commitment()?
                {
                    return Err(bark_state_error(
                        "revocation result differs from its accepted plan",
                    ));
                }
                validate_revocation_completed_result(&snapshot, &plan, transition)?;
            }
            FederationBarkOperation::ActionTransition(transition)
                if transition.phase == FederationBarkSemanticPhase::RefreshCompleted =>
            {
                let action =
                    action.ok_or_else(|| bark_state_error("refresh result has no action"))?;
                let plan_entry = self
                    .localstore()
                    .get_federation_journal_entry_by_operation_id(transition.plan_operation_id)
                    .await?
                    .ok_or_else(|| bark_state_error("accepted refresh plan is missing"))?;
                let FederationBarkOperation::ActionPlan(plan) =
                    bark_operation(&plan_entry.envelope.operation)?
                else {
                    return Err(bark_state_error("refresh result references a non-plan"));
                };
                validate_maintenance_transition_header(
                    action,
                    plan_entry.operation_id,
                    &plan,
                    transition,
                    FederationBarkActionKind::Refresh,
                )?;
                if action_effective_phase(action) != FederationBarkSemanticPhase::RefreshPlanned
                    || transition.predecessor_operation_id != action.latest_operation_id
                {
                    return Err(bark_state_error(
                        "refresh result does not follow its accepted plan",
                    ));
                }
                validate_maintenance_trigger_quorum(
                    &snapshot,
                    config.public_config().threshold_params.signature_threshold,
                    &plan,
                )?;
                validate_refresh_completed_result(&snapshot, &plan, transition)?;
            }
            FederationBarkOperation::ActionTransition(transition)
                if transition.phase == FederationBarkSemanticPhase::ExitRegistered =>
            {
                let action =
                    action.ok_or_else(|| bark_state_error("exit registration has no action"))?;
                let plan_entry = self
                    .localstore()
                    .get_federation_journal_entry_by_operation_id(transition.plan_operation_id)
                    .await?
                    .ok_or_else(|| bark_state_error("accepted exit plan is missing"))?;
                let FederationBarkOperation::ActionPlan(plan) =
                    bark_operation(&plan_entry.envelope.operation)?
                else {
                    return Err(bark_state_error("exit registration references a non-plan"));
                };
                validate_maintenance_transition_header(
                    action,
                    plan_entry.operation_id,
                    &plan,
                    transition,
                    FederationBarkActionKind::EmergencyExit,
                )?;
                if action.phase != FederationBarkSemanticPhase::ExitPlanned
                    || transition.predecessor_operation_id != action.latest_operation_id
                {
                    return Err(bark_state_error(
                        "exit registration does not follow its accepted plan",
                    ));
                }
                validate_maintenance_trigger_quorum(
                    &snapshot,
                    config.public_config().threshold_params.signature_threshold,
                    &plan,
                )?;
                validate_exit_registered_result(&snapshot, &plan, transition)?;
                validate_bark_recovery_package_set(
                    self,
                    &config,
                    plan_entry.operation_id,
                    &plan,
                    transition,
                )
                .await?;
            }
            FederationBarkOperation::ActionTransition(transition)
                if transition.phase == FederationBarkSemanticPhase::ExitPublished =>
            {
                let action =
                    action.ok_or_else(|| bark_state_error("exit publication has no action"))?;
                let plan_entry = self
                    .localstore()
                    .get_federation_journal_entry_by_operation_id(transition.plan_operation_id)
                    .await?
                    .ok_or_else(|| bark_state_error("accepted exit plan is missing"))?;
                let FederationBarkOperation::ActionPlan(plan) =
                    bark_operation(&plan_entry.envelope.operation)?
                else {
                    return Err(bark_state_error("exit publication references a non-plan"));
                };
                validate_maintenance_transition_header(
                    action,
                    plan_entry.operation_id,
                    &plan,
                    transition,
                    FederationBarkActionKind::EmergencyExit,
                )?;
                if action_effective_phase(action) != FederationBarkSemanticPhase::ExitRegistered
                    || transition.predecessor_operation_id != action.latest_operation_id
                {
                    return Err(bark_state_error(
                        "exit publication does not follow accepted registration",
                    ));
                }
                validate_exit_published_result(&plan, transition)?;
                validate_bark_recovery_publication(self, &snapshot, transition).await?;
            }
            FederationBarkOperation::ActionTransition(transition)
                if transition.phase == FederationBarkSemanticPhase::ExitConfirmed =>
            {
                let action =
                    action.ok_or_else(|| bark_state_error("exit confirmation has no action"))?;
                let plan_entry = self
                    .localstore()
                    .get_federation_journal_entry_by_operation_id(transition.plan_operation_id)
                    .await?
                    .ok_or_else(|| bark_state_error("accepted exit plan is missing"))?;
                let FederationBarkOperation::ActionPlan(plan) =
                    bark_operation(&plan_entry.envelope.operation)?
                else {
                    return Err(bark_state_error("exit confirmation references a non-plan"));
                };
                validate_maintenance_transition_header(
                    action,
                    plan_entry.operation_id,
                    &plan,
                    transition,
                    FederationBarkActionKind::EmergencyExit,
                )?;
                if action_effective_phase(action) != FederationBarkSemanticPhase::ExitPublished
                    || transition.predecessor_operation_id != action.latest_operation_id
                {
                    return Err(bark_state_error(
                        "exit confirmation does not follow accepted publication",
                    ));
                }
                let source_scope = ark_server_commitment(&application.ark_server_public_key)?;
                let observers = snapshot
                    .observations
                    .iter()
                    .filter_map(|record| match &record.operation {
                        FederationBarkOperation::MemberObservation(observation)
                            if observation.action_id == plan.action_id
                                && observation.source_scope == source_scope
                                && observation.status
                                    == FederationBarkObservationStatus::Succeeded
                                && observation.amount == plan.external_amount
                                && observation.fee == plan.fee_amount
                                && observation.result_hash == transition.result_hash =>
                        {
                            Some(observation.observer)
                        }
                        _ => None,
                    })
                    .collect::<BTreeSet<_>>();
                if observers.len()
                    < usize::from(config.public_config().threshold_params.signature_threshold)
                {
                    return Err(bark_state_error(
                        "exit confirmation has not reached the accepted member threshold",
                    ));
                }
                let published = snapshot
                    .transitions
                    .iter()
                    .find_map(|record| match &record.operation {
                        FederationBarkOperation::ActionTransition(published)
                            if record.action_id == action.action_id
                                && published.phase
                                    == FederationBarkSemanticPhase::ExitPublished =>
                        {
                            Some(published)
                        }
                        _ => None,
                    })
                    .ok_or_else(|| {
                        bark_state_error("exit confirmation predecessor has no publication")
                    })?;
                validate_exit_confirmed_result(&snapshot, &plan, published, transition)?;
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
                let FederationBarkOperation::ActionPlan(plan) =
                    bark_operation(&plan_entry.envelope.operation)?
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
                let expected_signers = action_custody_signers(action, &plan)?;
                if action_effective_phase(action) != FederationBarkSemanticPhase::ClaimPlanned
                    || transition.predecessor_operation_id != action.latest_operation_id
                    || transition.action_id != plan.action_id
                    || transition.quote != plan.quote
                    || transition.plan_operation_id != plan_entry.operation_id
                    || transition.payment_hash != plan.payment_hash
                    || transition.actor != plan.actor
                    || transition.attempt != action.attempt
                    || transition.signer_set != expected_signers
                    || transition.request_hash
                        != FederationBarkOperation::ActionPlan(plan.clone()).commitment()?
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
                let FederationBarkOperation::ActionPlan(plan) =
                    bark_operation(&plan_entry.envelope.operation)?
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

    fn validate_federation_bark_refresh_plan_submission(
        &self,
        config: &super::FederationRuntimeConfig,
        application: &cdk_federation_bark::FederationBarkFrostApplicationConfig,
        snapshot: &FederationBarkProjectionSnapshot,
        action: Option<&FederationBarkActionRecord>,
        plan: &FederationBarkActionPlan,
    ) -> Result<(), Error> {
        if action.is_some() {
            return Err(bark_state_error("refresh action already exists"));
        }
        let minimum_expiry = snapshot
            .vtxos
            .iter()
            .filter(|record| record.state == FederationBarkVtxoState::Spendable)
            .map(|record| record.expiry_height)
            .min()
            .ok_or_else(|| bark_state_error("refresh has no spendable candidate"))?;
        let records = cdk_federation_bark::federation_bark_refresh_candidates(snapshot);
        let predecessor = records
            .first()
            .ok_or_else(|| bark_state_error("refresh has no deterministic predecessor"))?
            .latest_operation_id;
        let expected_inputs = records
            .iter()
            .map(|record| FederationBarkVtxoInput {
                vtxo_id: record.vtxo_id,
                locator: record.locator,
                amount: record.amount,
            })
            .collect::<Vec<_>>();
        let input_amount = expected_inputs
            .iter()
            .try_fold(Amount::ZERO, |sum, input| {
                sum.checked_add(input.amount)
                    .ok_or_else(|| bark_state_error("refresh input amount overflow"))
            })?;
        let trigger_height = minimum_expiry
            .checked_sub(application.policy.refresh_before_expiry_blocks)
            .filter(|height| *height != 0)
            .ok_or_else(|| bark_state_error("refresh trigger is unsafe"))?;
        let emergency_height = minimum_expiry
            .checked_sub(application.policy.emergency_exit_before_expiry_blocks)
            .filter(|height| *height > trigger_height)
            .ok_or_else(|| bark_state_error("refresh emergency height is unsafe"))?;
        let expected_action = cdk_federation_bark::FederationBarkActionId::derive(
            config.federation_id(),
            predecessor,
            FederationBarkSemanticPhase::RefreshPlanned,
            records
                .first()
                .ok_or_else(|| bark_state_error("refresh has no bounded candidate"))?
                .locator
                .index,
        )?;
        let expected_actor = config
            .public_config()
            .members
            .keys()
            .next()
            .copied()
            .ok_or(Error::Internal)?;
        let expected_signers = config
            .public_config()
            .members
            .keys()
            .copied()
            .take(usize::from(application.threshold))
            .collect::<Vec<_>>();
        let output_index = next_bark_locator_index(snapshot)?;
        let output = plan
            .outputs
            .first()
            .filter(|_| plan.outputs.len() == 1)
            .ok_or_else(|| bark_state_error("refresh output is missing"))?;
        let request = plan.refresh_request()?;
        let request_artifact = plan
            .artifacts
            .iter()
            .find(|artifact| artifact.kind == FederationBarkArtifactKind::ArkRequest)
            .ok_or_else(|| bark_state_error("refresh request artifact is missing"))?;
        let mut expected_artifacts = records
            .iter()
            .map(|record| record.artifact.clone())
            .collect::<Vec<_>>();
        expected_artifacts.push(request_artifact.clone());
        expected_artifacts.sort();
        expected_artifacts.dedup();
        let fee_bound = application
            .policy
            .maximum_outgoing_fee(input_amount)
            .map_err(Error::from)?;
        if plan.action_id != expected_action
            || plan.actor != expected_actor
            || plan.predecessor_operation_id != predecessor
            || plan.inputs != expected_inputs
            || plan.signer_set != expected_signers
            || plan.attempt != 1
            || plan.expires_at != u64::from(emergency_height)
            || plan.fee_bound != fee_bound
            || plan.fee_amount > fee_bound
            || output.locator != FederationBarkVtxoLocator::vtxo_custody(output_index)
            || request.action_id != plan.action_id
            || request.predecessor_operation_id != predecessor
            || request.inputs != expected_inputs
            || request.output_locator != output.locator
            || request.output_amount != output.amount
            || request.fee != plan.fee_amount
            || request.trigger_height != trigger_height
            || request.emergency_height != emergency_height
            || request.minimum_expiry != minimum_expiry
            || plan.artifacts != expected_artifacts
        {
            return Err(bark_state_error(
                "refresh plan is not the deterministic earliest-expiry maintenance plan",
            ));
        }
        Ok(())
    }

    async fn validate_federation_bark_exit_plan_submission(
        &self,
        config: &super::FederationRuntimeConfig,
        application: &cdk_federation_bark::FederationBarkFrostApplicationConfig,
        snapshot: &FederationBarkProjectionSnapshot,
        action: Option<&FederationBarkActionRecord>,
        plan: &FederationBarkActionPlan,
        allocations: &cdk_common::federation::FederationAddressAllocationState,
    ) -> Result<(), Error> {
        let (records, predecessor, expected_action, expected_actor, expected_signers, attempt) =
            match action {
                Some(action) => {
                    let plan_operation_id = action.plan_operation_id.ok_or_else(|| {
                        bark_state_error("exit escalation has no accepted refresh plan")
                    })?;
                    let refresh_entry = self
                        .localstore()
                        .get_federation_journal_entry_by_operation_id(plan_operation_id)
                        .await?
                        .ok_or_else(|| {
                            bark_state_error("exit escalation refresh plan is missing")
                        })?;
                    let FederationBarkOperation::ActionPlan(refresh) =
                        bark_operation(&refresh_entry.envelope.operation)?
                    else {
                        return Err(bark_state_error(
                            "exit escalation predecessor is not a refresh plan",
                        ));
                    };
                    if action_effective_phase(action) != FederationBarkSemanticPhase::RefreshPlanned
                        || action.action_kind != Some(FederationBarkActionKind::Refresh)
                        || refresh.kind != FederationBarkActionKind::Refresh
                        || plan.predecessor_operation_id != action.latest_operation_id
                    {
                        return Err(bark_state_error(
                            "exit escalation does not follow an accepted refresh",
                        ));
                    }
                    let mut records = Vec::with_capacity(refresh.inputs.len());
                    for input in &refresh.inputs {
                        let record = snapshot
                            .vtxos
                            .iter()
                            .find(|record| record.vtxo_id == input.vtxo_id)
                            .ok_or_else(|| bark_state_error("exit escalation input is missing"))?;
                        if record.state != FederationBarkVtxoState::Reserved
                            || record.reservation_action_id != Some(refresh.action_id)
                            || record.locator != input.locator
                            || record.amount != input.amount
                        {
                            return Err(bark_state_error(
                                "exit escalation input differs from its refresh reservation",
                            ));
                        }
                        records.push(record);
                    }
                    (
                        records,
                        action.latest_operation_id,
                        refresh.action_id,
                        action.actor,
                        federation_bark_signer_set_for_attempt(
                            config.public_config(),
                            action.attempt,
                        )?,
                        action.attempt,
                    )
                }
                None => {
                    let minimum_expiry = snapshot
                        .vtxos
                        .iter()
                        .filter(|record| record.state == FederationBarkVtxoState::Spendable)
                        .map(|record| record.expiry_height)
                        .min()
                        .ok_or_else(|| bark_state_error("exit has no spendable candidate"))?;
                    let mut records = snapshot
                        .vtxos
                        .iter()
                        .filter(|record| {
                            record.state == FederationBarkVtxoState::Spendable
                                && record.expiry_height == minimum_expiry
                        })
                        .collect::<Vec<_>>();
                    records.sort_by_key(|record| record.vtxo_id);
                    let predecessor = records
                        .first()
                        .ok_or_else(|| bark_state_error("exit has no deterministic predecessor"))?
                        .latest_operation_id;
                    let action_id = cdk_federation_bark::FederationBarkActionId::derive(
                        config.federation_id(),
                        predecessor,
                        FederationBarkSemanticPhase::ExitPlanned,
                        0,
                    )?;
                    let actor = config
                        .public_config()
                        .members
                        .keys()
                        .next()
                        .copied()
                        .ok_or(Error::Internal)?;
                    let signers = config
                        .public_config()
                        .members
                        .keys()
                        .copied()
                        .take(usize::from(application.threshold))
                        .collect::<Vec<_>>();
                    (records, predecessor, action_id, actor, signers, 1)
                }
            };
        let mut records = records;
        records.sort_by_key(|record| record.vtxo_id);
        let expected_inputs = records
            .iter()
            .map(|record| FederationBarkVtxoInput {
                vtxo_id: record.vtxo_id,
                locator: record.locator,
                amount: record.amount,
            })
            .collect::<Vec<_>>();
        let input_amount = expected_inputs
            .iter()
            .try_fold(Amount::ZERO, |sum, input| {
                sum.checked_add(input.amount)
                    .ok_or_else(|| bark_state_error("exit input amount overflow"))
            })?;
        let minimum_expiry = records
            .iter()
            .map(|record| record.expiry_height)
            .min()
            .ok_or_else(|| bark_state_error("exit has no minimum expiry"))?;
        let trigger_height = minimum_expiry
            .checked_sub(application.policy.emergency_exit_before_expiry_blocks)
            .filter(|height| *height != 0)
            .ok_or_else(|| bark_state_error("exit trigger is unsafe"))?;
        let fee_bound = application
            .policy
            .maximum_outgoing_fee(input_amount)
            .map_err(Error::from)?;
        let request = plan.emergency_exit_request()?;
        let frost_wallet = config
            .public_config()
            .frost_wallet
            .as_ref()
            .ok_or(Error::Internal)?;
        let recovery = federation_bark_recovery_application(config.public_config())?
            .ok_or_else(|| bark_state_error("Bark recovery application is missing"))?;
        let expected_quote = federation_bark_exit_allocation_quote(expected_action);
        let scope = FederationAddressAllocationScope {
            application_id: recovery.application_id(),
            application_key_epoch: recovery.application_key_epoch(),
        };
        let allocation = allocations
            .allocation(scope, &expected_quote)
            .ok_or_else(|| bark_state_error("exit destination allocation is missing"))?;
        let expected_script = recovery.derive_script(
            config.public_config(),
            &frost_wallet.root,
            true,
            allocation.derivation_index,
        )?;
        let mut expected_vtxo_artifacts = records
            .iter()
            .map(|record| record.artifact.clone())
            .collect::<Vec<_>>();
        expected_vtxo_artifacts.sort();
        let mut actual_vtxo_artifacts = plan
            .artifacts
            .iter()
            .filter(|artifact| artifact.kind == FederationBarkArtifactKind::Vtxo)
            .cloned()
            .collect::<Vec<_>>();
        actual_vtxo_artifacts.sort();
        let request_count = plan
            .artifacts
            .iter()
            .filter(|artifact| artifact.kind == FederationBarkArtifactKind::ArkRequest)
            .count();
        let invalid_artifact = plan.artifacts.iter().any(|artifact| {
            !matches!(
                artifact.kind,
                FederationBarkArtifactKind::Vtxo
                    | FederationBarkArtifactKind::ArkRequest
                    | FederationBarkArtifactKind::Transaction
            )
        });
        if plan.action_id != expected_action
            || plan.actor != expected_actor
            || plan.predecessor_operation_id != predecessor
            || plan.inputs != expected_inputs
            || plan.signer_set != expected_signers
            || plan.attempt != attempt
            || plan.expires_at != u64::from(minimum_expiry)
            || plan.fee_amount != fee_bound
            || plan.fee_bound != fee_bound
            || plan.external_amount
                != input_amount.checked_sub(fee_bound).ok_or_else(|| {
                    bark_state_error("exit fee bound consumes its accepted inputs")
                })?
            || request.action_id != expected_action
            || request.predecessor_operation_id != predecessor
            || request.inputs != expected_inputs
            || request.destination.application_id != recovery.application_id()
            || request.destination.application_key_epoch != recovery.application_key_epoch()
            || request.destination.quote != expected_quote
            || request.destination.keychain != FederationAddressKeychain::Internal
            || request.destination.derivation_index != allocation.derivation_index
            || request.destination_script != expected_script.as_bytes()
            || request.network != recovery.network()
            || request.network != application.network
            || request.fee_rate_sat_per_kwu != FEDERATION_BARK_EXIT_FEE_RATE_SAT_PER_KWU
            || request.fee_bound != fee_bound
            || request.minimum_recovered != plan.external_amount
            || request.trigger_height != trigger_height
            || request.minimum_expiry != minimum_expiry
            || request.minimum_confirmations != recovery.minimum_confirmations()
            || request.external_id != plan.external_id
            || request.attempt != attempt
            || actual_vtxo_artifacts != expected_vtxo_artifacts
            || request_count != 1
            || invalid_artifact
        {
            return Err(bark_state_error(
                "emergency exit differs from its Bark recovery destination or fee policy",
            ));
        }
        Ok(())
    }
}

impl Mint {
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
        #[cfg(feature = "federation-bark")]
        if let Some(runtime) = self.federation_config() {
            if federation_bark_application(runtime.public_config())?.is_some() {
                let bark_projection: FederationBarkProjectionSnapshot = serde_json::from_value(
                    db.get_federation_application_projection_snapshot(
                        federation_bark_application_id()?,
                    )
                    .await?,
                )
                .map_err(|_| Error::Internal)?;
                for row in bark_projection.materialized_rows()? {
                    snapshot.push(row);
                }
                if let Some(bytes) = db
                    .kv_read(
                        CHAIN_OBSERVATION_PRIMARY_NAMESPACE,
                        FEDERATION_BARK_RECOVERY_SECONDARY_NAMESPACE,
                        FEDERATION_BARK_RECOVERY_STATE_KEY,
                    )
                    .await?
                {
                    for row in
                        FederationBarkRecoveryState::from_bytes(&bytes)?.materialized_rows()?
                    {
                        snapshot.push(row);
                    }
                }
            }
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
                FederationMaterializedStateKey::Application { .. }
                | FederationMaterializedStateKey::ExternalApplication(_) => {}
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

#[cfg(feature = "federation-bark")]
async fn validate_bark_recovery_package_set(
    mint: &Mint,
    config: &super::FederationRuntimeConfig,
    plan_operation_id: FederationOperationId,
    plan: &FederationBarkActionPlan,
    transition: &FederationBarkActionTransition,
) -> Result<(), Error> {
    let package_sets = transition
        .artifacts
        .iter()
        .filter_map(|artifact| FederationBarkCpfpPackageSet::from_artifact(artifact).ok())
        .collect::<Vec<_>>();
    let [package_set] = package_sets.as_slice() else {
        return Err(bark_state_error(
            "exit registration has no exact recovery package set",
        ));
    };
    if package_set.plan_operation_id != plan_operation_id
        || package_set.action_id != plan.action_id
        || package_set.actor != transition.actor
        || package_set.attempt != transition.attempt
        || package_set.signer_set != transition.signer_set
        || package_set
            .packages
            .iter()
            .any(|package| package.fee_ceiling_sat != u64::from(plan.fee_bound))
    {
        return Err(bark_state_error(
            "recovery package set differs from the accepted exit plan",
        ));
    }
    let wallet = config
        .public_config()
        .frost_wallet
        .as_ref()
        .ok_or(Error::Internal)?;
    let recovery = federation_bark_recovery_application(config.public_config())?
        .ok_or_else(|| bark_state_error("Bark recovery application is missing"))?;
    let scope = FederationAddressAllocationScope {
        application_id: recovery.application_id(),
        application_key_epoch: recovery.application_key_epoch(),
    };
    let allocations = mint.federation_address_allocation_state().await?;
    let chain_state = mint.federation_chain_observation_state().await?;
    for package in &package_set.packages {
        let expected_quote =
            federation_bark_cpfp_allocation_quote(plan.action_id, package.parent.compute_txid());
        let allocation = allocations
            .allocation(scope, &expected_quote)
            .ok_or_else(|| bark_state_error("recovery CPFP change allocation is missing"))?;
        package.change.validate_allocation(allocation)?;
        let expected_change = recovery.derive_script(
            config.public_config(),
            &wallet.root,
            true,
            allocation.derivation_index,
        )?;
        if package.change.quote != expected_quote
            || package.change.scope() != scope
            || package.change.keychain != FederationAddressKeychain::Internal
            || package.change_script != expected_change.as_bytes()
        {
            return Err(bark_state_error(
                "recovery CPFP change differs from its accepted allocation",
            ));
        }
        for input in &package.recovery_inputs {
            let utxo = chain_state
                .utxo(input.outpoint)
                .ok_or_else(|| bark_state_error("recovery CPFP input is missing"))?;
            let expected_script = recovery.derive_script(
                config.public_config(),
                &wallet.root,
                false,
                input.allocation.derivation_index,
            )?;
            if utxo.state != cdk_common::federation::FederationWalletUtxoState::Spendable
                || utxo.liability_alarm
                || utxo.txout != input.previous_output
                || utxo.allocation != input.allocation
                || input.allocation.scope() != scope
                || input.allocation.keychain != FederationAddressKeychain::External
                || input.previous_output.script_pubkey != expected_script
            {
                return Err(bark_state_error(
                    "recovery CPFP input is not an accepted spendable recovery output",
                ));
            }
        }
    }
    Ok(())
}

#[cfg(feature = "federation-bark")]
async fn validate_bark_recovery_publication(
    mint: &Mint,
    snapshot: &FederationBarkProjectionSnapshot,
    transition: &FederationBarkActionTransition,
) -> Result<(), Error> {
    let registered_operation_id = snapshot
        .transitions
        .iter()
        .find(|record| {
            record.action_id == transition.action_id
                && record.phase == FederationBarkSemanticPhase::ExitRegistered
        })
        .map(|record| record.operation_id)
        .ok_or_else(|| bark_state_error("exit publication has no accepted registration"))?;
    let registered_entry = mint
        .localstore()
        .get_federation_journal_entry_by_operation_id(registered_operation_id)
        .await?
        .ok_or(Error::Internal)?;
    let FederationBarkOperation::ActionTransition(registered) =
        bark_operation(&registered_entry.envelope.operation)?
    else {
        return Err(bark_state_error(
            "exit recovery registration is not a transition",
        ));
    };
    let package_sets = registered
        .artifacts
        .iter()
        .filter_map(|artifact| FederationBarkCpfpPackageSet::from_artifact(artifact).ok())
        .collect::<Vec<_>>();
    let publications = transition
        .artifacts
        .iter()
        .filter_map(|artifact| FederationBarkCpfpPublication::from_artifact(artifact).ok())
        .collect::<Vec<_>>();
    let ([package_set], [publication]) = (package_sets.as_slice(), publications.as_slice()) else {
        return Err(bark_state_error(
            "exit publication does not bind one accepted recovery package set",
        ));
    };
    let expected =
        FederationBarkCpfpPublication::new(package_set, publication.signed_children.clone())?;
    if publication != &expected {
        return Err(bark_state_error(
            "exit publication differs from its accepted recovery package set",
        ));
    }
    let chain_state = mint.federation_chain_observation_state().await?;
    if package_set
        .packages
        .iter()
        .flat_map(|package| &package.recovery_inputs)
        .any(|input| {
            chain_state.utxo(input.outpoint).is_none_or(|utxo| {
                utxo.state != cdk_common::federation::FederationWalletUtxoState::Reserved
                    || utxo.txout != input.previous_output
                    || utxo.allocation != input.allocation
            })
        })
    {
        return Err(bark_state_error(
            "exit publication recovery inputs are not reserved",
        ));
    }
    Ok(())
}

#[cfg(feature = "federation-bark")]
mod bark_validation {
    use super::*;

    pub(super) fn next_bark_locator_index(
        snapshot: &FederationBarkProjectionSnapshot,
    ) -> Result<u32, Error> {
        snapshot
            .derivation_allocations
            .iter()
            .map(|allocation| allocation.locator.index)
            .max()
            .map_or(Ok(0), |index| {
                index
                    .checked_add(1)
                    .filter(|index| *index <= FEDERATION_MAX_DERIVATION_INDEX)
                    .ok_or_else(|| bark_state_error("Bark locator space is exhausted"))
            })
    }

    pub(super) fn validate_maintenance_transition_header(
        action: &FederationBarkActionRecord,
        plan_operation_id: FederationOperationId,
        plan: &FederationBarkActionPlan,
        transition: &FederationBarkActionTransition,
        expected_kind: FederationBarkActionKind,
    ) -> Result<(), Error> {
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
            return Err(bark_state_error(
                "maintenance transition differs from its accepted plan",
            ));
        }
        Ok(())
    }

    pub(super) fn validate_maintenance_trigger_quorum(
        snapshot: &FederationBarkProjectionSnapshot,
        threshold: u16,
        plan: &FederationBarkActionPlan,
    ) -> Result<(), Error> {
        let commitment = FederationBarkOperation::ActionPlan(plan.clone()).commitment()?;
        let amount = if plan.kind == FederationBarkActionKind::Refresh {
            plan.inputs.iter().try_fold(Amount::ZERO, |sum, input| {
                sum.checked_add(input.amount)
                    .ok_or_else(|| bark_state_error("maintenance quorum amount overflow"))
            })?
        } else {
            plan.external_amount
        };
        let observers = snapshot
            .observations
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
            return Err(bark_state_error(
                "maintenance trigger has not reached the accepted member threshold",
            ));
        }
        Ok(())
    }

    pub(super) fn validate_refresh_completed_result(
        snapshot: &FederationBarkProjectionSnapshot,
        plan: &FederationBarkActionPlan,
        transition: &FederationBarkActionTransition,
    ) -> Result<(), Error> {
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
            return Err(bark_state_error(
                "refresh result does not spend every accepted input",
            ));
        }
        for input in &plan.inputs {
            let record = snapshot
                .vtxos
                .iter()
                .find(|record| record.vtxo_id == input.vtxo_id)
                .ok_or_else(|| bark_state_error("refresh reserved input is missing"))?;
            let mutation = transition
                .vtxo_transitions
                .iter()
                .find(|mutation| mutation.vtxo_id == input.vtxo_id)
                .ok_or_else(|| bark_state_error("refresh input mutation is missing"))?;
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
                    "refresh result differs from its accepted reservation",
                ));
            }
        }
        let output = plan
            .outputs
            .first()
            .ok_or_else(|| bark_state_error("refresh output allocation is missing"))?;
        let output_amount = transition
            .vtxo_transitions
            .iter()
            .filter(|mutation| {
                mutation.previous_state.is_none()
                    && mutation.new_state == FederationBarkVtxoState::Spendable
            })
            .try_fold(Amount::ZERO, |sum, mutation| {
                if mutation.locator != output.locator {
                    return Err(bark_state_error(
                        "refresh result uses an unallocated output locator",
                    ));
                }
                sum.checked_add(mutation.amount)
                    .ok_or_else(|| bark_state_error("refresh output amount overflow"))
            })?;
        if output_amount != output.amount
            || transition
                .artifacts
                .iter()
                .filter(|artifact| artifact.kind == FederationBarkArtifactKind::ArkResult)
                .all(|artifact| artifact.hash != transition.result_hash)
        {
            return Err(bark_state_error(
                "refresh result differs from its accepted output or evidence",
            ));
        }
        Ok(())
    }

    pub(super) fn validate_exit_registered_result(
        snapshot: &FederationBarkProjectionSnapshot,
        plan: &FederationBarkActionPlan,
        transition: &FederationBarkActionTransition,
    ) -> Result<(), Error> {
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
            return Err(bark_state_error(
                "exit registration does not transition every accepted input",
            ));
        }
        for input in &plan.inputs {
            let record = snapshot
                .vtxos
                .iter()
                .find(|record| record.vtxo_id == input.vtxo_id)
                .ok_or_else(|| bark_state_error("exit reserved input is missing"))?;
            let mutation = transition
                .vtxo_transitions
                .iter()
                .find(|mutation| mutation.vtxo_id == input.vtxo_id)
                .ok_or_else(|| bark_state_error("exit input mutation is missing"))?;
            if record.state != FederationBarkVtxoState::Reserved
                || record.reservation_action_id != Some(plan.action_id)
                || mutation.locator != record.locator
                || mutation.amount != record.amount
                || mutation.expiry_height != record.expiry_height
                || mutation.artifact_hash != record.artifact.hash
            {
                return Err(bark_state_error(
                    "exit registration differs from its accepted reservation",
                ));
            }
        }
        validate_exit_transaction_artifacts(plan, transition, false)
    }

    pub(super) fn validate_exit_published_result(
        plan: &FederationBarkActionPlan,
        transition: &FederationBarkActionTransition,
    ) -> Result<(), Error> {
        validate_exit_transaction_artifacts(plan, transition, true)
    }

    pub(super) fn validate_exit_confirmed_result(
        snapshot: &FederationBarkProjectionSnapshot,
        plan: &FederationBarkActionPlan,
        published: &FederationBarkActionTransition,
        transition: &FederationBarkActionTransition,
    ) -> Result<(), Error> {
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
            return Err(bark_state_error(
                "exit confirmation does not finalize every accepted input",
            ));
        }
        for input in &plan.inputs {
            let record = snapshot
                .vtxos
                .iter()
                .find(|record| record.vtxo_id == input.vtxo_id)
                .ok_or_else(|| bark_state_error("exiting input is missing"))?;
            let mutation = transition
                .vtxo_transitions
                .iter()
                .find(|mutation| mutation.vtxo_id == input.vtxo_id)
                .ok_or_else(|| bark_state_error("exit confirmation mutation is missing"))?;
            if record.state != FederationBarkVtxoState::Exiting
                || record.reservation_action_id != Some(plan.action_id)
                || mutation.locator != record.locator
                || mutation.amount != record.amount
                || mutation.expiry_height != record.expiry_height
                || mutation.artifact_hash != record.artifact.hash
            {
                return Err(bark_state_error(
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
            return Err(bark_state_error(
                "exit confirmation does not retain the published transaction package",
            ));
        }
        Ok(())
    }

    pub(super) fn validate_exit_transaction_artifacts(
        plan: &FederationBarkActionPlan,
        transition: &FederationBarkActionTransition,
        require_drain: bool,
    ) -> Result<(), Error> {
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
            return Err(bark_state_error(
                "exit evidence differs from its accepted transaction package",
            ));
        }
        Ok(())
    }

    pub(super) fn bark_state_error(reason: &str) -> Error {
        Error::Custom(format!("invalid federated Bark state: {reason}"))
    }

    pub(super) fn validate_send_transition_header(
        action: &FederationBarkActionRecord,
        plan_operation_id: FederationOperationId,
        plan: &FederationBarkActionPlan,
        transition: &FederationBarkActionTransition,
    ) -> Result<(), Error> {
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
            return Err(bark_state_error(
                "outgoing transition differs from its accepted send bindings",
            ));
        }
        Ok(())
    }

    pub(super) fn action_effective_phase(
        action: &FederationBarkActionRecord,
    ) -> FederationBarkSemanticPhase {
        action.signing_retry_phase.unwrap_or(action.phase)
    }

    pub(super) fn action_custody_signers<'a>(
        action: &'a FederationBarkActionRecord,
        plan: &'a FederationBarkActionPlan,
    ) -> Result<&'a [cdk_common::federation::MemberId], Error> {
        match &action.operation {
            FederationBarkOperation::ActionTransition(transition)
                if transition.attempt == action.attempt && !transition.signer_set.is_empty() =>
            {
                Ok(&transition.signer_set)
            }
            _ if action.attempt == plan.attempt => Ok(&plan.signer_set),
            _ => Err(bark_state_error(
                "action custody attempt has no accepted signer binding",
            )),
        }
    }

    pub(crate) fn validate_payment_success_proof(
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

    pub(super) fn payment_failed_result_hash(
        plan: &FederationBarkActionPlan,
        htlc_result_hash: FederationBarkDigest,
    ) -> Result<FederationBarkDigest, Error> {
        let bytes = payment_failed_result_bytes(plan, htlc_result_hash)?;
        FederationBarkDigest::from_bytes(sha256::Hash::hash(&bytes).to_byte_array())
            .map_err(Error::from)
    }

    pub(super) fn payment_failed_result_bytes(
        plan: &FederationBarkActionPlan,
        htlc_result_hash: FederationBarkDigest,
    ) -> Result<Vec<u8>, Error> {
        let mut bytes = b"cdk/bark/send-payment-failed/v1".to_vec();
        bytes.extend_from_slice(&plan.action_id.to_bytes());
        bytes.extend_from_slice(&htlc_result_hash.to_bytes());
        bytes.extend_from_slice(
            &plan
                .payment_hash
                .ok_or_else(|| bark_state_error("send payment hash is missing"))?
                .to_bytes(),
        );
        Ok(bytes)
    }

    pub(super) fn validate_payment_failed_result(
        plan: &FederationBarkActionPlan,
        transition: &FederationBarkActionTransition,
    ) -> Result<(), Error> {
        let bytes = payment_failed_result_bytes(plan, transition.request_hash)?;
        let expected = bark_projection_artifact(FederationBarkArtifactKind::ArkResult, bytes)?;
        let mut results = transition
            .artifacts
            .iter()
            .filter(|artifact| artifact.kind == FederationBarkArtifactKind::ArkResult);
        if results.next() != Some(&expected)
            || results.next().is_some()
            || transition.result_hash != expected.hash
        {
            return Err(bark_state_error(
                "payment failure result differs from its accepted HTLC result",
            ));
        }
        Ok(())
    }

    pub(super) fn failed_send_revocation_request_bytes(
        action_id: cdk_federation_bark::FederationBarkActionId,
        failed_operation_id: FederationOperationId,
        payment_hash: FederationBarkDigest,
        output_locator: FederationBarkVtxoLocator,
        inputs: &[FederationBarkVtxoInput],
    ) -> Result<Vec<u8>, Error> {
        let mut bytes = b"cdk/bark/send-revocation-request/v1".to_vec();
        bytes.extend_from_slice(&action_id.to_bytes());
        bytes.extend_from_slice(&failed_operation_id.to_bytes());
        bytes.extend_from_slice(&payment_hash.to_bytes());
        append_bark_locator(&mut bytes, output_locator);
        bytes.extend_from_slice(
            &u32::try_from(inputs.len())
                .map_err(|_| bark_state_error("too many revocation inputs"))?
                .to_be_bytes(),
        );
        for input in inputs {
            bytes.extend_from_slice(&input.vtxo_id.to_bytes());
            append_bark_locator(&mut bytes, input.locator);
            bytes.extend_from_slice(&u64::from(input.amount).to_be_bytes());
        }
        Ok(bytes)
    }

    pub(super) fn append_bark_locator(bytes: &mut Vec<u8>, locator: FederationBarkVtxoLocator) {
        bytes.push(locator.role);
        bytes.extend_from_slice(&locator.index.to_be_bytes());
        bytes.extend_from_slice(&locator.version.to_be_bytes());
    }

    pub(super) fn bark_projection_artifact(
        kind: FederationBarkArtifactKind,
        bytes: Vec<u8>,
    ) -> Result<FederationBarkArtifact, Error> {
        Ok(FederationBarkArtifact {
            kind,
            hash: FederationBarkDigest::from_bytes(sha256::Hash::hash(&bytes).to_byte_array())?,
            bytes,
        })
    }

    pub(super) fn validate_revocation_completed_result(
        snapshot: &FederationBarkProjectionSnapshot,
        plan: &FederationBarkActionPlan,
        transition: &FederationBarkActionTransition,
    ) -> Result<(), Error> {
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
            return Err(bark_state_error(
                "revocation result does not spend every accepted HTLC",
            ));
        }
        let mut expected_artifacts = Vec::new();
        for input in &plan.inputs {
            let record = snapshot
                .vtxos
                .iter()
                .find(|record| record.vtxo_id == input.vtxo_id)
                .ok_or_else(|| bark_state_error("reserved revocation input is missing"))?;
            let mutation = transition
                .vtxo_transitions
                .iter()
                .find(|mutation| mutation.vtxo_id == input.vtxo_id)
                .ok_or_else(|| bark_state_error("revocation input mutation is missing"))?;
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
                    "revocation input differs from its accepted reservation",
                ));
            }
            expected_artifacts.push(record.artifact.clone());
        }
        let output = plan
            .outputs
            .first()
            .ok_or_else(|| bark_state_error("revocation output allocation is missing"))?;
        let outputs = transition
            .vtxo_transitions
            .iter()
            .filter(|mutation| {
                mutation.previous_state.is_none()
                    && mutation.new_state == FederationBarkVtxoState::Spendable
            })
            .collect::<Vec<_>>();
        if outputs.is_empty()
            || transition.vtxo_transitions.len() != plan.inputs.len() + outputs.len()
        {
            return Err(bark_state_error(
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
                .ok_or_else(|| bark_state_error("revocation payment hash is missing"))?
                .to_bytes(),
        );
        result_bytes.extend_from_slice(
            &u32::try_from(plan.inputs.len())
                .map_err(|_| bark_state_error("too many revocation inputs"))?
                .to_be_bytes(),
        );
        for input in &plan.inputs {
            result_bytes.extend_from_slice(&input.vtxo_id.to_bytes());
        }
        result_bytes.extend_from_slice(
            &u32::try_from(outputs.len())
                .map_err(|_| bark_state_error("too many revocation outputs"))?
                .to_be_bytes(),
        );
        for mutation in outputs {
            if mutation.locator != output.locator {
                return Err(bark_state_error(
                    "revocation result uses an unallocated output locator",
                ));
            }
            let mut artifacts = transition.artifacts.iter().filter(|artifact| {
                artifact.kind == FederationBarkArtifactKind::Vtxo
                    && artifact.hash == mutation.artifact_hash
            });
            let artifact = artifacts
                .next()
                .ok_or_else(|| bark_state_error("revocation output artifact is missing"))?;
            if artifacts.next().is_some() {
                return Err(bark_state_error("revocation output artifact is ambiguous"));
            }
            result_bytes.extend_from_slice(
                &u64::try_from(artifact.bytes.len())
                    .map_err(|_| bark_state_error("revocation output artifact is oversized"))?
                    .to_be_bytes(),
            );
            result_bytes.extend_from_slice(&artifact.bytes);
            recovered = recovered
                .checked_add(mutation.amount)
                .ok_or_else(|| bark_state_error("revocation output amount overflow"))?;
            expected_artifacts.push(artifact.clone());
        }
        if recovered != output.amount {
            return Err(bark_state_error(
                "revocation result does not conserve the accepted HTLC value",
            ));
        }
        let result = bark_projection_artifact(FederationBarkArtifactKind::ArkResult, result_bytes)?;
        if result.hash != transition.result_hash {
            return Err(bark_state_error(
                "revocation result commitment differs from its exact outputs",
            ));
        }
        expected_artifacts.push(result);
        expected_artifacts.sort();
        expected_artifacts.dedup();
        if transition.artifacts != expected_artifacts {
            return Err(bark_state_error(
                "revocation result artifacts differ from its exact recovery",
            ));
        }
        Ok(())
    }
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

#[cfg(all(test, feature = "federation-bark"))]
mod outgoing_tests {
    use super::*;
    use cdk_common::federation::MemberId;
    use cdk_federation_bark::{
        FederationBarkActionId, FederationBarkArtifact, FederationBarkExternalId,
        FederationBarkVtxoOutput, FEDERATION_BARK_OPERATION_VERSION,
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
            certified_receive_htlcs: None,
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
            evidence_certificate: None,
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

    #[test]
    fn outgoing_failure_result_binds_the_accepted_htlc_result_and_payment_hash() {
        let plan = send_plan(digest(12));
        let htlc_result = digest(13);
        let bytes = payment_failed_result_bytes(&plan, htlc_result).expect("failure result bytes");
        let artifact = bark_projection_artifact(FederationBarkArtifactKind::ArkResult, bytes)
            .expect("failure artifact");
        let transition = FederationBarkActionTransition {
            version: FEDERATION_BARK_OPERATION_VERSION,
            action_id: plan.action_id,
            quote: plan.quote.clone(),
            phase: FederationBarkSemanticPhase::PaymentFailed,
            actor: plan.actor,
            plan_operation_id: FederationOperationId::from_bytes([14; 32]),
            predecessor_operation_id: FederationOperationId::from_bytes([15; 32]),
            payment_hash: plan.payment_hash,
            attempt: plan.attempt,
            signer_set: plan.signer_set.clone(),
            request_hash: htlc_result,
            result_hash: artifact.hash,
            evidence_certificate: None,
            external_id: plan.external_id,
            artifacts: vec![artifact],
            vtxo_transitions: Vec::new(),
        };
        validate_payment_failed_result(&plan, &transition).expect("valid failure result");

        let mut changed = transition.clone();
        changed.request_hash = digest(16);
        assert!(validate_payment_failed_result(&plan, &changed).is_err());
        let mut changed = transition;
        changed.artifacts[0].bytes.push(0);
        assert!(validate_payment_failed_result(&plan, &changed).is_err());
    }
}
