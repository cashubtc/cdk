//! Canonical federated on-chain payment intents and transaction proposals.
//!
//! These types deliberately contain only consensus-relevant PSBT data: the
//! unsigned transaction and one ordered witness UTXO per input. Proposer-local
//! PSBT maps, derivation hints, proprietary fields, and signer state are not
//! admitted into consensus. Policy validation and accepted-state mutation are
//! owned by later layers and milestones.

use core::cmp::Ordering;
use core::fmt;
use core::str::FromStr;
use std::collections::{BTreeMap, BTreeSet};

use bitcoin::consensus::serialize;
use bitcoin::hashes::{sha256, Hash, HashEngine};
use bitcoin::{Network, OutPoint, ScriptBuf, Transaction, TxOut, Txid};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use super::{
    FederationAllocatedAddressRef, FederationError, FederationFrostApplicationId,
    FederationMaterializedStateRow, FederationOperationConflictKey, FederationOperationId,
    MemberId, Result, FEDERATION_MAX_DERIVATION_INDEX,
};

const PAYMENT_INTENT_VERSION: u16 = 1;
const PAYMENT_BATCH_VERSION: u16 = 1;
const TRANSACTION_PROPOSAL_VERSION: u16 = 1;
const TRANSACTION_STATE_VERSION: u16 = 1;
const PAYMENT_INTENT_HASH_DOMAIN: &[u8] = b"cdk-federation-onchain-payment-intent-v1";
const PAYMENT_BATCH_HASH_DOMAIN: &[u8] = b"cdk-federation-onchain-payment-batch-v1";
const TRANSACTION_PROPOSAL_HASH_DOMAIN: &[u8] = b"cdk-federation-onchain-transaction-proposal-v1";
const PSBT_CONSENSUS_HASH_DOMAIN: &[u8] = b"cdk-federation-onchain-psbt-consensus-v1";
const FROST_SIGNING_AUTHORIZATION_HASH_DOMAIN: &[u8] =
    b"cdk-federation-frost-signing-authorization-v1";
const MAX_QUOTE_ID_BYTES: usize = 256;
const MAX_DESTINATION_SCRIPT_BYTES: usize = 10_000;
const MAX_FEE_TIER: u32 = 255;

/// Maximum number of payment intents in one consensus batch.
pub const FEDERATION_MAX_ONCHAIN_BATCH_INTENTS: usize = 64;
/// Maximum number of inputs in one proposed transaction.
pub const FEDERATION_MAX_ONCHAIN_PROPOSAL_INPUTS: usize = 64;
/// Maximum number of outputs in one proposed transaction.
pub const FEDERATION_MAX_ONCHAIN_PROPOSAL_OUTPUTS: usize = 65;
/// Maximum standard transaction weight admitted by the proposal format.
pub const FEDERATION_MAX_ONCHAIN_TRANSACTION_WEIGHT: u64 = 400_000;
/// Maximum bytes in the canonical unsigned-PSBT consensus projection.
pub const FEDERATION_MAX_ONCHAIN_PSBT_CONSENSUS_BYTES: usize = 256 * 1024;
/// Maximum canonical bytes in one proposal before operation-envelope overhead.
pub const FEDERATION_MAX_ONCHAIN_PROPOSAL_BYTES: usize = 512 * 1024;

macro_rules! canonical_hash_id {
    ($name:ident, $description:literal) => {
        #[doc = $description]
        #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
        pub struct $name([u8; 32]);

        impl $name {
            /// Construct an identifier from its canonical hash bytes.
            #[must_use]
            pub fn from_bytes(bytes: [u8; 32]) -> Self {
                Self(bytes)
            }

            /// Return the canonical hash bytes.
            #[must_use]
            pub fn to_bytes(self) -> [u8; 32] {
                self.0
            }

            /// Return the lowercase hexadecimal representation.
            #[must_use]
            pub fn to_hex(self) -> String {
                crate::util::hex::encode(self.0)
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(&self.to_hex())
            }
        }

        impl FromStr for $name {
            type Err = FederationError;

            fn from_str(value: &str) -> Result<Self> {
                let bytes = crate::util::hex::decode(value)?;
                let bytes: [u8; 32] = bytes.try_into().map_err(|_| {
                    invalid("canonical on-chain identifier must contain exactly 32 bytes")
                })?;
                Ok(Self(bytes))
            }
        }

        impl Serialize for $name {
            fn serialize<S>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error>
            where
                S: Serializer,
            {
                serializer.serialize_str(&self.to_hex())
            }
        }

        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
            where
                D: Deserializer<'de>,
            {
                String::deserialize(deserializer)?
                    .parse()
                    .map_err(serde::de::Error::custom)
            }
        }
    };
}

canonical_hash_id!(
    FederationOnchainPaymentIntentId,
    "Canonical identifier for one federated on-chain payment intent."
);
canonical_hash_id!(
    FederationOnchainBatchId,
    "Canonical identifier for an ordered federated on-chain payment batch."
);
canonical_hash_id!(
    FederationTransactionProposalId,
    "Canonical identifier for one complete unsigned transaction proposal."
);

/// Epoch, application, network, and policy scope shared by one batch.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FederationTransactionScope {
    /// FROST root key epoch.
    pub root_key_epoch: u64,
    /// Registered application identifier.
    pub application_id: FederationFrostApplicationId,
    /// CDK application key epoch.
    pub application_key_epoch: u64,
    /// Bitcoin network for every destination and wallet script.
    pub network: Network,
    /// Consensus transaction-policy version.
    pub policy_version: u16,
}

impl FederationTransactionScope {
    fn validate(self) -> Result<()> {
        if self.root_key_epoch == 0 || self.application_key_epoch == 0 || self.policy_version == 0 {
            return Err(invalid(
                "transaction scope epochs and policy must be non-zero",
            ));
        }
        Ok(())
    }
}

/// Canonical outgoing on-chain payment authorized by one melt quote.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FederationOnchainPaymentIntent {
    /// Intent schema version.
    pub version: u16,
    /// Unique melt quote authorizing this payment.
    pub quote: String,
    /// Exact recipient script, parsed on the expected network before creation.
    pub destination_script: ScriptBuf,
    /// Recipient value in satoshis.
    pub amount_sat: u64,
    /// Maximum fee contribution assigned to this intent.
    pub maximum_fee_sat: u64,
    /// Selected quote fee-option index.
    pub fee_tier: u32,
    /// Consensus creation time used for deterministic eligibility and ordering.
    pub creation_time: u64,
    /// Wallet and transaction policy scope.
    pub scope: FederationTransactionScope,
}

impl FederationOnchainPaymentIntent {
    /// Construct and validate a canonical payment intent.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        quote: String,
        destination_script: ScriptBuf,
        amount_sat: u64,
        maximum_fee_sat: u64,
        fee_tier: u32,
        creation_time: u64,
        scope: FederationTransactionScope,
    ) -> Result<Self> {
        let intent = Self {
            version: PAYMENT_INTENT_VERSION,
            quote,
            destination_script,
            amount_sat,
            maximum_fee_sat,
            fee_tier,
            creation_time,
            scope,
        };
        intent.validate()?;
        Ok(intent)
    }

    /// Validate bounded, application-neutral intent fields.
    pub fn validate(&self) -> Result<()> {
        self.scope.validate()?;
        if self.version != PAYMENT_INTENT_VERSION {
            return Err(invalid("unsupported payment intent version"));
        }
        if self.quote.is_empty() || self.quote.len() > MAX_QUOTE_ID_BYTES {
            return Err(invalid("payment intent quote is empty or oversized"));
        }
        if self.destination_script.is_empty()
            || self.destination_script.len() > MAX_DESTINATION_SCRIPT_BYTES
        {
            return Err(invalid(
                "payment intent destination script is empty or oversized",
            ));
        }
        if self.amount_sat == 0 || self.maximum_fee_sat == 0 || self.creation_time == 0 {
            return Err(invalid(
                "payment intent amount, maximum fee, and time must be non-zero",
            ));
        }
        if self.fee_tier > MAX_FEE_TIER {
            return Err(invalid("payment intent fee tier is out of range"));
        }
        self.amount_sat
            .checked_add(self.maximum_fee_sat)
            .ok_or_else(|| invalid("payment intent amount and maximum fee overflow"))?;
        Ok(())
    }

    /// Return true once the consensus cutoff reaches the intent creation time.
    #[must_use]
    pub fn is_eligible_at(&self, cutoff_time: u64) -> bool {
        self.creation_time <= cutoff_time
    }

    /// Compute the canonical intent identifier.
    pub fn intent_id(&self) -> Result<FederationOnchainPaymentIntentId> {
        self.validate()?;
        Ok(FederationOnchainPaymentIntentId::from_bytes(
            canonical_hash(PAYMENT_INTENT_HASH_DOMAIN, &canonical_json_bytes(self)?),
        ))
    }

    /// Return the quote-scoped conflict key for accepting this intent.
    pub fn conflict_key(&self) -> Result<FederationOperationConflictKey> {
        self.validate()?;
        Ok(FederationOperationConflictKey::OnchainPaymentIntent {
            quote: self.quote.clone(),
        })
    }

    fn canonical_order(&self, other: &Self) -> Ordering {
        self.fee_tier
            .cmp(&other.fee_tier)
            .then_with(|| self.creation_time.cmp(&other.creation_time))
            .then_with(|| self.quote.cmp(&other.quote))
    }
}

