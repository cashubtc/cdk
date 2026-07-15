//! Independent validation of untrusted federated BDK transaction proposals.
//!
//! Validation consumes only accepted federation state and public FROST wallet
//! configuration. A member's local BDK graph, proposer identity, coin selector,
//! fee estimator, or supplied sighash is never authoritative.

use std::collections::{BTreeMap, BTreeSet};

use cdk_common::bitcoin::absolute::LockTime;
use cdk_common::bitcoin::hashes::Hash;
use cdk_common::bitcoin::sighash::{Prevouts, SighashCache};
use cdk_common::bitcoin::transaction::Version;
use cdk_common::bitcoin::{Sequence, TxOut};
use cdk_common::error::Error;
use cdk_common::federation::{
    FederationAcceptedTransaction, FederationAcceptedTransactionState,
    FederationChainObservationState, FederationError, FederationOnchainPaymentBatch,
    FederationOnchainPaymentIntentId, FederationOnchainPaymentIntentRecord,
    FederationOnchainPaymentIntentState, FederationTaprootSighashType,
    FederationTransactionPolicyAudit, FederationTransactionProposal,
    FederationTransactionProposalId, FederationTransactionScope, FederationWalletUtxoState,
    FEDERATION_MAX_ONCHAIN_PROPOSAL_INPUTS, FEDERATION_MAX_ONCHAIN_PROPOSAL_OUTPUTS,
    FEDERATION_MAX_ONCHAIN_TRANSACTION_WEIGHT,
};
use serde::{Deserialize, Serialize};

use super::FederationRuntimeConfig;

const TRANSACTION_POLICY_VERSION: u16 = 1;
const MINIMUM_FEE_RATE_SAT_PER_KWU: u64 = 250;
const MAXIMUM_FEE_RATE_SAT_PER_KWU: u64 = 100_000;
const MAXIMUM_ABSOLUTE_FEE_SAT: u64 = 1_000_000;
const FEE_ESTIMATION_TIMEOUT_MILLIS: u64 = 3_000;
const FALLBACK_FEE_RATE_SAT_PER_VB: u64 = 2;
const ALLOWED_FEE_TARGET_BLOCKS: [u32; 3] = [1, 6, 144];
const MINIMUM_REPLACEMENT_FEE_DELTA_SAT_PER_KWU: u64 = 250;
const REPLACEMENT_AFTER_BROADCAST_ATTEMPTS: u32 = 3;

/// Proposer-local source class permitted by a consensus fee-policy version.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FederationFeeRateSource {
    /// The member's configured local BDK chain source.
    MemberLocalChainSource,
}

/// First-release child-pays-for-parent policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FederationCpfpPolicy {
    /// CPFP is not authorized; operators use bounded RBF replacement instead.
    Deferred,
}

/// Explicit consensus policy interpreted for one transaction-policy version.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FederationOnchainTransactionPolicy {
    /// Policy schema version committed by the active application.
    pub version: u16,
    /// Minimum fee rate in satoshis per 1,000 weight units.
    pub minimum_fee_rate_sat_per_kwu: u64,
    /// Maximum fee rate in satoshis per 1,000 weight units.
    pub maximum_fee_rate_sat_per_kwu: u64,
    /// Absolute transaction fee ceiling.
    pub maximum_absolute_fee_sat: u64,
    /// Permitted proposer-local fee estimate source.
    pub fee_rate_source: FederationFeeRateSource,
    /// Bounded wait for the member-local fee source.
    pub fee_estimation_timeout_millis: u64,
    /// Deterministic fallback when the local source is unavailable or invalid.
    pub fallback_fee_rate_sat_per_vb: u64,
    /// Allowed confirmation targets carried by accepted quote fee options.
    pub allowed_fee_target_blocks: [u32; 3],
    /// Maximum transaction inputs.
    pub maximum_inputs: usize,
    /// Maximum transaction outputs.
    pub maximum_outputs: usize,
    /// Maximum transaction weight units.
    pub maximum_weight: u64,
    /// Whether every input must opt into BIP125 replacement.
    pub require_rbf: bool,
    /// Minimum incremental replacement fee in satoshis per 1,000 weight units.
    pub minimum_replacement_fee_delta_sat_per_kwu: u64,
    /// Broadcast/reconciliation attempts before a member may propose replacement.
    pub replacement_after_broadcast_attempts: u32,
    /// Explicit first-release CPFP policy.
    pub cpfp_policy: FederationCpfpPolicy,
}

