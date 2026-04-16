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
    /// Output index in the batch transaction.
    pub vout: u32,
    /// Fee allocated to this intent in satoshis.
    pub fee_contribution_sat: u64,
}

/// Durable send batch state
#[derive(Debug, Clone, Serialize, Deserialize)]
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
    /// Transaction has been durably persisted for rebroadcast and reconciliation.
    ///
    /// This state is written before the backend/node broadcast call so recovery
    /// can safely retry the network send after a crash. It does not guarantee
    /// that the transaction was already accepted by the network.
    Broadcast {
        /// Transaction ID
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
