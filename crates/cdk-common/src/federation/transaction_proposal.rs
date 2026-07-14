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

use bitcoin::consensus::{deserialize, serialize};
use bitcoin::hashes::{sha256, Hash, HashEngine};
use bitcoin::secp256k1::{schnorr, Message, Secp256k1, XOnlyPublicKey};
use bitcoin::{BlockHash, Network, OutPoint, ScriptBuf, Transaction, TxOut, Txid};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use super::{
    FederationAllocatedAddressRef, FederationError, FederationFrostApplicationId,
    FederationMaterializedStateRow, FederationOperationConflictKey, FederationOperationId,
    MemberId, Result, FEDERATION_MAX_DERIVATION_INDEX,
};

const PAYMENT_INTENT_VERSION: u16 = 3;
const PAYMENT_BATCH_VERSION: u16 = 1;
const TRANSACTION_PROPOSAL_VERSION: u16 = 2;
const TRANSACTION_STATE_VERSION: u16 = 3;
const SIGNED_TRANSACTION_VERSION: u16 = 1;
const PAYMENT_INTENT_HASH_DOMAIN: &[u8] = b"cdk-federation-onchain-payment-intent-v3";
const PAYMENT_BATCH_HASH_DOMAIN: &[u8] = b"cdk-federation-onchain-payment-batch-v1";
const TRANSACTION_PROPOSAL_HASH_DOMAIN: &[u8] = b"cdk-federation-onchain-transaction-proposal-v2";
const PSBT_CONSENSUS_HASH_DOMAIN: &[u8] = b"cdk-federation-onchain-psbt-consensus-v1";
const FROST_SIGNING_AUTHORIZATION_HASH_DOMAIN: &[u8] =
    b"cdk-federation-frost-signing-authorization-v1";
const CDK_ONCHAIN_FROST_AUTHORIZATION_VERSION: u16 = 1;
const CDK_ONCHAIN_FROST_AUTHORIZATION_MAX_BYTES: usize = 16 * 1024;
const MAX_QUOTE_ID_BYTES: usize = 256;
const MAX_DESTINATION_SCRIPT_BYTES: usize = 10_000;
const MAX_FEE_TIER: u32 = 255;
const MAX_FEE_TARGET_BLOCKS: u32 = 1_008;
const MAX_BROADCAST_DETAIL_BYTES: usize = 512;
const TRANSACTION_SETTLEMENT_POLICY_VERSION: u16 = 1;

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
    /// Accepted federation melt operation authorizing this exact intent.
    pub melt_operation_id: FederationOperationId,
    /// Exact recipient script, parsed on the expected network before creation.
    pub destination_script: ScriptBuf,
    /// Recipient value in satoshis.
    pub amount_sat: u64,
    /// Maximum fee contribution assigned to this intent.
    pub maximum_fee_sat: u64,
    /// Selected quote fee-option index.
    pub fee_tier: u32,
    /// Confirmation target carried by the selected quote fee option.
    pub fee_target_blocks: u32,
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
        melt_operation_id: FederationOperationId,
        destination_script: ScriptBuf,
        amount_sat: u64,
        maximum_fee_sat: u64,
        fee_tier: u32,
        fee_target_blocks: u32,
        creation_time: u64,
        scope: FederationTransactionScope,
    ) -> Result<Self> {
        let intent = Self {
            version: PAYMENT_INTENT_VERSION,
            quote,
            melt_operation_id,
            destination_script,
            amount_sat,
            maximum_fee_sat,
            fee_tier,
            fee_target_blocks,
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
        if self.fee_target_blocks == 0 || self.fee_target_blocks > MAX_FEE_TARGET_BLOCKS {
            return Err(invalid(
                "payment intent fee confirmation target is out of range",
            ));
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
        self.fee_target_blocks
            .cmp(&other.fee_target_blocks)
            .then_with(|| self.fee_tier.cmp(&other.fee_tier))
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
    /// Exact projected key-path-signed transaction weight.
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
    /// Select a bounded canonical prefix from an unordered intent set.
    pub fn select(
        mut intents: Vec<FederationOnchainPaymentIntent>,
        eligibility_cutoff: u64,
        maximum_intents: usize,
    ) -> Result<Self> {
        if maximum_intents == 0 || maximum_intents > FEDERATION_MAX_ONCHAIN_BATCH_INTENTS {
            return Err(invalid("on-chain batch selection limit is out of bounds"));
        }
        intents.retain(|intent| intent.is_eligible_at(eligibility_cutoff));
        intents.sort_by(Self::canonical_intent_order);
        intents.truncate(maximum_intents);
        Self::new(intents, eligibility_cutoff)
    }

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

        let weight = self.estimated_signed_weight();
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

    /// Return the exact projected weight after one key-path signature per input.
    ///
    /// Proposal inputs are deliberately witness-free. The current policy uses
    /// one 64-byte BIP340 signature for `SIGHASH_DEFAULT`, or 65 bytes when an
    /// explicit sighash byte is present.
    #[must_use]
    pub fn estimated_signed_weight(&self) -> u64 {
        let signature_len = match self.sighash_type {
            FederationTaprootSighashType::Default => 64,
            FederationTaprootSighashType::All
            | FederationTaprootSighashType::None
            | FederationTaprootSighashType::Single
            | FederationTaprootSighashType::AllAnyoneCanPay
            | FederationTaprootSighashType::NoneAnyoneCanPay
            | FederationTaprootSighashType::SingleAnyoneCanPay => 65,
        };
        let signature = vec![0_u8; signature_len];
        let mut transaction = self.unsigned_transaction.clone();
        for input in &mut transaction.input {
            input.witness.push(&signature);
        }
        transaction.weight().to_wu()
    }

    /// Return all conflict scopes required when this proposal enters consensus.
    pub fn conflict_keys(&self) -> Result<Vec<FederationOperationConflictKey>> {
        self.validate()?;
        if self.replacement_of.is_some() {
            return Ok(Vec::new());
        }
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
        let weight = self.estimated_signed_weight();
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
    /// Fully signed and durably recorded before broadcast.
    Signed,
    /// Broadcast is authorized and recoverable by any ready member.
    BroadcastPending,
    /// At least one success-equivalent broadcast observation was accepted.
    Broadcast,
    /// The versioned confirmation policy was satisfied by distinct members.
    Confirmed,
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
    /// Exact finalized transaction and public signing audit, when signed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub signed: Option<FederationSignedTransaction>,
    /// Durable broadcast authorization and authenticated member observations.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub broadcast: Option<FederationTransactionBroadcastState>,
}

/// Consensus authorization to submit one exact signed transaction to Bitcoin.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FederationTransactionBroadcastIntent {
    /// Accepted proposal identity.
    pub proposal_id: FederationTransactionProposalId,
    /// Exact signed transaction identity.
    pub txid: Txid,
    /// Versioned rule for completing the transaction's melt intents.
    pub settlement_policy: FederationTransactionSettlementPolicy,
}

/// Versioned consensus rule for treating an outgoing transaction as paid.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FederationTransactionSettlementPolicy {
    /// Policy schema version.
    pub version: u16,
    /// Required confirmation depth in each accepted member observation.
    pub minimum_confirmations: u32,
    /// Required distinct member observations for the same confirmed block.
    pub required_observers: u16,
}

impl FederationTransactionSettlementPolicy {
    /// Construct the current outgoing settlement policy.
    pub fn new(minimum_confirmations: u32, required_observers: u16) -> Result<Self> {
        let policy = Self {
            version: TRANSACTION_SETTLEMENT_POLICY_VERSION,
            minimum_confirmations,
            required_observers,
        };
        policy.validate()?;
        Ok(policy)
    }

    /// Validate the current policy schema and non-zero thresholds.
    pub fn validate(self) -> Result<()> {
        if self.version != TRANSACTION_SETTLEMENT_POLICY_VERSION
            || self.minimum_confirmations == 0
            || self.required_observers == 0
        {
            return Err(invalid(
                "unsupported outgoing transaction settlement policy",
            ));
        }
        Ok(())
    }
}

/// Classified result of one member's exact-byte broadcast or reconciliation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "result", content = "data", rename_all = "snake_case")]
pub enum FederationTransactionBroadcastResult {
    /// The local backend accepted the transaction.
    Accepted,
    /// The local backend already knew the transaction.
    AlreadyKnown,
    /// The local backend deterministically rejected the transaction.
    Rejected,
    /// A retryable transport or upstream failure occurred.
    Transient,
    /// The network result is ambiguous and must be retried conservatively.
    Unknown,
    /// Local chain synchronization observed the transaction in a block.
    Confirmed {
        /// Block containing the transaction.
        block_hash: BlockHash,
        /// Height of the containing block.
        block_height: u32,
        /// Depth relative to the observer's locally persisted chain tip.
        confirmation_depth: u32,
    },
}

impl FederationTransactionBroadcastResult {
    /// Return whether the result proves success or on-chain inclusion.
    #[must_use]
    pub fn is_success_equivalent(self) -> bool {
        matches!(
            self,
            Self::Accepted | Self::AlreadyKnown | Self::Confirmed { .. }
        )
    }
}

/// Authenticated observation produced by one federation member.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FederationTransactionBroadcastObservation {
    /// Accepted proposal identity.
    pub proposal_id: FederationTransactionProposalId,
    /// Exact signed transaction identity.
    pub txid: Txid,
    /// Member whose local chain backend produced the result.
    pub observer: MemberId,
    /// Monotonic member-local attempt number.
    pub attempt: u32,
    /// Bounded result classification.
    pub result: FederationTransactionBroadcastResult,
    /// Redacted operator detail for failures.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

impl FederationTransactionBroadcastObservation {
    /// Validate bounded, consensus-safe observation fields.
    pub fn validate(&self) -> Result<()> {
        if self.attempt == 0 {
            return Err(invalid("broadcast attempt is zero"));
        }
        if matches!(
            self.result,
            FederationTransactionBroadcastResult::Confirmed {
                confirmation_depth: 0,
                ..
            }
        ) {
            return Err(invalid("confirmed broadcast observation has zero depth"));
        }
        if self.detail.as_ref().is_some_and(|detail| {
            detail.is_empty()
                || detail.len() > MAX_BROADCAST_DETAIL_BYTES
                || detail.chars().any(char::is_control)
                || super::redact_federation_operator_message(detail) != *detail
        }) {
            return Err(invalid(
                "broadcast detail is empty, oversized, or contains control bytes",
            ));
        }
        Ok(())
    }
}

/// One ordered broadcast observation and its journal identity.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FederationTransactionBroadcastObservationRecord {
    /// Authenticated member observation.
    pub observation: FederationTransactionBroadcastObservation,
    /// Consensus operation identity.
    pub operation_id: FederationOperationId,
    /// Accepted journal position.
    pub operation_index: u64,
}