impl FederationOnchainTransactionPolicy {
    /// Resolve the immutable policy contract for a public version.
    pub fn for_version(version: u16) -> Result<Self, Error> {
        if version != TRANSACTION_POLICY_VERSION {
            return Err(invalid("unsupported federated on-chain transaction policy"));
        }
        Ok(Self {
            version,
            minimum_fee_rate_sat_per_kwu: MINIMUM_FEE_RATE_SAT_PER_KWU,
            maximum_fee_rate_sat_per_kwu: MAXIMUM_FEE_RATE_SAT_PER_KWU,
            maximum_absolute_fee_sat: MAXIMUM_ABSOLUTE_FEE_SAT,
            fee_rate_source: FederationFeeRateSource::MemberLocalChainSource,
            fee_estimation_timeout_millis: FEE_ESTIMATION_TIMEOUT_MILLIS,
            fallback_fee_rate_sat_per_vb: FALLBACK_FEE_RATE_SAT_PER_VB,
            allowed_fee_target_blocks: ALLOWED_FEE_TARGET_BLOCKS,
            maximum_inputs: FEDERATION_MAX_ONCHAIN_PROPOSAL_INPUTS,
            maximum_outputs: FEDERATION_MAX_ONCHAIN_PROPOSAL_OUTPUTS,
            maximum_weight: FEDERATION_MAX_ONCHAIN_TRANSACTION_WEIGHT,
            require_rbf: true,
            minimum_replacement_fee_delta_sat_per_kwu: MINIMUM_REPLACEMENT_FEE_DELTA_SAT_PER_KWU,
            replacement_after_broadcast_attempts: REPLACEMENT_AFTER_BROADCAST_ATTEMPTS,
            cpfp_policy: FederationCpfpPolicy::Deferred,
        })
    }

    /// Return whether a quote confirmation target belongs to this policy.
    #[must_use]
    pub fn permits_fee_target(self, target_blocks: u32) -> bool {
        self.allowed_fee_target_blocks.contains(&target_blocks)
    }

    /// Minimum absolute fee increase for a replacement of `weight` units.
    pub fn minimum_replacement_fee_delta(self, weight: u64) -> Result<u64, Error> {
        self.minimum_replacement_fee_delta_sat_per_kwu
            .checked_mul(weight)
            .and_then(|scaled| scaled.checked_add(999))
            .map(|scaled| (scaled / 1_000).max(1))
            .ok_or_else(|| invalid("replacement fee delta calculation overflow"))
    }
}

impl FederationRuntimeConfig {
    /// Independently validate an untrusted on-chain transaction proposal.
    ///
    /// This checks accepted UTXOs and pending intents, derives every wallet
    /// script, enforces policy, and recomputes transaction identity and Taproot
    /// sighashes. It performs no persistence, reservation, signing, or network
    /// side effect.
    pub fn validate_onchain_transaction_proposal(
        &self,
        chain_state: &FederationChainObservationState,
        intent_records: &BTreeMap<
            FederationOnchainPaymentIntentId,
            FederationOnchainPaymentIntentRecord,
        >,
        proposal: &FederationTransactionProposal,
    ) -> Result<FederationTransactionPolicyAudit, Error> {
        self.validate_onchain_transaction_proposal_lifecycle(
            chain_state,
            intent_records,
            proposal,
            ProposalLifecycle::New,
        )
    }

    /// Validate a fee-bumping replacement against the exact active lineage head.
    pub fn validate_onchain_transaction_replacement(
        &self,
        chain_state: &FederationChainObservationState,
        intent_records: &BTreeMap<
            FederationOnchainPaymentIntentId,
            FederationOnchainPaymentIntentRecord,
        >,
        original: &FederationAcceptedTransaction,
        proposal: &FederationTransactionProposal,
    ) -> Result<FederationTransactionPolicyAudit, Error> {
        if !matches!(
            original.state,
            FederationAcceptedTransactionState::BroadcastPending
                | FederationAcceptedTransactionState::Broadcast
        ) || original.signed.is_none()
        {
            return Err(invalid(
                "replacement requires an unconfirmed signed lineage head",
            ));
        }
        self.validate_onchain_transaction_proposal_lifecycle(
            chain_state,
            intent_records,
            proposal,
            ProposalLifecycle::Replacement { original },
        )
    }

