//! Durable receive intent record types

use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Durable receive intent state
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ReceiveIntentState {
    /// Confirmed UTXO has been detected at a tracked address.
    Detected {
        /// Bitcoin address that received the payment
        address: String,
        /// Transaction ID containing the payment
        txid: String,
        /// Outpoint string (txid:vout) identifying the specific UTXO
        outpoint: String,
        /// Payment amount in satoshis
        amount_sat: u64,
        /// Block height at which the confirmed UTXO was detected
        block_height: u32,
        /// When the intent was created (unix timestamp seconds)
        created_at: u64,
    },
}

/// Full durable record for a receive intent
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReceiveIntentRecord {
    /// Unique intent identifier
    pub intent_id: Uuid,
    /// Quote ID linking this intent to a mint quote
    pub quote_id: String,
    /// Current state
    pub state: ReceiveIntentState,
}