/// Durable broadcast authorization and convergence record.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FederationTransactionBroadcastState {
    /// Consensus operation that first persisted broadcast intent.
    pub intent_operation_id: FederationOperationId,
    /// Accepted journal position of the first intent.
    pub intent_operation_index: u64,
    /// Policy fixed before the first network request.
    pub settlement_policy: FederationTransactionSettlementPolicy,
    /// Latest authenticated observation per member, ordered by member ID.
    pub observations: Vec<FederationTransactionBroadcastObservationRecord>,
    /// First success-equivalent observation, retained across later uncertainty.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub first_success: Option<FederationTransactionBroadcastObservationRecord>,
    /// First threshold set proving the required confirmation rule.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub settlement: Option<FederationTransactionSettlement>,
}

/// Sticky threshold evidence that one exact transaction satisfied settlement policy.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FederationTransactionSettlement {
    /// Block containing the exact signed transaction.
    pub block_hash: BlockHash,
    /// Height of the containing block.
    pub block_height: u32,
    /// Canonically ordered distinct-member confirmations retained at settlement.
    pub confirmations: Vec<FederationTransactionBroadcastObservationRecord>,
}

/// Existing melt response data derived solely from accepted transaction state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FederationOnchainMeltPaymentReceipt {
    /// Melt quote authorizing the payment.
    pub quote: String,
    /// Federation melt operation bound into the intent identity.
    pub melt_operation_id: FederationOperationId,
    /// Exact confirmed transaction identifier.
    pub txid: Txid,
    /// Recipient output proving payment.
    pub outpoint: OutPoint,
    /// Recipient amount in satoshis.
    pub amount_sat: u64,
    /// Actual deterministic fee contribution assigned to this intent.
    pub fee_sat: u64,
}

/// Public audit for one input's completed threshold signing attempt.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FederationFrostInputSignatureAudit {
    /// Transaction input position.
    pub input_index: u32,
    /// Canonical FROST session identifier.
    pub session_id: [u8; 32],
    /// Monotonic signing attempt.
    pub attempt: u32,
    /// Exact selected signer identifiers.
    pub signer_set: Vec<u16>,
    /// Canonical signing-package commitment.
    pub package_hash: [u8; 32],
    /// Aggregate BIP340 signature inserted into the PSBT.
    pub signature: Vec<u8>,
}

/// Exact finalized transaction persisted before broadcast.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FederationSignedTransaction {
    /// Record schema version.
    pub version: u16,
    /// Accepted proposal identity.
    pub proposal_id: FederationTransactionProposalId,
    /// Final transaction identity.
    pub txid: Txid,
    /// Consensus-serialized final transaction including witnesses.
    pub transaction_bytes: Vec<u8>,
    /// Independently recomputed policy and fee audit retained at signing.
    pub policy_audit: FederationTransactionPolicyAudit,
    /// One public signing audit per input, in input order.
    pub input_signatures: Vec<FederationFrostInputSignatureAudit>,
}

impl FederationSignedTransaction {
    /// Construct and validate a durable signed record.
    pub fn new(
        accepted: &FederationAcceptedTransaction,
        transaction: &Transaction,
        input_signatures: Vec<FederationFrostInputSignatureAudit>,
    ) -> Result<Self> {
        let value = Self {
            version: SIGNED_TRANSACTION_VERSION,
            proposal_id: accepted.proposal.proposal_id()?,
            txid: transaction.compute_txid(),
            transaction_bytes: serialize(transaction),
            policy_audit: accepted.policy_audit.clone(),
            input_signatures,
        };
        value.validate(&accepted.proposal)?;
        Ok(value)
    }

    /// Decode the exact final transaction.
    pub fn transaction(&self) -> Result<Transaction> {
        deserialize(&self.transaction_bytes)
            .map_err(|_| invalid("signed transaction bytes are invalid"))
    }

    /// Verify every aggregate signature against its accepted prevout and sighash.
    pub fn verify_signatures(&self, proposal: &FederationTransactionProposal) -> Result<()> {
        self.validate(proposal)?;
        let secp = Secp256k1::verification_only();
        for (index, audit) in self.input_signatures.iter().enumerate() {
            let script = proposal.selected_inputs[index]
                .previous_output
                .script_pubkey
                .as_bytes();
            if script.len() != 34 || script[0] != 0x51 || script[1] != 0x20 {
                return Err(invalid(
                    "signed input prevout is not Taproot key-path output",
                ));
            }
            let output_key = XOnlyPublicKey::from_slice(&script[2..])
                .map_err(|_| invalid("signed input Taproot output key is invalid"))?;
            let signature = schnorr::Signature::from_slice(&audit.signature)
                .map_err(|_| invalid("aggregate input signature is malformed"))?;
            let message = Message::from_digest(self.policy_audit.taproot_sighashes[index]);
            secp.verify_schnorr(&signature, &message, &output_key)
                .map_err(|_| invalid("aggregate input signature verification failed"))?;
        }
        Ok(())
    }

    fn validate(&self, proposal: &FederationTransactionProposal) -> Result<()> {
        if self.version != SIGNED_TRANSACTION_VERSION
            || self.proposal_id != proposal.proposal_id()?
            || self.policy_audit.proposal_id != self.proposal_id
            || self.input_signatures.len() != proposal.selected_inputs.len()
            || self.transaction_bytes.len() > FEDERATION_MAX_ONCHAIN_PSBT_CONSENSUS_BYTES
        {
            return Err(invalid("signed transaction audit shape is invalid"));
        }
        validate_policy_audit(proposal, &self.policy_audit)?;
        let transaction = self.transaction()?;
        if transaction.compute_txid() != self.txid {
            return Err(invalid("signed transaction txid mismatch"));
        }
        let mut unsigned = transaction.clone();
        for input in &mut unsigned.input {
            input.script_sig = ScriptBuf::new();
            input.witness.clear();
        }
        if unsigned != proposal.unsigned_transaction {
            return Err(invalid(
                "signed transaction differs from accepted non-witness data",
            ));
        }
        let expected_attempt = self.input_signatures[0].attempt;
        let expected_signer_set = self.input_signatures[0].signer_set.as_slice();
        for (index, audit) in self.input_signatures.iter().enumerate() {
            if audit.input_index != u32::try_from(index).map_err(|_| invalid("input overflow"))?
                || audit.session_id == [0; 32]
                || audit.attempt == 0
                || audit.attempt != expected_attempt
                || audit.signer_set.is_empty()
                || audit.signer_set.as_slice() != expected_signer_set
                || audit.signer_set.contains(&0)
                || audit
                    .signer_set
                    .windows(2)
                    .any(|members| members[0] >= members[1])
                || audit.package_hash == [0; 32]
                || audit.signature.len() != 64
                || transaction.input[index].witness.len() != 1
                || transaction.input[index].witness.iter().next()
                    != Some(audit.signature.as_slice())
            {
                return Err(invalid("signed transaction input audit mismatch"));
            }
        }
        Ok(())
    }
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

/// Typed CDK on-chain authorization payload accepted by the FROST adapter.
///
/// The private REST signer must first reload the proposal through
/// `Mint::federation_transaction_signing_authorization`, rebuild this value,
/// and compare it exactly before invoking the key manager.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FederationCdkOnchainFrostAuthorization {
    /// Payload schema version.
    pub version: u16,
    /// Exact accepted proposal identity.
    pub proposal_id: FederationTransactionProposalId,
    /// Transaction input being signed.
    pub input_index: u32,
    /// Canonical accepted-state authorization commitment.
    pub authorization_reference: [u8; 32],
    /// Exact BIP341 sighash authorized by local policy recomputation.
    pub message: [u8; 32],
    /// Canonical application-derived BIP32/Taproot signing plan.
    pub derivation: Vec<u8>,
}