/// Durable lifecycle for one accepted on-chain payment intent.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", content = "data", rename_all = "snake_case")]
pub enum FederationOnchainPaymentIntentState {
    /// Eligible for deterministic batching.
    Pending,
    /// Owned by one accepted transaction proposal.
    Accepted {
        /// Accepted proposal identity.
        proposal_id: FederationTransactionProposalId,
    },
    /// Completed by a confirmed Bitcoin transaction.
    Completed {
        /// Confirmed transaction identifier.
        txid: Txid,
    },
    /// Terminally failed under an accepted policy transition.
    Failed,
}

/// Canonical payment intent together with its durable lifecycle.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FederationOnchainPaymentIntentRecord {
    /// Canonical intent identity.
    pub intent_id: FederationOnchainPaymentIntentId,
    /// Full authorized intent.
    pub intent: FederationOnchainPaymentIntent,
    /// Current consensus lifecycle.
    pub state: FederationOnchainPaymentIntentState,
}

/// Deterministic values independently recomputed before proposal acceptance.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FederationTransactionPolicyAudit {
    /// Canonical proposal identity.
    pub proposal_id: FederationTransactionProposalId,
    /// Transaction ID recomputed from exact unsigned bytes.
    pub txid: Txid,
    /// Checked absolute fee.
    pub fee_sat: u64,
    /// Checked fee rate in satoshis per 1,000 weight units.
    pub fee_rate_sat_per_kwu: u64,
    /// Exact unsigned transaction weight.
    pub weight: u64,
    /// Per-intent fee allocation in canonical intent order.
    pub fee_contributions: BTreeMap<FederationOnchainPaymentIntentId, u64>,
    /// BIP341 key-spend sighashes recomputed in transaction-input order.
    pub taproot_sighashes: Vec<[u8; 32]>,
}

impl FederationOnchainPaymentIntentRecord {
    /// Construct a validated pending intent record.
    pub fn pending(intent: FederationOnchainPaymentIntent) -> Result<Self> {
        let intent_id = intent.intent_id()?;
        Ok(Self {
            intent_id,
            intent,
            state: FederationOnchainPaymentIntentState::Pending,
        })
    }

    /// Validate identity and lifecycle-independent intent invariants.
    pub fn validate(&self) -> Result<()> {
        self.intent.validate()?;
        if self.intent.intent_id()? != self.intent_id {
            return Err(invalid("on-chain intent record identity mismatch"));
        }
        Ok(())
    }
}

/// Taproot key-spend sighash mode committed by a proposal.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FederationTaprootSighashType {
    /// BIP341 default, equivalent to ALL without an appended sighash byte.
    Default,
    /// Sign all outputs.
    All,
    /// Sign no outputs.
    None,
    /// Sign only the corresponding output.
    Single,
    /// Sign all outputs while permitting additional inputs.
    AllAnyoneCanPay,
    /// Sign no outputs while permitting additional inputs.
    NoneAnyoneCanPay,
    /// Sign one output while permitting additional inputs.
    SingleAnyoneCanPay,
}

impl FederationTaprootSighashType {
    /// Convert to the rust-bitcoin Taproot sighash type.
    #[must_use]
    pub fn to_bitcoin(self) -> bitcoin::sighash::TapSighashType {
        match self {
            Self::Default => bitcoin::sighash::TapSighashType::Default,
            Self::All => bitcoin::sighash::TapSighashType::All,
            Self::None => bitcoin::sighash::TapSighashType::None,
            Self::Single => bitcoin::sighash::TapSighashType::Single,
            Self::AllAnyoneCanPay => bitcoin::sighash::TapSighashType::AllPlusAnyoneCanPay,
            Self::NoneAnyoneCanPay => bitcoin::sighash::TapSighashType::NonePlusAnyoneCanPay,
            Self::SingleAnyoneCanPay => bitcoin::sighash::TapSighashType::SinglePlusAnyoneCanPay,
        }
    }

    fn consensus_code(self) -> u8 {
        match self {
            Self::Default => 0,
            Self::All => 1,
            Self::None => 2,
            Self::Single => 3,
            Self::AllAnyoneCanPay => 4,
            Self::NoneAnyoneCanPay => 5,
            Self::SingleAnyoneCanPay => 6,
        }
    }
}

/// Deterministic ordered set of eligible payment intents.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FederationOnchainPaymentBatch {
    /// Batch schema version.
    pub version: u16,
    /// Hash of the exact ordered batch payload.
    pub batch_id: FederationOnchainBatchId,
    /// Intent IDs in canonical fee-tier/time/quote/hash order.
    pub intent_ids: Vec<FederationOnchainPaymentIntentId>,
    /// Complete intents in the same canonical order for independent validation.
    pub intents: Vec<FederationOnchainPaymentIntent>,
    /// Inclusive creation-time cutoff used for eligibility.
    pub eligibility_cutoff: u64,
    /// Shared wallet and transaction policy scope.
    pub scope: FederationTransactionScope,
}

impl FederationOnchainPaymentBatch {
    /// Create a deterministic batch from an unordered set of eligible intents.
    pub fn new(
        mut intents: Vec<FederationOnchainPaymentIntent>,
        eligibility_cutoff: u64,
    ) -> Result<Self> {
        if intents.is_empty() || intents.len() > FEDERATION_MAX_ONCHAIN_BATCH_INTENTS {
            return Err(invalid("on-chain batch intent count is out of bounds"));
        }
        if eligibility_cutoff == 0 {
            return Err(invalid(
                "on-chain batch eligibility cutoff must be non-zero",
            ));
        }
        for intent in &intents {
            intent.validate()?;
            if !intent.is_eligible_at(eligibility_cutoff) {
                return Err(invalid("on-chain batch contains an ineligible intent"));
            }
        }
        let scope = intents[0].scope;
        if intents.iter().any(|intent| intent.scope != scope) {
            return Err(invalid("on-chain batch mixes transaction scopes"));
        }
        intents.sort_by(Self::canonical_intent_order);
        let mut quotes = BTreeSet::new();
        let mut intent_ids = Vec::with_capacity(intents.len());
        for intent in &intents {
            if !quotes.insert(intent.quote.as_str()) {
                return Err(invalid("on-chain batch contains a duplicate quote"));
            }
            intent_ids.push(intent.intent_id()?);
        }
        let batch_id = batch_id(&intent_ids, eligibility_cutoff, scope)?;
        let batch = Self {
            version: PAYMENT_BATCH_VERSION,
            batch_id,
            intent_ids,
            intents,
            eligibility_cutoff,
            scope,
        };
        batch.validate()?;
        Ok(batch)
    }

    /// Validate the ordering, scope, and self-authenticating batch ID.
    pub fn validate(&self) -> Result<()> {
        self.scope.validate()?;
        if self.version != PAYMENT_BATCH_VERSION
            || self.intent_ids.is_empty()
            || self.intent_ids.len() > FEDERATION_MAX_ONCHAIN_BATCH_INTENTS
            || self.intents.len() != self.intent_ids.len()
            || self.eligibility_cutoff == 0
        {
            return Err(invalid("on-chain batch version or bounds are invalid"));
        }
        let unique = self.intent_ids.iter().copied().collect::<BTreeSet<_>>();
        if unique.len() != self.intent_ids.len() {
            return Err(invalid("on-chain batch contains duplicate intent ids"));
        }
        let mut canonical = self.intents.clone();
        for intent in &canonical {
            intent.validate()?;
            if intent.scope != self.scope || !intent.is_eligible_at(self.eligibility_cutoff) {
                return Err(invalid(
                    "on-chain batch intent scope or eligibility is invalid",
                ));
            }
        }
        canonical.sort_by(Self::canonical_intent_order);
        if canonical != self.intents {
            return Err(invalid(
                "on-chain batch intents are not canonically ordered",
            ));
        }
        let mut quotes = BTreeSet::new();
        let expected_ids = canonical
            .iter()
            .map(|intent| {
                if !quotes.insert(intent.quote.as_str()) {
                    return Err(invalid("on-chain batch contains a duplicate quote"));
                }
                intent.intent_id()
            })
            .collect::<Result<Vec<_>>>()?;
        if expected_ids != self.intent_ids {
            return Err(invalid(
                "on-chain batch intent ids do not match their payloads",
            ));
        }
        if self.batch_id != batch_id(&self.intent_ids, self.eligibility_cutoff, self.scope)? {
            return Err(invalid("on-chain batch id does not match its payload"));
        }
        Ok(())
    }

    fn canonical_intent_order(
        left: &FederationOnchainPaymentIntent,
        right: &FederationOnchainPaymentIntent,
    ) -> Ordering {
        left.canonical_order(right)
    }
}

/// Exact previous output and wallet derivation metadata for one transaction input.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FederationTransactionInput {
    /// Input outpoint, which must match the unsigned transaction at this position.
    pub outpoint: OutPoint,
    /// Trusted previous output used for value accounting and Taproot sighashes.
    pub previous_output: TxOut,
    /// Consensus allocation that derives the spent wallet script.
    pub allocation: FederationAllocatedAddressRef,
}