    pub(crate) fn revalidate_accepted_onchain_transaction_proposal(
        &self,
        chain_state: &FederationChainObservationState,
        intent_records: &BTreeMap<
            FederationOnchainPaymentIntentId,
            FederationOnchainPaymentIntentRecord,
        >,
        proposal: &FederationTransactionProposal,
        original: Option<&FederationAcceptedTransaction>,
    ) -> Result<FederationTransactionPolicyAudit, Error> {
        let proposal_id = proposal.proposal_id()?;
        self.validate_onchain_transaction_proposal_lifecycle(
            chain_state,
            intent_records,
            proposal,
            ProposalLifecycle::Accepted {
                proposal_id,
                original,
            },
        )
    }

    fn validate_onchain_transaction_proposal_lifecycle(
        &self,
        chain_state: &FederationChainObservationState,
        intent_records: &BTreeMap<
            FederationOnchainPaymentIntentId,
            FederationOnchainPaymentIntentRecord,
        >,
        proposal: &FederationTransactionProposal,
        lifecycle: ProposalLifecycle<'_>,
    ) -> Result<FederationTransactionPolicyAudit, Error> {
        proposal.validate()?;
        chain_state.to_bytes()?;
        let application = self.cdk_onchain_application()?;
        let expected_scope = FederationTransactionScope {
            root_key_epoch: application.root_key_epoch,
            application_id: application.application_id,
            application_key_epoch: application.application_key_epoch,
            network: application.network,
            policy_version: application.fee_policy_version,
        };
        proposal.validate_scope(expected_scope)?;
        let policy =
            FederationOnchainTransactionPolicy::for_version(application.fee_policy_version)?;
        validate_transaction_shape(proposal, policy)?;

        let wallet =
            self.public_config().frost_wallet.as_ref().ok_or_else(|| {
                invalid("federated transaction policy has no FROST wallet config")
            })?;
        let expected_utxo_state = match lifecycle {
            ProposalLifecycle::New => FederationWalletUtxoState::Spendable,
            ProposalLifecycle::Replacement { .. } | ProposalLifecycle::Accepted { .. } => {
                FederationWalletUtxoState::Reserved
            }
        };
        for selected in &proposal.selected_inputs {
            let accepted = chain_state
                .utxo(selected.outpoint)
                .ok_or_else(|| invalid("transaction proposal references an unknown UTXO"))?;
            if accepted.state != expected_utxo_state
                || accepted.liability_alarm
                || accepted.outpoint != selected.outpoint
                || accepted.txout != selected.previous_output
                || accepted.allocation != selected.allocation
            {
                return Err(invalid(
                    "transaction input is not an exact accepted UTXO in the required lifecycle state",
                ));
            }
            let expected_script = wallet.derive_cdk_onchain_script(
                self.public_config(),
                proposal.scope.application_id,
                proposal.scope.application_key_epoch,
                selected.allocation.keychain,
                selected.allocation.derivation_index,
            )?;
            if !expected_script.is_p2tr()
                || selected.previous_output.script_pubkey != expected_script
            {
                return Err(invalid(
                    "transaction input previous output does not match its Taproot derivation",
                ));
            }
        }

        let batch = validate_intents(intent_records, proposal, lifecycle)?;
        if batch.batch_id != proposal.batch_id || batch.intent_ids != proposal.intent_ids {
            return Err(invalid(
                "transaction proposal batch does not match accepted pending intents",
            ));
        }

        let mut fee_contributions = BTreeMap::new();
        let mut maximum_fee_sum = 0_u64;
        for recipient in &proposal.recipient_outputs {
            let record = intent_records
                .get(&recipient.intent_id)
                .ok_or_else(|| invalid("transaction recipient has no accepted intent"))?;
            let intent = &record.intent;
            if recipient.output.script_pubkey != intent.destination_script
                || recipient.output.value.to_sat() != intent.amount_sat
                || recipient.output.script_pubkey.is_op_return()
                || is_dust(&recipient.output)
                || !policy.permits_fee_target(intent.fee_target_blocks)
                || recipient.fee_contribution_sat > intent.maximum_fee_sat
            {
                return Err(invalid(
                    "transaction recipient differs from its authorized payment intent",
                ));
            }
            maximum_fee_sum = maximum_fee_sum
                .checked_add(intent.maximum_fee_sat)
                .ok_or_else(|| invalid("transaction intent maximum-fee sum overflow"))?;
            if fee_contributions
                .insert(recipient.intent_id, recipient.fee_contribution_sat)
                .is_some()
            {
                return Err(invalid("transaction contains a duplicate fee allocation"));
            }
        }
        if proposal.fee_sat > maximum_fee_sum || proposal.fee_sat > policy.maximum_absolute_fee_sat
        {
            return Err(invalid(
                "transaction fee exceeds an intent or policy maximum",
            ));
        }

        if let Some(change) = &proposal.change_output {
            let expected_script = wallet.derive_cdk_onchain_script(
                self.public_config(),
                proposal.scope.application_id,
                proposal.scope.application_key_epoch,
                cdk_common::federation::FederationAddressKeychain::Internal,
                change.derivation_index,
            )?;
            if !expected_script.is_p2tr()
                || change.output.script_pubkey != expected_script
                || is_dust(&change.output)
            {
                return Err(invalid(
                    "transaction change does not match its internal Taproot derivation",
                ));
            }
        }

        match lifecycle {
            ProposalLifecycle::Replacement { original } => {
                validate_replacement_shape(policy, original, proposal)?;
            }
            ProposalLifecycle::Accepted {
                original: Some(original),
                ..
            } => validate_replacement_shape(policy, original, proposal)?,
            ProposalLifecycle::Accepted { original: None, .. } => {
                if proposal.replacement_of.is_some() {
                    return Err(invalid(
                        "accepted replacement is missing its original transaction",
                    ));
                }
            }
            ProposalLifecycle::New => {
                if proposal.replacement_of.is_some() {
                    return Err(invalid(
                        "initial transaction proposal unexpectedly names a replacement",
                    ));
                }
            }
        }

        let previous_outputs = proposal.previous_outputs();
        let sighash_type = proposal.sighash_type.to_bitcoin();
        let mut taproot_sighashes = Vec::with_capacity(proposal.selected_inputs.len());
        for input_index in 0..proposal.selected_inputs.len() {
            let sighash = SighashCache::new(&proposal.unsigned_transaction)
                .taproot_key_spend_signature_hash(
                    input_index,
                    &Prevouts::All(&previous_outputs),
                    sighash_type,
                )
                .map_err(|_| invalid("transaction Taproot sighash computation failed"))?;
            taproot_sighashes.push(sighash.to_byte_array());
        }

        Ok(FederationTransactionPolicyAudit {
            proposal_id: proposal.proposal_id()?,
            txid: proposal.unsigned_transaction.compute_txid(),
            fee_sat: proposal.fee_sat,
            fee_rate_sat_per_kwu: proposal.fee_rate_sat_per_kwu,
            weight: proposal.estimated_signed_weight(),
            fee_contributions,
            taproot_sighashes,
        })
    }
}

