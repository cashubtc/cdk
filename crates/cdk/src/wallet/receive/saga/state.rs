//! State types for the Receive saga.
//!
//! Each state is a distinct type holding data relevant to that stage.
//! The typestate pattern ensures only valid operations are available at each stage.
//!
//! # Type State Flow
//!
//! ```text
//! Initial
//!   └─> prepare() -> Prepared
//!                      └─> execute() -> Finalized
//!                                         └─> amount(), into_amount()
//! ```

use std::collections::HashMap;

use bitcoin::XOnlyPublicKey;
use uuid::Uuid;

use crate::nuts::{Id, Proofs, SecretKey};
use crate::wallet::receive::ReceiveOptions;
use crate::Amount;

/// Initial state - operation ID assigned but no work done yet.
/// Only `prepare()` is available in this state.
#[derive(Debug)]
pub struct Initial {
    /// Unique operation identifier for tracking and crash recovery
    pub operation_id: Uuid,
}

/// Prepared state - token has been parsed and proofs extracted.
/// `execute()` is available in this state.
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
    /// P2PK signing keys (from options + wallet database lookups)
    pub p2pk_signing_keys: HashMap<XOnlyPublicKey, SecretKey>,
}

/// Finalized state - receive operation completed successfully.
/// The received amount can be retrieved from this state.
#[derive(Debug)]
pub struct Finalized {
    /// Total amount received (after fees)
    pub amount: Amount,
}
