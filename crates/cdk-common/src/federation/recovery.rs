//! Redacted recovery classification for federated wallet durable state.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use super::{
    FederationAcceptedTransactionState, FederationAddressAllocationState,
    FederationChainObservationState, FederationOnchainPaymentIntentState,
    FederationTransactionState, FederationWalletUtxoState, Result,
};

const FEDERATION_WALLET_RECOVERY_STATUS_VERSION: u16 = 1;

/// Durable subsystem represented by one recovery action.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FederationRecoveryPhase {
    /// Consensus address allocation and BDK revealed-index restoration.
    Address,
    /// Chain observation and conservative UTXO materialization.
    Observation,
    /// Pending outgoing intent and proposal construction.
    Proposal,
    /// Sticky accepted-input ownership.
    Reservation,
    /// FROST commitment/share collection.
    Signing,
    /// Durable signed bytes and BDK graph restoration.
    Signed,
    /// Broadcast intent and exact-byte rebroadcast.
    Broadcast,
    /// Member confirmation evidence and settlement.
    Confirmation,
    /// Abandonment or replacement lineage.
    Replacement,
    /// Completed or otherwise terminal state.
    Terminal,
}

/// Safe action selected for a durable state during restart or steady-state repair.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FederationRecoveryAction {
    /// Raise local BDK revealed counters to accepted consensus indexes.
    ReconcileBdkIndexes,
    /// Continue collecting independently authenticated chain evidence.
    CollectChainEvidence,
    /// Await deterministic proposal construction for pending intents.
    AwaitProposal,
    /// Retain an accepted input lock without releasing it during recovery.
    PreserveStickyReservation,
    /// Start a new signing attempt; restart recovery never resumes old nonces.
    StartFreshSigningAttempt,
    /// Reapply exact persisted transaction bytes to the local BDK graph.
    ReapplyPersistedTransaction,
    /// Order broadcast authorization before any Bitcoin network request.
    PersistBroadcastIntent,
    /// Retry only the exact persisted signed transaction bytes.
    RebroadcastPersistedTransaction,
    /// Continue collecting matching confirmation evidence from distinct members.
    CollectConfirmationEvidence,
    /// Require human inspection while all custody locks remain fail-closed.
    OperatorReview,
    /// The durable state requires no recovery mutation.
    NoOp,
}

/// Count of records sharing one redacted state-to-action recovery rule.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FederationRecoveryActionCount {
    /// Durable subsystem.
    pub phase: FederationRecoveryPhase,
    /// Stable, non-secret durable state name.
    pub state: String,
    /// Safe restart or repair action.
    pub action: FederationRecoveryAction,
    /// Number of matching records.
    pub count: u64,
}

/// Bounded operator-facing recovery classification derived from consensus state.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FederationWalletRecoveryStatus {
    /// Recovery status schema version.
    pub version: u16,
    /// Accepted address allocation count.
    pub address_allocations: u64,
    /// Canonical wallet UTXO count.
    pub wallet_utxos: u64,
    /// Canonical outgoing payment-intent count.
    pub payment_intents: u64,
    /// Accepted transaction count.
    pub transactions: u64,
    /// Sticky input reservations that recovery must never release.
    pub sticky_reservations: u64,
    /// Canonically ordered state-to-action counts; no descriptors, scripts, or secrets.
    pub actions: Vec<FederationRecoveryActionCount>,
}

