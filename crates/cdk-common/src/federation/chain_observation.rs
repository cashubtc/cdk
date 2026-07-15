//! Canonical chain evidence and conservative federation UTXO materialization.
//!
//! Member-local BDK scans remain untrusted input. Only bounded observations
//! ordered by consensus enter this state machine, and no single observation can
//! make a deposit spendable when the configured member quorum is greater than
//! one.

use std::collections::{BTreeMap, BTreeSet};

use bitcoin::hashes::{sha256, Hash, HashEngine};
use bitcoin::{BlockHash, OutPoint, Script, TxOut};
use serde::{Deserialize, Serialize};

use super::{
    FederationAddressAllocation, FederationAddressAllocationScope, FederationAddressKeychain,
    FederationError, FederationFrostApplicationId, FederationMaterializedStateRow,
    FederationOperationId, MemberId, Result,
};

const CHAIN_OBSERVATION_STATE_VERSION: u16 = 1;
const MAX_QUOTE_ID_BYTES: usize = 256;
const MAX_SOURCE_ID_BYTES: usize = 128;
const ONCHAIN_QUOTE_PAYMENT_DOMAIN: &[u8] = b"cdk-federation-onchain-quote-payment-v1";

/// Whether multiple members are backed by one operational chain source.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FederationChainSourceScope {
    /// This source is independently operated for one member.
    Independent,
    /// This source is intentionally shared by multiple members.
    Shared,
}

impl core::fmt::Display for FederationChainSourceScope {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter.write_str(match self {
            Self::Independent => "independent",
            Self::Shared => "shared",
        })
    }
}

/// Bounded, non-secret chain-source identity used only for diagnostics.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FederationChainSourceDescriptor {
    /// Operational independence classification.
    pub scope: FederationChainSourceScope,
    /// Stable opaque identifier; never an endpoint or credential.
    pub source_id: String,
}

impl FederationChainSourceDescriptor {
    /// Construct a safe diagnostic source descriptor.
    pub fn new(scope: FederationChainSourceScope, source_id: impl Into<String>) -> Result<Self> {
        let source = Self {
            scope,
            source_id: source_id.into(),
        };
        source.validate()?;
        Ok(source)
    }

    fn validate(&self) -> Result<()> {
        if self.source_id.is_empty()
            || self.source_id.len() > MAX_SOURCE_ID_BYTES
            || !self
                .source_id
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
        {
            return Err(invalid_observation(
                "chain source id must be non-empty, bounded, and opaque",
            ));
        }
        Ok(())
    }
}

/// Immutable reference to one consensus-allocated descriptor address.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FederationAllocatedAddressRef {
    /// Registered CDK on-chain application.
    pub application_id: FederationFrostApplicationId,
    /// Descriptor application key epoch.
    pub application_key_epoch: u64,
    /// Mint quote/allocation key.
    pub quote: String,
    /// Descriptor branch.
    pub keychain: FederationAddressKeychain,
    /// Accepted non-hardened descriptor index.
    pub derivation_index: u32,
}

impl FederationAllocatedAddressRef {
    /// Return the application/epoch allocation namespace.
    #[must_use]
    pub fn scope(&self) -> FederationAddressAllocationScope {
        FederationAddressAllocationScope {
            application_id: self.application_id,
            application_key_epoch: self.application_key_epoch,
        }
    }

    /// Validate this reference against the accepted allocation projection.
    pub fn validate_allocation(&self, allocation: &FederationAddressAllocation) -> Result<()> {
        if self.application_key_epoch == 0
            || self.quote.is_empty()
            || self.quote.len() > MAX_QUOTE_ID_BYTES
            || self.quote != allocation.quote
            || self.keychain != allocation.keychain
            || self.derivation_index != allocation.derivation_index
        {
            return Err(invalid_observation(
                "deposit does not match its accepted address allocation",
            ));
        }
        Ok(())
    }
}

/// Agreed block anchor and claimed confirmation depth for one observation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FederationDepositConfirmation {
    /// Block containing the transaction.
    pub block_hash: BlockHash,
    /// Height of the containing block.
    pub block_height: u32,
    /// Confirmation depth seen by this observer.
    pub confirmation_depth: u32,
}

impl FederationDepositConfirmation {
    fn validate(self) -> Result<()> {
        if self.confirmation_depth == 0 {
            return Err(invalid_observation(
                "confirmed deposit evidence must have non-zero depth",
            ));
        }
        Ok(())
    }
}

/// One member's bounded observation of a wallet transaction output.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FederationDepositObservation {
    /// Consensus allocation that owns the output script.
    pub allocation: FederationAllocatedAddressRef,
    /// Observed transaction output identifier.
    pub outpoint: OutPoint,
    /// Exact amount and script at the outpoint.
    pub txout: TxOut,
    /// Block evidence, or `None` while unconfirmed.
    pub confirmation: Option<FederationDepositConfirmation>,
    /// Federation member reporting its local chain view.
    pub observer: MemberId,
    /// Non-secret source-independence diagnostic.
    pub source: FederationChainSourceDescriptor,
}

impl FederationDepositObservation {
    /// Validate bounded operation-local fields.
    pub fn validate(&self) -> Result<()> {
        if self.allocation.application_key_epoch == 0
            || self.allocation.quote.is_empty()
            || self.allocation.quote.len() > MAX_QUOTE_ID_BYTES
            || self.txout.value.to_sat() == 0
            || self.txout.script_pubkey.is_empty()
        {
            return Err(invalid_observation(
                "deposit allocation, value, or script is invalid",
            ));
        }
        self.source.validate()?;
        if let Some(confirmation) = self.confirmation {
            confirmation.validate()?;
        }
        Ok(())
    }