/// Intent-to-transaction-output assignment.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FederationTransactionRecipientOutput {
    /// Intent satisfied by this output.
    pub intent_id: FederationOnchainPaymentIntentId,
    /// Output position in the exact unsigned transaction.
    pub output_index: u32,
    /// Exact output duplicated here to make assignments independently auditable.
    pub output: TxOut,
    /// Deterministic share of the transaction fee charged to this intent.
    pub fee_contribution_sat: u64,
}

/// Consensus-allocated internal change output.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FederationTransactionChangeOutput {
    /// Internal descriptor derivation index.
    pub derivation_index: u32,
    /// Output position in the exact unsigned transaction.
    pub output_index: u32,
    /// Exact output duplicated here for deterministic validation.
    pub output: TxOut,
}

/// Complete bounded candidate transaction, before policy acceptance or signing.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FederationTransactionProposal {
    /// Proposal schema version.
    pub version: u16,
    /// Deterministic payment batch.
    pub batch_id: FederationOnchainBatchId,
    /// Canonically ordered intent IDs.
    pub intent_ids: Vec<FederationOnchainPaymentIntentId>,
    /// Inclusive intent-creation cutoff committed by the batch ID.
    pub eligibility_cutoff: u64,
    /// FROST/application/network/policy binding.
    pub scope: FederationTransactionScope,
    /// Diagnostic proposer identity; this grants no authority.
    pub proposer: MemberId,
    /// Selected wallet inputs in unsigned-transaction order.
    pub selected_inputs: Vec<FederationTransactionInput>,
    /// Recipient assignments ordered by transaction output index.
    pub recipient_outputs: Vec<FederationTransactionRecipientOutput>,
    /// Optional consensus-allocated internal change assignment.
    pub change_output: Option<FederationTransactionChangeOutput>,
    /// Exact unsigned Bitcoin transaction.
    pub unsigned_transaction: Transaction,
    /// Exact absolute fee in satoshis.
    pub fee_sat: u64,
    /// Integer ceiling of satoshis per 1,000 weight units.
    pub fee_rate_sat_per_kwu: u64,
    /// Taproot key-spend sighash mode required for every input.
    pub sighash_type: FederationTaprootSighashType,
    /// Commitment to the transaction and ordered witness UTXOs only.
    pub psbt_commitment: [u8; 32],
    /// Optional proposal replaced by this candidate.
    pub replacement_of: Option<FederationTransactionProposalId>,
    /// Stable lineage root used to serialize original and replacement candidates.
    pub replacement_lineage: FederationOnchainBatchId,
}

impl FederationTransactionProposal {
    /// Construct and normalize a proposal from proposer-local metadata order.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        batch: &FederationOnchainPaymentBatch,
        proposer: MemberId,
        unsigned_transaction: Transaction,
        selected_inputs: Vec<FederationTransactionInput>,
        recipient_outputs: Vec<FederationTransactionRecipientOutput>,
        change_output: Option<FederationTransactionChangeOutput>,
        fee_sat: u64,
        replacement_of: Option<FederationTransactionProposalId>,
    ) -> Result<Self> {
        batch.validate()?;
        let mut proposal = Self {
            version: TRANSACTION_PROPOSAL_VERSION,
            batch_id: batch.batch_id,
            intent_ids: batch.intent_ids.clone(),
            eligibility_cutoff: batch.eligibility_cutoff,
            scope: batch.scope,
            proposer,
            selected_inputs,
            recipient_outputs,
            change_output,
            unsigned_transaction,
            fee_sat,
            fee_rate_sat_per_kwu: 0,
            sighash_type: FederationTaprootSighashType::Default,
            psbt_commitment: [0; 32],
            replacement_of,
            replacement_lineage: batch.batch_id,
        };
        proposal.normalize_metadata()?;
        proposal.fee_rate_sat_per_kwu = proposal.expected_fee_rate_sat_per_kwu()?;
        proposal.psbt_commitment = proposal.expected_psbt_commitment()?;
        proposal.validate()?;
        Ok(proposal)
    }

    /// Decode, normalize, and fully validate a JSON proposal.
    pub fn from_json_slice(bytes: &[u8]) -> Result<Self> {
        if bytes.len() > FEDERATION_MAX_ONCHAIN_PROPOSAL_BYTES {
            return Err(invalid(
                "on-chain proposal JSON exceeds the canonical size limit",
            ));
        }
        let mut proposal: Self = serde_json::from_slice(bytes)?;
        proposal.normalize_metadata()?;
        proposal.validate()?;
        Ok(proposal)
    }

    /// Validate structural, arithmetic, ordering, and size invariants.
    pub fn validate(&self) -> Result<()> {
        self.scope.validate()?;
        if self.version != TRANSACTION_PROPOSAL_VERSION {
            return Err(invalid("unsupported transaction proposal version"));
        }
        if self.intent_ids.is_empty()
            || self.intent_ids.len() > FEDERATION_MAX_ONCHAIN_BATCH_INTENTS
            || self.eligibility_cutoff == 0
            || self.selected_inputs.is_empty()
            || self.selected_inputs.len() > FEDERATION_MAX_ONCHAIN_PROPOSAL_INPUTS
            || self.unsigned_transaction.output.is_empty()
            || self.unsigned_transaction.output.len() > FEDERATION_MAX_ONCHAIN_PROPOSAL_OUTPUTS
        {
            return Err(invalid("transaction proposal count is out of bounds"));
        }
        if self.selected_inputs.len() != self.unsigned_transaction.input.len() {
            return Err(invalid(
                "transaction proposal previous-output count differs from inputs",
            ));
        }
        if self.recipient_outputs.len() != self.intent_ids.len()
            || self.unsigned_transaction.output.len()
                != self.recipient_outputs.len() + usize::from(self.change_output.is_some())
        {
            return Err(invalid(
                "transaction proposal output assignments are incomplete",
            ));
        }
        if self.replacement_lineage != self.batch_id {
            return Err(invalid(
                "transaction proposal replacement lineage is not batch-bound",
            ));
        }

        let intent_set = self.intent_ids.iter().copied().collect::<BTreeSet<_>>();
        let recipient_set = self
            .recipient_outputs
            .iter()
            .map(|recipient| recipient.intent_id)
            .collect::<BTreeSet<_>>();
        if intent_set.len() != self.intent_ids.len()
            || recipient_set.len() != self.recipient_outputs.len()
            || intent_set != recipient_set
        {
            return Err(invalid(
                "transaction proposal has duplicate or missing intent assignments",
            ));
        }

        let mut input_outpoints = BTreeSet::new();
        for (position, selected) in self.selected_inputs.iter().enumerate() {
            let txin = self
                .unsigned_transaction
                .input
                .get(position)
                .ok_or_else(|| invalid("transaction proposal input position is missing"))?;
            if selected.outpoint != txin.previous_output
                || !txin.script_sig.is_empty()
                || !txin.witness.is_empty()
            {
                return Err(invalid(
                    "transaction proposal is not a canonical unsigned transaction",
                ));
            }
            if !input_outpoints.insert(selected.outpoint) {
                return Err(invalid("transaction proposal contains a duplicate input"));
            }
            if selected.previous_output.value.to_sat() == 0 {
                return Err(invalid(
                    "transaction proposal contains a zero-valued previous output",
                ));
            }
            if selected.allocation.application_id != self.scope.application_id
                || selected.allocation.application_key_epoch != self.scope.application_key_epoch
            {
                return Err(invalid(
                    "transaction input allocation uses the wrong application scope",
                ));
            }
        }

        let mut output_indexes = BTreeSet::new();
        for recipient in &self.recipient_outputs {
            let output_index = usize::try_from(recipient.output_index)
                .map_err(|_| invalid("recipient output index does not fit usize"))?;
            if !output_indexes.insert(recipient.output_index)
                || self.unsigned_transaction.output.get(output_index) != Some(&recipient.output)
                || recipient.output.value.to_sat() == 0
            {
                return Err(invalid(
                    "transaction recipient assignment is duplicate or mismatched",
                ));
            }
        }
        if let Some(change) = &self.change_output {
            let output_index = usize::try_from(change.output_index)
                .map_err(|_| invalid("change output index does not fit usize"))?;
            if change.derivation_index > FEDERATION_MAX_DERIVATION_INDEX
                || !output_indexes.insert(change.output_index)
                || self.unsigned_transaction.output.get(output_index) != Some(&change.output)
                || change.output.value.to_sat() == 0
            {
                return Err(invalid(
                    "transaction change assignment is duplicate or mismatched",
                ));
            }
        }
        if output_indexes.len() != self.unsigned_transaction.output.len() {
            return Err(invalid(
                "transaction proposal contains an unassigned output",
            ));
        }
        let assigned_fee = self
            .recipient_outputs
            .iter()
            .try_fold(0_u64, |sum, recipient| {
                sum.checked_add(recipient.fee_contribution_sat)
                    .ok_or_else(|| invalid("transaction proposal fee allocation overflow"))
            })?;
        if assigned_fee != self.fee_sat {
            return Err(invalid(
                "transaction proposal fee allocations do not equal the transaction fee",
            ));
        }

        let input_sum = checked_txout_sum(
            self.selected_inputs
                .iter()
                .map(|selected| &selected.previous_output),
        )?;
        let output_sum = checked_txout_sum(self.unsigned_transaction.output.iter())?;
        let expected_fee = input_sum
            .checked_sub(output_sum)
            .ok_or_else(|| invalid("transaction proposal output value exceeds input value"))?;
        if self.fee_sat != expected_fee {
            return Err(invalid(
                "transaction proposal fee does not match input/output sums",
            ));
        }

        let weight = self.unsigned_transaction.weight().to_wu();
        if weight == 0 || weight > FEDERATION_MAX_ONCHAIN_TRANSACTION_WEIGHT {
            return Err(invalid("transaction proposal weight is out of bounds"));
        }
        if self.fee_rate_sat_per_kwu != self.expected_fee_rate_sat_per_kwu()? {
            return Err(invalid(
                "transaction proposal fee rate does not match fee and weight",
            ));
        }
        if self.psbt_consensus_bytes()?.len() > FEDERATION_MAX_ONCHAIN_PSBT_CONSENSUS_BYTES {
            return Err(invalid(
                "transaction proposal PSBT consensus data is oversized",
            ));
        }
        if self.psbt_commitment != self.expected_psbt_commitment()? {
            return Err(invalid("transaction proposal PSBT commitment is invalid"));
        }
        if self.canonical_bytes_unchecked()?.len() > FEDERATION_MAX_ONCHAIN_PROPOSAL_BYTES {
            return Err(invalid(
                "transaction proposal canonical encoding is oversized",
            ));
        }
        Ok(())
    }

    /// Validate the proposal against an independently selected public scope.
    pub fn validate_scope(&self, expected: FederationTransactionScope) -> Result<()> {
        expected.validate()?;
        if self.scope != expected {
            return Err(invalid(
                "transaction proposal uses the wrong epoch, network, or policy",
            ));
        }
        Ok(())
    }

    /// Return canonical JSON after normalizing non-semantic metadata ordering.
    pub fn canonical_bytes(&self) -> Result<Vec<u8>> {
        let mut normalized = self.clone();
        normalized.normalize_metadata()?;
        normalized.validate()?;
        normalized.canonical_bytes_unchecked()
    }

    /// Compute the canonical proposal identifier.
    pub fn proposal_id(&self) -> Result<FederationTransactionProposalId> {
        Ok(FederationTransactionProposalId::from_bytes(canonical_hash(
            TRANSACTION_PROPOSAL_HASH_DOMAIN,
            &self.canonical_bytes()?,
        )))
    }

    /// Return the trusted previous outputs in transaction-input order.
    #[must_use]
    pub fn previous_outputs(&self) -> Vec<TxOut> {
        self.selected_inputs
            .iter()
            .map(|input| input.previous_output.clone())
            .collect()
    }

    /// Return all conflict scopes required when this proposal enters consensus.
    pub fn conflict_keys(&self) -> Result<Vec<FederationOperationConflictKey>> {
        self.validate()?;
        let mut keys = Vec::with_capacity(self.intent_ids.len() + self.selected_inputs.len() + 2);
        keys.push(FederationOperationConflictKey::OnchainBatch {
            batch_id: self.batch_id,
        });
        keys.push(FederationOperationConflictKey::OnchainReplacementLineage {
            lineage: self.replacement_lineage,
        });
        keys.extend(self.intent_ids.iter().copied().map(|intent_id| {
            FederationOperationConflictKey::OnchainPaymentIntentAssignment { intent_id }
        }));
        keys.extend(self.selected_inputs.iter().map(|input| {
            FederationOperationConflictKey::OnchainUtxo {
                outpoint: input.outpoint,
            }
        }));
        Ok(keys)
    }

    fn normalize_metadata(&mut self) -> Result<()> {
        let position_by_outpoint = self
            .unsigned_transaction
            .input
            .iter()
            .enumerate()
            .map(|(position, input)| (input.previous_output, position))
            .collect::<BTreeMap<_, _>>();
        if position_by_outpoint.len() != self.unsigned_transaction.input.len() {
            return Err(invalid("unsigned transaction contains duplicate inputs"));
        }
        self.selected_inputs.sort_by_key(|input| {
            position_by_outpoint
                .get(&input.outpoint)
                .copied()
                .unwrap_or(usize::MAX)
        });
        self.recipient_outputs
            .sort_by_key(|recipient| recipient.output_index);
        Ok(())
    }

    fn expected_fee_rate_sat_per_kwu(&self) -> Result<u64> {
        let weight = self.unsigned_transaction.weight().to_wu();
        if weight == 0 {
            return Err(invalid("transaction proposal weight is zero"));
        }
        self.fee_sat
            .checked_mul(1_000)
            .and_then(|scaled| scaled.checked_add(weight - 1))
            .map(|scaled| scaled / weight)
            .ok_or_else(|| invalid("transaction proposal fee-rate calculation overflow"))
    }

    fn psbt_consensus_bytes(&self) -> Result<Vec<u8>> {
        let mut bytes = Vec::new();
        append_bytes(&mut bytes, PSBT_CONSENSUS_HASH_DOMAIN)?;
        append_bytes(&mut bytes, &serialize(&self.unsigned_transaction))?;
        append_len(&mut bytes, self.selected_inputs.len())?;
        for input in &self.selected_inputs {
            append_bytes(&mut bytes, &serialize(&input.outpoint))?;
            append_bytes(&mut bytes, &serialize(&input.previous_output))?;
        }
        Ok(bytes)
    }

    fn expected_psbt_commitment(&self) -> Result<[u8; 32]> {
        Ok(sha256::Hash::hash(&self.psbt_consensus_bytes()?).to_byte_array())
    }

    fn canonical_bytes_unchecked(&self) -> Result<Vec<u8>> {
        canonical_json_bytes(self)
    }
}

