//! Typestate markers for SendIntent state transitions

use uuid::Uuid;

/// Marker for a newly created intent awaiting batch assignment
#[derive(Debug, Clone)]
pub struct Pending;

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