    /// Return true for evidence submitted after block inclusion.
    #[must_use]
    pub fn is_confirmed(&self) -> bool {
        self.confirmation.is_some()
    }

    fn same_output(&self, other: &Self) -> bool {
        self.allocation == other.allocation
            && self.outpoint == other.outpoint
            && self.txout == other.txout
    }
}

/// Conservative evidence that a previously accepted block anchor disconnected.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FederationDepositReorgObservation {
    /// Output whose containing block disconnected.
    pub outpoint: OutPoint,
    /// Previously accepted containing block hash.
    pub disconnected_block_hash: BlockHash,
    /// Previously accepted containing block height.
    pub disconnected_block_height: u32,
    /// Federation member reporting its local chain view.
    pub observer: MemberId,
    /// Non-secret source-independence diagnostic.
    pub source: FederationChainSourceDescriptor,
}

impl FederationDepositReorgObservation {
    /// Validate bounded operation-local fields.
    pub fn validate(&self) -> Result<()> {
        self.source.validate()
    }
}

/// Canonical chain evidence admitted to federation consensus.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "type",
    content = "data",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum FederationChainObservation {
    /// A member observed an allocated wallet output.
    Deposit(FederationDepositObservation),
    /// A member observed disconnection of its accepted block anchor.
    Reorg(FederationDepositReorgObservation),
}

impl FederationChainObservation {
    /// Validate bounded operation-local fields.
    pub fn validate(&self) -> Result<()> {
        match self {
            Self::Deposit(observation) => observation.validate(),
            Self::Reorg(observation) => observation.validate(),
        }
    }

    /// Member whose authorization must match this observation.
    #[must_use]
    pub fn observer(&self) -> MemberId {
        match self {
            Self::Deposit(observation) => observation.observer,
            Self::Reorg(observation) => observation.observer,
        }
    }

    /// Output affected by this evidence.
    #[must_use]
    pub fn outpoint(&self) -> OutPoint {
        match self {
            Self::Deposit(observation) => observation.outpoint,
            Self::Reorg(observation) => observation.outpoint,
        }
    }
}

/// Consensus wallet lifecycle state for one outpoint.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FederationWalletUtxoState {
    /// Seen without a block anchor.
    Observed,
    /// Confirmed, but below depth or observer quorum.
    Confirmed,
    /// Confirmation and member quorums are satisfied.
    Spendable,
    /// Reserved by an accepted transaction proposal (used by later milestones).
    Reserved,
    /// Spent by an accepted transaction (used by later milestones).
    Spent,
    /// The accepted block anchor disconnected.
    Reorged,
    /// Conflicting immutable evidence requires operator review.
    Quarantined,
}

impl core::fmt::Display for FederationWalletUtxoState {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter.write_str(match self {
            Self::Observed => "observed",
            Self::Confirmed => "confirmed",
            Self::Spendable => "spendable",
            Self::Reserved => "reserved",
            Self::Spent => "spent",
            Self::Reorged => "reorged",
            Self::Quarantined => "quarantined",
        })
    }
}

/// Durable journal provenance for one member observation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FederationChainObservationEvidence {
    /// Accepted observation payload.
    pub observation: FederationChainObservation,
    /// Accepted operation identifier.
    pub operation_id: FederationOperationId,
    /// Accepted journal index.
    pub operation_index: u64,
}

/// Canonical materialized record for one wallet UTXO.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FederationWalletUtxo {
    /// Output identifier.
    pub outpoint: OutPoint,
    /// Accepted amount and allocated script.
    pub txout: TxOut,
    /// Accepted address allocation.
    pub allocation: FederationAllocatedAddressRef,
    /// First non-conflicting block anchor.
    pub block: Option<(BlockHash, u32)>,
    /// Current conservative lifecycle state.
    pub state: FederationWalletUtxoState,
    /// Member count required for spendability.
    pub required_observers: u16,
    /// Confirmation depth required for spendability.
    pub minimum_confirmations: u32,
    /// Latest non-conflicting deposit evidence from each member.
    pub observations: BTreeMap<MemberId, FederationChainObservationEvidence>,
    /// Reorg evidence by member.
    pub reorg_observations: BTreeMap<MemberId, FederationChainObservationEvidence>,
    /// Members that supplied conflicting immutable evidence.
    pub conflicting_observers: BTreeSet<MemberId>,
    /// Whether Cashu liability has been associated with this UTXO.
    pub liability_issued: bool,
    /// Sticky alarm when unsafe evidence arrives after liability issuance.
    pub liability_alarm: bool,
}

impl FederationWalletUtxo {
    /// Number of distinct members reporting sufficient confirmations.
    #[must_use]
    pub fn matching_confirmed_observers(&self) -> u16 {
        self.observations
            .values()
            .filter(|evidence| match &evidence.observation {
                FederationChainObservation::Deposit(observation) => {
                    observation.confirmation.is_some_and(|confirmation| {
                        confirmation.confirmation_depth >= self.minimum_confirmations
                            && self.block
                                == Some((confirmation.block_hash, confirmation.block_height))
                    })
                }
                FederationChainObservation::Reorg(_) => false,
            })
            .count()
            .try_into()
            .unwrap_or(u16::MAX)
    }