/// Durable lifecycle of a policy-accepted transaction.
///
/// Abandonment and replacement are deliberately sticky: neither transition
/// releases inputs, because an accepted transaction may still reach Bitcoin.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", content = "data", rename_all = "snake_case")]
pub enum FederationAcceptedTransactionState {
    /// Accepted by federation consensus and eligible for threshold signing.
    Accepted,
    /// No longer preferred, while every input remains reserved.
    Abandoned,
    /// Superseded by a consensus-designated replacement proposal.
    Replaced {
        /// Replacement proposal identity.
        by: FederationTransactionProposalId,
    },
}

/// Exact durable transaction and policy audit accepted at one journal entry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FederationAcceptedTransaction {
    /// Complete unsigned transaction proposal.
    pub proposal: FederationTransactionProposal,
    /// Independently recomputed policy values and Taproot sighashes.
    pub policy_audit: FederationTransactionPolicyAudit,
    /// Consensus operation that first accepted this proposal.
    pub accepted_operation_id: FederationOperationId,
    /// Accepted journal position.
    pub accepted_operation_index: u64,
    /// Sticky transaction lifecycle.
    pub state: FederationAcceptedTransactionState,
}

/// CDK-specific authoritative reference committed into one FROST input slot.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FederationFrostSigningAuthorizationReference {
    accepted_operation_id: FederationOperationId,
    proposal_id: FederationTransactionProposalId,
    txid: Txid,
    input_index: u32,
    sighash_type: FederationTaprootSighashType,
    sighash: [u8; 32],
}

impl FederationFrostSigningAuthorizationReference {
    /// Build a reference only from an internally consistent accepted transaction.
    pub fn new(accepted: &FederationAcceptedTransaction, input_index: usize) -> Result<Self> {
        if accepted.state != FederationAcceptedTransactionState::Accepted {
            return Err(invalid(
                "FROST signing authorization requires an active accepted transaction",
            ));
        }
        accepted.proposal.validate()?;
        validate_policy_audit(&accepted.proposal, &accepted.policy_audit)?;
        let input_index = u32::try_from(input_index)
            .map_err(|_| invalid("FROST signing input index does not fit u32"))?;
        let sighash = accepted
            .policy_audit
            .taproot_sighashes
            .get(input_index as usize)
            .copied()
            .ok_or_else(|| invalid("FROST signing input index is out of bounds"))?;
        Ok(Self {
            accepted_operation_id: accepted.accepted_operation_id,
            proposal_id: accepted.policy_audit.proposal_id,
            txid: accepted.policy_audit.txid,
            input_index,
            sighash_type: accepted.proposal.sighash_type,
            sighash,
        })
    }

    /// Consensus operation that authorized the transaction.
    pub fn accepted_operation_id(self) -> FederationOperationId {
        self.accepted_operation_id
    }

    /// Exact accepted proposal identity.
    pub fn proposal_id(self) -> FederationTransactionProposalId {
        self.proposal_id
    }

    /// Bitcoin transaction identity of the exact unsigned transaction.
    pub fn txid(self) -> Txid {
        self.txid
    }

    /// Input-specific signing slot.
    pub fn input_index(self) -> u32 {
        self.input_index
    }

    /// Accepted Taproot sighash mode.
    pub fn sighash_type(self) -> FederationTaprootSighashType {
        self.sighash_type
    }

    /// Independently recomputed input sighash used as the message commitment.
    pub fn sighash(self) -> [u8; 32] {
        self.sighash
    }

