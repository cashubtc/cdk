//! Durable send batch record types

use serde::{Deserialize, Serialize};
use uuid::Uuid;

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
        /// Intent IDs included in this batch
        intent_ids: Vec<Uuid>,
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
        /// Intent IDs included in this batch
        intent_ids: Vec<Uuid>,
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
