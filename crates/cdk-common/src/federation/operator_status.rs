//! Bounded, redacted operator status for the federated on-chain wallet.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use super::{
    FederationAcceptedTransactionState, FederationAddressAllocationScope,
    FederationAddressAllocationState, FederationChainObservationState, FederationError,
    FederationFrostWalletConfig, FederationOnchainPaymentIntentState,
    FederationTransactionProposalId, FederationTransactionState, FederationWalletLivenessStatus,
    FederationWalletUtxoState, Result,
};

const FEDERATION_WALLET_OPERATOR_STATUS_VERSION: u16 = 1;

/// Stable wallet operation phase suitable for status output and bounded metrics.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FederationWalletOperationPhase {
    /// Deposits are waiting for independent chain evidence or confirmation depth.
    ChainObservation,
    /// Accepted melt intents are waiting for deterministic proposal construction.
    Proposal,
    /// A consensus-accepted transaction is waiting for threshold signatures.
    Signing,
    /// Exact signed bytes are waiting for durable broadcast authorization.
    BroadcastAuthorization,
    /// Authorized exact bytes are being submitted or reconciled with Bitcoin.
    Broadcast,
    /// A broadcast transaction is waiting for quorum confirmation evidence.
    Confirmation,
    /// Confirmed, abandoned, replaced, completed, or failed records.
    Terminal,
}

impl core::fmt::Display for FederationWalletOperationPhase {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter.write_str(match self {
            Self::ChainObservation => "chain_observation",
            Self::Proposal => "proposal",
            Self::Signing => "signing",
            Self::BroadcastAuthorization => "broadcast_authorization",
            Self::Broadcast => "broadcast",
            Self::Confirmation => "confirmation",
            Self::Terminal => "terminal",
        })
    }
}

/// Count and value grouped by one fixed UTXO lifecycle state.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FederationWalletUtxoStatusCount {
    /// Stable UTXO state.
    pub state: FederationWalletUtxoState,
    /// Number of UTXOs in the state.
    pub count: u64,
    /// Saturating aggregate value in satoshis.
    pub value_sat: u64,
}

/// Count grouped by one fixed wallet operation phase.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FederationWalletOperationPhaseCount {
    /// Stable operation phase.
    pub phase: FederationWalletOperationPhase,
    /// Number of durable records in the phase.
    pub count: u64,
}

/// Bounded broadcast health classification with no transaction identifiers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FederationWalletBroadcastHealth {
    /// No signed outgoing transaction needs broadcast work.
    Idle,
    /// Signed bytes exist but broadcast authorization is not yet durable.
    AwaitingAuthorization,
    /// Authorized bytes have not yet received a success-equivalent observation.
    Broadcasting,
    /// At least one backend accepted the bytes and confirmation evidence is pending.
    Confirming,
    /// A latest member observation is rejected, transient, or ambiguous.
    Degraded,
}

impl core::fmt::Display for FederationWalletBroadcastHealth {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter.write_str(match self {
            Self::Idle => "idle",
            Self::AwaitingAuthorization => "awaiting_authorization",
            Self::Broadcasting => "broadcasting",
            Self::Confirming => "confirming",
            Self::Degraded => "degraded",
        })
    }
}

/// Safe wallet status derived only from accepted public configuration and projections.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FederationWalletOperatorStatus {
    /// Status schema version.
    pub version: u16,
    /// Root epoch serving the active CDK wallet application.
    pub root_key_epoch: u64,
    /// Active CDK application key epoch.
    pub application_key_epoch: u64,
    /// Public commitment to the active external/internal descriptors.
    pub descriptor_commitment: [u8; 32],
    /// Consensus Bitcoin network.
    pub network: String,
    /// Consensus confirmation depth.
    pub minimum_confirmations: u32,
    /// Consensus fee-policy version.
    pub fee_policy_version: u16,
    /// Number of externally allocated descriptor children.
    pub external_revealed_count: u64,
    /// Number of internally allocated change children.
    pub internal_revealed_count: u64,
    /// Fixed-state UTXO counts and values.
    pub utxos: Vec<FederationWalletUtxoStatusCount>,
    /// Value retained under sticky accepted-input reservations.
    pub reserved_value_sat: u64,
    /// Fixed operation-phase counts.
    pub operation_phases: Vec<FederationWalletOperationPhaseCount>,
    /// Pending deterministic batches reported by the local liveness worker.
    pub pending_batches: u64,
    /// FROST coordination attempts made by the local liveness worker.
    pub signing_attempts: u64,
    /// Saturating signer/transport failure count.
    pub signing_failures: u64,
    /// Redacted broadcast health.
    pub broadcast_health: FederationWalletBroadcastHealth,
    /// Number of accepted non-success broadcast observations.
    pub broadcast_failures: u64,
}