    /// Canonical authorization commitment supplied to the FROST key manager.
    pub fn commitment(self) -> [u8; 32] {
        let mut payload = Vec::with_capacity(32 + 32 + 32 + 4 + 1 + 32);
        payload.extend_from_slice(&self.accepted_operation_id.to_bytes());
        payload.extend_from_slice(&self.proposal_id.to_bytes());
        payload.extend_from_slice(&self.txid.to_byte_array());
        payload.extend_from_slice(&self.input_index.to_be_bytes());
        payload.push(self.sighash_type.consensus_code());
        payload.extend_from_slice(&self.sighash);
        canonical_hash(FROST_SIGNING_AUTHORIZATION_HASH_DOMAIN, &payload)
    }
}

/// Result of adding an intent or accepting a transaction to durable state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FederationTransactionStateOutcome {
    /// New state was materialized.
    Updated,
    /// An exact replay was already materialized.
    Existing,
}

/// Versioned consensus projection for outgoing intents and wallet reservations.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FederationTransactionState {
    version: u16,
    intents: BTreeMap<FederationOnchainPaymentIntentId, FederationOnchainPaymentIntentRecord>,
    quote_intents: BTreeMap<String, FederationOnchainPaymentIntentId>,
    accepted_transactions: BTreeMap<FederationTransactionProposalId, FederationAcceptedTransaction>,
    batch_owners: BTreeMap<FederationOnchainBatchId, FederationTransactionProposalId>,
    utxo_reservations: BTreeMap<String, FederationTransactionProposalId>,
}

impl Default for FederationTransactionState {
    fn default() -> Self {
        Self::new()
    }
}

impl FederationTransactionState {
    /// Construct an empty current-version transaction projection.
    #[must_use]
    pub fn new() -> Self {
        Self {
            version: TRANSACTION_STATE_VERSION,
            intents: BTreeMap::new(),
            quote_intents: BTreeMap::new(),
            accepted_transactions: BTreeMap::new(),
            batch_owners: BTreeMap::new(),
            utxo_reservations: BTreeMap::new(),
        }
    }

    /// Decode and fully validate durable transaction state.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self> {
        let state: Self = serde_json::from_slice(bytes)?;
        state.validate()?;
        Ok(state)
    }

    /// Encode fully validated durable transaction state.
    pub fn to_bytes(&self) -> Result<Vec<u8>> {
        self.validate()?;
        Ok(serde_json::to_vec(self)?)
    }

    /// Return all payment intents keyed by canonical identity.
    #[must_use]
    pub fn intents(
        &self,
    ) -> &BTreeMap<FederationOnchainPaymentIntentId, FederationOnchainPaymentIntentRecord> {
        &self.intents
    }

    /// Look up an intent by its authorizing quote.
    #[must_use]
    pub fn intent_for_quote(&self, quote: &str) -> Option<&FederationOnchainPaymentIntentRecord> {
        self.quote_intents
            .get(quote)
            .and_then(|intent_id| self.intents.get(intent_id))
    }

    /// Look up an accepted transaction.
    #[must_use]
    pub fn accepted_transaction(
        &self,
        proposal_id: FederationTransactionProposalId,
    ) -> Option<&FederationAcceptedTransaction> {
        self.accepted_transactions.get(&proposal_id)
    }

    /// Return the proposal that owns one wallet reservation.
    #[must_use]
    pub fn reservation(&self, outpoint: OutPoint) -> Option<FederationTransactionProposalId> {
        self.utxo_reservations.get(&outpoint.to_string()).copied()
    }

    /// Add one consensus-authorized pending melt intent idempotently.
    pub fn add_pending_intent(
        &mut self,
        record: FederationOnchainPaymentIntentRecord,
    ) -> Result<FederationTransactionStateOutcome> {
        record.validate()?;
        if record.state != FederationOnchainPaymentIntentState::Pending {
            return Err(invalid("new on-chain payment intent is not pending"));
        }
        if let Some(existing_id) = self.quote_intents.get(&record.intent.quote) {
            let existing = self
                .intents
                .get(existing_id)
                .ok_or_else(|| invalid("on-chain quote index is corrupt"))?;
            if existing == &record {
                return Ok(FederationTransactionStateOutcome::Existing);
            }
            return Err(invalid(
                "on-chain melt quote already authorizes a different payment intent",
            ));
        }
        if self.intents.contains_key(&record.intent_id) {
            return Err(invalid(
                "on-chain payment intent identity is already assigned to another quote",
            ));
        }
        self.quote_intents
            .insert(record.intent.quote.clone(), record.intent_id);
        self.intents.insert(record.intent_id, record);
        self.validate()?;
        Ok(FederationTransactionStateOutcome::Updated)
    }

    /// Accept an independently validated transaction and reserve all its scopes.
    pub fn accept_transaction(
        &mut self,
        proposal: FederationTransactionProposal,
        policy_audit: FederationTransactionPolicyAudit,
        accepted_operation_id: FederationOperationId,
        accepted_operation_index: u64,
    ) -> Result<FederationTransactionStateOutcome> {
        proposal.validate()?;
        if accepted_operation_index == 0 {
            return Err(invalid("accepted transaction operation index is zero"));
        }
        let proposal_id = proposal.proposal_id()?;
        validate_policy_audit(&proposal, &policy_audit)?;
        if let Some(existing) = self.accepted_transactions.get(&proposal_id) {
            if existing.proposal == proposal && existing.policy_audit == policy_audit {
                return Ok(FederationTransactionStateOutcome::Existing);
            }
            return Err(invalid(
                "accepted proposal identity maps to different durable transaction data",
            ));
        }
        if self
            .batch_owners
            .get(&proposal.batch_id)
            .is_some_and(|owner| *owner != proposal_id)
        {
            return Err(invalid(
                "on-chain batch is already owned by another accepted transaction",
            ));
        }
        for intent_id in &proposal.intent_ids {
            let record = self
                .intents
                .get(intent_id)
                .ok_or_else(|| invalid("accepted transaction references an unknown intent"))?;
            if record.state != FederationOnchainPaymentIntentState::Pending {
                return Err(invalid(
                    "accepted transaction references an intent that is not pending",
                ));
            }
        }
        for input in &proposal.selected_inputs {
            if self
                .utxo_reservations
                .get(&input.outpoint.to_string())
                .is_some_and(|owner| *owner != proposal_id)
            {
                return Err(invalid(
                    "wallet UTXO is already reserved by another accepted transaction",
                ));
            }
        }

        for intent_id in &proposal.intent_ids {
            let record = self
                .intents
                .get_mut(intent_id)
                .ok_or_else(|| invalid("accepted transaction intent disappeared"))?;
            record.state = FederationOnchainPaymentIntentState::Accepted { proposal_id };
        }
        for input in &proposal.selected_inputs {
            self.utxo_reservations
                .insert(input.outpoint.to_string(), proposal_id);
        }
        self.batch_owners.insert(proposal.batch_id, proposal_id);
        self.accepted_transactions.insert(
            proposal_id,
            FederationAcceptedTransaction {
                proposal,
                policy_audit,
                accepted_operation_id,
                accepted_operation_index,
                state: FederationAcceptedTransactionState::Accepted,
            },
        );
        self.validate()?;
        Ok(FederationTransactionStateOutcome::Updated)
    }

    /// Mark a transaction abandoned without releasing any input reservation.
    pub fn abandon_transaction(
        &mut self,
        proposal_id: FederationTransactionProposalId,
    ) -> Result<()> {
        let accepted = self
            .accepted_transactions
            .get_mut(&proposal_id)
            .ok_or_else(|| invalid("cannot abandon an unknown accepted transaction"))?;
        accepted.state = FederationAcceptedTransactionState::Abandoned;
        self.validate()
    }

    /// Record a replacement relationship without releasing original inputs.
    pub fn mark_transaction_replaced(
        &mut self,
        proposal_id: FederationTransactionProposalId,
        replacement_id: FederationTransactionProposalId,
    ) -> Result<()> {
        if proposal_id == replacement_id {
            return Err(invalid("transaction replacement identity is invalid"));
        }
        let original = self
            .accepted_transactions
            .get_mut(&proposal_id)
            .ok_or_else(|| invalid("cannot replace an unknown accepted transaction"))?;
        original.state = FederationAcceptedTransactionState::Replaced { by: replacement_id };
        self.validate()
    }

    /// Build stable state-digest rows for every intent, transaction, and reservation.
    pub fn materialized_rows(&self) -> Result<Vec<FederationMaterializedStateRow>> {
        self.validate()?;
        let mut rows = Vec::new();
        for (intent_id, record) in &self.intents {
            rows.push(
                FederationMaterializedStateRow::new(
                    "federation_onchain_payment_intents",
                    intent_id.to_string(),
                )
                .with_field("record", serde_json::to_string(record)?),
            );
        }
        for (proposal_id, accepted) in &self.accepted_transactions {
            rows.push(
                FederationMaterializedStateRow::new(
                    "federation_onchain_transactions",
                    proposal_id.to_string(),
                )
                .with_field("accepted", serde_json::to_string(accepted)?),
            );
        }
        for (outpoint, proposal_id) in &self.utxo_reservations {
            rows.push(
                FederationMaterializedStateRow::new(
                    "federation_onchain_utxo_reservations",
                    outpoint.clone(),
                )
                .with_field("proposal_id", proposal_id.to_string()),
            );
        }
        Ok(rows)
    }

    /// Validate every identity, ownership index, and sticky reservation.
    pub fn validate(&self) -> Result<()> {
        if self.version != TRANSACTION_STATE_VERSION {
            return Err(invalid("unsupported transaction state version"));
        }
        if self.intents.len() != self.quote_intents.len() {
            return Err(invalid("on-chain intent quote index has the wrong size"));
        }
        for (intent_id, record) in &self.intents {
            record.validate()?;
            if intent_id != &record.intent_id
                || self.quote_intents.get(&record.intent.quote) != Some(intent_id)
            {
                return Err(invalid("on-chain intent durable identity mismatch"));
            }
            if let FederationOnchainPaymentIntentState::Accepted { proposal_id } = record.state {
                let accepted = self
                    .accepted_transactions
                    .get(&proposal_id)
                    .ok_or_else(|| invalid("accepted intent references an unknown transaction"))?;
                if !accepted.proposal.intent_ids.contains(intent_id) {
                    return Err(invalid(
                        "accepted intent is absent from its owning transaction",
                    ));
                }
            }
        }
        if self.accepted_transactions.len() != self.batch_owners.len() {
            return Err(invalid(
                "accepted transaction batch index has the wrong size",
            ));
        }
        for (proposal_id, accepted) in &self.accepted_transactions {
            accepted.proposal.validate()?;
            if accepted.accepted_operation_index == 0
                || accepted.proposal.proposal_id()? != *proposal_id
                || self.batch_owners.get(&accepted.proposal.batch_id) != Some(proposal_id)
            {
                return Err(invalid("accepted transaction durable identity mismatch"));
            }
            validate_policy_audit(&accepted.proposal, &accepted.policy_audit)?;
            for intent_id in &accepted.proposal.intent_ids {
                let record = self
                    .intents
                    .get(intent_id)
                    .ok_or_else(|| invalid("accepted transaction references a missing intent"))?;
                if record.state
                    != (FederationOnchainPaymentIntentState::Accepted {
                        proposal_id: *proposal_id,
                    })
                {
                    return Err(invalid("accepted transaction does not own its intent"));
                }
            }
            for input in &accepted.proposal.selected_inputs {
                if self.utxo_reservations.get(&input.outpoint.to_string()) != Some(proposal_id) {
                    return Err(invalid(
                        "accepted transaction does not own its wallet reservation",
                    ));
                }
            }
            if let FederationAcceptedTransactionState::Replaced { by } = accepted.state {
                if by == *proposal_id {
                    return Err(invalid("transaction cannot replace itself"));
                }
                if let Some(replacement) = self.accepted_transactions.get(&by) {
                    if replacement.proposal.replacement_lineage
                        != accepted.proposal.replacement_lineage
                    {
                        return Err(invalid("replacement lineage is inconsistent"));
                    }
                }
            }
        }
        for (outpoint, proposal_id) in &self.utxo_reservations {
            let parsed = OutPoint::from_str(outpoint)
                .map_err(|_| invalid("wallet reservation outpoint is invalid"))?;
            let accepted = self
                .accepted_transactions
                .get(proposal_id)
                .ok_or_else(|| invalid("wallet reservation references an unknown transaction"))?;
            if !accepted
                .proposal
                .selected_inputs
                .iter()
                .any(|input| input.outpoint == parsed)
            {
                return Err(invalid("wallet reservation is not a transaction input"));
            }
        }
        Ok(())
    }
}

