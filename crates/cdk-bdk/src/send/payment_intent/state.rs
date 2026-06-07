//! Typestate markers for SendIntent state transitions

use uuid::Uuid;

/// Marker for a newly created intent awaiting batch assignment
#[derive(Debug, Clone)]
pub struct Pending;

/// Marker for an intent claimed by the normal batch builder
#[derive(Debug, Clone)]
pub struct BatchClaimed {
    /// The batch this intent is claimed for
    pub batch_id: Uuid,
}

/// Marker for an intent reserved by a Payjoin cut-through settlement
#[derive(Debug, Clone)]
pub struct CutThroughReserved {
    /// Settlement this intent is reserved for
    pub settlement_id: Uuid,
}

/// Marker for an intent negotiating a Payjoin transaction before broadcast
#[derive(Debug, Clone)]
pub struct PayjoinNegotiating {
    /// Consensus-serialized signed original transaction.
    pub original_tx_bytes: Vec<u8>,
    /// Fee of the signed original transaction in satoshis.
    pub original_fee_sat: u64,
    /// Persisted Payjoin sender event log.
    pub events: Vec<payjoin::send::v2::SessionEvent>,
}

/// Marker for an intent assigned to a batch
#[derive(Debug, Clone)]
pub struct Batched {
    /// The batch this intent belongs to
    pub batch_id: Uuid,
}

/// Marker for an intent whose batch has been broadcast
#[derive(Debug, Clone)]
pub struct AwaitingConfirmation {
    /// The batch this intent belongs to
    #[allow(dead_code)]
    pub batch_id: Uuid,
    /// Transaction ID of the broadcast transaction
    pub txid: String,
    /// The outpoint (txid:vout) for this intent's output
    pub outpoint: String,
    /// Fee allocated to this intent from the batch fee
    pub fee_contribution_sat: u64,
}

/// Marker for an intent that failed before a signed transaction was committed
#[derive(Debug, Clone)]
pub struct Failed;