/// Targeted private transaction diagnosis with no transaction contents or scripts.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FederationWalletTransactionStatus {
    /// Status schema version.
    pub version: u16,
    /// Operator-requested canonical proposal identity.
    pub proposal_id: FederationTransactionProposalId,
    /// Current fixed operation phase.
    pub phase: FederationWalletOperationPhase,
    /// Durable transaction state.
    pub state: FederationAcceptedTransactionState,
    /// Root key epoch authorizing the proposal.
    pub root_key_epoch: u64,
    /// Application key epoch authorizing the proposal.
    pub application_key_epoch: u64,
    /// Whether exact signed bytes have been durably persisted.
    pub signed: bool,
    /// Whether durable broadcast authorization exists.
    pub broadcast_authorized: bool,
    /// Number of bounded authenticated broadcast observations.
    pub broadcast_observations: u64,
    /// Number of success-equivalent broadcast observations.
    pub successful_broadcast_observations: u64,
    /// Number of confirmation observations.
    pub confirmation_observations: u64,
}

impl FederationWalletTransactionStatus {
    /// Build targeted status from one validated transaction projection.
    pub fn from_transactions(
        transactions: &FederationTransactionState,
        proposal_id: FederationTransactionProposalId,
    ) -> Result<Self> {
        transactions.validate()?;
        let accepted = transactions
            .accepted_transaction(proposal_id)
            .ok_or_else(|| FederationError::TransactionProposalInvalid {
                reason: "operator status proposal is unknown".to_string(),
            })?;
        let phase = transaction_phase(accepted.state);
        let observations = accepted
            .broadcast
            .as_ref()
            .map(|broadcast| broadcast.observations.as_slice())
            .unwrap_or_default();
        let broadcast_observations = saturating_u64(observations.len());
        let successful_broadcast_observations = saturating_u64(
            observations
                .iter()
                .filter(|record| record.observation.result.is_success_equivalent())
                .count(),
        );
        let confirmation_observations = saturating_u64(
            observations
                .iter()
                .filter(|record| {
                    matches!(
                        record.observation.result,
                        super::FederationTransactionBroadcastResult::Confirmed { .. }
                    )
                })
                .count(),
        );
        Ok(Self {
            version: FEDERATION_WALLET_OPERATOR_STATUS_VERSION,
            proposal_id,
            phase,
            state: accepted.state,
            root_key_epoch: accepted.proposal.scope.root_key_epoch,
            application_key_epoch: accepted.proposal.scope.application_key_epoch,
            signed: accepted.signed.is_some(),
            broadcast_authorized: accepted.broadcast.is_some(),
            broadcast_observations,
            successful_broadcast_observations,
            confirmation_observations,
        })
    }
}