fn validate_policy_audit(
    proposal: &FederationTransactionProposal,
    audit: &FederationTransactionPolicyAudit,
) -> Result<()> {
    if audit.proposal_id != proposal.proposal_id()?
        || audit.txid != proposal.unsigned_transaction.compute_txid()
        || audit.fee_sat != proposal.fee_sat
        || audit.fee_rate_sat_per_kwu != proposal.fee_rate_sat_per_kwu
        || audit.weight != proposal.unsigned_transaction.weight().to_wu()
        || audit.taproot_sighashes.len() != proposal.selected_inputs.len()
    {
        return Err(invalid(
            "transaction policy audit does not match its proposal",
        ));
    }
    let expected_fees = proposal
        .recipient_outputs
        .iter()
        .map(|recipient| (recipient.intent_id, recipient.fee_contribution_sat))
        .collect::<BTreeMap<_, _>>();
    if audit.fee_contributions != expected_fees {
        return Err(invalid("transaction policy audit fee allocation mismatch"));
    }
    Ok(())
}

fn batch_id(
    intent_ids: &[FederationOnchainPaymentIntentId],
    eligibility_cutoff: u64,
    scope: FederationTransactionScope,
) -> Result<FederationOnchainBatchId> {
    #[derive(Serialize)]
    struct Payload<'a> {
        version: u16,
        intent_ids: &'a [FederationOnchainPaymentIntentId],
        eligibility_cutoff: u64,
        scope: FederationTransactionScope,
    }

    let payload = Payload {
        version: PAYMENT_BATCH_VERSION,
        intent_ids,
        eligibility_cutoff,
        scope,
    };
    Ok(FederationOnchainBatchId::from_bytes(canonical_hash(
        PAYMENT_BATCH_HASH_DOMAIN,
        &canonical_json_bytes(&payload)?,
    )))
}

fn checked_txout_sum<'a, I>(outputs: I) -> Result<u64>
where
    I: IntoIterator<Item = &'a TxOut>,
{
    outputs.into_iter().try_fold(0u64, |sum, output| {
        sum.checked_add(output.value.to_sat())
            .ok_or_else(|| invalid("transaction value sum overflow"))
    })
}

fn canonical_json_bytes<T>(value: &T) -> Result<Vec<u8>>
where
    T: Serialize + ?Sized,
{
    let mut value = serde_json::to_value(value)?;
    value.sort_all_objects();
    Ok(serde_json::to_vec(&value)?)
}

fn canonical_hash(domain: &[u8], payload: &[u8]) -> [u8; 32] {
    let mut engine = sha256::Hash::engine();
    engine.input(&(domain.len() as u64).to_be_bytes());
    engine.input(domain);
    engine.input(&(payload.len() as u64).to_be_bytes());
    engine.input(payload);
    sha256::Hash::from_engine(engine).to_byte_array()
}

fn append_len(bytes: &mut Vec<u8>, len: usize) -> Result<()> {
    let len = u64::try_from(len).map_err(|_| invalid("canonical length does not fit u64"))?;
    bytes.extend_from_slice(&len.to_be_bytes());
    Ok(())
}

fn append_bytes(bytes: &mut Vec<u8>, value: &[u8]) -> Result<()> {
    append_len(bytes, value.len())?;
    bytes.extend_from_slice(value);
    Ok(())
}

fn invalid(reason: impl Into<String>) -> FederationError {
    FederationError::TransactionProposalInvalid {
        reason: reason.into(),
    }
}

#[cfg(test)]
mod tests {
    use bitcoin::absolute::LockTime;
    use bitcoin::hashes::Hash;
    use bitcoin::sighash::{Prevouts, SighashCache, TapSighashType};
    use bitcoin::transaction::Version;
    use bitcoin::{Amount, Sequence, TxIn, Txid, Witness};
    use rand::rngs::StdRng;
    use rand::{Rng, SeedableRng};

    use super::*;
    use crate::federation::FederationAddressKeychain;

    fn member(value: u16) -> MemberId {
        MemberId::new(value).expect("member")
    }

    fn scope() -> FederationTransactionScope {
        FederationTransactionScope {
            root_key_epoch: 2,
            application_id: FederationFrostApplicationId::from_bytes([7; 32])
                .expect("application id"),
            application_key_epoch: 3,
            network: Network::Regtest,
            policy_version: 1,
        }
    }

    fn intent(
        quote: &str,
        fee_tier: u32,
        creation_time: u64,
        marker: u8,
    ) -> FederationOnchainPaymentIntent {
        FederationOnchainPaymentIntent::new(
            quote.to_string(),
            ScriptBuf::from_bytes(vec![0x51, marker]),
            10_000 + u64::from(marker),
            1_000,
            fee_tier,
            creation_time,
            scope(),
        )
        .expect("intent")
    }

    fn outpoint(marker: u8, vout: u32) -> OutPoint {
        OutPoint::new(Txid::from_byte_array([marker; 32]), vout)
    }

    fn allocation(marker: u8, outpoint: OutPoint) -> FederationTransactionInput {
        FederationTransactionInput {
            outpoint,
            previous_output: TxOut {
                value: Amount::from_sat(30_000 + u64::from(marker)),
                script_pubkey: ScriptBuf::from_bytes(vec![0x51, marker]),
            },
            allocation: FederationAllocatedAddressRef {
                application_id: scope().application_id,
                application_key_epoch: scope().application_key_epoch,
                quote: format!("deposit-{marker}"),
                keychain: FederationAddressKeychain::External,
                derivation_index: u32::from(marker),
            },
        }
    }

