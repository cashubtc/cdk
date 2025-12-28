//! State types for the Melt saga.
//!
//! Each state is a distinct type that holds the data relevant to that stage
//! of the melt operation. The type state pattern ensures that only valid
//! operations are available at each stage.
//!
//! # Type State Flow
//!
//! ```text
//! Initial
//!   └─> prepare() -> Prepared
//!                      └─> request_melt_with_options() -> MeltRequested
//!                                              └─> execute() -> Finalized
//!                                                                 └─> amount(), fee(), change(), etc.
//! ```
//!
//! Note: `PaymentPending` is a persistence state in `WalletSaga`, not a typestate.
//! When payment is pending, the saga returns an error and recovery handles it later.

use cdk_common::MeltQuoteState;
use uuid::Uuid;

use crate::nuts::{BlindedMessage, PreMintSecrets, Proofs};
use crate::wallet::MeltQuote;
use crate::Amount;

/// Initial state - operation ID assigned but no work done yet.
///
/// The melt saga starts in this state. Only `prepare()` is available.
#[derive(Debug)]
pub struct Initial {
    /// Unique operation identifier for tracking and crash recovery
    pub operation_id: Uuid,
}

/// Prepared state - proofs have been selected and reserved.
///
/// After successful preparation, the saga transitions to this state.
/// From here, `request_melt_with_options()` builds the melt request and transitions to `MeltRequested`.
pub struct Prepared {
    /// Unique operation identifier
    pub operation_id: Uuid,
    /// The melt quote
    pub quote: MeltQuote,
    /// Proofs that will be used for the melt
    pub proofs: Proofs,
    /// Proofs that need to be swapped first (if any)
    pub proofs_to_swap: Proofs,
    /// Fee for the swap operation
    pub swap_fee: Amount,
    /// Input fee for the melt (after swap, on optimized proofs)
    pub input_fee: Amount,
    /// Input fee if swap is skipped (on all proofs directly)
    pub input_fee_without_swap: Amount,
}

/// MeltRequested state - melt request has been built and is ready to send.
///
/// The saga transitions to this state after building the melt request.
/// `execute()` sends the request and processes the response.
pub struct MeltRequested {
    /// Unique operation identifier
    pub operation_id: Uuid,
    /// The melt quote
    pub quote: MeltQuote,
    /// Final proofs used for the melt (after any swaps)
    pub final_proofs: Proofs,
    /// Pre-mint secrets for change
    pub premint_secrets: PreMintSecrets,
    /// Counter start for recovery
    pub counter_start: u32,
    /// Counter end for recovery
    pub counter_end: u32,
    /// Change amount (if any)
    pub change_amount: Amount,
    /// Blinded messages for change (for recovery)
    pub change_blinded_messages: Option<Vec<BlindedMessage>>,
}

/// Finalized state - melt completed successfully.
///
/// The saga transitions to this state after a successful melt.
/// Accessor methods provide the result data.
pub struct Finalized {
    /// Quote ID
    pub quote_id: String,
    /// The state of the melt quote (Paid)
    pub state: MeltQuoteState,
    /// Amount that was melted
    pub amount: Amount,
    /// Fee paid for the melt
    pub fee: Amount,
    /// Payment proof (e.g., Lightning preimage)
    pub payment_proof: Option<String>,
    /// Change proofs returned from the melt
    pub change: Option<Proofs>,
}
