//! State types for the Receive saga.
//!
//! Each state is a distinct type that holds the data relevant to that stage
//! of the receive operation. The type state pattern ensures that only valid
//! operations are available at each stage.
//!
//! # Type State Flow
//!
//! ```text
//! Initial
//!   └─> prepare() -> Prepared
//!                      └─> execute() -> Finalized
//!                                         └─> amount(), into_amount()
//! ```

use uuid::Uuid;

use crate::nuts::{Id, Proofs};
use crate::wallet::receive::ReceiveOptions;
use crate::Amount;

/// Initial state - operation ID assigned but no work done yet.
///
/// The receive saga starts in this state. Only `prepare()` is available.
#[derive(Debug)]
pub struct Initial {
    /// Unique operation identifier for tracking and crash recovery
    pub operation_id: Uuid,
}

/// Prepared state - token has been parsed and proofs extracted.
///
/// After successful preparation, the saga transitions to this state.
/// Methods available: `execute()`
#[derive(Debug)]
pub struct Prepared {
    /// Unique operation identifier
    pub operation_id: Uuid,
    /// Options for the receive operation
    pub options: ReceiveOptions,
    /// Memo from the token (if any)
    pub memo: Option<String>,
    /// Token string (if any)
    pub token: Option<String>,
    /// Proofs extracted from the token (potentially signed for P2PK/HTLC)
    pub proofs: Proofs,
    /// Total amount of the incoming proofs
    pub proofs_amount: Amount,
    /// Active keyset ID for the swap
    pub active_keyset_id: Id,
}

/// Finalized state - receive operation completed successfully.
///
/// After successful execution, the saga transitions to this state.
/// The received amount can be retrieved and the saga is complete.
#[derive(Debug)]
pub struct Finalized {
    /// Total amount received (after fees)
    pub amount: Amount,
}