    fn proposal() -> FederationTransactionProposal {
        let first = intent("quote-b", 1, 20, 2);
        let second = intent("quote-a", 0, 30, 1);
        let batch = FederationOnchainPaymentBatch::new(vec![first.clone(), second.clone()], 30)
            .expect("batch");
        let first_input = allocation(1, outpoint(1, 0));
        let second_input = allocation(2, outpoint(2, 1));
        let first_output = TxOut {
            value: Amount::from_sat(second.amount_sat),
            script_pubkey: second.destination_script.clone(),
        };
        let second_output = TxOut {
            value: Amount::from_sat(first.amount_sat),
            script_pubkey: first.destination_script.clone(),
        };
        let change = TxOut {
            value: Amount::from_sat(
                first_input.previous_output.value.to_sat()
                    + second_input.previous_output.value.to_sat()
                    - first_output.value.to_sat()
                    - second_output.value.to_sat()
                    - 500,
            ),
            script_pubkey: ScriptBuf::from_bytes(vec![0x51, 0x09]),
        };
        let transaction = Transaction {
            version: Version::TWO,
            lock_time: LockTime::ZERO,
            input: vec![
                TxIn {
                    previous_output: first_input.outpoint,
                    script_sig: ScriptBuf::new(),
                    sequence: Sequence::ENABLE_RBF_NO_LOCKTIME,
                    witness: Witness::new(),
                },
                TxIn {
                    previous_output: second_input.outpoint,
                    script_sig: ScriptBuf::new(),
                    sequence: Sequence::ENABLE_RBF_NO_LOCKTIME,
                    witness: Witness::new(),
                },
            ],
            output: vec![first_output.clone(), second_output.clone(), change.clone()],
        };
        let assignments = batch
            .intent_ids
            .iter()
            .copied()
            .zip([first_output, second_output])
            .enumerate()
            .map(
                |(index, (intent_id, output))| FederationTransactionRecipientOutput {
                    intent_id,
                    output_index: index as u32,
                    output,
                    fee_contribution_sat: 250,
                },
            )
            .collect();
        FederationTransactionProposal::new(
            &batch,
            member(1),
            transaction,
            vec![second_input, first_input],
            assignments,
            Some(FederationTransactionChangeOutput {
                derivation_index: 8,
                output_index: 2,
                output: change,
            }),
            500,
            None,
        )
        .expect("proposal")
    }

    #[test]
    fn batch_order_is_independent_of_input_iteration() {
        let intents = vec![
            intent("quote-c", 1, 20, 3),
            intent("quote-a", 0, 30, 1),
            intent("quote-b", 1, 10, 2),
        ];
        let mut reversed = intents.clone();
        reversed.reverse();
        let first = FederationOnchainPaymentBatch::new(intents, 30).expect("first batch");
        let second = FederationOnchainPaymentBatch::new(reversed, 30).expect("second batch");
        assert_eq!(first, second);
        assert!(FederationOnchainPaymentBatch::new(vec![intent("late", 0, 31, 4)], 30).is_err());
        let mut reordered_payload = first.clone();
        reordered_payload.intents.reverse();
        assert!(reordered_payload.validate().is_err());
        assert!(matches!(
            first.intents[0].conflict_key().expect("intent conflict key"),
            FederationOperationConflictKey::OnchainPaymentIntent { quote }
                if quote == first.intents[0].quote
        ));
    }

    #[test]
    fn proposal_metadata_reordering_is_canonical_and_hash_stable() {
        let proposal = proposal();
        let mut reordered = proposal.clone();
        reordered.selected_inputs.reverse();
        reordered.recipient_outputs.reverse();
        let decoded = FederationTransactionProposal::from_json_slice(
            &serde_json::to_vec(&reordered).expect("json"),
        )
        .expect("normalized proposal");
        assert_eq!(decoded, proposal);
        assert_eq!(
            decoded.canonical_bytes().expect("bytes"),
            proposal.canonical_bytes().expect("bytes")
        );
        assert_eq!(
            decoded.proposal_id().expect("id"),
            proposal.proposal_id().expect("id")
        );
    }

    #[test]
    fn structural_mutations_and_scope_mismatches_fail_closed() {
        let proposal = proposal();
        let mut duplicate_input = proposal.clone();
        duplicate_input.selected_inputs[1].outpoint = duplicate_input.selected_inputs[0].outpoint;
        assert!(duplicate_input.validate().is_err());
        let mut duplicate_intent = proposal.clone();
        duplicate_intent.intent_ids[1] = duplicate_intent.intent_ids[0];
        assert!(duplicate_intent.validate().is_err());
        let mut unknown_output = proposal.clone();
        unknown_output.change_output = None;
        assert!(unknown_output.validate().is_err());
        let mut duplicate_output = proposal.clone();
        duplicate_output.recipient_outputs[1].output_index =
            duplicate_output.recipient_outputs[0].output_index;
        assert!(duplicate_output.validate().is_err());
        let mut partially_signed = proposal.clone();
        partially_signed.unsigned_transaction.input[0].script_sig =
            ScriptBuf::from_bytes(vec![0x51]);
        assert!(partially_signed.validate().is_err());
        let mut wrong_fee = proposal.clone();
        wrong_fee.fee_sat += 1;
        assert!(wrong_fee.validate().is_err());
        let mut wrong_psbt = proposal.clone();
        wrong_psbt.psbt_commitment[0] ^= 1;
        assert!(wrong_psbt.validate().is_err());
        for wrong_scope in [
            FederationTransactionScope {
                network: Network::Bitcoin,
                ..scope()
            },
            FederationTransactionScope {
                root_key_epoch: 9,
                ..scope()
            },
            FederationTransactionScope {
                application_key_epoch: 9,
                ..scope()
            },
            FederationTransactionScope {
                policy_version: 9,
                ..scope()
            },
        ] {
            assert!(proposal.validate_scope(wrong_scope).is_err());
        }
    }

    #[test]
    fn proposal_conflict_keys_cover_batch_intents_utxos_and_lineage() {
        let proposal = proposal();
        let keys = proposal.conflict_keys().expect("conflict keys");
        assert_eq!(
            keys.len(),
            proposal.intent_ids.len() + proposal.selected_inputs.len() + 2
        );
        let mut different = proposal.clone();
        different.proposer = member(2);
        assert_ne!(
            different.proposal_id().expect("different id"),
            proposal.proposal_id().expect("id")
        );
        assert_eq!(
            different.conflict_keys().expect("different conflict keys"),
            keys,
            "a different proposer cannot evade batch, intent, UTXO, or lineage conflicts"
        );
        assert!(keys.iter().any(|key| matches!(key, FederationOperationConflictKey::OnchainBatch { batch_id } if *batch_id == proposal.batch_id)));
        assert!(keys.iter().any(|key| matches!(key, FederationOperationConflictKey::OnchainReplacementLineage { lineage } if *lineage == proposal.batch_id)));
    }

    #[test]
    fn stable_proposal_hash_vector_and_previous_outputs_are_complete() {
        let proposal = proposal();
        let previous_outputs = proposal.previous_outputs();
        assert_eq!(
            previous_outputs.len(),
            proposal.unsigned_transaction.input.len()
        );
        for input_index in 0..proposal.unsigned_transaction.input.len() {
            SighashCache::new(&proposal.unsigned_transaction)
                .taproot_key_spend_signature_hash(
                    input_index,
                    &Prevouts::All(&previous_outputs),
                    TapSighashType::Default,
                )
                .expect("proposal contains complete Taproot sighash data");
        }
        assert_eq!(
            proposal.proposal_id().expect("proposal id").to_hex(),
            "e7dc8e8ef9abf0a8cb618398c38185bffff72ac94e69d943b2136c095ab0fbee"
        );
    }

    #[test]
    fn randomized_metadata_permutations_preserve_canonical_identity() {
        let proposal = proposal();
        let expected = proposal.proposal_id().expect("expected id");
        let mut rng = StdRng::seed_from_u64(0x5eed_f1057);
        for _ in 0..256 {
            let mut candidate = proposal.clone();
            if rng.random::<bool>() {
                candidate.selected_inputs.reverse();
            }
            if rng.random::<bool>() {
                candidate.recipient_outputs.reverse();
            }
            let decoded = FederationTransactionProposal::from_json_slice(
                &serde_json::to_vec(&candidate).expect("candidate json"),
            )
            .expect("canonical candidate");
            assert_eq!(decoded.proposal_id().expect("candidate id"), expected);
        }
    }

    #[test]
    fn size_count_and_amount_bounds_are_enforced() {
        let mut too_many = proposal();
        too_many.intent_ids =
            vec![too_many.intent_ids[0]; FEDERATION_MAX_ONCHAIN_BATCH_INTENTS + 1];
        assert!(too_many.validate().is_err());
        let mut overflow = intent("overflow", 0, 1, 1);
        overflow.amount_sat = u64::MAX;
        overflow.maximum_fee_sat = 1;
        assert!(overflow.validate().is_err());
        let mut too_many_inputs = proposal();
        let input = too_many_inputs.unsigned_transaction.input[0].clone();
        too_many_inputs.unsigned_transaction.input =
            vec![input; FEDERATION_MAX_ONCHAIN_PROPOSAL_INPUTS + 1];
        assert!(too_many_inputs.validate().is_err());
        let mut too_many_outputs = proposal();
        let output = too_many_outputs.unsigned_transaction.output[0].clone();
        too_many_outputs.unsigned_transaction.output =
            vec![output; FEDERATION_MAX_ONCHAIN_PROPOSAL_OUTPUTS + 1];
        assert!(too_many_outputs.validate().is_err());
        let mut overweight = proposal();
        let large_script = ScriptBuf::from_bytes(vec![0x51; 110_000]);
        overweight.unsigned_transaction.output[0].script_pubkey = large_script.clone();
        overweight.recipient_outputs[0].output.script_pubkey = large_script;
        assert!(overweight.validate().is_err());
        let mut oversized_psbt_consensus = proposal();
        oversized_psbt_consensus.selected_inputs[0]
            .previous_output
            .script_pubkey =
            ScriptBuf::from_bytes(vec![0x51; FEDERATION_MAX_ONCHAIN_PSBT_CONSENSUS_BYTES]);
        assert!(oversized_psbt_consensus.validate().is_err());
        let mut output_over_input = proposal();
        output_over_input.unsigned_transaction.output[0].value = Amount::MAX_MONEY;
        output_over_input.recipient_outputs[0].output.value = Amount::MAX_MONEY;
        assert!(output_over_input.validate().is_err());
        let oversized = vec![b' '; FEDERATION_MAX_ONCHAIN_PROPOSAL_BYTES + 1];
        assert!(FederationTransactionProposal::from_json_slice(&oversized).is_err());
    }