impl FederationCdkOnchainFrostAuthorization {
    /// Build a typed authorization from revalidated accepted state.
    pub fn new(
        accepted: &FederationAcceptedTransaction,
        input_index: usize,
        derivation: &cdk_frost::SigningDerivation,
    ) -> Result<Self> {
        let reference = FederationFrostSigningAuthorizationReference::new(accepted, input_index)?;
        Ok(Self {
            version: CDK_ONCHAIN_FROST_AUTHORIZATION_VERSION,
            proposal_id: reference.proposal_id(),
            input_index: reference.input_index(),
            authorization_reference: reference.commitment(),
            message: reference.sighash(),
            derivation: derivation.encode(),
        })
    }

    /// Decode, bound, and fully validate a typed adapter payload.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self> {
        if bytes.len() > CDK_ONCHAIN_FROST_AUTHORIZATION_MAX_BYTES {
            return Err(invalid("CDK FROST authorization payload is oversized"));
        }
        let value: Self = serde_json::from_slice(bytes)?;
        value.validate()?;
        if value.to_bytes()? != bytes {
            return Err(invalid("CDK FROST authorization is not canonical"));
        }
        Ok(value)
    }

    /// Canonically encode this typed adapter payload.
    pub fn to_bytes(&self) -> Result<Vec<u8>> {
        self.validate()?;
        let bytes = serde_json::to_vec(self)?;
        if bytes.len() > CDK_ONCHAIN_FROST_AUTHORIZATION_MAX_BYTES {
            return Err(invalid("CDK FROST authorization payload is oversized"));
        }
        Ok(bytes)
    }

    /// Decode the exact reviewed signing derivation.
    pub fn signing_derivation(&self) -> Result<cdk_frost::SigningDerivation> {
        cdk_frost::SigningDerivation::decode(&self.derivation)
            .map_err(|error| invalid(error.to_string()))
    }

    fn validate(&self) -> Result<()> {
        if self.version != CDK_ONCHAIN_FROST_AUTHORIZATION_VERSION
            || self.authorization_reference == [0; 32]
            || self.message == [0; 32]
        {
            return Err(invalid("CDK FROST authorization metadata is invalid"));
        }
        self.signing_derivation()?;
        Ok(())
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

    /// Return all accepted transactions keyed by canonical proposal identity.
    #[must_use]
    pub fn accepted_transactions(
        &self,
    ) -> &BTreeMap<FederationTransactionProposalId, FederationAcceptedTransaction> {
        &self.accepted_transactions
    }

    /// Return the current active or confirmed head of one replacement lineage.
    #[must_use]
    pub fn lineage_head(
        &self,
        batch_id: FederationOnchainBatchId,
    ) -> Option<FederationTransactionProposalId> {
        self.batch_owners.get(&batch_id).copied()
    }

    /// Derive one terminal on-chain melt receipt from accepted transaction state.
    pub fn completed_payment_for_quote(
        &self,
        quote: &str,
    ) -> Result<Option<FederationOnchainMeltPaymentReceipt>> {
        let Some(intent) = self.intent_for_quote(quote) else {
            return Ok(None);
        };
        let FederationOnchainPaymentIntentState::Completed { txid } = intent.state else {
            return Ok(None);
        };
        let mut owners = self.accepted_transactions.iter().filter(|(_, accepted)| {
            accepted.proposal.intent_ids.contains(&intent.intent_id)
                && accepted.state == FederationAcceptedTransactionState::Confirmed
        });
        let (_proposal_id, accepted) = owners
            .next()
            .ok_or_else(|| invalid("completed payment intent has no accepted transaction"))?;
        if owners.next().is_some()
            || accepted.signed.as_ref().map(|signed| signed.txid) != Some(txid)
        {
            return Err(invalid(
                "completed payment intent has ambiguous transaction ownership",
            ));
        }
        let recipient = accepted
            .proposal
            .recipient_outputs
            .iter()
            .find(|recipient| recipient.intent_id == intent.intent_id)
            .ok_or_else(|| invalid("completed payment intent has no recipient output"))?;
        if recipient.output.value.to_sat() != intent.intent.amount_sat
            || recipient.fee_contribution_sat > intent.intent.maximum_fee_sat
        {
            return Err(invalid("completed payment receipt violates intent terms"));
        }
        Ok(Some(FederationOnchainMeltPaymentReceipt {
            quote: intent.intent.quote.clone(),
            melt_operation_id: intent.intent.melt_operation_id,
            txid,
            outpoint: OutPoint {
                txid,
                vout: recipient.output_index,
            },
            amount_sat: intent.intent.amount_sat,
            fee_sat: recipient.fee_contribution_sat,
        }))
    }

    /// Return the proposal that owns one wallet reservation.
    #[must_use]
    pub fn reservation(&self, outpoint: OutPoint) -> Option<FederationTransactionProposalId> {
        self.utxo_reservations.get(&outpoint.to_string()).copied()
    }

    /// Number of sticky wallet UTXO reservations retained across recovery.
    #[must_use]
    pub fn reservation_count(&self) -> usize {
        self.utxo_reservations.len()
    }

    /// Return the sticky reservation owner for an accepted lineage transaction.
    pub fn lineage_reservation_owner(
        &self,
        proposal_id: FederationTransactionProposalId,
    ) -> Result<FederationTransactionProposalId> {
        self.lineage_root(proposal_id)
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
                signed: None,
                broadcast: None,
            },
        );
        self.validate()?;
        Ok(FederationTransactionStateOutcome::Updated)
    }

    /// Accept a fully validated replacement without releasing lineage reservations.
    pub fn accept_replacement(
        &mut self,
        proposal: FederationTransactionProposal,
        policy_audit: FederationTransactionPolicyAudit,
        accepted_operation_id: FederationOperationId,
        accepted_operation_index: u64,
    ) -> Result<FederationTransactionStateOutcome> {
        proposal.validate()?;
        if accepted_operation_index == 0 {
            return Err(invalid("accepted replacement operation index is zero"));
        }
        let proposal_id = proposal.proposal_id()?;
        validate_policy_audit(&proposal, &policy_audit)?;
        if let Some(existing) = self.accepted_transactions.get(&proposal_id) {
            if existing.proposal == proposal && existing.policy_audit == policy_audit {
                return Ok(FederationTransactionStateOutcome::Existing);
            }
            return Err(invalid(
                "replacement identity maps to different durable transaction data",
            ));
        }
        let original_id = proposal
            .replacement_of
            .ok_or_else(|| invalid("replacement proposal has no active predecessor"))?;
        if self.batch_owners.get(&proposal.batch_id) != Some(&original_id) {
            return Err(invalid(
                "replacement does not extend the current transaction lineage head",
            ));
        }
        let original = self
            .accepted_transactions
            .get(&original_id)
            .ok_or_else(|| invalid("replacement predecessor is unknown"))?;
        if !matches!(
            original.state,
            FederationAcceptedTransactionState::BroadcastPending
                | FederationAcceptedTransactionState::Broadcast
        ) || original.signed.is_none()
            || original.proposal.batch_id != proposal.batch_id
            || original.proposal.intent_ids != proposal.intent_ids
            || original.proposal.selected_inputs != proposal.selected_inputs
        {
            return Err(invalid(
                "replacement predecessor is not an active signed lineage head",
            ));
        }
        let reservation_owner = self.lineage_root(original_id)?;
        for intent_id in &proposal.intent_ids {
            let record = self
                .intents
                .get(intent_id)
                .ok_or_else(|| invalid("replacement references an unknown intent"))?;
            if record.state
                != (FederationOnchainPaymentIntentState::Accepted {
                    proposal_id: original_id,
                })
            {
                return Err(invalid(
                    "replacement intent is not owned by its active predecessor",
                ));
            }
        }
        for input in &proposal.selected_inputs {
            if self.utxo_reservations.get(&input.outpoint.to_string()) != Some(&reservation_owner) {
                return Err(invalid(
                    "replacement input is not reserved by its lineage root",
                ));
            }
        }

        self.accepted_transactions
            .get_mut(&original_id)
            .ok_or_else(|| invalid("replacement predecessor disappeared"))?
            .state = FederationAcceptedTransactionState::Replaced { by: proposal_id };
        for intent_id in &proposal.intent_ids {
            self.intents
                .get_mut(intent_id)
                .ok_or_else(|| invalid("replacement intent disappeared"))?
                .state = FederationOnchainPaymentIntentState::Accepted { proposal_id };
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
                signed: None,
                broadcast: None,
            },
        );
        self.validate()?;
        Ok(FederationTransactionStateOutcome::Updated)
    }

    /// Persist a finalized transaction idempotently before broadcast.
    pub fn mark_transaction_signed(
        &mut self,
        proposal_id: FederationTransactionProposalId,
        signed: FederationSignedTransaction,
    ) -> Result<FederationTransactionStateOutcome> {
        let accepted = self
            .accepted_transactions
            .get_mut(&proposal_id)
            .ok_or_else(|| invalid("cannot sign an unknown accepted transaction"))?;
        if let Some(existing) = &accepted.signed {
            if existing == &signed {
                return Ok(FederationTransactionStateOutcome::Existing);
            }
            return Err(invalid(
                "signed transaction replay conflicts with durable bytes",
            ));
        }
        if accepted.state != FederationAcceptedTransactionState::Accepted {
            return Err(invalid(
                "transaction is not eligible for signed persistence",
            ));
        }
        if signed.policy_audit != accepted.policy_audit {
            return Err(invalid(
                "signed transaction policy audit differs from accepted state",
            ));
        }
        signed.validate(&accepted.proposal)?;
        accepted.signed = Some(signed);
        accepted.state = FederationAcceptedTransactionState::Signed;
        self.validate()?;
        Ok(FederationTransactionStateOutcome::Updated)
    }

    /// Persist broadcast authorization before any member performs a network request.
    pub fn authorize_transaction_broadcast(
        &mut self,
        intent: FederationTransactionBroadcastIntent,
        operation_id: FederationOperationId,
        operation_index: u64,
    ) -> Result<FederationTransactionStateOutcome> {
        if operation_index == 0 {
            return Err(invalid("broadcast intent operation index is zero"));
        }
        intent.settlement_policy.validate()?;
        let accepted = self
            .accepted_transactions
            .get_mut(&intent.proposal_id)
            .ok_or_else(|| invalid("cannot broadcast an unknown accepted transaction"))?;
        let signed = accepted
            .signed
            .as_ref()
            .ok_or_else(|| invalid("cannot broadcast a transaction before signed persistence"))?;
        if signed.txid != intent.txid {
            return Err(invalid(
                "broadcast intent txid differs from signed transaction",
            ));
        }
        if let Some(existing) = &accepted.broadcast {
            if existing.settlement_policy != intent.settlement_policy {
                return Err(invalid(
                    "broadcast intent conflicts with durable settlement policy",
                ));
            }
            if matches!(
                accepted.state,
                FederationAcceptedTransactionState::BroadcastPending
                    | FederationAcceptedTransactionState::Broadcast
                    | FederationAcceptedTransactionState::Confirmed
            ) {
                return Ok(FederationTransactionStateOutcome::Existing);
            }
            return Err(invalid(
                "broadcast intent conflicts with transaction lifecycle",
            ));
        }
        if accepted.state != FederationAcceptedTransactionState::Signed {
            return Err(invalid(
                "transaction is not eligible for broadcast authorization",
            ));
        }
        accepted.broadcast = Some(FederationTransactionBroadcastState {
            intent_operation_id: operation_id,
            intent_operation_index: operation_index,
            settlement_policy: intent.settlement_policy,
            observations: Vec::new(),
            first_success: None,
            settlement: None,
        });
        accepted.state = FederationAcceptedTransactionState::BroadcastPending;
        self.validate()?;
        Ok(FederationTransactionStateOutcome::Updated)
    }

    /// Record one authenticated member result without changing transaction bytes.
    pub fn record_transaction_broadcast_observation(
        &mut self,
        observation: FederationTransactionBroadcastObservation,
        operation_id: FederationOperationId,
        operation_index: u64,
    ) -> Result<FederationTransactionStateOutcome> {
        observation.validate()?;
        let observed_proposal_id = observation.proposal_id;
        if operation_index == 0 {
            return Err(invalid("broadcast observation operation index is zero"));
        }
        let observed_batch_id = self
            .accepted_transactions
            .get(&observed_proposal_id)
            .ok_or_else(|| invalid("broadcast observation references an unknown transaction"))?
            .proposal
            .batch_id;
        if self
            .accepted_transactions
            .iter()
            .any(|(proposal_id, accepted)| {
                *proposal_id != observed_proposal_id
                    && accepted.proposal.batch_id == observed_batch_id
                    && accepted.state == FederationAcceptedTransactionState::Confirmed
            })
        {
            return Err(invalid(
                "transaction lineage already has a different terminal winner",
            ));
        }
        let mut completed = None;
        {
            let accepted = self
                .accepted_transactions
                .get_mut(&observation.proposal_id)
                .ok_or_else(|| {
                    invalid("broadcast observation references an unknown transaction")
                })?;
            let signed = accepted
                .signed
                .as_ref()
                .ok_or_else(|| invalid("broadcast observation has no signed transaction"))?;
            if signed.txid != observation.txid {
                return Err(invalid(
                    "broadcast observation txid differs from signed transaction",
                ));
            }
            let broadcast = accepted
                .broadcast
                .as_mut()
                .ok_or_else(|| invalid("broadcast observation precedes durable intent"))?;
            let success = observation.result.is_success_equivalent();
            let record = FederationTransactionBroadcastObservationRecord {
                observation,
                operation_id,
                operation_index,
            };
            match broadcast
                .observations
                .iter_mut()
                .find(|existing| existing.observation.observer == record.observation.observer)
            {
                Some(existing) if existing.observation.attempt == record.observation.attempt => {
                    if existing.observation == record.observation {
                        return Ok(FederationTransactionStateOutcome::Existing);
                    }
                    return Err(invalid(
                        "broadcast attempt replay carries a different result",
                    ));
                }
                Some(existing) if existing.observation.attempt > record.observation.attempt => {
                    return Err(invalid("broadcast observation attempt is stale"));
                }
                Some(existing) => *existing = record.clone(),
                None => broadcast.observations.push(record.clone()),
            }
            broadcast
                .observations
                .sort_by_key(|existing| existing.observation.observer);
            if success && broadcast.first_success.is_none() {
                broadcast.first_success = Some(record);
            }
            if success
                && matches!(
                    accepted.state,
                    FederationAcceptedTransactionState::BroadcastPending
                        | FederationAcceptedTransactionState::Broadcast
                )
            {
                accepted.state = FederationAcceptedTransactionState::Broadcast;
            }
            if broadcast.settlement.is_none() {
                broadcast.settlement = transaction_settlement_from_observations(
                    broadcast.settlement_policy,
                    &broadcast.observations,
                );
                if broadcast.settlement.is_some() {
                    accepted.state = FederationAcceptedTransactionState::Confirmed;
                    completed = Some((
                        observed_proposal_id,
                        accepted.proposal.batch_id,
                        accepted.proposal.intent_ids.clone(),
                        signed.txid,
                    ));
                }
            }
        }
        if let Some((winner, batch_id, intent_ids, txid)) = completed {
            for (proposal_id, accepted) in &mut self.accepted_transactions {
                if accepted.proposal.batch_id == batch_id && *proposal_id != winner {
                    accepted.state = FederationAcceptedTransactionState::Abandoned;
                }
            }
            self.batch_owners.insert(batch_id, winner);
            for intent_id in intent_ids {
                let intent = self
                    .intents
                    .get_mut(&intent_id)
                    .ok_or_else(|| invalid("confirmed transaction intent disappeared"))?;
                intent.state = FederationOnchainPaymentIntentState::Completed { txid };
            }
        }
        self.validate()?;
        Ok(FederationTransactionStateOutcome::Updated)
    }

    fn lineage_root(
        &self,
        mut proposal_id: FederationTransactionProposalId,
    ) -> Result<FederationTransactionProposalId> {
        let mut visited = BTreeSet::new();
        loop {
            if !visited.insert(proposal_id) {
                return Err(invalid("replacement lineage contains a cycle"));
            }
            let accepted = self
                .accepted_transactions
                .get(&proposal_id)
                .ok_or_else(|| invalid("replacement lineage references an unknown transaction"))?;
            match accepted.proposal.replacement_of {
                Some(previous) => proposal_id = previous,
                None => return Ok(proposal_id),
            }
        }
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
            match record.state {
                FederationOnchainPaymentIntentState::Accepted { proposal_id } => {
                    let accepted =
                        self.accepted_transactions
                            .get(&proposal_id)
                            .ok_or_else(|| {
                                invalid("accepted intent references an unknown transaction")
                            })?;
                    if !accepted.proposal.intent_ids.contains(intent_id) {
                        return Err(invalid(
                            "accepted intent is absent from its owning transaction",
                        ));
                    }
                }
                FederationOnchainPaymentIntentState::Completed { txid } => {
                    let owners = self
                        .accepted_transactions
                        .values()
                        .filter(|accepted| {
                            accepted.proposal.intent_ids.contains(intent_id)
                                && accepted.state == FederationAcceptedTransactionState::Confirmed
                        })
                        .collect::<Vec<_>>();
                    if owners.len() != 1
                        || owners[0].state != FederationAcceptedTransactionState::Confirmed
                        || owners[0].signed.as_ref().map(|signed| signed.txid) != Some(txid)
                    {
                        return Err(invalid(
                            "completed intent does not have one confirmed transaction owner",
                        ));
                    }
                }
                FederationOnchainPaymentIntentState::Pending
                | FederationOnchainPaymentIntentState::Failed => {}
            }
        }
        let accepted_batches = self
            .accepted_transactions
            .values()
            .map(|accepted| accepted.proposal.batch_id)
            .collect::<BTreeSet<_>>();
        if accepted_batches.len() != self.batch_owners.len() {
            return Err(invalid(
                "accepted transaction batch index has the wrong size",
            ));
        }
        for (batch_id, owner) in &self.batch_owners {
            let accepted = self
                .accepted_transactions
                .get(owner)
                .ok_or_else(|| invalid("transaction lineage head is unknown"))?;
            if accepted.proposal.batch_id != *batch_id
                || matches!(
                    accepted.state,
                    FederationAcceptedTransactionState::Replaced { .. }
                )
            {
                return Err(invalid("transaction lineage head is inconsistent"));
            }
        }
        for (proposal_id, accepted) in &self.accepted_transactions {
            accepted.proposal.validate()?;
            if accepted.accepted_operation_index == 0
                || accepted.proposal.proposal_id()? != *proposal_id
                || !self.batch_owners.contains_key(&accepted.proposal.batch_id)
            {
                return Err(invalid("accepted transaction durable identity mismatch"));
            }
            validate_policy_audit(&accepted.proposal, &accepted.policy_audit)?;
            match (&accepted.state, &accepted.signed, &accepted.broadcast) {
                (FederationAcceptedTransactionState::Accepted, Some(_), _)
                | (FederationAcceptedTransactionState::Accepted, _, Some(_))
                | (FederationAcceptedTransactionState::Signed, None, _)
                | (FederationAcceptedTransactionState::Signed, _, Some(_))
                | (FederationAcceptedTransactionState::BroadcastPending, None, _)
                | (FederationAcceptedTransactionState::BroadcastPending, _, None)
                | (FederationAcceptedTransactionState::Broadcast, None, _)
                | (FederationAcceptedTransactionState::Broadcast, _, None)
                | (FederationAcceptedTransactionState::Confirmed, None, _)
                | (FederationAcceptedTransactionState::Confirmed, _, None) => {
                    return Err(invalid("accepted transaction signed-state mismatch"));
                }
                (_, Some(signed), _) => {
                    if signed.policy_audit != accepted.policy_audit {
                        return Err(invalid(
                            "signed transaction policy audit differs from accepted state",
                        ));
                    }
                    signed.validate(&accepted.proposal)?;
                }
                (_, None, _) => {}
            }
            if let Some(broadcast) = &accepted.broadcast {
                broadcast.settlement_policy.validate()?;
                if broadcast.intent_operation_index == 0 {
                    return Err(invalid("broadcast intent operation index is zero"));
                }
                let mut observers = BTreeSet::new();
                for record in &broadcast.observations {
                    record.observation.validate()?;
                    if record.observation.proposal_id != *proposal_id
                        || accepted
                            .signed
                            .as_ref()
                            .is_none_or(|signed| record.observation.txid != signed.txid)
                        || record.operation_index <= broadcast.intent_operation_index
                        || !observers.insert(record.observation.observer)
                    {
                        return Err(invalid("broadcast observation durable identity mismatch"));
                    }
                }
                if !broadcast
                    .observations
                    .windows(2)
                    .all(|window| window[0].observation.observer < window[1].observation.observer)
                {
                    return Err(invalid("broadcast observers are not canonical"));
                }
                if let Some(success) = &broadcast.first_success {
                    success.observation.validate()?;
                    if success.observation.proposal_id != *proposal_id
                        || accepted
                            .signed
                            .as_ref()
                            .is_none_or(|signed| success.observation.txid != signed.txid)
                        || success.operation_index <= broadcast.intent_operation_index
                        || !success.observation.result.is_success_equivalent()
                    {
                        return Err(invalid("broadcast success durable identity mismatch"));
                    }
                }
                if let Some(settlement) = &broadcast.settlement {
                    if settlement.confirmations.len()
                        != usize::from(broadcast.settlement_policy.required_observers)
                    {
                        return Err(invalid("transaction settlement has the wrong quorum size"));
                    }
                    let mut settlement_observers = BTreeSet::new();
                    for confirmation in &settlement.confirmations {
                        confirmation.observation.validate()?;
                        let FederationTransactionBroadcastResult::Confirmed {
                            block_hash,
                            block_height,
                            confirmation_depth,
                        } = confirmation.observation.result
                        else {
                            return Err(invalid(
                                "transaction settlement contains non-confirmation evidence",
                            ));
                        };
                        if confirmation.observation.proposal_id != *proposal_id
                            || accepted
                                .signed
                                .as_ref()
                                .is_none_or(|signed| confirmation.observation.txid != signed.txid)
                            || confirmation.operation_index <= broadcast.intent_operation_index
                            || block_hash != settlement.block_hash
                            || block_height != settlement.block_height
                            || confirmation_depth
                                < broadcast.settlement_policy.minimum_confirmations
                            || !settlement_observers.insert(confirmation.observation.observer)
                        {
                            return Err(invalid(
                                "transaction settlement confirmation is inconsistent",
                            ));
                        }
                    }
                    if !settlement.confirmations.windows(2).all(|window| {
                        window[0].observation.observer < window[1].observation.observer
                    }) {
                        return Err(invalid(
                            "transaction settlement confirmations are not canonical",
                        ));
                    }
                }
                match accepted.state {
                    FederationAcceptedTransactionState::Broadcast
                        if broadcast.first_success.is_none() =>
                    {
                        return Err(invalid("broadcast lifecycle does not match observations"));
                    }
                    FederationAcceptedTransactionState::BroadcastPending
                        if broadcast.first_success.is_some() =>
                    {
                        return Err(invalid("broadcast lifecycle does not match observations"));
                    }
                    FederationAcceptedTransactionState::Confirmed
                        if broadcast.first_success.is_none() || broadcast.settlement.is_none() =>
                    {
                        return Err(invalid("confirmed lifecycle lacks settlement evidence"));
                    }
                    FederationAcceptedTransactionState::Broadcast
                    | FederationAcceptedTransactionState::BroadcastPending
                        if broadcast.settlement.is_some() =>
                    {
                        return Err(invalid("unconfirmed lifecycle carries settlement evidence"));
                    }
                    _ => {}
                }
            }
            let head = self
                .batch_owners
                .get(&accepted.proposal.batch_id)
                .copied()
                .ok_or_else(|| invalid("accepted transaction has no lineage head"))?;
            let confirmed = self
                .accepted_transactions
                .iter()
                .filter(|(_, candidate)| {
                    candidate.proposal.batch_id == accepted.proposal.batch_id
                        && candidate.state == FederationAcceptedTransactionState::Confirmed
                })
                .collect::<Vec<_>>();
            if confirmed.len() > 1 {
                return Err(invalid(
                    "replacement lineage has multiple confirmed transactions",
                ));
            }
            if head == *proposal_id {
                for intent_id in &accepted.proposal.intent_ids {
                    let record = self.intents.get(intent_id).ok_or_else(|| {
                        invalid("accepted transaction references a missing intent")
                    })?;
                    let expected_state = match confirmed.as_slice() {
                        [] => FederationOnchainPaymentIntentState::Accepted {
                            proposal_id: *proposal_id,
                        },
                        [(confirmed_id, winner)] if **confirmed_id == *proposal_id => {
                            FederationOnchainPaymentIntentState::Completed {
                                txid: winner
                                    .signed
                                    .as_ref()
                                    .ok_or_else(|| {
                                        invalid("confirmed transaction has no signed txid")
                                    })?
                                    .txid,
                            }
                        }
                        _ => {
                            return Err(invalid(
                                "transaction lineage head differs from confirmed winner",
                            ));
                        }
                    };
                    if record.state != expected_state {
                        return Err(invalid("lineage head does not own its intent"));
                    }
                }
            } else if !matches!(
                accepted.state,
                FederationAcceptedTransactionState::Replaced { .. }
                    | FederationAcceptedTransactionState::Abandoned
            ) {
                return Err(invalid("non-head lineage transaction is still active"));
            }
            if let Some(original_id) = accepted.proposal.replacement_of {
                let original = self
                    .accepted_transactions
                    .get(&original_id)
                    .ok_or_else(|| invalid("replacement predecessor is unknown"))?;
                let predecessor_is_linked = matches!(
                    original.state,
                    FederationAcceptedTransactionState::Replaced { by }
                        if by == *proposal_id
                ) || matches!(
                    original.state,
                    FederationAcceptedTransactionState::Abandoned
                        | FederationAcceptedTransactionState::Confirmed
                );
                if original.proposal.batch_id != accepted.proposal.batch_id
                    || !predecessor_is_linked
                {
                    return Err(invalid("replacement predecessor linkage is inconsistent"));
                }
            }
            let reservation_owner = self.lineage_root(*proposal_id)?;
            for input in &accepted.proposal.selected_inputs {
                if self.utxo_reservations.get(&input.outpoint.to_string())
                    != Some(&reservation_owner)
                {
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
        || audit.weight != proposal.estimated_signed_weight()
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

fn transaction_settlement_from_observations(
    policy: FederationTransactionSettlementPolicy,
    observations: &[FederationTransactionBroadcastObservationRecord],
) -> Option<FederationTransactionSettlement> {
    let required = usize::from(policy.required_observers);
    let mut by_block = BTreeMap::<([u8; 32], u32), Vec<_>>::new();
    for record in observations {
        let FederationTransactionBroadcastResult::Confirmed {
            block_hash,
            block_height,
            confirmation_depth,
        } = record.observation.result
        else {
            continue;
        };
        if confirmation_depth < policy.minimum_confirmations {
            continue;
        }
        by_block
            .entry((block_hash.to_byte_array(), block_height))
            .or_default()
            .push(record.clone());
    }
    by_block.into_iter().find_map(|((hash, height), records)| {
        (records.len() >= required).then(|| FederationTransactionSettlement {
            block_hash: BlockHash::from_byte_array(hash),
            block_height: height,
            confirmations: records.into_iter().take(required).collect(),
        })
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
    use crate::federation::{
        FederationAddressAllocationState, FederationAddressKeychain,
        FederationChainObservationState, FederationRecoveryAction, FederationWalletRecoveryStatus,
    };

    fn recovery_action_count(
        state: &FederationTransactionState,
        action: FederationRecoveryAction,
    ) -> u64 {
        FederationWalletRecoveryStatus::from_states(
            &FederationAddressAllocationState::new(),
            &FederationChainObservationState::new(),
            state,
        )
        .expect("recovery status")
        .actions
        .iter()
        .filter(|entry| entry.action == action)
        .map(|entry| entry.count)
        .sum()
    }

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
            FederationOperationId::from_bytes([marker; 32]),
            ScriptBuf::from_bytes(vec![0x51, marker]),
            10_000 + u64::from(marker),
            1_000,
            fee_tier,
            match fee_tier {
                0 => 1,
                1 => 6,
                _ => 144,
            },
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

    fn replacement_proposal(
        original: &FederationTransactionProposal,
        additional_fee_sat: u64,
    ) -> FederationTransactionProposal {
        let batch = FederationOnchainPaymentBatch::new(
            vec![intent("quote-a", 0, 30, 1), intent("quote-b", 1, 20, 2)],
            original.eligibility_cutoff,
        )
        .expect("replacement batch");
        assert_eq!(batch.batch_id, original.batch_id);
        let mut transaction = original.unsigned_transaction.clone();
        let original_change = original.change_output.as_ref().expect("original change");
        let change_index = original_change.output_index as usize;
        transaction.output[change_index].value = transaction.output[change_index]
            .value
            .checked_sub(Amount::from_sat(additional_fee_sat))
            .expect("replacement change");
        let fee_sat = original.fee_sat + additional_fee_sat;
        let mut recipients = original.recipient_outputs.clone();
        let base = fee_sat / recipients.len() as u64;
        let remainder = fee_sat % recipients.len() as u64;
        for (index, recipient) in recipients.iter_mut().enumerate() {
            recipient.fee_contribution_sat = base + u64::from(index == 0) * remainder;
        }
        let mut change = original_change.clone();
        change.output = transaction.output[change_index].clone();
        FederationTransactionProposal::new(
            &batch,
            member(2),
            transaction,
            original.selected_inputs.clone(),
            recipients,
            Some(change),
            fee_sat,
            Some(original.proposal_id().expect("original id")),
        )
        .expect("replacement proposal")
    }

    fn structurally_sign(accepted: &FederationAcceptedTransaction) -> FederationSignedTransaction {
        let mut transaction = accepted.proposal.unsigned_transaction.clone();
        let audits = transaction
            .input
            .iter_mut()
            .enumerate()
            .map(|(input_index, input)| {
                let signature = vec![u8::try_from(input_index + 31).expect("signature"); 64];
                input.witness.push(&signature);
                FederationFrostInputSignatureAudit {
                    input_index: input_index as u32,
                    session_id: [u8::try_from(input_index + 41).expect("session"); 32],
                    attempt: 2,
                    signer_set: vec![1, 2],
                    package_hash: [u8::try_from(input_index + 51).expect("package"); 32],
                    signature,
                }
            })
            .collect();
        FederationSignedTransaction::new(accepted, &transaction, audits)
            .expect("structural signed transaction")
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
    fn mixed_fee_tiers_select_the_same_bounded_batch() {
        let unordered = vec![
            intent("economy", 2, 1, 3),
            intent("standard", 1, 3, 2),
            intent("immediate-late", 0, 4, 4),
            intent("immediate-early", 0, 2, 1),
        ];
        let mut reversed = unordered.clone();
        reversed.reverse();
        let first = FederationOnchainPaymentBatch::select(unordered, 10, 3)
            .expect("selected fee-tier batch");
        let second = FederationOnchainPaymentBatch::select(reversed, 10, 3)
            .expect("reversed fee-tier batch");
        assert_eq!(first, second);
        assert_eq!(
            first
                .intents
                .iter()
                .map(|intent| intent.quote.as_str())
                .collect::<Vec<_>>(),
            vec!["immediate-early", "immediate-late", "standard"]
        );
        assert_eq!(
            first
                .intents
                .iter()
                .map(|intent| intent.fee_target_blocks)
                .collect::<Vec<_>>(),
            vec![1, 1, 6]
        );
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
        assert!(replacement_proposal(&proposal, 500)
            .conflict_keys()
            .expect("replacement conflict keys")
            .is_empty());
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
            "0a1ea7aeccbb4d99a8cdcb970ef4c8ca0f533be34c78e7ac9c855bed923f9b88"
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
            weight: proposal.estimated_signed_weight(),
            fee_contributions: proposal
                .recipient_outputs
                .iter()
                .map(|recipient| (recipient.intent_id, recipient.fee_contribution_sat))
                .collect(),
            taproot_sighashes: vec![[9; 32]; proposal.selected_inputs.len()],
        }
    }

    fn signed_transaction_state() -> (
        FederationTransactionState,
        FederationTransactionProposalId,
        Txid,
    ) {
        let proposal = proposal();
        let proposal_id = proposal.proposal_id().expect("proposal id");
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
            .expect("accept transaction");
        let accepted = state
            .accepted_transaction(proposal_id)
            .expect("accepted transaction")
            .clone();
        assert_eq!(
            recovery_action_count(&state, FederationRecoveryAction::StartFreshSigningAttempt),
            1
        );
        assert_eq!(state.reservation_count(), proposal.selected_inputs.len());
        assert_eq!(
            recovery_action_count(&state, FederationRecoveryAction::PreserveStickyReservation),
            proposal.selected_inputs.len() as u64
        );
        let mut transaction = proposal.unsigned_transaction.clone();
        let input_signatures = transaction
            .input
            .iter_mut()
            .enumerate()
            .map(|(input_index, input)| {
                let signature = vec![u8::try_from(input_index + 7).expect("signature byte"); 64];
                input.witness.push(&signature);
                FederationFrostInputSignatureAudit {
                    input_index: u32::try_from(input_index).expect("input index"),
                    session_id: [u8::try_from(input_index + 8).expect("session byte"); 32],
                    attempt: 1,
                    signer_set: vec![1, 2],
                    package_hash: [u8::try_from(input_index + 9).expect("package byte"); 32],
                    signature,
                }
            })
            .collect();
        let signed = FederationSignedTransaction::new(&accepted, &transaction, input_signatures)
            .expect("signed record");
        let txid = signed.txid;
        state
            .mark_transaction_signed(proposal_id, signed)
            .expect("persist signed transaction");
        (state, proposal_id, txid)
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
            signed: None,
            broadcast: None,
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

    #[test]
    fn signed_transaction_persistence_is_exact_idempotent_and_digest_visible() {
        let proposal = proposal();
        let proposal_id = proposal.proposal_id().expect("proposal id");
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
        let accepted = state
            .accepted_transaction(proposal_id)
            .expect("accepted transaction")
            .clone();
        let mut transaction = proposal.unsigned_transaction.clone();
        let input_signatures = transaction
            .input
            .iter_mut()
            .enumerate()
            .map(|(input_index, input)| {
                let signature = vec![u8::try_from(input_index + 7).expect("signature byte"); 64];
                input.witness.push(&signature);
                FederationFrostInputSignatureAudit {
                    input_index: u32::try_from(input_index).expect("input index"),
                    session_id: [u8::try_from(input_index + 8).expect("session byte"); 32],
                    attempt: 1,
                    signer_set: vec![1, 2],
                    package_hash: [u8::try_from(input_index + 9).expect("package byte"); 32],
                    signature,
                }
            })
            .collect();
        let signed = FederationSignedTransaction::new(&accepted, &transaction, input_signatures)
            .expect("signed record");
        assert_eq!(
            state
                .mark_transaction_signed(proposal_id, signed.clone())
                .expect("persist signed transaction"),
            FederationTransactionStateOutcome::Updated
        );
        assert_eq!(
            state
                .mark_transaction_signed(proposal_id, signed.clone())
                .expect("exact signed retry"),
            FederationTransactionStateOutcome::Existing
        );
        assert_eq!(
            state
                .accepted_transaction(proposal_id)
                .expect("signed transaction")
                .state,
            FederationAcceptedTransactionState::Signed
        );
        let mut conflicting = signed;
        conflicting.input_signatures[0].package_hash[0] ^= 1;
        assert!(state
            .mark_transaction_signed(proposal_id, conflicting)
            .is_err());
        assert_eq!(
            FederationTransactionState::from_bytes(&state.to_bytes().expect("state bytes"))
                .expect("state round trip"),
            state
        );
    }

    #[test]
    fn signed_transaction_verifies_every_exact_taproot_key_path_signature() {
        use bitcoin::secp256k1::{Keypair, SecretKey};

        let secp = Secp256k1::new();
        let mut proposal = proposal();
        let keypairs = proposal
            .selected_inputs
            .iter_mut()
            .enumerate()
            .map(|(index, selected)| {
                let mut secret = [0u8; 32];
                secret[31] = u8::try_from(index + 1).expect("secret marker");
                let secret = SecretKey::from_slice(&secret).expect("secret key");
                let keypair = Keypair::from_secret_key(&secp, &secret);
                let (output_key, _) = XOnlyPublicKey::from_keypair(&keypair);
                let mut script = vec![0x51, 0x20];
                script.extend_from_slice(&output_key.serialize());
                selected.previous_output.script_pubkey = ScriptBuf::from_bytes(script);
                keypair
            })
            .collect::<Vec<_>>();
        proposal.psbt_commitment = proposal
            .expected_psbt_commitment()
            .expect("PSBT commitment");
        let mut audit = policy_audit(&proposal);
        audit.taproot_sighashes = keypairs
            .iter()
            .enumerate()
            .map(|(index, _)| [u8::try_from(index + 17).expect("sighash marker"); 32])
            .collect();
        let accepted = FederationAcceptedTransaction {
            proposal: proposal.clone(),
            policy_audit: audit.clone(),
            accepted_operation_id: FederationOperationId::from_bytes([5; 32]),
            accepted_operation_index: 50,
            state: FederationAcceptedTransactionState::Accepted,
            signed: None,
            broadcast: None,
        };
        let mut transaction = proposal.unsigned_transaction.clone();
        let input_signatures = keypairs
            .iter()
            .enumerate()
            .map(|(index, keypair)| {
                let signature = secp
                    .sign_schnorr_no_aux_rand(
                        &Message::from_digest(audit.taproot_sighashes[index]),
                        keypair,
                    )
                    .serialize()
                    .to_vec();
                transaction.input[index].witness.push(&signature);
                FederationFrostInputSignatureAudit {
                    input_index: u32::try_from(index).expect("input index"),
                    session_id: [u8::try_from(index + 31).expect("session marker"); 32],
                    attempt: 1,
                    signer_set: vec![1, 2],
                    package_hash: [u8::try_from(index + 41).expect("package marker"); 32],
                    signature,
                }
            })
            .collect();
        let signed = FederationSignedTransaction::new(&accepted, &transaction, input_signatures)
            .expect("signed record");
        assert_eq!(transaction.weight().to_wu(), proposal.estimated_signed_weight());
        assert_eq!(signed.policy_audit.weight, transaction.weight().to_wu());
        signed
            .verify_signatures(&proposal)
            .expect("every exact signature verifies");

        let mut invalid = signed;
        invalid.input_signatures[1].signature[0] ^= 1;
        let mut invalid_transaction = invalid.transaction().expect("signed transaction");
        invalid_transaction.input[1].witness.clear();
        invalid_transaction.input[1]
            .witness
            .push(&invalid.input_signatures[1].signature);
        invalid.transaction_bytes = serialize(&invalid_transaction);
        assert!(invalid.verify_signatures(&proposal).is_err());
    }

    #[test]
    fn broadcast_intent_and_member_results_are_exact_restart_safe_and_convergent() {
        let (mut state, proposal_id, txid) = signed_transaction_state();
        assert_eq!(
            recovery_action_count(&state, FederationRecoveryAction::PersistBroadcastIntent),
            1
        );
        assert_eq!(
            recovery_action_count(
                &state,
                FederationRecoveryAction::ReapplyPersistedTransaction
            ),
            1
        );
        let intent = FederationTransactionBroadcastIntent {
            proposal_id,
            txid,
            settlement_policy: FederationTransactionSettlementPolicy::new(1, 1)
                .expect("settlement policy"),
        };
        assert_eq!(
            state
                .authorize_transaction_broadcast(
                    intent,
                    FederationOperationId::from_bytes([51; 32]),
                    51,
                )
                .expect("authorize broadcast"),
            FederationTransactionStateOutcome::Updated
        );
        assert_eq!(
            state
                .authorize_transaction_broadcast(
                    intent,
                    FederationOperationId::from_bytes([52; 32]),
                    52,
                )
                .expect("intent failover replay"),
            FederationTransactionStateOutcome::Existing
        );
        assert_eq!(
            recovery_action_count(
                &state,
                FederationRecoveryAction::RebroadcastPersistedTransaction
            ),
            1
        );
        state = FederationTransactionState::from_bytes(
            &state.to_bytes().expect("broadcast-not-observed bytes"),
        )
        .expect("restart broadcast-not-observed state");

        let results = [
            FederationTransactionBroadcastResult::Rejected,
            FederationTransactionBroadcastResult::Transient,
            FederationTransactionBroadcastResult::Unknown,
        ];
        for (offset, result) in results.into_iter().enumerate() {
            let index = u64::try_from(offset).expect("offset") + 52;
            state
                .record_transaction_broadcast_observation(
                    FederationTransactionBroadcastObservation {
                        proposal_id,
                        txid,
                        observer: member(u16::try_from(offset + 1).expect("member")),
                        attempt: 1,
                        result,
                        detail: Some("redacted backend result".to_string()),
                    },
                    FederationOperationId::from_bytes(
                        [u8::try_from(index).expect("operation marker"); 32],
                    ),
                    index,
                )
                .expect("record retryable or rejected result");
        }
        assert_eq!(
            state
                .accepted_transaction(proposal_id)
                .expect("accepted")
                .state,
            FederationAcceptedTransactionState::BroadcastPending
        );

        let accepted = FederationTransactionBroadcastObservation {
            proposal_id,
            txid,
            observer: member(4),
            attempt: 1,
            result: FederationTransactionBroadcastResult::Accepted,
            detail: None,
        };
        state
            .record_transaction_broadcast_observation(
                accepted.clone(),
                FederationOperationId::from_bytes([55; 32]),
                55,
            )
            .expect("accepted broadcast");
        assert_eq!(
            state
                .record_transaction_broadcast_observation(
                    accepted.clone(),
                    FederationOperationId::from_bytes([56; 32]),
                    56,
                )
                .expect("exact response replay"),
            FederationTransactionStateOutcome::Existing
        );
        let mut conflicting = accepted;
        conflicting.result = FederationTransactionBroadcastResult::Unknown;
        assert!(state
            .record_transaction_broadcast_observation(
                conflicting,
                FederationOperationId::from_bytes([56; 32]),
                56,
            )
            .is_err());

        state
            .record_transaction_broadcast_observation(
                FederationTransactionBroadcastObservation {
                    proposal_id,
                    txid,
                    observer: member(5),
                    attempt: 1,
                    result: FederationTransactionBroadcastResult::AlreadyKnown,
                    detail: None,
                },
                FederationOperationId::from_bytes([56; 32]),
                56,
            )
            .expect("concurrent already-known result");
        state
            .record_transaction_broadcast_observation(
                FederationTransactionBroadcastObservation {
                    proposal_id,
                    txid,
                    observer: member(4),
                    attempt: 2,
                    result: FederationTransactionBroadcastResult::Unknown,
                    detail: Some("mempool recheck was ambiguous".to_string()),
                },
                FederationOperationId::from_bytes([57; 32]),
                57,
            )
            .expect("ambiguous post-success recheck");
        assert_eq!(
            state
                .accepted_transaction(proposal_id)
                .expect("sticky broadcast")
                .state,
            FederationAcceptedTransactionState::Broadcast
        );
        assert_eq!(
            recovery_action_count(
                &state,
                FederationRecoveryAction::CollectConfirmationEvidence
            ),
            1
        );
        state
            .record_transaction_broadcast_observation(
                FederationTransactionBroadcastObservation {
                    proposal_id,
                    txid,
                    observer: member(4),
                    attempt: 3,
                    result: FederationTransactionBroadcastResult::Accepted,
                    detail: None,
                },
                FederationOperationId::from_bytes([58; 32]),
                58,
            )
            .expect("later exact-byte rebroadcast");
        state
            .record_transaction_broadcast_observation(
                FederationTransactionBroadcastObservation {
                    proposal_id,
                    txid,
                    observer: member(1),
                    attempt: 2,
                    result: FederationTransactionBroadcastResult::Confirmed {
                        block_hash: BlockHash::from_byte_array([9; 32]),
                        block_height: 101,
                        confirmation_depth: 1,
                    },
                    detail: None,
                },
                FederationOperationId::from_bytes([59; 32]),
                59,
            )
            .expect("confirmed reconciliation");
        assert_eq!(
            state
                .accepted_transaction(proposal_id)
                .expect("broadcast transaction")
                .state,
            FederationAcceptedTransactionState::Confirmed
        );
        assert!(state.intents().values().all(|record| matches!(
            record.state,
            FederationOnchainPaymentIntentState::Completed { txid: completed } if completed == txid
        )));
        assert!(state
            .completed_payment_for_quote("quote-a")
            .expect("completed payment receipt")
            .is_some());
        assert_eq!(
            recovery_action_count(&state, FederationRecoveryAction::NoOp),
            3,
            "one confirmed transaction and two completed intents are terminal no-ops"
        );

        let restarted = FederationTransactionState::from_bytes(
            &state.to_bytes().expect("persist broadcast state"),
        )
        .expect("restart broadcast state");
        assert_eq!(restarted, state);
        assert_eq!(
            restarted
                .accepted_transaction(proposal_id)
                .expect("restarted transaction")
                .signed
                .as_ref()
                .expect("signed bytes")
                .txid,
            txid
        );
    }

    #[test]
    fn either_original_or_replacement_confirmation_yields_one_terminal_lineage() {
        let (mut state, original_id, original_txid) = signed_transaction_state();
        let settlement_policy =
            FederationTransactionSettlementPolicy::new(1, 1).expect("settlement policy");
        state
            .authorize_transaction_broadcast(
                FederationTransactionBroadcastIntent {
                    proposal_id: original_id,
                    txid: original_txid,
                    settlement_policy,
                },
                FederationOperationId::from_bytes([60; 32]),
                60,
            )
            .expect("authorize original broadcast");

        let original = state
            .accepted_transaction(original_id)
            .expect("original transaction")
            .proposal
            .clone();
        let replacement = replacement_proposal(&original, 500);
        let replacement_id = replacement.proposal_id().expect("replacement id");
        state
            .accept_replacement(
                replacement.clone(),
                policy_audit(&replacement),
                FederationOperationId::from_bytes([61; 32]),
                61,
            )
            .expect("accept replacement");
        assert_eq!(state.lineage_head(original.batch_id), Some(replacement_id));
        assert_eq!(
            state
                .lineage_reservation_owner(replacement_id)
                .expect("replacement reservation owner"),
            original_id
        );
        assert_eq!(
            state
                .accepted_transaction(original_id)
                .expect("replaced original")
                .state,
            FederationAcceptedTransactionState::Replaced { by: replacement_id }
        );
        assert!(replacement
            .selected_inputs
            .iter()
            .all(|input| state.reservation(input.outpoint) == Some(original_id)));

        let mut original_wins = state.clone();
        original_wins
            .record_transaction_broadcast_observation(
                FederationTransactionBroadcastObservation {
                    proposal_id: original_id,
                    txid: original_txid,
                    observer: member(1),
                    attempt: 1,
                    result: FederationTransactionBroadcastResult::Confirmed {
                        block_hash: BlockHash::from_byte_array([11; 32]),
                        block_height: 101,
                        confirmation_depth: 1,
                    },
                    detail: None,
                },
                FederationOperationId::from_bytes([62; 32]),
                62,
            )
            .expect("confirm original after replacement acceptance");
        assert_eq!(
            original_wins.lineage_head(original.batch_id),
            Some(original_id)
        );
        assert_eq!(
            original_wins
                .accepted_transaction(original_id)
                .expect("confirmed original")
                .state,
            FederationAcceptedTransactionState::Confirmed
        );
        assert_eq!(
            original_wins
                .accepted_transaction(replacement_id)
                .expect("abandoned replacement")
                .state,
            FederationAcceptedTransactionState::Abandoned
        );
        assert_eq!(
            original_wins
                .completed_payment_for_quote("quote-a")
                .expect("original receipt")
                .expect("completed original payment")
                .txid,
            original_txid
        );

        let accepted_replacement = state
            .accepted_transaction(replacement_id)
            .expect("accepted replacement")
            .clone();
        let signed_replacement = structurally_sign(&accepted_replacement);
        let replacement_txid = signed_replacement.txid;
        state
            .mark_transaction_signed(replacement_id, signed_replacement)
            .expect("persist replacement signature");
        state
            .authorize_transaction_broadcast(
                FederationTransactionBroadcastIntent {
                    proposal_id: replacement_id,
                    txid: replacement_txid,
                    settlement_policy,
                },
                FederationOperationId::from_bytes([63; 32]),
                63,
            )
            .expect("authorize replacement broadcast");
        state
            .record_transaction_broadcast_observation(
                FederationTransactionBroadcastObservation {
                    proposal_id: original_id,
                    txid: original_txid,
                    observer: member(1),
                    attempt: 2,
                    result: FederationTransactionBroadcastResult::Accepted,
                    detail: None,
                },
                FederationOperationId::from_bytes([64; 32]),
                64,
            )
            .expect("observe original still in mempool");
        state
            .record_transaction_broadcast_observation(
                FederationTransactionBroadcastObservation {
                    proposal_id: replacement_id,
                    txid: replacement_txid,
                    observer: member(2),
                    attempt: 1,
                    result: FederationTransactionBroadcastResult::Confirmed {
                        block_hash: BlockHash::from_byte_array([12; 32]),
                        block_height: 102,
                        confirmation_depth: 1,
                    },
                    detail: None,
                },
                FederationOperationId::from_bytes([65; 32]),
                65,
            )
            .expect("confirm replacement while original was observed");
        assert_eq!(state.lineage_head(original.batch_id), Some(replacement_id));
        assert_eq!(
            state
                .accepted_transaction(replacement_id)
                .expect("confirmed replacement")
                .state,
            FederationAcceptedTransactionState::Confirmed
        );
        assert_eq!(
            state
                .accepted_transaction(original_id)
                .expect("abandoned original")
                .state,
            FederationAcceptedTransactionState::Abandoned
        );
        assert_eq!(
            state
                .completed_payment_for_quote("quote-a")
                .expect("replacement receipt")
                .expect("completed replacement payment")
                .txid,
            replacement_txid
        );
        assert_eq!(
            state
                .accepted_transactions()
                .values()
                .filter(|accepted| {
                    accepted.state == FederationAcceptedTransactionState::Confirmed
                })
                .count(),
            1
        );
        let terminal = state.clone();
        assert!(state
            .record_transaction_broadcast_observation(
                FederationTransactionBroadcastObservation {
                    proposal_id: original_id,
                    txid: original_txid,
                    observer: member(1),
                    attempt: 3,
                    result: FederationTransactionBroadcastResult::Confirmed {
                        block_hash: BlockHash::from_byte_array([13; 32]),
                        block_height: 103,
                        confirmation_depth: 1,
                    },
                    detail: None,
                },
                FederationOperationId::from_bytes([66; 32]),
                66,
            )
            .is_err());
        assert_eq!(state, terminal, "a terminal lineage winner cannot flip");
    }

    #[test]
    fn settlement_requires_matching_member_and_confirmation_thresholds() {
        let (mut state, proposal_id, txid) = signed_transaction_state();
        state
            .authorize_transaction_broadcast(
                FederationTransactionBroadcastIntent {
                    proposal_id,
                    txid,
                    settlement_policy: FederationTransactionSettlementPolicy::new(2, 2)
                        .expect("settlement policy"),
                },
                FederationOperationId::from_bytes([61; 32]),
                61,
            )
            .expect("authorize broadcast");
        for (index, observer, block_hash, confirmation_depth) in [
            (62, member(1), BlockHash::from_byte_array([1; 32]), 1),
            (63, member(2), BlockHash::from_byte_array([2; 32]), 2),
        ] {
            state
                .record_transaction_broadcast_observation(
                    FederationTransactionBroadcastObservation {
                        proposal_id,
                        txid,
                        observer,
                        attempt: 1,
                        result: FederationTransactionBroadcastResult::Confirmed {
                            block_hash,
                            block_height: 101,
                            confirmation_depth,
                        },
                        detail: None,
                    },
                    FederationOperationId::from_bytes(
                        [u8::try_from(index).expect("operation marker"); 32],
                    ),
                    index,
                )
                .expect("record non-settling confirmation");
        }
        assert_eq!(
            state
                .accepted_transaction(proposal_id)
                .expect("accepted transaction")
                .state,
            FederationAcceptedTransactionState::Broadcast
        );
        assert!(state
            .completed_payment_for_quote("quote-a")
            .expect("pending receipt")
            .is_none());

        state
            .record_transaction_broadcast_observation(
                FederationTransactionBroadcastObservation {
                    proposal_id,
                    txid,
                    observer: member(1),
                    attempt: 2,
                    result: FederationTransactionBroadcastResult::Confirmed {
                        block_hash: BlockHash::from_byte_array([2; 32]),
                        block_height: 101,
                        confirmation_depth: 2,
                    },
                    detail: None,
                },
                FederationOperationId::from_bytes([64; 32]),
                64,
            )
            .expect("record threshold confirmation");
        let accepted = state
            .accepted_transaction(proposal_id)
            .expect("confirmed transaction");
        assert_eq!(
            accepted.state,
            FederationAcceptedTransactionState::Confirmed
        );
        assert_eq!(
            accepted
                .broadcast
                .as_ref()
                .and_then(|broadcast| broadcast.settlement.as_ref())
                .expect("settlement evidence")
                .confirmations
                .len(),
            2
        );
        let receipt = state
            .completed_payment_for_quote("quote-a")
            .expect("completed receipt")
            .expect("receipt");
        assert_eq!(receipt.txid, txid);
        assert_eq!(
            receipt.melt_operation_id,
            FederationOperationId::from_bytes([1; 32])
        );
        assert_eq!(
            state
                .completed_payment_for_quote("quote-b")
                .expect("batched receipt")
                .expect("second receipt")
                .txid,
            txid
        );
    }

    #[test]
    fn broadcast_never_accepts_a_second_txid_or_pre_intent_observation() {
        let (state, proposal_id, txid) = signed_transaction_state();
        let mut state = FederationTransactionState::from_bytes(
            &state.to_bytes().expect("signed-not-broadcast bytes"),
        )
        .expect("restart signed-not-broadcast state");
        let wrong_txid = Txid::from_byte_array([99; 32]);
        assert!(state
            .record_transaction_broadcast_observation(
                FederationTransactionBroadcastObservation {
                    proposal_id,
                    txid,
                    observer: member(1),
                    attempt: 1,
                    result: FederationTransactionBroadcastResult::Unknown,
                    detail: None,
                },
                FederationOperationId::from_bytes([51; 32]),
                51,
            )
            .is_err());
        assert!(state
            .authorize_transaction_broadcast(
                FederationTransactionBroadcastIntent {
                    proposal_id,
                    txid: wrong_txid,
                    settlement_policy: FederationTransactionSettlementPolicy::new(1, 1)
                        .expect("settlement policy"),
                },
                FederationOperationId::from_bytes([51; 32]),
                51,
            )
            .is_err());
        state
            .authorize_transaction_broadcast(
                FederationTransactionBroadcastIntent {
                    proposal_id,
                    txid,
                    settlement_policy: FederationTransactionSettlementPolicy::new(1, 1)
                        .expect("settlement policy"),
                },
                FederationOperationId::from_bytes([51; 32]),
                51,
            )
            .expect("valid intent");
        assert!(state
            .record_transaction_broadcast_observation(
                FederationTransactionBroadcastObservation {
                    proposal_id,
                    txid: wrong_txid,
                    observer: member(2),
                    attempt: 1,
                    result: FederationTransactionBroadcastResult::Accepted,
                    detail: None,
                },
                FederationOperationId::from_bytes([52; 32]),
                52,
            )
            .is_err());
    }
}