    /// Distinct diagnostic source identities represented in accepted evidence.
    #[must_use]
    pub fn source_summary(&self) -> FederationChainSourceSummary {
        let sources = self
            .observations
            .values()
            .filter_map(|evidence| match &evidence.observation {
                FederationChainObservation::Deposit(observation) => {
                    Some(observation.source.clone())
                }
                FederationChainObservation::Reorg(_) => None,
            })
            .collect::<BTreeSet<_>>();
        FederationChainSourceSummary {
            independent_sources: sources
                .iter()
                .filter(|source| source.scope == FederationChainSourceScope::Independent)
                .count()
                .try_into()
                .unwrap_or(u16::MAX),
            shared_sources: sources
                .iter()
                .filter(|source| source.scope == FederationChainSourceScope::Shared)
                .count()
                .try_into()
                .unwrap_or(u16::MAX),
        }
    }
}

/// Redacted source-independence diagnostics for an accepted UTXO.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FederationChainSourceSummary {
    /// Distinct independently operated source IDs.
    pub independent_sources: u16,
    /// Distinct explicitly shared source IDs.
    pub shared_sources: u16,
}

/// Exact accepted wallet outputs that currently fund one on-chain mint quote.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FederationOnchainQuotePayment {
    /// Cumulative amount credited to the quote in satoshis.
    pub amount_sat: u64,
    /// Canonically ordered outputs contributing to the cumulative amount.
    pub outpoints: Vec<OutPoint>,
    /// Domain-separated commitment used as the normal payment identifier.
    pub payment_id: [u8; 32],
}

/// Result of applying one accepted chain observation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FederationChainObservationOutcome {
    /// State or evidence changed.
    Updated(FederationWalletUtxoState),
    /// Replayed or stale evidence did not change state.
    Existing(FederationWalletUtxoState),
    /// Conflicting evidence moved the UTXO into quarantine.
    Quarantined,
}

/// Complete versioned consensus projection for wallet UTXOs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FederationChainObservationState {
    version: u16,
    utxos: BTreeMap<String, FederationWalletUtxo>,
}

impl Default for FederationChainObservationState {
    fn default() -> Self {
        Self::new()
    }
}

impl FederationChainObservationState {
    /// Construct an empty current-version state.
    #[must_use]
    pub fn new() -> Self {
        Self {
            version: CHAIN_OBSERVATION_STATE_VERSION,
            utxos: BTreeMap::new(),
        }
    }