impl FederationWalletRecoveryStatus {
    /// Classify every federated wallet durable state into its safe recovery action.
    pub fn from_states(
        addresses: &FederationAddressAllocationState,
        chain: &FederationChainObservationState,
        transactions: &FederationTransactionState,
    ) -> Result<Self> {
        addresses.validate()?;
        chain.validate()?;
        transactions.validate()?;

        let mut counts = BTreeMap::<
            (
                FederationRecoveryPhase,
                &'static str,
                FederationRecoveryAction,
            ),
            u64,
        >::new();
        let mut address_allocations = 0_u64;
        for (_, wallet) in addresses.wallets() {
            let allocation_count = saturating_u64(wallet.allocations.len());
            address_allocations = address_allocations.saturating_add(allocation_count);
            increment(
                &mut counts,
                FederationRecoveryPhase::Address,
                "allocated",
                FederationRecoveryAction::ReconcileBdkIndexes,
                allocation_count,
            );
        }

        let mut wallet_utxos = 0_u64;
        for utxo in chain.utxos() {
            wallet_utxos = wallet_utxos.saturating_add(1);
            let (phase, state, action) = match utxo.state {
                FederationWalletUtxoState::Observed => (
                    FederationRecoveryPhase::Observation,
                    "observed",
                    FederationRecoveryAction::CollectChainEvidence,
                ),
                FederationWalletUtxoState::Confirmed => (
                    FederationRecoveryPhase::Observation,
                    "confirmed",
                    FederationRecoveryAction::CollectChainEvidence,
                ),
                FederationWalletUtxoState::Spendable => (
                    FederationRecoveryPhase::Observation,
                    "spendable",
                    FederationRecoveryAction::NoOp,
                ),
                FederationWalletUtxoState::Reserved => (
                    FederationRecoveryPhase::Reservation,
                    "reserved",
                    FederationRecoveryAction::PreserveStickyReservation,
                ),
                FederationWalletUtxoState::Spent => (
                    FederationRecoveryPhase::Terminal,
                    "spent",
                    FederationRecoveryAction::NoOp,
                ),
                FederationWalletUtxoState::Reorged => (
                    FederationRecoveryPhase::Observation,
                    "reorged",
                    FederationRecoveryAction::OperatorReview,
                ),
                FederationWalletUtxoState::Quarantined => (
                    FederationRecoveryPhase::Observation,
                    "quarantined",
                    FederationRecoveryAction::OperatorReview,
                ),
            };
            increment(&mut counts, phase, state, action, 1);
        }

        for intent in transactions.intents().values() {
            let (phase, state, action) = match intent.state {
                FederationOnchainPaymentIntentState::Pending => (
                    FederationRecoveryPhase::Proposal,
                    "pending_intent",
                    FederationRecoveryAction::AwaitProposal,
                ),
                FederationOnchainPaymentIntentState::Accepted { .. } => (
                    FederationRecoveryPhase::Proposal,
                    "accepted_intent",
                    FederationRecoveryAction::NoOp,
                ),
                FederationOnchainPaymentIntentState::Completed { .. } => (
                    FederationRecoveryPhase::Terminal,
                    "completed_intent",
                    FederationRecoveryAction::NoOp,
                ),
                FederationOnchainPaymentIntentState::Failed => (
                    FederationRecoveryPhase::Terminal,
                    "failed_intent",
                    FederationRecoveryAction::NoOp,
                ),
            };
            increment(&mut counts, phase, state, action, 1);
        }

        increment(
            &mut counts,
            FederationRecoveryPhase::Reservation,
            "accepted_input",
            FederationRecoveryAction::PreserveStickyReservation,
            saturating_u64(transactions.reservation_count()),
        );

        for accepted in transactions.accepted_transactions().values() {
            if accepted.signed.is_some() {
                increment(
                    &mut counts,
                    FederationRecoveryPhase::Signed,
                    "persisted",
                    FederationRecoveryAction::ReapplyPersistedTransaction,
                    1,
                );
            }
            let (phase, state, action) = match accepted.state {
                FederationAcceptedTransactionState::Accepted => (
                    FederationRecoveryPhase::Signing,
                    "accepted",
                    FederationRecoveryAction::StartFreshSigningAttempt,
                ),
                FederationAcceptedTransactionState::Signed => (
                    FederationRecoveryPhase::Broadcast,
                    "signed",
                    FederationRecoveryAction::PersistBroadcastIntent,
                ),
                FederationAcceptedTransactionState::BroadcastPending => (
                    FederationRecoveryPhase::Broadcast,
                    "broadcast_pending",
                    FederationRecoveryAction::RebroadcastPersistedTransaction,
                ),
                FederationAcceptedTransactionState::Broadcast => (
                    FederationRecoveryPhase::Confirmation,
                    "broadcast",
                    FederationRecoveryAction::CollectConfirmationEvidence,
                ),
                FederationAcceptedTransactionState::Confirmed => (
                    FederationRecoveryPhase::Terminal,
                    "confirmed",
                    FederationRecoveryAction::NoOp,
                ),
                FederationAcceptedTransactionState::Abandoned => (
                    FederationRecoveryPhase::Replacement,
                    "abandoned",
                    FederationRecoveryAction::PreserveStickyReservation,
                ),
                FederationAcceptedTransactionState::Replaced { .. } => (
                    FederationRecoveryPhase::Replacement,
                    "replaced",
                    FederationRecoveryAction::NoOp,
                ),
            };
            increment(&mut counts, phase, state, action, 1);
        }

        let actions = counts
            .into_iter()
            .filter(|(_, count)| *count > 0)
            .map(
                |((phase, state, action), count)| FederationRecoveryActionCount {
                    phase,
                    state: state.to_string(),
                    action,
                    count,
                },
            )
            .collect();
        Ok(Self {
            version: FEDERATION_WALLET_RECOVERY_STATUS_VERSION,
            address_allocations,
            wallet_utxos,
            payment_intents: saturating_u64(transactions.intents().len()),
            transactions: saturating_u64(transactions.accepted_transactions().len()),
            sticky_reservations: saturating_u64(transactions.reservation_count()),
            actions,
        })
    }
}

