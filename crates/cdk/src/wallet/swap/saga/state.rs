//! State types for the Swap saga.
//!
//! Each state is a distinct type that holds the data relevant to that stage
//! of the swap operation. The type state pattern ensures that only valid
//! operations are available at each stage.

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
}

/// Prepared state - swap request created, proofs reserved.
///
/// After successful preparation, the saga transitions to this state.
/// Methods available: `execute()`
#[derive(Debug)]
pub struct Prepared {
    /// Unique operation identifier
    pub operation_id: Uuid,
    /// Amount to swap (None means swap all)
    pub amount: Option<Amount>,
    /// Amount split target for output proofs
    pub amount_split_target: SplitTarget,
    /// Input proofs (already reserved)
    pub input_proofs: Proofs,
    /// Y values of input proofs (for cleanup)
    pub input_ys: Vec<PublicKey>,
    /// Spending conditions for output proofs
    pub spending_conditions: Option<SpendingConditions>,
    /// Pre-swap data (request and secrets)
    pub pre_swap: PreSwap,
    /// Fee paid for the swap
    pub fee: Amount,
    /// Counter start (for recovery)
    pub counter_start: u32,
    /// Counter end (for recovery)
    pub counter_end: u32,
}

/// Finalized state - swap completed successfully.
///
/// After successful execution, the saga transitions to this state.
/// The output proofs can be retrieved and the saga is complete.
#[derive(Debug)]
pub struct Finalized {
    /// Output proofs to send (if amount was specified)
    pub send_proofs: Option<Proofs>,
}
