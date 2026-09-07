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
    /// Consensus observation epoch for this output.
    pub observation_epoch: u64,
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
    /// Consensus observation epoch invalidated by this reorg evidence.
    pub observation_epoch: u64,
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
    /// Current consensus observation epoch for this output.
    pub observation_epoch: u64,
    /// Latest non-conflicting deposit evidence from each member.
    pub observations: BTreeMap<MemberId, FederationChainObservationEvidence>,
    /// Latest reorg evidence from each member.
    pub reorg_observations: BTreeMap<MemberId, FederationChainObservationEvidence>,
    /// Members that supplied conflicting immutable evidence.
    pub conflicting_observers: BTreeSet<MemberId>,
    /// Whether Cashu liability has been associated with this UTXO.
    pub liability_issued: bool,
    /// Sticky alarm when unsafe evidence arrives after liability issuance.
    pub liability_alarm: bool,
}

/// Immutable UTXO field set that member observers vote on with their
/// deposit evidence: exact output contents, allocation, and block anchor.
type UtxoFieldSet = (
    TxOut,
    FederationAllocatedAddressRef,
    Option<(BlockHash, u32)>,
);

impl FederationWalletUtxo {
    /// Number of distinct members reporting sufficient confirmations.
    ///
    /// Only evidence matching the established immutable fields counts toward
    /// the spendability quorum: conflicting retained votes are diagnostic
    /// until a quorum-backed set is adopted.
    #[must_use]
    pub fn matching_confirmed_observers(&self) -> u16 {
        self.observations
            .values()
            .filter(|evidence| match &evidence.observation {
                FederationChainObservation::Deposit(observation) => {
                    observation.observation_epoch == self.observation_epoch
                        && observation.txout == self.txout
                        && observation.allocation == self.allocation
                        && observation.confirmation.is_some_and(|confirmation| {
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

    /// Quorum-backed immutable field set among current-epoch observers, when
    /// exactly one distinct set reaches the spendability quorum. Conflicting
    /// evidence stays recorded either way; this only determines whether one
    /// alternative field set is backed by enough members to be adopted over
    /// a minority-fixed one.
    #[must_use]
    fn quorum_backed_field_set(&self) -> Option<UtxoFieldSet> {
        let mut votes: Vec<(UtxoFieldSet, BTreeSet<MemberId>)> = Vec::new();
        for evidence in self.observations.values() {
            let FederationChainObservation::Deposit(observation) = &evidence.observation else {
                continue;
            };
            if observation.observation_epoch != self.observation_epoch {
                continue;
            }
            let block = observation
                .confirmation
                .map(|confirmation| (confirmation.block_hash, confirmation.block_height));
            let fields = (
                observation.txout.clone(),
                observation.allocation.clone(),
                block,
            );
            match votes.iter_mut().find(|(candidate, _)| *candidate == fields) {
                Some((_, observers)) => {
                    observers.insert(observation.observer);
                }
                None => votes.push((fields, BTreeSet::from([observation.observer]))),
            }
        }
        let mut quorum_sets = votes
            .into_iter()
            .filter(|(_, observers)| observers.len() >= usize::from(self.required_observers));
        match (quorum_sets.next(), quorum_sets.next()) {
            (Some(((txout, allocation, block), _)), None) => Some((txout, allocation, block)),
            _ => None,
        }
    }

    /// Distinct diagnostic source identities represented in accepted evidence.
    #[must_use]
    pub fn source_summary(&self) -> FederationChainSourceSummary {
        let sources =
            self.observations
                .values()
                .filter_map(|evidence| match &evidence.observation {
                    FederationChainObservation::Deposit(observation)
                        if observation.observation_epoch == self.observation_epoch =>
                    {
                        Some(observation.source.clone())
                    }
                    FederationChainObservation::Deposit(_)
                    | FederationChainObservation::Reorg(_) => None,
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

    /// Number of distinct members reporting a reorg for the current epoch.
    #[must_use]
    pub fn matching_current_epoch_reorg_observers(&self) -> u16 {
        self.reorg_observations
            .values()
            .filter(|evidence| {
                matches!(
                    &evidence.observation,
                    FederationChainObservation::Reorg(observation)
                        if observation.observation_epoch == self.observation_epoch
                )
            })
            .count()
            .try_into()
            .unwrap_or(u16::MAX)
    }

    /// Return whether accepted reorg evidence invalidates the current epoch.
    ///
    /// One member cannot reorg accepted funds: the current epoch is
    /// invalidated only when the accepted reorg evidence reaches the same
    /// observer quorum that spendability requires.
    #[must_use]
    pub fn has_current_epoch_reorg(&self) -> bool {
        usize::from(self.required_observers)
            <= usize::from(self.matching_current_epoch_reorg_observers())
    }

    /// Return the epoch a new local deposit observation should use.
    ///
    /// A pre-liability reorg requires a new epoch. Once one recovery
    /// observation advances the UTXO, the remaining members join that epoch.
    /// Pre-liability quarantines keep the current epoch so honest observers
    /// keep reporting and can repair the UTXO through the matching quorum.
    pub fn next_deposit_observation_epoch(&self) -> Result<Option<u64>> {
        match self.state {
            FederationWalletUtxoState::Reorged if self.liability_issued => Ok(None),
            FederationWalletUtxoState::Reorged if self.has_current_epoch_reorg() => self
                .observation_epoch
                .checked_add(1)
                .map(Some)
                .ok_or_else(|| invalid_state("wallet observation epoch overflow")),
            FederationWalletUtxoState::Reorged => Ok(Some(self.observation_epoch)),
            FederationWalletUtxoState::Quarantined if self.liability_issued => Ok(None),
            FederationWalletUtxoState::Quarantined => Ok(Some(self.observation_epoch)),
            FederationWalletUtxoState::Reserved | FederationWalletUtxoState::Spent => Ok(None),
            FederationWalletUtxoState::Observed
            | FederationWalletUtxoState::Confirmed
            | FederationWalletUtxoState::Spendable => Ok(Some(self.observation_epoch)),
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
        if !self.utxos.contains_key(&key) && observation.observation_epoch != 0 {
            return Err(invalid_observation(
                "initial deposit observation must use epoch zero",
            ));
        }
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
                observation_epoch: 0,
                observations: BTreeMap::new(),
                reorg_observations: BTreeMap::new(),
                conflicting_observers: BTreeSet::new(),
                liability_issued: false,
                liability_alarm: false,
            });

        if utxo.required_observers != required_observers
            || utxo.minimum_confirmations != minimum_confirmations
            || utxo.outpoint != observation.outpoint
        {
            quarantine(utxo, observation.observer);
            self.validate()?;
            return Ok(FederationChainObservationOutcome::Quarantined);
        }

        if observation.observation_epoch < utxo.observation_epoch {
            return Ok(FederationChainObservationOutcome::Existing(utxo.state));
        }

        let next_epoch = utxo.next_deposit_observation_epoch()?;
        if observation.observation_epoch > utxo.observation_epoch {
            if next_epoch != Some(observation.observation_epoch) {
                return Err(invalid_observation(
                    "deposit observation skips the expected recovery epoch",
                ));
            }
            let sufficiently_confirmed = observation.confirmation.is_some_and(|confirmation| {
                confirmation.confirmation_depth >= utxo.minimum_confirmations
            });
            if !sufficiently_confirmed {
                return Err(invalid_observation(
                    "recovery deposit must satisfy the confirmation policy",
                ));
            }
        } else if utxo.state == FederationWalletUtxoState::Reorged
            && !utxo.liability_issued
            && next_epoch == Some(utxo.observation_epoch)
            && !observation.confirmation.is_some_and(|confirmation| {
                confirmation.confirmation_depth >= utxo.minimum_confirmations
            })
        {
            return Err(invalid_observation(
                "recovery deposit must satisfy the confirmation policy",
            ));
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
            if observation.observation_epoch < existing_observation.observation_epoch
                || (observation.observation_epoch == existing_observation.observation_epoch
                    && confirmation_depth(observation) <= confirmation_depth(existing_observation))
            {
                return Ok(FederationChainObservationOutcome::Existing(utxo.state));
            }
        }

        // Conflicting immutable evidence is retained as a per-observer field
        // vote instead of being discarded: a quorum-backed field set can then
        // be adopted over a minority-fixed one before liability issuance.
        let fields_match =
            utxo.txout == observation.txout && utxo.allocation == observation.allocation;
        let block_conflict = match (utxo.block, observation.confirmation) {
            (Some(block), Some(confirmation)) => {
                block != (confirmation.block_hash, confirmation.block_height)
            }
            _ => false,
        };
        if fields_match && !block_conflict {
            if let Some(confirmation) = observation.confirmation {
                if utxo.block.is_none() {
                    utxo.block = Some((confirmation.block_hash, confirmation.block_height));
                }
            }
        }
        utxo.observations.insert(observation.observer, evidence);
        utxo.observation_epoch = observation.observation_epoch;
        if !fields_match || block_conflict {
            quarantine(utxo, observation.observer);
        }

        // Pre-liability quorum adoption: when exactly one current-epoch
        // field set reaches the spendability quorum and it differs from the
        // established fields, adopt it and recompute the conflict diagnostic
        // so a first liar cannot permanently fix incorrect fields. A
        // confirmed block is never replaced by unconfirmed votes.
        if !utxo.liability_issued {
            if let Some((txout, allocation, block)) = utxo.quorum_backed_field_set() {
                let block_adoption_allowed = block.is_some() || utxo.block.is_none();
                if block_adoption_allowed
                    && (utxo.txout != txout || utxo.allocation != allocation || utxo.block != block)
                {
                    utxo.txout = txout;
                    utxo.allocation = allocation;
                    utxo.block = block;
                    utxo.conflicting_observers = utxo
                        .observations
                        .values()
                        .filter_map(|evidence| match &evidence.observation {
                            FederationChainObservation::Deposit(observation)
                                if observation.observation_epoch == utxo.observation_epoch
                                    && (observation.txout != utxo.txout
                                        || observation.allocation != utxo.allocation
                                        || observation.confirmation.is_some_and(
                                            |confirmation| {
                                                Some((
                                                    confirmation.block_hash,
                                                    confirmation.block_height,
                                                )) != utxo.block
                                            },
                                        )) =>
                            {
                                Some(observation.observer)
                            }
                            _ => None,
                        })
                        .collect();
                }
            }
        }

        // One member cannot permanently quarantine accepted funds: while no
        // liability is issued, a matching observer quorum repairs the UTXO
        // and recomputes its state. Post-liability quarantines remain sticky.
        let repair_quarantine = utxo.state == FederationWalletUtxoState::Quarantined
            && !utxo.liability_issued
            && usize::from(utxo.required_observers)
                <= usize::from(utxo.matching_confirmed_observers());
        if !(matches!(
            utxo.state,
            FederationWalletUtxoState::Quarantined
                | FederationWalletUtxoState::Reserved
                | FederationWalletUtxoState::Spent
        ) || utxo.state == FederationWalletUtxoState::Reorged && utxo.liability_issued)
            || repair_quarantine
        {
            utxo.state = if utxo.has_current_epoch_reorg() {
                FederationWalletUtxoState::Reorged
            } else if utxo.matching_confirmed_observers() >= required_observers {
                FederationWalletUtxoState::Spendable
            } else if utxo.state == FederationWalletUtxoState::Reorged {
                FederationWalletUtxoState::Reorged
            } else if utxo.block.is_some() {
                FederationWalletUtxoState::Confirmed
            } else {
                FederationWalletUtxoState::Observed
            };
        }
        let outcome_state = utxo.state;
        self.validate()?;
        if outcome_state == FederationWalletUtxoState::Quarantined
            && (!fields_match || block_conflict)
        {
            return Ok(FederationChainObservationOutcome::Quarantined);
        }
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
        if observation.observation_epoch < utxo.observation_epoch {
            return Ok(FederationChainObservationOutcome::Existing(utxo.state));
        }
        if observation.observation_epoch > utxo.observation_epoch {
            return Err(invalid_observation(
                "reorg observation references a future observation epoch",
            ));
        }
        if let Some(existing) = utxo.reorg_observations.get(&observation.observer) {
            let FederationChainObservation::Reorg(existing_observation) = &existing.observation
            else {
                return Err(invalid_state("reorg evidence contains a deposit payload"));
            };
            if existing_observation.observation_epoch >= observation.observation_epoch {
                return Ok(FederationChainObservationOutcome::Existing(utxo.state));
            }
        }
        utxo.reorg_observations.insert(
            observation.observer,
            FederationChainObservationEvidence {
                observation: FederationChainObservation::Reorg(observation.clone()),
                operation_id,
                operation_index,
            },
        );
        // One member cannot reorg or alarm accepted funds: the fail-closed
        // transition and the post-liability alarm require the same observer
        // quorum that spendability requires. Minority reorg evidence is
        // retained as diagnostic only.
        if utxo.has_current_epoch_reorg() {
            utxo.state = FederationWalletUtxoState::Reorged;
            utxo.liability_alarm |= utxo.liability_issued;
        }
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

    /// Atomically mark an exact set of sticky reservations as spent.
    ///
    /// Application-specific reservation ownership must be checked by the
    /// caller in the same durable transaction before invoking this neutral
    /// lifecycle transition.
    pub fn spend_reserved_utxos(&mut self, outpoints: &[OutPoint]) -> Result<()> {
        if outpoints.is_empty() {
            return Err(invalid_state("wallet spent reservation set is empty"));
        }
        let unique = outpoints.iter().copied().collect::<BTreeSet<_>>();
        if unique.len() != outpoints.len() {
            return Err(invalid_state(
                "wallet spent reservation set contains duplicates",
            ));
        }
        for outpoint in outpoints {
            let utxo = self
                .utxos
                .get(&outpoint.to_string())
                .ok_or_else(|| invalid_state("spent reservation references an unknown UTXO"))?;
            if utxo.state != FederationWalletUtxoState::Reserved {
                return Err(invalid_state(
                    "spent reservation references a non-reserved wallet UTXO",
                ));
            }
        }
        for outpoint in outpoints {
            self.utxos
                .get_mut(&outpoint.to_string())
                .ok_or_else(|| invalid_state("spent reservation wallet UTXO disappeared"))?
                .state = FederationWalletUtxoState::Spent;
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
            let utxo_row =
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
                    .with_field("liability_alarm", utxo.liability_alarm.to_string())
                    .with_field("observation_epoch", utxo.observation_epoch.to_string());
            rows.push(utxo_row);
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
                let evidence_row = FederationMaterializedStateRow::new(
                    "federation_wallet_deposit_evidence",
                    format!("{key}:{observer}"),
                )
                .with_field("block_hash", hash)
                .with_field("block_height", height)
                .with_field("confirmation_depth", depth)
                .with_field("source_scope", observation.source.scope.to_string())
                .with_field("source_id", observation.source.source_id.clone())
                .with_field("operation_id", evidence.operation_id.to_string())
                .with_field("operation_index", evidence.operation_index.to_string())
                .with_field(
                    "observation_epoch",
                    observation.observation_epoch.to_string(),
                );
                rows.push(evidence_row);
            }
            for (observer, evidence) in &utxo.reorg_observations {
                let FederationChainObservation::Reorg(observation) = &evidence.observation else {
                    return Err(invalid_state("reorg evidence contains a deposit payload"));
                };
                let evidence_row = FederationMaterializedStateRow::new(
                    "federation_wallet_reorg_evidence",
                    format!("{key}:{observer}"),
                )
                .with_field("operation_id", evidence.operation_id.to_string())
                .with_field("operation_index", evidence.operation_index.to_string())
                .with_field(
                    "observation_epoch",
                    observation.observation_epoch.to_string(),
                );
                rows.push(evidence_row);
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
                    || observation.observation_epoch > utxo.observation_epoch
                {
                    return Err(invalid_state("wallet UTXO evidence identity mismatch"));
                }
            }
            // Conflicting per-observer field votes are retained so a
            // quorum-backed set can be adopted; the established fields must
            // still be backed by at least one stored observation.
            let established_fields_backed = utxo.observations.values().any(|evidence| {
                matches!(
                    &evidence.observation,
                    FederationChainObservation::Deposit(observation)
                        if observation.txout == utxo.txout
                            && observation.allocation == utxo.allocation
                )
            });
            if !established_fields_backed {
                return Err(invalid_state("wallet UTXO fields have no backing evidence"));
            }
            for (observer, evidence) in &utxo.reorg_observations {
                let FederationChainObservation::Reorg(observation) = &evidence.observation else {
                    return Err(invalid_state("reorg evidence contains a deposit payload"));
                };
                if observer != &observation.observer
                    || observation.outpoint != utxo.outpoint
                    || observation.observation_epoch > utxo.observation_epoch
                {
                    return Err(invalid_state("wallet reorg evidence identity mismatch"));
                }
            }
            if utxo.state == FederationWalletUtxoState::Spendable
                && (utxo.has_current_epoch_reorg()
                    || utxo.matching_confirmed_observers() < utxo.required_observers)
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
    let current_epoch_reorg_observers = utxo
        .reorg_observations
        .values()
        .filter(|evidence| {
            evidence.operation_index <= max_operation_index
                && matches!(
                    &evidence.observation,
                    FederationChainObservation::Reorg(observation)
                        if observation.observation_epoch == utxo.observation_epoch
                )
        })
        .count();
    if utxo.state == FederationWalletUtxoState::Quarantined
        || current_epoch_reorg_observers >= usize::from(utxo.required_observers)
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
                        if observation.observation_epoch == utxo.observation_epoch
                            && observation.confirmation.is_some_and(|confirmation| {
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
            observation_epoch: 0,
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

    fn apply_with_policy(
        state: &mut FederationChainObservationState,
        observation: &FederationDepositObservation,
        required_observers: u16,
        minimum_confirmations: u32,
        operation: u8,
    ) -> FederationChainObservationOutcome {
        let (allocation, _) = allocation();
        state
            .apply_deposit(
                observation,
                &allocation,
                ScriptBuf::from_bytes(vec![0x51]).as_script(),
                required_observers,
                minimum_confirmations,
                FederationOperationId::from_bytes([operation; 32]),
                u64::from(operation),
            )
            .expect("apply observation with policy")
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

        let reorg = |observer: u16| FederationDepositReorgObservation {
            outpoint: outpoint(4),
            disconnected_block_hash: BlockHash::from_byte_array([10; 32]),
            disconnected_block_height: 101,
            observation_epoch: 0,
            observer: member(observer),
            source: FederationChainSourceDescriptor::new(
                FederationChainSourceScope::Independent,
                format!("source-{observer}"),
            )
            .expect("source"),
        };
        state
            .apply_reorg(&reorg(1), FederationOperationId::from_bytes([6; 32]), 6)
            .expect("minority reorg evidence");
        assert!(
            !state
                .utxo(outpoint(4))
                .expect("output after one reorg")
                .liability_alarm,
            "one observer's reorg evidence must not raise the alarm"
        );
        state
            .apply_reorg(&reorg(2), FederationOperationId::from_bytes([7; 32]), 7)
            .expect("reorg quorum");
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
            observation_epoch: 0,
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
            let first_reorg = FederationDepositReorgObservation {
                outpoint: outpoint(3),
                disconnected_block_hash: BlockHash::from_byte_array([9; 32]),
                disconnected_block_height: 100,
                observation_epoch: 0,
                observer: member(1),
                source: FederationChainSourceDescriptor::new(
                    FederationChainSourceScope::Shared,
                    "shared-source",
                )
                .expect("source"),
            };
            state
                .apply_reorg(&first_reorg, FederationOperationId::from_bytes([8; 32]), 8)
                .expect("first reorg");
            let utxo = state.utxo(outpoint(3)).expect("utxo");
            assert_eq!(
                utxo.state,
                FederationWalletUtxoState::Spendable,
                "one observer's reorg evidence must not transition the UTXO"
            );
            assert!(
                !utxo.liability_alarm,
                "one observer's reorg evidence must not raise the alarm"
            );

            let second_reorg = FederationDepositReorgObservation {
                observer: member(2),
                ..first_reorg
            };
            state
                .apply_reorg(&second_reorg, FederationOperationId::from_bytes([9; 32]), 9)
                .expect("quorum reorg");
            let utxo = state.utxo(outpoint(3)).expect("utxo");
            assert_eq!(utxo.state, FederationWalletUtxoState::Reorged);
            assert_eq!(utxo.liability_alarm, liability);

            if liability {
                let mut reconfirmed = observation(
                    1,
                    Some(block(9, 100, 2)),
                    FederationChainSourceScope::Shared,
                );
                reconfirmed.observation_epoch = 1;
                assert!(state
                    .apply_deposit(
                        &reconfirmed,
                        &allocation().0,
                        ScriptBuf::from_bytes(vec![0x51]).as_script(),
                        2,
                        2,
                        FederationOperationId::from_bytes([10; 32]),
                        10,
                    )
                    .is_err());
                let utxo = state.utxo(outpoint(3)).expect("sticky liability UTXO");
                assert_eq!(utxo.state, FederationWalletUtxoState::Reorged);
                assert!(utxo.liability_alarm);
            }
        }
    }

    #[test]
    fn pre_liability_reorg_requires_a_fresh_epoch_quorum_and_can_repeat() {
        let mut state = FederationChainObservationState::new();
        for observer in 1..=5 {
            let evidence = observation(
                observer,
                Some(block(9, 100, 1)),
                FederationChainSourceScope::Shared,
            );
            apply_with_policy(&mut state, &evidence, 4, 1, observer as u8);
        }
        assert_eq!(
            state.utxo(outpoint(3)).expect("spendable UTXO").state,
            FederationWalletUtxoState::Spendable
        );

        for (operation, observer) in [(6, 1), (7, 3), (8, 5)] {
            let reorg = FederationDepositReorgObservation {
                outpoint: outpoint(3),
                disconnected_block_hash: BlockHash::from_byte_array([9; 32]),
                disconnected_block_height: 100,
                observation_epoch: 0,
                observer: member(observer),
                source: FederationChainSourceDescriptor::new(
                    FederationChainSourceScope::Shared,
                    "shared-source",
                )
                .expect("source"),
            };
            state
                .apply_reorg(
                    &reorg,
                    FederationOperationId::from_bytes([operation; 32]),
                    u64::from(operation),
                )
                .expect("accepted reorg");
        }
        let minority_reorged = state.utxo(outpoint(3)).expect("minority reorg UTXO");
        assert_eq!(
            minority_reorged.state,
            FederationWalletUtxoState::Spendable,
            "three of four required observers must not reorg the UTXO"
        );

        let quorum_reorg = FederationDepositReorgObservation {
            outpoint: outpoint(3),
            disconnected_block_hash: BlockHash::from_byte_array([9; 32]),
            disconnected_block_height: 100,
            observation_epoch: 0,
            observer: member(2),
            source: FederationChainSourceDescriptor::new(
                FederationChainSourceScope::Shared,
                "shared-source",
            )
            .expect("source"),
        };
        state
            .apply_reorg(&quorum_reorg, FederationOperationId::from_bytes([9; 32]), 9)
            .expect("quorum reorg");
        let reorged = state.utxo(outpoint(3)).expect("reorged UTXO");
        assert_eq!(reorged.state, FederationWalletUtxoState::Reorged);
        assert_eq!(
            reorged
                .next_deposit_observation_epoch()
                .expect("recovery epoch"),
            Some(1)
        );

        for (operation, observer) in [(9, 1), (10, 2), (11, 3)] {
            let mut reconfirmed = observation(
                observer,
                Some(block(9, 100, 1)),
                FederationChainSourceScope::Shared,
            );
            reconfirmed.observation_epoch = 1;
            assert_eq!(
                apply_with_policy(&mut state, &reconfirmed, 4, 1, operation),
                FederationChainObservationOutcome::Updated(FederationWalletUtxoState::Reorged)
            );
        }
        let threshold_minus_one = state.utxo(outpoint(3)).expect("recovering UTXO");
        assert_eq!(threshold_minus_one.observation_epoch, 1);
        assert_eq!(threshold_minus_one.matching_confirmed_observers(), 3);
        assert_eq!(threshold_minus_one.reorg_observations.len(), 4);
        assert!(state
            .quote_payment_at_or_before("quote-1", u64::MAX)
            .expect("recovering quote")
            .is_none());

        let mut quorum_reconfirmation = observation(
            4,
            Some(block(9, 100, 1)),
            FederationChainSourceScope::Shared,
        );
        quorum_reconfirmation.observation_epoch = 1;
        assert_eq!(
            apply_with_policy(&mut state, &quorum_reconfirmation, 4, 1, 12),
            FederationChainObservationOutcome::Updated(FederationWalletUtxoState::Spendable)
        );
        let recovered = state.utxo(outpoint(3)).expect("recovered UTXO");
        assert_eq!(recovered.matching_confirmed_observers(), 4);
        assert_eq!(recovered.reorg_observations.len(), 4);
        assert_eq!(
            state
                .quote_payment_at_or_before("quote-1", u64::MAX)
                .expect("recovered quote")
                .expect("recovered payment")
                .amount_sat,
            50_000
        );

        let stale_confirmation = observation(
            5,
            Some(block(9, 100, 1)),
            FederationChainSourceScope::Shared,
        );
        assert_eq!(
            apply_with_policy(&mut state, &stale_confirmation, 4, 1, 13),
            FederationChainObservationOutcome::Existing(FederationWalletUtxoState::Spendable)
        );
        let stale_reorg = FederationDepositReorgObservation {
            outpoint: outpoint(3),
            disconnected_block_hash: BlockHash::from_byte_array([9; 32]),
            disconnected_block_height: 100,
            observation_epoch: 0,
            observer: member(2),
            source: FederationChainSourceDescriptor::new(
                FederationChainSourceScope::Shared,
                "shared-source",
            )
            .expect("source"),
        };
        assert_eq!(
            state
                .apply_reorg(
                    &stale_reorg,
                    FederationOperationId::from_bytes([14; 32]),
                    14,
                )
                .expect("stale reorg"),
            FederationChainObservationOutcome::Existing(FederationWalletUtxoState::Spendable)
        );
        let mut unexpected_future_confirmation = observation(
            1,
            Some(block(9, 100, 1)),
            FederationChainSourceScope::Shared,
        );
        unexpected_future_confirmation.observation_epoch = 2;
        assert!(state
            .apply_deposit(
                &unexpected_future_confirmation,
                &allocation().0,
                ScriptBuf::from_bytes(vec![0x51]).as_script(),
                4,
                1,
                FederationOperationId::from_bytes([15; 32]),
                15,
            )
            .is_err());

        let repeated_reorg = |observer: u16| FederationDepositReorgObservation {
            outpoint: outpoint(3),
            disconnected_block_hash: BlockHash::from_byte_array([9; 32]),
            disconnected_block_height: 100,
            observation_epoch: 1,
            observer: member(observer),
            source: FederationChainSourceDescriptor::new(
                FederationChainSourceScope::Shared,
                "shared-source",
            )
            .expect("source"),
        };
        assert_eq!(
            state
                .apply_reorg(
                    &repeated_reorg(1),
                    FederationOperationId::from_bytes([16; 32]),
                    16,
                )
                .expect("repeated reorg"),
            FederationChainObservationOutcome::Updated(FederationWalletUtxoState::Spendable)
        );
        assert!(
            !state
                .utxo(outpoint(3))
                .expect("one repeated reorg")
                .has_current_epoch_reorg(),
            "one observer's repeated reorg must not invalidate the epoch"
        );
        for (operation, observer) in [(17, 2), (18, 3), (19, 5)] {
            state
                .apply_reorg(
                    &repeated_reorg(observer),
                    FederationOperationId::from_bytes([operation; 32]),
                    u64::from(operation),
                )
                .expect("repeated reorg quorum");
        }
        let reorged_again = state.utxo(outpoint(3)).expect("reorged again");
        assert_eq!(reorged_again.state, FederationWalletUtxoState::Reorged);
        assert!(reorged_again.has_current_epoch_reorg());
        assert_eq!(
            reorged_again
                .next_deposit_observation_epoch()
                .expect("second recovery epoch"),
            Some(2)
        );

        let mut overflow = reorged_again.clone();
        overflow.observation_epoch = u64::MAX;
        for reorg_member in [member(1), member(2), member(3), member(5)] {
            let FederationChainObservation::Reorg(overflow_reorg) = &mut overflow
                .reorg_observations
                .get_mut(&reorg_member)
                .expect("reorg evidence")
                .observation
            else {
                panic!("reorg evidence");
            };
            overflow_reorg.observation_epoch = u64::MAX;
        }
        assert!(overflow.next_deposit_observation_epoch().is_err());
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
        let encoded =
            String::from_utf8(state.to_bytes().expect("current state")).expect("state JSON");
        assert!(encoded.contains("observation_epoch"));
        assert_eq!(decoded, state);
        assert_eq!(
            decoded.utxo(outpoint(3)).expect("utxo").source_summary(),
            FederationChainSourceSummary {
                independent_sources: 0,
                shared_sources: 1,
            }
        );
        let rows = state.materialized_rows().expect("rows");
        assert!(rows
            .iter()
            .all(|row| row.fields.get("observation_epoch").map(String::as_str) == Some("0")));
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

    fn reorg_observation(
        observer: u16,
        observation_epoch: u64,
    ) -> FederationDepositReorgObservation {
        FederationDepositReorgObservation {
            outpoint: outpoint(3),
            disconnected_block_hash: BlockHash::from_byte_array([9; 32]),
            disconnected_block_height: 100,
            observation_epoch,
            observer: member(observer),
            source: FederationChainSourceDescriptor::new(
                FederationChainSourceScope::Shared,
                "shared-source",
            )
            .expect("source"),
        }
    }

    #[test]
    fn single_observer_reorg_does_not_reorg_or_block_credit_without_quorum() {
        // required_observers = 4 of 5 with one confirmation (the nocean profile).
        let mut state = FederationChainObservationState::new();
        for observer in 1..=5 {
            let evidence = observation(
                observer,
                Some(block(9, 100, 1)),
                FederationChainSourceScope::Shared,
            );
            apply_with_policy(&mut state, &evidence, 4, 1, observer as u8);
        }
        assert_eq!(
            state.utxo(outpoint(3)).expect("spendable UTXO").state,
            FederationWalletUtxoState::Spendable
        );
        state
            .mark_liability_issued(outpoint(3))
            .expect("liability issued");

        // One observer (a Byzantine member or one lagging/inconsistent chain
        // backend) reports the recorded confirmation block as disconnected.
        state
            .apply_reorg(
                &reorg_observation(5, 0),
                FederationOperationId::from_bytes([6; 32]),
                6,
            )
            .expect("reorg evidence accepted");

        let utxo = state.utxo(outpoint(3)).expect("UTXO after one reorg");
        assert_eq!(
            utxo.state,
            FederationWalletUtxoState::Spendable,
            "one observer must not reorg a quorum-established UTXO"
        );
        assert!(
            !utxo.liability_alarm,
            "one observer must not raise the liability alarm"
        );
        assert!(
            state
                .quote_payment_at_or_before("quote-1", u64::MAX)
                .expect("credit query")
                .is_some(),
            "one reorg report must not block quote credit"
        );

        // A full observer quorum reporting the same reorg still fails closed.
        for (operation, observer) in [(7, 1), (8, 2), (9, 3)] {
            state
                .apply_reorg(
                    &reorg_observation(observer, 0),
                    FederationOperationId::from_bytes([operation; 32]),
                    u64::from(operation),
                )
                .expect("reorg evidence accepted");
        }
        let utxo = state.utxo(outpoint(3)).expect("UTXO after reorg quorum");
        assert_eq!(utxo.state, FederationWalletUtxoState::Reorged);
        assert!(utxo.liability_alarm);
    }

    #[test]
    fn single_conflicting_observer_cannot_permanently_quarantine_established_utxo() {
        let mut state = FederationChainObservationState::new();
        for observer in 1..=4 {
            let evidence = observation(
                observer,
                Some(block(9, 100, 2)),
                FederationChainSourceScope::Independent,
            );
            apply_with_policy(&mut state, &evidence, 3, 2, observer as u8);
        }
        assert_eq!(
            state.utxo(outpoint(3)).expect("spendable UTXO").state,
            FederationWalletUtxoState::Spendable
        );

        // One Byzantine observer reports a conflicting value for the same
        // public outpoint. The conflicting vote is retained and recorded,
        // and because the honest quorum's stored votes already back the
        // established fields, the UTXO repairs within the same apply instead
        // of staying quarantined.
        let mut conflict = observation(
            5,
            Some(block(9, 100, 2)),
            FederationChainSourceScope::Independent,
        );
        conflict.txout.value = Amount::from_sat(49_999);
        let outcome = state
            .apply_deposit(
                &conflict,
                &allocation().0,
                ScriptBuf::from_bytes(vec![0x51]).as_script(),
                3,
                2,
                FederationOperationId::from_bytes([5; 32]),
                5,
            )
            .expect("conflict materializes");
        assert_eq!(
            outcome,
            FederationChainObservationOutcome::Updated(FederationWalletUtxoState::Spendable)
        );
        {
            let utxo = state.utxo(outpoint(3)).expect("repaired UTXO");
            assert_eq!(
                utxo.state,
                FederationWalletUtxoState::Spendable,
                "the quorum-backed honest fields must repair the conflict immediately"
            );
            assert!(
                utxo.conflicting_observers.contains(&member(5)),
                "the conflicting observer stays recorded"
            );
            assert_eq!(utxo.txout.value, Amount::from_sat(50_000));
        }

        // Honest observers keep reporting the true fields with deepening
        // confirmations; the honest quorum must repair the UTXO.
        for (operation, observer) in [(6, 1), (7, 2), (8, 3)] {
            let evidence = observation(
                observer,
                Some(block(9, 100, 3)),
                FederationChainSourceScope::Independent,
            );
            apply_with_policy(&mut state, &evidence, 3, 2, operation);
        }
        let utxo = state.utxo(outpoint(3)).expect("repaired UTXO");
        assert_eq!(
            utxo.state,
            FederationWalletUtxoState::Spendable,
            "an honest quorum must repair a one-observer quarantine"
        );
        assert!(utxo.conflicting_observers.contains(&member(5)));
        assert!(
            state
                .quote_payment_at_or_before("quote-1", u64::MAX)
                .expect("credit query")
                .is_some(),
            "a repaired UTXO must fund quote credit again"
        );
    }

    #[test]
    fn first_lying_observer_cannot_brick_deposit_fields() {
        let mut state = FederationChainObservationState::new();

        // A Byzantine observer is the first to report a real public outpoint
        // but claims an incorrect value while keeping the correct allocated
        // script, so its report fixes the UTXO's immutable fields.
        let mut liar = observation(
            1,
            Some(block(9, 100, 2)),
            FederationChainSourceScope::Independent,
        );
        liar.txout.value = Amount::from_sat(49_999);
        apply_with_policy(&mut state, &liar, 2, 2, 2);

        // The honest quorum then reports the true fields for the same
        // outpoint.
        for (operation, observer) in [(3, 2), (4, 3)] {
            let honest = observation(
                observer,
                Some(block(9, 100, 3)),
                FederationChainSourceScope::Independent,
            );
            apply_with_policy(&mut state, &honest, 2, 2, operation);
        }

        // The quorum-backed fields must be adopted over the minority-fixed
        // ones: the UTXO becomes spendable with the true value, and the liar
        // is the recorded conflict.
        let utxo = state.utxo(outpoint(3)).expect("utxo");
        assert_eq!(
            utxo.state,
            FederationWalletUtxoState::Spendable,
            "an honest quorum must repair a liar-first quarantine"
        );
        assert_eq!(utxo.txout.value, Amount::from_sat(50_000));
        assert!(
            utxo.conflicting_observers.contains(&member(1)),
            "the first liar must be the recorded conflict"
        );
        assert!(!utxo.conflicting_observers.contains(&member(2)));
        assert!(!utxo.conflicting_observers.contains(&member(3)));
        assert!(
            state
                .quote_payment_at_or_before("quote-1", u64::MAX)
                .expect("credit query")
                .is_some(),
            "an adopted UTXO must fund quote credit"
        );
    }

    #[test]
    fn first_lying_observer_cannot_brick_deposit_block() {
        let mut state = FederationChainObservationState::new();

        // Liar-first variant on the block anchor: the value is correct but
        // the claimed block is wrong.
        let mut liar = observation(
            1,
            Some(block(8, 100, 2)),
            FederationChainSourceScope::Independent,
        );
        liar.confirmation = Some(block(8, 100, 2));
        apply_with_policy(&mut state, &liar, 2, 2, 2);

        for (operation, observer) in [(3, 2), (4, 3)] {
            let honest = observation(
                observer,
                Some(block(9, 100, 3)),
                FederationChainSourceScope::Independent,
            );
            apply_with_policy(&mut state, &honest, 2, 2, operation);
        }

        let utxo = state.utxo(outpoint(3)).expect("utxo");
        assert_eq!(utxo.state, FederationWalletUtxoState::Spendable);
        assert_eq!(
            utxo.block,
            Some((BlockHash::from_byte_array([9; 32]), 100,)),
            "the quorum-backed block must replace the liar's anchor"
        );
        assert!(utxo.conflicting_observers.contains(&member(1)));
    }

    #[test]
    fn lying_observer_changing_story_cannot_double_vote() {
        let mut state = FederationChainObservationState::new();

        // The liar reports one false value, then equivocates with a second
        // false value for the same outpoint.
        let mut first_story = observation(
            1,
            Some(block(9, 100, 2)),
            FederationChainSourceScope::Independent,
        );
        first_story.txout.value = Amount::from_sat(49_999);
        apply_with_policy(&mut state, &first_story, 2, 2, 2);
        let mut second_story = first_story.clone();
        second_story.txout.value = Amount::from_sat(48_000);
        let outcome = state
            .apply_deposit(
                &second_story,
                &allocation().0,
                ScriptBuf::from_bytes(vec![0x51]).as_script(),
                2,
                2,
                FederationOperationId::from_bytes([3; 32]),
                3,
            )
            .expect("equivocation materializes");
        assert_eq!(outcome, FederationChainObservationOutcome::Quarantined);
        assert_eq!(
            state.utxo(outpoint(3)).expect("utxo").txout.value,
            Amount::from_sat(49_999),
            "an equivocating observer cannot replace its own recorded vote"
        );

        // The honest quorum repairs the UTXO; the liar's first story is the
        // recorded conflict and cannot double-vote for either false set.
        for (operation, observer) in [(4, 2), (5, 3)] {
            let honest = observation(
                observer,
                Some(block(9, 100, 3)),
                FederationChainSourceScope::Independent,
            );
            apply_with_policy(&mut state, &honest, 2, 2, operation);
        }
        let utxo = state.utxo(outpoint(3)).expect("utxo");
        assert_eq!(utxo.state, FederationWalletUtxoState::Spendable);
        assert_eq!(utxo.txout.value, Amount::from_sat(50_000));
        assert!(utxo.conflicting_observers.contains(&member(1)));
    }

    #[test]
    fn quorum_split_across_two_false_sets_adopts_the_honest_set_or_stays_closed() {
        // Two liars split across two false sets; the honest quorum still
        // reaches the spendability threshold and is adopted.
        let mut state = FederationChainObservationState::new();
        let mut false_one = observation(
            1,
            Some(block(9, 100, 2)),
            FederationChainSourceScope::Independent,
        );
        false_one.txout.value = Amount::from_sat(49_999);
        apply_with_policy(&mut state, &false_one, 3, 2, 2);
        let mut false_two = observation(
            2,
            Some(block(9, 100, 2)),
            FederationChainSourceScope::Independent,
        );
        false_two.txout.value = Amount::from_sat(48_000);
        apply_with_policy(&mut state, &false_two, 3, 2, 3);
        for (operation, observer) in [(4, 3), (5, 4), (6, 5)] {
            let honest = observation(
                observer,
                Some(block(9, 100, 3)),
                FederationChainSourceScope::Independent,
            );
            apply_with_policy(&mut state, &honest, 3, 2, operation);
        }
        let utxo = state.utxo(outpoint(3)).expect("utxo");
        assert_eq!(utxo.state, FederationWalletUtxoState::Spendable);
        assert_eq!(utxo.txout.value, Amount::from_sat(50_000));
        assert!(utxo.conflicting_observers.contains(&member(1)));
        assert!(utxo.conflicting_observers.contains(&member(2)));

        // Without any set reaching the quorum the UTXO fails closed and
        // stays quarantined with the conflict recorded.
        let mut stuck = FederationChainObservationState::new();
        apply_with_policy(&mut stuck, &false_one, 3, 2, 2);
        apply_with_policy(&mut stuck, &false_two, 3, 2, 3);
        let honest = observation(
            3,
            Some(block(9, 100, 3)),
            FederationChainSourceScope::Independent,
        );
        apply_with_policy(&mut stuck, &honest, 3, 2, 4);
        let utxo = stuck.utxo(outpoint(3)).expect("stuck utxo");
        assert_eq!(
            utxo.state,
            FederationWalletUtxoState::Quarantined,
            "no quorum-backed field set must keep the UTXO quarantined"
        );
        assert!(stuck
            .quote_payment_at_or_before("quote-1", u64::MAX)
            .expect("credit query")
            .is_none());
    }

    #[test]
    fn liar_first_repair_survives_restart_replay() {
        let mut state = FederationChainObservationState::new();
        let mut liar = observation(
            1,
            Some(block(9, 100, 2)),
            FederationChainSourceScope::Independent,
        );
        liar.txout.value = Amount::from_sat(49_999);
        apply_with_policy(&mut state, &liar, 2, 2, 2);

        // Restart from the durable bytes with only the liar's fields fixed.
        let bytes = state.to_bytes().expect("serialize liar-first state");
        let mut replayed = FederationChainObservationState::from_bytes(&bytes)
            .expect("liar-first state must validate after replay");
        let honest = observation(
            2,
            Some(block(9, 100, 3)),
            FederationChainSourceScope::Independent,
        );
        apply_with_policy(&mut replayed, &honest, 2, 2, 3);

        // Replay again mid-repair, then finish the honest quorum.
        let bytes = replayed.to_bytes().expect("serialize mid-repair state");
        let mut replayed = FederationChainObservationState::from_bytes(&bytes)
            .expect("mid-repair state must validate after replay");
        let honest = observation(
            3,
            Some(block(9, 100, 4)),
            FederationChainSourceScope::Independent,
        );
        apply_with_policy(&mut replayed, &honest, 2, 2, 4);
        let utxo = replayed.utxo(outpoint(3)).expect("utxo");
        assert_eq!(utxo.state, FederationWalletUtxoState::Spendable);
        assert_eq!(utxo.txout.value, Amount::from_sat(50_000));
        assert!(utxo.conflicting_observers.contains(&member(1)));
    }

    #[test]
    fn post_liability_conflict_stays_sticky_with_alarm_and_never_adopts() {
        let mut state = FederationChainObservationState::new();
        for (operation, observer) in [(2, 1), (3, 2)] {
            let evidence = observation(
                observer,
                Some(block(9, 100, 2)),
                FederationChainSourceScope::Independent,
            );
            apply_with_policy(&mut state, &evidence, 2, 2, operation);
        }
        let payment = state
            .quote_payment_at_or_before("quote-1", u64::MAX)
            .expect("payment query")
            .expect("spendable payment");
        state
            .mark_quote_liability_issued_at_or_before(
                "quote-1",
                u64::MAX,
                payment.amount_sat,
                payment.payment_id,
            )
            .expect("issue liability");

        // A late liar cannot adopt anything after liability: the conflict
        // stays quarantined with the alarm raised.
        let mut liar = observation(
            3,
            Some(block(9, 100, 2)),
            FederationChainSourceScope::Independent,
        );
        liar.txout.value = Amount::from_sat(49_999);
        apply_with_policy(&mut state, &liar, 2, 2, 4);
        let utxo = state.utxo(outpoint(3)).expect("utxo");
        assert_eq!(utxo.state, FederationWalletUtxoState::Quarantined);
        assert!(utxo.liability_alarm);
        assert_eq!(
            utxo.txout.value,
            Amount::from_sat(50_000),
            "post-liability fields never adopt a minority set"
        );
        assert!(utxo.conflicting_observers.contains(&member(3)));
    }

    #[test]
    fn reservation_lifecycle_accepts_only_exact_state_transitions() {
        let mut state = FederationChainObservationState::new();
        for (operation, observer) in [(2, 1), (3, 2)] {
            let evidence = observation(
                observer,
                Some(block(9, 100, 2)),
                FederationChainSourceScope::Independent,
            );
            apply(&mut state, &evidence, operation);
        }

        let reserved = outpoint(3);
        state.reserve_utxos(&[reserved]).expect("reserve spendable");
        assert_eq!(
            state.utxo(reserved).expect("reserved UTXO").state,
            FederationWalletUtxoState::Reserved
        );
        assert!(state.reserve_utxos(&[reserved]).is_err());
        assert!(state.spend_reserved_utxos(&[outpoint(99)]).is_err());

        state
            .spend_reserved_utxos(&[reserved])
            .expect("spend exact reservation");
        assert_eq!(
            state.utxo(reserved).expect("spent UTXO").state,
            FederationWalletUtxoState::Spent
        );
        assert!(state.spend_reserved_utxos(&[reserved]).is_err());
        assert!(state.reserve_utxos(&[reserved]).is_err());

        let bytes = state.to_bytes().expect("state bytes");
        assert_eq!(
            FederationChainObservationState::from_bytes(&bytes).expect("state replay"),
            state
        );
    }
}