    /// Decode and fully validate durable state.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self> {
        let state: Self = serde_json::from_slice(bytes)?;
        state.validate()?;
        Ok(state)
    }

    /// Encode fully validated durable state.
    pub fn to_bytes(&self) -> Result<Vec<u8>> {
        self.validate()?;
        Ok(serde_json::to_vec(self)?)
    }

    /// Look up one materialized UTXO.
    #[must_use]
    pub fn utxo(&self, outpoint: OutPoint) -> Option<&FederationWalletUtxo> {
        self.utxos.get(&outpoint.to_string())
    }

    /// Iterate over materialized UTXOs in canonical outpoint order.
    pub fn utxos(&self) -> impl Iterator<Item = &FederationWalletUtxo> {
        self.utxos.values()
    }

    /// Return the canonical cumulative payment for a quote at a journal boundary.
    ///
    /// Only outputs whose accepted evidence already satisfies confirmation and
    /// member-observation policy are included. Reorged or quarantined outputs
    /// fail closed and do not contribute.
    pub fn quote_payment_at_or_before(
        &self,
        quote: &str,
        max_operation_index: u64,
    ) -> Result<Option<FederationOnchainQuotePayment>> {
        if quote.is_empty() || quote.len() > MAX_QUOTE_ID_BYTES {
            return Err(invalid_state("on-chain quote payment key is invalid"));
        }
        self.validate()?;
        if self
            .utxos
            .values()
            .any(|utxo| utxo.allocation.quote == quote && utxo.liability_alarm)
        {
            return Err(invalid_state(
                "on-chain quote payment is halted by a liability alarm",
            ));
        }

        let mut amount_sat = 0_u64;
        let mut outpoints = Vec::new();
        let mut engine = sha256::Hash::engine();
        engine.input(ONCHAIN_QUOTE_PAYMENT_DOMAIN);
        engine.input(&(quote.len() as u64).to_be_bytes());
        engine.input(quote.as_bytes());

        for utxo in self.utxos.values().filter(|utxo| {
            utxo.allocation.quote == quote
                && utxo_is_spendable_at_or_before(utxo, max_operation_index)
        }) {
            amount_sat = amount_sat
                .checked_add(utxo.txout.value.to_sat())
                .ok_or_else(|| invalid_state("on-chain quote payment amount overflow"))?;
            outpoints.push(utxo.outpoint);
            engine.input(&utxo.outpoint.txid.to_byte_array());
            engine.input(&utxo.outpoint.vout.to_be_bytes());
            engine.input(&utxo.txout.value.to_sat().to_be_bytes());
            engine.input(&(utxo.txout.script_pubkey.len() as u64).to_be_bytes());
            engine.input(utxo.txout.script_pubkey.as_bytes());
        }

        if outpoints.is_empty() {
            return Ok(None);
        }

        Ok(Some(FederationOnchainQuotePayment {
            amount_sat,
            outpoints,
            payment_id: sha256::Hash::from_engine(engine).to_byte_array(),
        }))
    }

    /// Atomically associate accepted Cashu liability with an exact quote payment.
    ///
    /// The expected amount and commitment prevent a stale or synthetic payment
    /// observation from marking a different set of wallet outputs.
    pub fn mark_quote_liability_issued_at_or_before(
        &mut self,
        quote: &str,
        max_operation_index: u64,
        expected_amount_sat: u64,
        expected_payment_id: [u8; 32],
    ) -> Result<Vec<OutPoint>> {
        let payment = self
            .quote_payment_at_or_before(quote, max_operation_index)?
            .ok_or_else(|| invalid_state("liability has no accepted on-chain quote payment"))?;
        if payment.amount_sat != expected_amount_sat || payment.payment_id != expected_payment_id {
            return Err(invalid_state(
                "liability does not match the accepted on-chain quote payment",
            ));
        }
        for outpoint in &payment.outpoints {
            self.mark_liability_issued(*outpoint)?;
        }
        Ok(payment.outpoints)
    }

    /// Apply one accepted deposit observation after allocation/script validation.
    #[allow(clippy::too_many_arguments)]
    pub fn apply_deposit(
        &mut self,
        observation: &FederationDepositObservation,
        allocation: &FederationAddressAllocation,
        expected_script: &Script,
        required_observers: u16,
        minimum_confirmations: u32,
        operation_id: FederationOperationId,
        operation_index: u64,
    ) -> Result<FederationChainObservationOutcome> {
        observation.validate()?;
        observation.allocation.validate_allocation(allocation)?;
        if observation.txout.script_pubkey.as_script() != expected_script {
            return Err(invalid_observation(
                "deposit script does not match the accepted descriptor allocation",
            ));
        }
        if required_observers == 0 || minimum_confirmations == 0 {
            return Err(invalid_state("deposit quorum policy is invalid"));
        }
        self.validate()?;

        let key = observation.outpoint.to_string();
        let evidence = FederationChainObservationEvidence {
            observation: FederationChainObservation::Deposit(observation.clone()),
            operation_id,
            operation_index,
        };
        let utxo = self
            .utxos
            .entry(key)
            .or_insert_with(|| FederationWalletUtxo {
                outpoint: observation.outpoint,
                txout: observation.txout.clone(),
                allocation: observation.allocation.clone(),
                block: observation
                    .confirmation
                    .map(|confirmation| (confirmation.block_hash, confirmation.block_height)),
                state: if observation.confirmation.is_some() {
                    FederationWalletUtxoState::Confirmed
                } else {
                    FederationWalletUtxoState::Observed
                },
                required_observers,
                minimum_confirmations,
                observations: BTreeMap::new(),
                reorg_observations: BTreeMap::new(),
                conflicting_observers: BTreeSet::new(),
                liability_issued: false,
                liability_alarm: false,
            });

        if utxo.required_observers != required_observers
            || utxo.minimum_confirmations != minimum_confirmations
            || utxo.outpoint != observation.outpoint
            || utxo.txout != observation.txout
            || utxo.allocation != observation.allocation
        {
            quarantine(utxo, observation.observer);
            self.validate()?;
            return Ok(FederationChainObservationOutcome::Quarantined);
        }

        if let Some(confirmation) = observation.confirmation {
            match utxo.block {
                Some(block) if block != (confirmation.block_hash, confirmation.block_height) => {
                    quarantine(utxo, observation.observer);
                    self.validate()?;
                    return Ok(FederationChainObservationOutcome::Quarantined);
                }
                None => {
                    utxo.block = Some((confirmation.block_hash, confirmation.block_height));
                }
                Some(_) => {}
            }
        }

        if let Some(existing) = utxo.observations.get(&observation.observer) {
            let FederationChainObservation::Deposit(existing_observation) = &existing.observation
            else {
                return Err(invalid_state("deposit evidence contains a reorg payload"));
            };
            if !observation.same_output(existing_observation) {
                quarantine(utxo, observation.observer);
                self.validate()?;
                return Ok(FederationChainObservationOutcome::Quarantined);
            }
            if confirmation_depth(observation) <= confirmation_depth(existing_observation) {
                return Ok(FederationChainObservationOutcome::Existing(utxo.state));
            }
        }
        utxo.observations.insert(observation.observer, evidence);

        if !matches!(
            utxo.state,
            FederationWalletUtxoState::Reorged
                | FederationWalletUtxoState::Quarantined
                | FederationWalletUtxoState::Reserved
                | FederationWalletUtxoState::Spent
        ) {
            utxo.state = if utxo.matching_confirmed_observers() >= required_observers {
                FederationWalletUtxoState::Spendable
            } else if utxo.block.is_some() {
                FederationWalletUtxoState::Confirmed
            } else {
                FederationWalletUtxoState::Observed
            };
        }
        let outcome_state = utxo.state;
        self.validate()?;
        Ok(FederationChainObservationOutcome::Updated(outcome_state))
    }

    /// Apply one accepted reorg observation and fail closed immediately.
    pub fn apply_reorg(
        &mut self,
        observation: &FederationDepositReorgObservation,
        operation_id: FederationOperationId,
        operation_index: u64,
    ) -> Result<FederationChainObservationOutcome> {
        observation.validate()?;
        self.validate()?;
        let utxo = self
            .utxos
            .get_mut(&observation.outpoint.to_string())
            .ok_or_else(|| invalid_observation("reorg references an unknown wallet outpoint"))?;
        if utxo.block
            != Some((
                observation.disconnected_block_hash,
                observation.disconnected_block_height,
            ))
        {
            quarantine(utxo, observation.observer);
            self.validate()?;
            return Ok(FederationChainObservationOutcome::Quarantined);
        }
        if utxo.reorg_observations.contains_key(&observation.observer) {
            return Ok(FederationChainObservationOutcome::Existing(utxo.state));
        }
        utxo.reorg_observations.insert(
            observation.observer,
            FederationChainObservationEvidence {
                observation: FederationChainObservation::Reorg(observation.clone()),
                operation_id,
                operation_index,
            },
        );
        utxo.state = FederationWalletUtxoState::Reorged;
        utxo.liability_alarm |= utxo.liability_issued;
        let outcome_state = utxo.state;
        self.validate()?;
        Ok(FederationChainObservationOutcome::Updated(outcome_state))
    }

    /// Mark that later accepted mint logic associated Cashu liability with an output.
    ///
    /// Milestone 9 calls this only from its accepted mint-payment projection.
    pub fn mark_liability_issued(&mut self, outpoint: OutPoint) -> Result<()> {
        let utxo = self
            .utxos
            .get_mut(&outpoint.to_string())
            .ok_or_else(|| invalid_state("liability references an unknown wallet outpoint"))?;
        if !matches!(
            utxo.state,
            FederationWalletUtxoState::Spendable
                | FederationWalletUtxoState::Reserved
                | FederationWalletUtxoState::Spent
        ) {
            return Err(invalid_state(
                "liability cannot be issued from a non-spendable wallet output",
            ));
        }
        utxo.liability_issued = true;
        self.validate()
    }

    /// Atomically move an exact set of spendable UTXOs into sticky reservation.
    ///
    /// Reservation ownership is tracked by the separate transaction projection.
    /// This state only records that no later proposal may select the output.
    pub fn reserve_utxos(&mut self, outpoints: &[OutPoint]) -> Result<()> {
        if outpoints.is_empty() {
            return Err(invalid_state("wallet reservation set is empty"));
        }
        let unique = outpoints.iter().copied().collect::<BTreeSet<_>>();
        if unique.len() != outpoints.len() {
            return Err(invalid_state("wallet reservation set contains duplicates"));
        }
        for outpoint in outpoints {
            let utxo = self
                .utxos
                .get(&outpoint.to_string())
                .ok_or_else(|| invalid_state("reservation references an unknown wallet UTXO"))?;
            if utxo.state != FederationWalletUtxoState::Spendable || utxo.liability_alarm {
                return Err(invalid_state(
                    "reservation references a non-spendable wallet UTXO",
                ));
            }
        }
        for outpoint in outpoints {
            let utxo = self
                .utxos
                .get_mut(&outpoint.to_string())
                .ok_or_else(|| invalid_state("reservation wallet UTXO disappeared"))?;
            utxo.state = FederationWalletUtxoState::Reserved;
        }
        self.validate()
    }

    /// Build stable state-digest rows for UTXOs and all accepted evidence.
    pub fn materialized_rows(&self) -> Result<Vec<FederationMaterializedStateRow>> {
        self.validate()?;
        let mut rows = Vec::new();
        for utxo in self.utxos.values() {
            let key = utxo.outpoint.to_string();
            let (block_hash, block_height) = match utxo.block {
                Some((hash, height)) => (hash.to_string(), height.to_string()),
                None => (String::new(), String::new()),
            };
            rows.push(
                FederationMaterializedStateRow::new("federation_wallet_utxos", key.clone())
                    .with_field("value_sat", utxo.txout.value.to_sat().to_string())
                    .with_field("script", utxo.txout.script_pubkey.to_hex_string())
                    .with_field("allocation_quote", utxo.allocation.quote.clone())
                    .with_field("keychain", utxo.allocation.keychain.to_string())
                    .with_field(
                        "derivation_index",
                        utxo.allocation.derivation_index.to_string(),
                    )
                    .with_field("block_hash", block_hash)
                    .with_field("block_height", block_height)
                    .with_field("state", utxo.state.to_string())
                    .with_field("required_observers", utxo.required_observers.to_string())
                    .with_field(
                        "minimum_confirmations",
                        utxo.minimum_confirmations.to_string(),
                    )
                    .with_field("liability_issued", utxo.liability_issued.to_string())
                    .with_field("liability_alarm", utxo.liability_alarm.to_string()),
            );
            for (observer, evidence) in &utxo.observations {
                let FederationChainObservation::Deposit(observation) = &evidence.observation else {
                    return Err(invalid_state("deposit evidence contains a reorg payload"));
                };
                let (hash, height, depth) = match observation.confirmation {
                    Some(confirmation) => (
                        confirmation.block_hash.to_string(),
                        confirmation.block_height.to_string(),
                        confirmation.confirmation_depth.to_string(),
                    ),
                    None => (String::new(), String::new(), "0".to_string()),
                };
                rows.push(
                    FederationMaterializedStateRow::new(
                        "federation_wallet_deposit_evidence",
                        format!("{key}:{observer}"),
                    )
                    .with_field("block_hash", hash)
                    .with_field("block_height", height)
                    .with_field("confirmation_depth", depth)
                    .with_field("source_scope", observation.source.scope.to_string())
                    .with_field("source_id", observation.source.source_id.clone())
                    .with_field("operation_id", evidence.operation_id.to_string())
                    .with_field("operation_index", evidence.operation_index.to_string()),
                );
            }
            for (observer, evidence) in &utxo.reorg_observations {
                rows.push(
                    FederationMaterializedStateRow::new(
                        "federation_wallet_reorg_evidence",
                        format!("{key}:{observer}"),
                    )
                    .with_field("operation_id", evidence.operation_id.to_string())
                    .with_field("operation_index", evidence.operation_index.to_string()),
                );
            }
        }
        Ok(rows)
    }

    /// Validate all durable invariants and fail closed on corrupted state.
    pub fn validate(&self) -> Result<()> {
        if self.version != CHAIN_OBSERVATION_STATE_VERSION {
            return Err(invalid_state("unsupported chain-observation state version"));
        }
        for (key, utxo) in &self.utxos {
            if key != &utxo.outpoint.to_string()
                || utxo.required_observers == 0
                || utxo.minimum_confirmations == 0
                || utxo.txout.value.to_sat() == 0
                || utxo.txout.script_pubkey.is_empty()
            {
                return Err(invalid_state("wallet UTXO identity or policy is invalid"));
            }
            for (observer, evidence) in &utxo.observations {
                let FederationChainObservation::Deposit(observation) = &evidence.observation else {
                    return Err(invalid_state("deposit evidence contains a reorg payload"));
                };
                observation.validate()?;
                if observer != &observation.observer
                    || observation.outpoint != utxo.outpoint
                    || observation.allocation != utxo.allocation
                    || observation.txout != utxo.txout
                {
                    return Err(invalid_state("wallet UTXO evidence identity mismatch"));
                }
                if let Some(confirmation) = observation.confirmation {
                    if utxo.block != Some((confirmation.block_hash, confirmation.block_height)) {
                        return Err(invalid_state("wallet UTXO block evidence mismatch"));
                    }
                }
            }
            for (observer, evidence) in &utxo.reorg_observations {
                let FederationChainObservation::Reorg(observation) = &evidence.observation else {
                    return Err(invalid_state("reorg evidence contains a deposit payload"));
                };
                if observer != &observation.observer || observation.outpoint != utxo.outpoint {
                    return Err(invalid_state("wallet reorg evidence identity mismatch"));
                }
            }
            if utxo.state == FederationWalletUtxoState::Spendable
                && utxo.matching_confirmed_observers() < utxo.required_observers
            {
                return Err(invalid_state(
                    "spendable UTXO does not have observation quorum",
                ));
            }
            if utxo.state == FederationWalletUtxoState::Reorged
                && utxo.reorg_observations.is_empty()
            {
                return Err(invalid_state("reorged UTXO has no reorg evidence"));
            }
            if utxo.state == FederationWalletUtxoState::Quarantined
                && utxo.conflicting_observers.is_empty()
            {
                return Err(invalid_state(
                    "quarantined UTXO has no conflicting observer",
                ));
            }
            if utxo.liability_alarm && !utxo.liability_issued {
                return Err(invalid_state("liability alarm has no issued liability"));
            }
        }
        Ok(())
    }
}