impl FederationWalletOperatorStatus {
    /// Derive bounded status from validated accepted state without exposing row identifiers.
    pub fn from_states(
        wallet: &FederationFrostWalletConfig,
        addresses: &FederationAddressAllocationState,
        chain: &FederationChainObservationState,
        transactions: &FederationTransactionState,
        liveness: &FederationWalletLivenessStatus,
    ) -> Result<Self> {
        addresses.validate()?;
        chain.validate()?;
        transactions.validate()?;
        let application = &wallet.cdk_onchain_application;
        let address_scope = FederationAddressAllocationScope {
            application_id: application.application_id,
            application_key_epoch: application.application_key_epoch,
        };
        let address_wallet = addresses.wallet(address_scope);
        let external_revealed_count = address_wallet
            .map(|state| state.external_next_index)
            .unwrap_or_default();
        let internal_revealed_count = address_wallet
            .map(|state| state.internal_next_index)
            .unwrap_or_default();

        let mut utxo_counts = BTreeMap::<FederationWalletUtxoState, (u64, u64)>::new();
        for state in all_utxo_states() {
            utxo_counts.insert(state, (0, 0));
        }
        for utxo in chain.utxos() {
            let value = utxo.txout.value.to_sat();
            let entry = utxo_counts.entry(utxo.state).or_default();
            entry.0 = entry.0.saturating_add(1);
            entry.1 = entry.1.saturating_add(value);
        }
        let reserved_value_sat = utxo_counts
            .get(&FederationWalletUtxoState::Reserved)
            .map(|(_, value)| *value)
            .unwrap_or_default();
        let utxos = utxo_counts
            .into_iter()
            .map(
                |(state, (count, value_sat))| FederationWalletUtxoStatusCount {
                    state,
                    count,
                    value_sat,
                },
            )
            .collect();

        let mut phases = BTreeMap::<FederationWalletOperationPhase, u64>::new();
        for phase in all_operation_phases() {
            phases.insert(phase, 0);
        }
        for utxo in chain.utxos() {
            if matches!(
                utxo.state,
                FederationWalletUtxoState::Observed
                    | FederationWalletUtxoState::Confirmed
                    | FederationWalletUtxoState::Reorged
                    | FederationWalletUtxoState::Quarantined
            ) {
                increment(
                    &mut phases,
                    FederationWalletOperationPhase::ChainObservation,
                );
            }
        }
        for intent in transactions.intents().values() {
            match intent.state {
                FederationOnchainPaymentIntentState::Pending => {
                    increment(&mut phases, FederationWalletOperationPhase::Proposal);
                }
                FederationOnchainPaymentIntentState::Completed { .. }
                | FederationOnchainPaymentIntentState::Failed => {
                    increment(&mut phases, FederationWalletOperationPhase::Terminal);
                }
                FederationOnchainPaymentIntentState::Accepted { .. } => {}
            }
        }

        let mut broadcast_health = FederationWalletBroadcastHealth::Idle;
        let mut broadcast_failures = 0_u64;
        for accepted in transactions.accepted_transactions().values() {
            let phase = match accepted.state {
                FederationAcceptedTransactionState::Accepted => {
                    FederationWalletOperationPhase::Signing
                }
                FederationAcceptedTransactionState::Signed => {
                    if broadcast_health == FederationWalletBroadcastHealth::Idle {
                        broadcast_health = FederationWalletBroadcastHealth::AwaitingAuthorization;
                    }
                    FederationWalletOperationPhase::BroadcastAuthorization
                }
                FederationAcceptedTransactionState::BroadcastPending => {
                    if broadcast_health != FederationWalletBroadcastHealth::Degraded {
                        broadcast_health = FederationWalletBroadcastHealth::Broadcasting;
                    }
                    FederationWalletOperationPhase::Broadcast
                }
                FederationAcceptedTransactionState::Broadcast => {
                    if broadcast_health != FederationWalletBroadcastHealth::Degraded {
                        broadcast_health = FederationWalletBroadcastHealth::Confirming;
                    }
                    FederationWalletOperationPhase::Confirmation
                }
                FederationAcceptedTransactionState::Confirmed
                | FederationAcceptedTransactionState::Abandoned
                | FederationAcceptedTransactionState::Replaced { .. } => {
                    FederationWalletOperationPhase::Terminal
                }
            };
            increment(&mut phases, phase);
            if let Some(broadcast) = &accepted.broadcast {
                for record in &broadcast.observations {
                    if !record.observation.result.is_success_equivalent() {
                        broadcast_failures = broadcast_failures.saturating_add(1);
                        broadcast_health = FederationWalletBroadcastHealth::Degraded;
                    }
                }
            }
        }
        let operation_phases = phases
            .into_iter()
            .map(|(phase, count)| FederationWalletOperationPhaseCount { phase, count })
            .collect();

        Ok(Self {
            version: FEDERATION_WALLET_OPERATOR_STATUS_VERSION,
            root_key_epoch: application.root_key_epoch,
            application_key_epoch: application.application_key_epoch,
            descriptor_commitment: application.descriptor_commitment,
            network: application.network.to_string(),
            minimum_confirmations: application.confirmation_policy.minimum_confirmations,
            fee_policy_version: application.fee_policy_version,
            external_revealed_count,
            internal_revealed_count,
            utxos,
            reserved_value_sat,
            operation_phases,
            pending_batches: liveness.pending_batches,
            signing_attempts: liveness.signing_attempts,
            signing_failures: liveness.signer_failures,
            broadcast_health,
            broadcast_failures,
        })
    }
}

