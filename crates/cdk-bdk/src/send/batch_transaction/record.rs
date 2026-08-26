//! Durable send batch record types

use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Records which transaction output (vout) was assigned to which intent at
/// batch-build time.
///
/// Written once when the batch transitions `Built -> Signed`, and preserved
/// through `Broadcast`. Recovery reads this mapping directly instead of
/// re-deriving vouts from transaction outputs, eliminating ambiguity when
/// multiple intents in the same batch target identical address+amount pairs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BatchOutputAssignment {
    /// The intent that owns this output.
    pub intent_id: Uuid,
    /// Unique generation of the send attempt that owns this output.
    ///
    /// Assignments written before attempt generations were introduced
    /// deserialize to the nil UUID. Signed-batch claims and recovery treat that
    /// sentinel as unbound and cancel conservatively rather than guessing.
    #[serde(default)]
    pub attempt_id: Uuid,
    /// Output index in the batch transaction.
    pub vout: u32,
    /// Fee allocated to this intent in satoshis.
    pub fee_contribution_sat: u64,
}

/// Durable send batch state
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum SendBatchState {
    /// PSBT has been constructed but not yet signed
    Built {
        /// Serialized PSBT bytes
        psbt_bytes: Vec<u8>,
        /// Intent IDs included in this batch
        intent_ids: Vec<Uuid>,
    },
    /// Transaction has been signed but not yet broadcast
    Signed {
        /// Serialized signed transaction bytes
        tx_bytes: Vec<u8>,
        /// Per-intent vout and fee assignments (supersedes a bare intent-id list)
        assignments: Vec<BatchOutputAssignment>,
        /// Total transaction fee in satoshis
        fee_sat: u64,
    },
    /// A signed transaction that must never be broadcast.
    ///
    /// This state is persisted before evicting the transaction from BDK or
    /// reverting any claimed intents. Recovery completes those operations
    /// idempotently and deletes the batch only after compensation succeeds.
    Cancelled {
        /// Serialized signed transaction bytes used to derive the txid to evict.
        tx_bytes: Vec<u8>,
        /// Per-intent assignments retained for compensation recovery.
        assignments: Vec<BatchOutputAssignment>,
        /// Total transaction fee in satoshis.
        fee_sat: u64,
        /// BDK eviction timestamp, chosen after the original apply timestamp.
        evict_at: u64,
    },
    /// Transaction has been durably persisted for rebroadcast and reconciliation.
    ///
    /// This state is written before the backend/node broadcast call so recovery
    /// can safely retry the network send after a crash. It does not guarantee
    /// that the transaction was already accepted by the network.
    Broadcast {
        /// Transaction ID. Informational only: consumers must derive the
        /// canonical txid from `tx_bytes` rather than trusting this string.
        txid: String,
        /// Serialized signed transaction bytes (kept for rebroadcast)
        tx_bytes: Vec<u8>,
        /// Per-intent vout and fee assignments
        assignments: Vec<BatchOutputAssignment>,
        /// Total transaction fee in satoshis
        fee_sat: u64,
    },
}

/// Full durable record for a send batch
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SendBatchRecord {
    /// Unique batch identifier
    pub batch_id: Uuid,
    /// Current state
    pub state: SendBatchState,
}