fn confirmation_depth(observation: &FederationDepositObservation) -> u32 {
    observation
        .confirmation
        .map_or(0, |confirmation| confirmation.confirmation_depth)
}

fn utxo_is_spendable_at_or_before(utxo: &FederationWalletUtxo, max_operation_index: u64) -> bool {
    if utxo.state == FederationWalletUtxoState::Quarantined
        || utxo
            .reorg_observations
            .values()
            .any(|evidence| evidence.operation_index <= max_operation_index)
    {
        return false;
    }

    utxo.observations
        .values()
        .filter(|evidence| {
            evidence.operation_index <= max_operation_index
                && matches!(
                    &evidence.observation,
                    FederationChainObservation::Deposit(observation)
                        if observation.confirmation.is_some_and(|confirmation| {
                            confirmation.confirmation_depth >= utxo.minimum_confirmations
                                && utxo.block
                                    == Some((confirmation.block_hash, confirmation.block_height))
                        })
                )
        })
        .count()
        >= usize::from(utxo.required_observers)
}

fn quarantine(utxo: &mut FederationWalletUtxo, observer: MemberId) {
    utxo.conflicting_observers.insert(observer);
    utxo.state = FederationWalletUtxoState::Quarantined;
    utxo.liability_alarm |= utxo.liability_issued;
}