#[derive(Debug, Clone, Copy)]
enum ProposalLifecycle<'a> {
    New,
    Replacement {
        original: &'a FederationAcceptedTransaction,
    },
    Accepted {
        proposal_id: FederationTransactionProposalId,
        original: Option<&'a FederationAcceptedTransaction>,
    },
}

fn validate_transaction_shape(
    proposal: &FederationTransactionProposal,
    policy: FederationOnchainTransactionPolicy,
) -> Result<(), Error> {
    let transaction = &proposal.unsigned_transaction;
    let weight = proposal.estimated_signed_weight();
    if transaction.version != Version::TWO
        || transaction.lock_time != LockTime::ZERO
        || transaction.input.len() > policy.maximum_inputs
        || transaction.output.len() > policy.maximum_outputs
        || weight > policy.maximum_weight
        || proposal.fee_rate_sat_per_kwu < policy.minimum_fee_rate_sat_per_kwu
        || proposal.fee_rate_sat_per_kwu > policy.maximum_fee_rate_sat_per_kwu
        || proposal.sighash_type != FederationTaprootSighashType::Default
    {
        return Err(invalid(
            "transaction version, locktime, bounds, fee rate, or sighash policy is invalid",
        ));
    }
    let sequences = transaction
        .input
        .iter()
        .map(|input| input.sequence)
        .collect::<BTreeSet<_>>();
    if sequences.len() != 1
        || (policy.require_rbf && sequences.first() != Some(&Sequence::ENABLE_RBF_NO_LOCKTIME))
    {
        return Err(invalid(
            "transaction input sequence or RBF policy is invalid",
        ));
    }
    Ok(())
}

