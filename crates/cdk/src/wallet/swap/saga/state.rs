//! State types for the Swap saga.
//!
//! Each state is a distinct type that holds the data relevant to that stage
//! of the swap operation. The type state pattern ensures that only valid
//! operations are available at each stage.

use core::fmt;

use cdk_common::wallet::{KeysetLoadPolicy, WalletSaga};
use uuid::Uuid;

use crate::amount::SplitTarget;
use crate::nuts::{PreSwap, Proofs, PublicKey, SpendingConditions};
use crate::Amount;

/// Initial state - operation ID assigned but no work done yet.
///
/// The swap saga starts in this state. Only `prepare()` is available.
#[derive(Debug)]
pub struct Initial {
    /// Unique operation identifier for tracking and crash recovery
    pub operation_id: Uuid,
    /// Policy controlling how keysets are loaded during this saga
    pub keyset_policy: KeysetLoadPolicy,
}

/// Prepared state - swap request created, proofs reserved.
///
/// After successful preparation, the saga transitions to this state.
/// Methods available: `execute()`
pub struct Prepared {
    /// Unique operation identifier
    pub operation_id: Uuid,
    /// Amount to swap (None means swap all)
    pub amount: Option<Amount>,
    /// Amount split target for output proofs
    pub amount_split_target: SplitTarget,
    /// Y values of input proofs (for cleanup)
    pub input_ys: Vec<PublicKey>,
    /// Spending conditions for output proofs
    pub spending_conditions: Option<SpendingConditions>,
    /// Pre-swap data (request and secrets)
    pub pre_swap: PreSwap,
    /// Ephemeral key if P2BK was used
    /// The persisted saga for optimistic locking (contains recovery data)
    pub saga: WalletSaga,
}

impl fmt::Debug for Prepared {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Prepared")
            .field("operation_id", &self.operation_id)
            .field("amount", &self.amount)
            .field("amount_split_target", &self.amount_split_target)
            .field("input_ys", &self.input_ys)
            .field("spending_conditions", &self.spending_conditions)
            .field(
                "input_amounts",
                &self
                    .pre_swap
                    .swap_request
                    .inputs()
                    .iter()
                    .map(|proof| proof.amount)
                    .collect::<Vec<_>>(),
            )
            .field(
                "output_amounts",
                &self
                    .pre_swap
                    .swap_request
                    .outputs()
                    .iter()
                    .map(|output| output.amount)
                    .collect::<Vec<_>>(),
            )
            .field(
                "pre_mint_amounts",
                &self
                    .pre_swap
                    .pre_mint_secrets
                    .secrets
                    .iter()
                    .map(|secret| secret.amount)
                    .collect::<Vec<_>>(),
            )
            .field("derived_secret_count", &self.pre_swap.derived_secret_count)
            .field("fee", &self.pre_swap.fee)
            .field(
                "p2bk_secret_key_count",
                &self.pre_swap.p2bk_secret_keys.as_ref().map(Vec::len),
            )
            .field("saga_id", &self.saga.id)
            .field("saga_state", &self.saga.state)
            .field("saga_version", &self.saga.version)
            .finish()
    }
}

/// Finalized state - swap completed successfully.
///
/// After successful execution, the saga transitions to this state.
/// The output proofs can be retrieved and the saga is complete.
pub struct Finalized {
    /// Output proofs to send (if amount was specified)
    pub send_proofs: Option<Proofs>,
}

impl fmt::Debug for Finalized {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Finalized")
            .field(
                "send_proof_amounts",
                &self
                    .send_proofs
                    .as_ref()
                    .map(|proofs| proofs.iter().map(|proof| proof.amount).collect::<Vec<_>>()),
            )
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use cdk_common::wallet::{
        OperationData, SwapOperationData, SwapSagaState, WalletSaga, WalletSagaState,
    };
    use cdk_common::CurrencyUnit;
    use uuid::Uuid;

    use super::{Finalized, Prepared};
    use crate::amount::SplitTarget;
    use crate::nuts::{PreMintSecrets, PreSwap, SwapRequest};
    use crate::secret::Secret;
    use crate::wallet::test_utils::{test_keyset_id, test_mint_url, test_proof};
    use crate::Amount;

    const SECRET_MARKER: &str = "super_secret_spending_material_xyz";

    fn swap_saga(operation_id: Uuid) -> WalletSaga {
        WalletSaga::new(
            operation_id,
            WalletSagaState::Swap(SwapSagaState::ProofsReserved),
            Amount::from(1),
            test_mint_url(),
            CurrencyUnit::Sat,
            OperationData::Swap(SwapOperationData {
                input_amount: Amount::from(1),
                output_amount: Amount::from(1),
                counter_start: Some(0),
                counter_end: Some(1),
                blinded_messages: None,
            }),
        )
    }

    #[allow(clippy::use_debug)]
    #[test]
    fn swap_state_debug_does_not_leak_spending_secrets() {
        let operation_id = Uuid::new_v4();
        let keyset_id = test_keyset_id();
        let mut input_proof = test_proof(keyset_id, 1);
        input_proof.secret = Secret::new(SECRET_MARKER);
        let pre_mint_secrets = PreMintSecrets::from_secrets(
            keyset_id,
            vec![Amount::from(1)],
            vec![Secret::new(SECRET_MARKER)],
        )
        .expect("valid pre-mint secret");
        let outputs = pre_mint_secrets
            .secrets
            .iter()
            .map(|secret| secret.blinded_message.clone())
            .collect();

        let prepared = Prepared {
            operation_id,
            amount: Some(Amount::from(1)),
            amount_split_target: SplitTarget::default(),
            input_ys: Vec::new(),
            spending_conditions: None,
            pre_swap: PreSwap {
                pre_mint_secrets,
                swap_request: SwapRequest::new(vec![input_proof.clone()], outputs),
                derived_secret_count: 1,
                fee: Amount::from(0),
                p2bk_secret_keys: None,
            },
            saga: swap_saga(operation_id),
        };
        let finalized = Finalized {
            send_proofs: Some(vec![input_proof]),
        };

        let prepared_debug = format!("{prepared:?}");
        let finalized_debug = format!("{finalized:?}");
        assert!(!prepared_debug.contains(SECRET_MARKER));
        assert!(!finalized_debug.contains(SECRET_MARKER));
    }
}