fn increment(
    counts: &mut BTreeMap<FederationWalletOperationPhase, u64>,
    phase: FederationWalletOperationPhase,
) {
    let count = counts.entry(phase).or_default();
    *count = count.saturating_add(1);
}

fn transaction_phase(state: FederationAcceptedTransactionState) -> FederationWalletOperationPhase {
    match state {
        FederationAcceptedTransactionState::Accepted => FederationWalletOperationPhase::Signing,
        FederationAcceptedTransactionState::Signed => {
            FederationWalletOperationPhase::BroadcastAuthorization
        }
        FederationAcceptedTransactionState::BroadcastPending => {
            FederationWalletOperationPhase::Broadcast
        }
        FederationAcceptedTransactionState::Broadcast => {
            FederationWalletOperationPhase::Confirmation
        }
        FederationAcceptedTransactionState::Confirmed
        | FederationAcceptedTransactionState::Abandoned
        | FederationAcceptedTransactionState::Replaced { .. } => {
            FederationWalletOperationPhase::Terminal
        }
    }
}

fn saturating_u64(value: usize) -> u64 {
    u64::try_from(value).unwrap_or(u64::MAX)
}

fn all_utxo_states() -> [FederationWalletUtxoState; 7] {
    [
        FederationWalletUtxoState::Observed,
        FederationWalletUtxoState::Confirmed,
        FederationWalletUtxoState::Spendable,
        FederationWalletUtxoState::Reserved,
        FederationWalletUtxoState::Spent,
        FederationWalletUtxoState::Reorged,
        FederationWalletUtxoState::Quarantined,
    ]
}

fn all_operation_phases() -> [FederationWalletOperationPhase; 7] {
    [
        FederationWalletOperationPhase::ChainObservation,
        FederationWalletOperationPhase::Proposal,
        FederationWalletOperationPhase::Signing,
        FederationWalletOperationPhase::BroadcastAuthorization,
        FederationWalletOperationPhase::Broadcast,
        FederationWalletOperationPhase::Confirmation,
        FederationWalletOperationPhase::Terminal,
    ]
}

#[cfg(test)]
mod tests {
    use super::{
        all_operation_phases, transaction_phase, FederationAcceptedTransactionState,
        FederationTransactionProposalId, FederationWalletOperationPhase,
    };

    #[test]
    fn operator_status_has_every_fixed_wallet_phase() {
        assert_eq!(
            all_operation_phases(),
            [
                FederationWalletOperationPhase::ChainObservation,
                FederationWalletOperationPhase::Proposal,
                FederationWalletOperationPhase::Signing,
                FederationWalletOperationPhase::BroadcastAuthorization,
                FederationWalletOperationPhase::Broadcast,
                FederationWalletOperationPhase::Confirmation,
                FederationWalletOperationPhase::Terminal,
            ]
        );
        assert_eq!(
            transaction_phase(FederationAcceptedTransactionState::Accepted),
            FederationWalletOperationPhase::Signing
        );
        assert_eq!(
            transaction_phase(FederationAcceptedTransactionState::Signed),
            FederationWalletOperationPhase::BroadcastAuthorization
        );
        assert_eq!(
            transaction_phase(FederationAcceptedTransactionState::BroadcastPending),
            FederationWalletOperationPhase::Broadcast
        );
        assert_eq!(
            transaction_phase(FederationAcceptedTransactionState::Broadcast),
            FederationWalletOperationPhase::Confirmation
        );
        for terminal in [
            FederationAcceptedTransactionState::Confirmed,
            FederationAcceptedTransactionState::Abandoned,
            FederationAcceptedTransactionState::Replaced {
                by: FederationTransactionProposalId::from_bytes([3; 32]),
            },
        ] {
            assert_eq!(
                transaction_phase(terminal),
                FederationWalletOperationPhase::Terminal
            );
        }
    }
}