fn increment(
    counts: &mut BTreeMap<
        (
            FederationRecoveryPhase,
            &'static str,
            FederationRecoveryAction,
        ),
        u64,
    >,
    phase: FederationRecoveryPhase,
    state: &'static str,
    action: FederationRecoveryAction,
    count: u64,
) {
    let value = counts.entry((phase, state, action)).or_default();
    *value = value.saturating_add(count);
}

fn saturating_u64(value: usize) -> u64 {
    u64::try_from(value).unwrap_or(u64::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::federation::{
        FederationAddressAllocationOperation, FederationAddressKeychain,
        FederationFrostApplicationId, FederationOperationId,
    };

    #[test]
    fn empty_recovery_status_is_stable_and_contains_no_detail() {
        let status = FederationWalletRecoveryStatus::from_states(
            &FederationAddressAllocationState::new(),
            &FederationChainObservationState::new(),
            &FederationTransactionState::new(),
        )
        .expect("empty recovery status");
        assert_eq!(status.version, 1);
        assert_eq!(status.address_allocations, 0);
        assert_eq!(status.wallet_utxos, 0);
        assert_eq!(status.payment_intents, 0);
        assert_eq!(status.transactions, 0);
        assert_eq!(status.sticky_reservations, 0);
        assert!(status.actions.is_empty());
        assert!(!serde_json::to_string(&status)
            .expect("recovery status JSON")
            .contains("descriptor"));
    }

    #[test]
    fn accepted_address_is_classified_for_bdk_index_reconciliation() {
        let application =
            FederationFrostApplicationId::from_bytes([7; 32]).expect("application ID");
        let operation = FederationAddressAllocationOperation::new(
            "quote-a",
            application,
            1,
            FederationAddressKeychain::External,
        )
        .expect("address allocation operation");
        let mut addresses = FederationAddressAllocationState::new();
        addresses
            .apply(&operation, FederationOperationId::from_bytes([8; 32]), 1)
            .expect("accepted address allocation");
        let status = FederationWalletRecoveryStatus::from_states(
            &addresses,
            &FederationChainObservationState::new(),
            &FederationTransactionState::new(),
        )
        .expect("address recovery status");
        assert_eq!(status.address_allocations, 1);
        assert!(status.actions.iter().any(|entry| {
            entry.phase == FederationRecoveryPhase::Address
                && entry.action == FederationRecoveryAction::ReconcileBdkIndexes
                && entry.count == 1
        }));
    }
}