    #[test]
    fn arbitrary_psbt_maps_are_rejected_instead_of_entering_consensus() {
        let proposal = proposal();
        let mut value = serde_json::to_value(&proposal).expect("proposal json");
        value.as_object_mut().expect("proposal object").insert(
            "proprietary_psbt_map".to_string(),
            serde_json::json!({"vendor": "untrusted"}),
        );
        assert!(FederationTransactionProposal::from_json_slice(
            &serde_json::to_vec(&value).expect("mutated json")
        )
        .is_err());
    }

    fn policy_audit(proposal: &FederationTransactionProposal) -> FederationTransactionPolicyAudit {
        FederationTransactionPolicyAudit {
            proposal_id: proposal.proposal_id().expect("proposal id"),
            txid: proposal.unsigned_transaction.compute_txid(),
            fee_sat: proposal.fee_sat,
            fee_rate_sat_per_kwu: proposal.fee_rate_sat_per_kwu,
            weight: proposal.unsigned_transaction.weight().to_wu(),
            fee_contributions: proposal
                .recipient_outputs
                .iter()
                .map(|recipient| (recipient.intent_id, recipient.fee_contribution_sat))
                .collect(),
            taproot_sighashes: vec![[9; 32]; proposal.selected_inputs.len()],
        }
    }

    #[test]
    fn frost_signing_authorization_binds_operation_transaction_input_and_sighash() {
        let proposal = proposal();
        let accepted = FederationAcceptedTransaction {
            policy_audit: policy_audit(&proposal),
            proposal,
            accepted_operation_id: FederationOperationId::from_bytes([3; 32]),
            accepted_operation_index: 42,
            state: FederationAcceptedTransactionState::Accepted,
        };
        let reference = FederationFrostSigningAuthorizationReference::new(&accepted, 0)
            .expect("signing authorization reference");
        assert_eq!(reference.input_index(), 0);
        assert_eq!(reference.sighash(), [9; 32]);
        let mut variants = Vec::new();
        let mut changed = reference;
        changed.accepted_operation_id = FederationOperationId::from_bytes([4; 32]);
        variants.push(changed.commitment());
        let mut changed = reference;
        changed.proposal_id = FederationTransactionProposalId::from_bytes([5; 32]);
        variants.push(changed.commitment());
        let mut changed = reference;
        changed.txid = Txid::from_byte_array([6; 32]);
        variants.push(changed.commitment());
        let mut changed = reference;
        changed.input_index = 1;
        variants.push(changed.commitment());
        let mut changed = reference;
        changed.sighash_type = FederationTaprootSighashType::All;
        variants.push(changed.commitment());
        let mut changed = reference;
        changed.sighash = [7; 32];
        variants.push(changed.commitment());
        assert!(variants
            .iter()
            .all(|commitment| commitment != &reference.commitment()));
        assert_eq!(
            variants.iter().copied().collect::<BTreeSet<_>>().len(),
            variants.len()
        );

        let mut inactive = accepted.clone();
        inactive.state = FederationAcceptedTransactionState::Abandoned;
        assert!(FederationFrostSigningAuthorizationReference::new(&inactive, 0).is_err());
        assert!(FederationFrostSigningAuthorizationReference::new(
            &accepted,
            accepted.proposal.selected_inputs.len(),
        )
        .is_err());
    }

    #[test]
    fn transaction_state_accepts_exact_replay_and_keeps_reservations_sticky() {
        let proposal = proposal();
        let proposal_id = proposal.proposal_id().expect("proposal id");
        let audit = policy_audit(&proposal);
        let mut state = FederationTransactionState::new();
        for intent_id in [&proposal.intent_ids[0], &proposal.intent_ids[1]] {
            let authorized = proposal
                .recipient_outputs
                .iter()
                .find(|recipient| &recipient.intent_id == intent_id)
                .expect("recipient");
            let original =
                if authorized.output.script_pubkey == ScriptBuf::from_bytes(vec![0x51, 1]) {
                    intent("quote-a", 0, 30, 1)
                } else {
                    intent("quote-b", 1, 20, 2)
                };
            state
                .add_pending_intent(
                    FederationOnchainPaymentIntentRecord::pending(original).expect("record"),
                )
                .expect("add intent");
        }
        assert_eq!(
            state
                .accept_transaction(
                    proposal.clone(),
                    audit.clone(),
                    FederationOperationId::from_bytes([3; 32]),
                    42,
                )
                .expect("accept"),
            FederationTransactionStateOutcome::Updated
        );
        assert_eq!(
            state
                .accept_transaction(
                    proposal.clone(),
                    audit,
                    FederationOperationId::from_bytes([4; 32]),
                    43,
                )
                .expect("exact replay"),
            FederationTransactionStateOutcome::Existing
        );
        for input in &proposal.selected_inputs {
            assert_eq!(state.reservation(input.outpoint), Some(proposal_id));
        }
        state
            .abandon_transaction(proposal_id)
            .expect("abandon transaction");
        for input in &proposal.selected_inputs {
            assert_eq!(state.reservation(input.outpoint), Some(proposal_id));
        }
        state
            .mark_transaction_replaced(
                proposal_id,
                FederationTransactionProposalId::from_bytes([7; 32]),
            )
            .expect("mark replacement");
        for input in &proposal.selected_inputs {
            assert_eq!(state.reservation(input.outpoint), Some(proposal_id));
        }
        let decoded = FederationTransactionState::from_bytes(
            &state.to_bytes().expect("serialize transaction state"),
        )
        .expect("decode transaction state");
        assert_eq!(decoded, state);
        assert_eq!(state.materialized_rows().expect("digest rows").len(), 5);
    }

    #[test]
    fn transaction_state_rejects_different_transaction_for_owned_batch_and_tampering() {
        let proposal = proposal();
        let mut state = FederationTransactionState::new();
        for intent in [intent("quote-a", 0, 30, 1), intent("quote-b", 1, 20, 2)] {
            state
                .add_pending_intent(
                    FederationOnchainPaymentIntentRecord::pending(intent).expect("record"),
                )
                .expect("add intent");
        }
        state
            .accept_transaction(
                proposal.clone(),
                policy_audit(&proposal),
                FederationOperationId::from_bytes([5; 32]),
                50,
            )
            .expect("accept");

        let mut competing = proposal.clone();
        competing.proposer = member(2);
        assert!(state
            .accept_transaction(
                competing.clone(),
                policy_audit(&competing),
                FederationOperationId::from_bytes([6; 32]),
                51,
            )
            .is_err());

        let original_digest = crate::federation::FederationMaterializedStateSnapshot::from_rows(
            state.materialized_rows().expect("original digest rows"),
        )
        .digest()
        .expect("original digest");
        let mut sighash_tamper = serde_json::to_value(&state).expect("state json");
        let accepted = sighash_tamper["accepted_transactions"]
            .as_object_mut()
            .and_then(|transactions| transactions.values_mut().next())
            .expect("accepted transaction JSON");
        accepted["policy_audit"]["taproot_sighashes"][0][0] = serde_json::json!(8);
        let sighash_tamper = FederationTransactionState::from_bytes(
            &serde_json::to_vec(&sighash_tamper).expect("sighash tamper bytes"),
        )
        .expect("structurally valid sighash tamper");
        let tampered_digest = crate::federation::FederationMaterializedStateSnapshot::from_rows(
            sighash_tamper
                .materialized_rows()
                .expect("tampered digest rows"),
        )
        .digest()
        .expect("tampered digest");
        assert_ne!(tampered_digest, original_digest);

        let mut transaction_tamper = serde_json::to_value(&state).expect("state json");
        let accepted = transaction_tamper["accepted_transactions"]
            .as_object_mut()
            .and_then(|transactions| transactions.values_mut().next())
            .expect("accepted transaction JSON");
        accepted["proposal"]["unsigned_transaction"]["output"][0]["value"] =
            serde_json::json!(999_999);
        assert!(FederationTransactionState::from_bytes(
            &serde_json::to_vec(&transaction_tamper).expect("transaction tamper bytes")
        )
        .is_err());

        let mut value = serde_json::to_value(&state).expect("state json");
        value["utxo_reservations"] = serde_json::json!({});
        assert!(FederationTransactionState::from_bytes(
            &serde_json::to_vec(&value).expect("tampered state")
        )
        .is_err());
    }
}