fn validate_intents(
    records: &BTreeMap<FederationOnchainPaymentIntentId, FederationOnchainPaymentIntentRecord>,
    proposal: &FederationTransactionProposal,
    lifecycle: ProposalLifecycle<'_>,
) -> Result<FederationOnchainPaymentBatch, Error> {
    if records.len() < proposal.intent_ids.len() {
        return Err(invalid("transaction proposal is missing accepted intents"));
    }
    let intents = proposal
        .intent_ids
        .iter()
        .map(|intent_id| {
            let record = records
                .get(intent_id)
                .ok_or_else(|| invalid("transaction proposal references an unknown intent"))?;
            record.validate()?;
            let expected_state = match lifecycle {
                ProposalLifecycle::New => FederationOnchainPaymentIntentState::Pending,
                ProposalLifecycle::Replacement { original } => {
                    FederationOnchainPaymentIntentState::Accepted {
                        proposal_id: original.proposal.proposal_id()?,
                    }
                }
                ProposalLifecycle::Accepted { proposal_id, .. } => {
                    FederationOnchainPaymentIntentState::Accepted { proposal_id }
                }
            };
            if record.state != expected_state || record.intent.scope != proposal.scope {
                return Err(invalid(
                    "transaction proposal intent is not in the required lifecycle state and scope",
                ));
            }
            Ok(record.intent.clone())
        })
        .collect::<Result<Vec<_>, Error>>()?;
    FederationOnchainPaymentBatch::new(intents, proposal.eligibility_cutoff).map_err(Error::from)
}

fn validate_replacement_shape(
    policy: FederationOnchainTransactionPolicy,
    original: &FederationAcceptedTransaction,
    replacement: &FederationTransactionProposal,
) -> Result<(), Error> {
    let original_id = original.proposal.proposal_id()?;
    if replacement.replacement_of != Some(original_id)
        || replacement.batch_id != original.proposal.batch_id
        || replacement.replacement_lineage != original.proposal.replacement_lineage
        || replacement.intent_ids != original.proposal.intent_ids
        || replacement.scope != original.proposal.scope
        || replacement.selected_inputs != original.proposal.selected_inputs
        || replacement.unsigned_transaction.input != original.proposal.unsigned_transaction.input
        || replacement.recipient_outputs.len() != original.proposal.recipient_outputs.len()
    {
        return Err(invalid(
            "replacement lineage, intents, or inputs differ from the active transaction",
        ));
    }
    for (replacement_recipient, original_recipient) in replacement
        .recipient_outputs
        .iter()
        .zip(&original.proposal.recipient_outputs)
    {
        if replacement_recipient.intent_id != original_recipient.intent_id
            || replacement_recipient.output_index != original_recipient.output_index
            || replacement_recipient.output != original_recipient.output
        {
            return Err(invalid(
                "replacement changes an authorized recipient or amount",
            ));
        }
    }
    let (Some(replacement_change), Some(original_change)) = (
        replacement.change_output.as_ref(),
        original.proposal.change_output.as_ref(),
    ) else {
        return Err(invalid(
            "first-release replacement requires an existing change output",
        ));
    };
    if replacement_change.derivation_index != original_change.derivation_index
        || replacement_change.output_index != original_change.output_index
        || replacement_change.output.script_pubkey != original_change.output.script_pubkey
        || replacement_change.output.value >= original_change.output.value
    {
        return Err(invalid(
            "replacement change is not a strict value reduction of the allocated output",
        ));
    }
    let minimum_delta =
        policy.minimum_replacement_fee_delta(replacement.estimated_signed_weight())?;
    if replacement.fee_sat < original.proposal.fee_sat.saturating_add(minimum_delta)
        || replacement.fee_rate_sat_per_kwu <= original.proposal.fee_rate_sat_per_kwu
    {
        return Err(invalid(
            "replacement fee does not satisfy the incremental relay policy",
        ));
    }
    Ok(())
}

fn is_dust(output: &TxOut) -> bool {
    output.value < TxOut::minimal_non_dust(output.script_pubkey.clone()).value
}

fn invalid(reason: impl Into<String>) -> Error {
    FederationError::TransactionProposalInvalid {
        reason: reason.into(),
    }
    .into()
}