fn invalid_observation(reason: &str) -> FederationError {
    FederationError::ChainObservationInvalid {
        reason: reason.to_string(),
    }
}

fn invalid_state(reason: &str) -> FederationError {
    FederationError::ChainObservationStateInvalid {
        reason: reason.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use bitcoin::hashes::Hash;
    use bitcoin::{Amount, ScriptBuf, Txid};

    use super::*;

    fn member(id: u16) -> MemberId {
        MemberId::new(id).expect("member")
    }

    fn outpoint(seed: u8) -> OutPoint {
        OutPoint::new(Txid::from_byte_array([seed; 32]), 0)
    }

    fn block(seed: u8, height: u32, depth: u32) -> FederationDepositConfirmation {
        FederationDepositConfirmation {
            block_hash: BlockHash::from_byte_array([seed; 32]),
            block_height: height,
            confirmation_depth: depth,
        }
    }

    fn allocation() -> (FederationAddressAllocation, FederationAllocatedAddressRef) {
        let application_id =
            FederationFrostApplicationId::from_bytes([7; 32]).expect("application");
        let allocation = FederationAddressAllocation {
            quote: "quote-1".to_string(),
            keychain: FederationAddressKeychain::External,
            derivation_index: 4,
            operation_id: FederationOperationId::from_bytes([1; 32]),
            operation_index: 1,
        };
        let reference = FederationAllocatedAddressRef {
            application_id,
            application_key_epoch: 2,
            quote: allocation.quote.clone(),
            keychain: allocation.keychain,
            derivation_index: allocation.derivation_index,
        };
        (allocation, reference)
    }

    fn observation(
        observer: u16,
        confirmation: Option<FederationDepositConfirmation>,
        source_scope: FederationChainSourceScope,
    ) -> FederationDepositObservation {
        let (_, allocation) = allocation();
        FederationDepositObservation {
            allocation,
            outpoint: outpoint(3),
            txout: TxOut {
                value: Amount::from_sat(50_000),
                script_pubkey: ScriptBuf::from_bytes(vec![0x51]),
            },
            confirmation,
            observer: member(observer),
            source: FederationChainSourceDescriptor::new(
                source_scope,
                format!("source-{observer}"),
            )
            .expect("source"),
        }
    }

    fn apply(
        state: &mut FederationChainObservationState,
        observation: &FederationDepositObservation,
        operation: u8,
    ) -> FederationChainObservationOutcome {
        let (allocation, _) = allocation();
        state
            .apply_deposit(
                observation,
                &allocation,
                ScriptBuf::from_bytes(vec![0x51]).as_script(),
                2,
                2,
                FederationOperationId::from_bytes([operation; 32]),
                u64::from(operation),
            )
            .expect("apply observation")
    }

    #[test]
    fn confirmation_and_member_quorums_control_spendability() {
        let mut state = FederationChainObservationState::new();
        let unconfirmed = observation(1, None, FederationChainSourceScope::Independent);
        assert_eq!(
            apply(&mut state, &unconfirmed, 2),
            FederationChainObservationOutcome::Updated(FederationWalletUtxoState::Observed)
        );
        let shallow = observation(
            1,
            Some(block(9, 100, 1)),
            FederationChainSourceScope::Independent,
        );
        apply(&mut state, &shallow, 3);
        assert_eq!(
            state.utxo(outpoint(3)).expect("utxo").state,
            FederationWalletUtxoState::Confirmed
        );
        let first_deep = observation(
            1,
            Some(block(9, 100, 2)),
            FederationChainSourceScope::Independent,
        );
        apply(&mut state, &first_deep, 4);
        assert_eq!(
            state.utxo(outpoint(3)).expect("utxo").state,
            FederationWalletUtxoState::Confirmed,
            "one member cannot make the output spendable"
        );
        let second_deep = observation(
            2,
            Some(block(9, 100, 3)),
            FederationChainSourceScope::Independent,
        );
        apply(&mut state, &second_deep, 5);
        let utxo = state.utxo(outpoint(3)).expect("utxo");
        assert_eq!(utxo.state, FederationWalletUtxoState::Spendable);
        assert_eq!(utxo.matching_confirmed_observers(), 2);
        assert_eq!(utxo.source_summary().independent_sources, 2);
    }

    #[test]
    fn quote_payment_is_cumulative_exact_and_bound_to_accepted_outputs() {
        let mut state = FederationChainObservationState::new();
        for (operation, observer) in [(2, 1), (3, 2)] {
            let evidence = observation(
                observer,
                Some(block(9, 100, 2)),
                FederationChainSourceScope::Independent,
            );
            apply(&mut state, &evidence, operation);
        }
        assert!(state
            .quote_payment_at_or_before("quote-1", 2)
            .expect("historical payment")
            .is_none());
        let first = state
            .quote_payment_at_or_before("quote-1", 3)
            .expect("first payment")
            .expect("spendable payment");
        assert_eq!(first.amount_sat, 50_000);
        assert_eq!(first.outpoints, vec![outpoint(3)]);
        assert_eq!(
            state
                .quote_payment_at_or_before("quote-1", u64::MAX)
                .expect("replayed payment")
                .expect("payment"),
            first,
            "duplicate scans cannot add a second payment"
        );

        let (accepted_allocation, _) = allocation();
        for (operation, observer) in [(4, 1), (5, 2)] {
            let mut evidence = observation(
                observer,
                Some(block(10, 101, 2)),
                FederationChainSourceScope::Independent,
            );
            evidence.outpoint = outpoint(4);
            evidence.txout.value = Amount::from_sat(20_000);
            state
                .apply_deposit(
                    &evidence,
                    &accepted_allocation,
                    ScriptBuf::from_bytes(vec![0x51]).as_script(),
                    2,
                    2,
                    FederationOperationId::from_bytes([operation; 32]),
                    u64::from(operation),
                )
                .expect("second output");
        }
        assert_eq!(
            state
                .quote_payment_at_or_before("quote-1", 4)
                .expect("delayed observer")
                .expect("first output remains")
                .amount_sat,
            50_000
        );
        let cumulative = state
            .quote_payment_at_or_before("quote-1", 5)
            .expect("cumulative payment")
            .expect("payment");
        assert_eq!(cumulative.amount_sat, 70_000);
        assert_eq!(cumulative.outpoints.len(), 2);
        assert!(state
            .mark_quote_liability_issued_at_or_before("quote-1", 5, 69_999, cumulative.payment_id,)
            .is_err());
        state
            .mark_quote_liability_issued_at_or_before(
                "quote-1",
                5,
                cumulative.amount_sat,
                cumulative.payment_id,
            )
            .expect("exact liability");
        assert!(cumulative.outpoints.iter().all(|outpoint| state
            .utxo(*outpoint)
            .expect("liability output")
            .liability_issued));

        let reorg = FederationDepositReorgObservation {
            outpoint: outpoint(4),
            disconnected_block_hash: BlockHash::from_byte_array([10; 32]),
            disconnected_block_height: 101,
            observer: member(1),
            source: FederationChainSourceDescriptor::new(
                FederationChainSourceScope::Independent,
                "source-1",
            )
            .expect("source"),
        };
        state
            .apply_reorg(&reorg, FederationOperationId::from_bytes([6; 32]), 6)
            .expect("reorg");
        assert!(state
            .quote_payment_at_or_before("quote-1", u64::MAX)
            .is_err());
        assert!(
            state
                .utxo(outpoint(4))
                .expect("reorged output")
                .liability_alarm
        );
    }

    #[test]
    fn duplicate_stale_and_reordered_observations_are_idempotent() {
        let mut state = FederationChainObservationState::new();
        let deep = observation(
            1,
            Some(block(9, 100, 4)),
            FederationChainSourceScope::Independent,
        );
        apply(&mut state, &deep, 2);
        let bytes = state.to_bytes().expect("state bytes");
        assert!(matches!(
            apply(&mut state, &deep, 3),
            FederationChainObservationOutcome::Existing(_)
        ));
        let stale = observation(
            1,
            Some(block(9, 100, 2)),
            FederationChainSourceScope::Independent,
        );
        assert!(matches!(
            apply(&mut state, &stale, 4),
            FederationChainObservationOutcome::Existing(_)
        ));
        assert_eq!(state.to_bytes().expect("stable bytes"), bytes);
    }

    #[test]
    fn conflicting_amount_script_block_and_height_quarantine() {
        for conflict in ["amount", "script", "block", "height"] {
            let mut state = FederationChainObservationState::new();
            let first = observation(
                1,
                Some(block(9, 100, 2)),
                FederationChainSourceScope::Independent,
            );
            apply(&mut state, &first, 2);
            let mut second = observation(
                2,
                Some(block(9, 100, 2)),
                FederationChainSourceScope::Independent,
            );
            match conflict {
                "amount" => second.txout.value = Amount::from_sat(49_999),
                "script" => second.txout.script_pubkey = ScriptBuf::from_bytes(vec![0x52]),
                "block" => second.confirmation = Some(block(8, 100, 2)),
                "height" => second.confirmation = Some(block(9, 101, 2)),
                _ => unreachable!(),
            }
            let (allocation, _) = allocation();
            let outcome = state
                .apply_deposit(
                    &second,
                    &allocation,
                    second.txout.script_pubkey.as_script(),
                    2,
                    2,
                    FederationOperationId::from_bytes([3; 32]),
                    3,
                )
                .expect("conflict materializes");
            assert_eq!(outcome, FederationChainObservationOutcome::Quarantined);
            assert_eq!(
                state.utxo(outpoint(3)).expect("utxo").state,
                FederationWalletUtxoState::Quarantined
            );
        }
    }

    #[test]
    fn reorg_is_conservative_and_raises_post_liability_alarm() {
        let unknown_reorg = FederationDepositReorgObservation {
            outpoint: outpoint(99),
            disconnected_block_hash: BlockHash::from_byte_array([9; 32]),
            disconnected_block_height: 100,
            observer: member(1),
            source: FederationChainSourceDescriptor::new(
                FederationChainSourceScope::Independent,
                "unknown-source",
            )
            .expect("source"),
        };
        assert!(FederationChainObservationState::new()
            .apply_reorg(
                &unknown_reorg,
                FederationOperationId::from_bytes([1; 32]),
                1,
            )
            .is_err());

        for liability in [false, true] {
            let mut state = FederationChainObservationState::new();
            for observer in 1..=2 {
                let evidence = observation(
                    observer,
                    Some(block(9, 100, 2)),
                    FederationChainSourceScope::Shared,
                );
                apply(&mut state, &evidence, observer as u8 + 1);
            }
            if liability {
                state
                    .mark_liability_issued(outpoint(3))
                    .expect("liability issued");
            }
            let reorg = FederationDepositReorgObservation {
                outpoint: outpoint(3),
                disconnected_block_hash: BlockHash::from_byte_array([9; 32]),
                disconnected_block_height: 100,
                observer: member(1),
                source: FederationChainSourceDescriptor::new(
                    FederationChainSourceScope::Shared,
                    "shared-source",
                )
                .expect("source"),
            };
            state
                .apply_reorg(&reorg, FederationOperationId::from_bytes([8; 32]), 8)
                .expect("reorg");
            let utxo = state.utxo(outpoint(3)).expect("utxo");
            assert_eq!(utxo.state, FederationWalletUtxoState::Reorged);
            assert_eq!(utxo.liability_alarm, liability);
        }
    }

    #[test]
    fn serialization_digest_rows_and_shared_source_diagnostics_are_stable() {
        let mut state = FederationChainObservationState::new();
        for observer in 1..=2 {
            let mut evidence = observation(
                observer,
                Some(block(9, 100, 2)),
                FederationChainSourceScope::Shared,
            );
            evidence.source.source_id = "shared-source".to_string();
            apply(&mut state, &evidence, observer as u8 + 1);
        }
        let decoded =
            FederationChainObservationState::from_bytes(&state.to_bytes().expect("state bytes"))
                .expect("state decode");
        assert_eq!(decoded, state);
        assert_eq!(
            decoded.utxo(outpoint(3)).expect("utxo").source_summary(),
            FederationChainSourceSummary {
                independent_sources: 0,
                shared_sources: 1,
            }
        );
        let rows = state.materialized_rows().expect("rows");
        let mut changed = state.clone();
        let evidence = changed
            .utxos
            .get_mut(&outpoint(3).to_string())
            .expect("utxo")
            .observations
            .get_mut(&member(1))
            .expect("evidence");
        let FederationChainObservation::Deposit(observation) = &mut evidence.observation else {
            panic!("deposit evidence");
        };
        observation.source.source_id = "other-shared-source".to_string();
        assert_ne!(
            rows,
            changed.materialized_rows().expect("changed rows"),
            "state digest inputs cover policy and evidence"
        );
    }
}
