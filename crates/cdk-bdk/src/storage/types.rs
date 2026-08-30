use uuid::Uuid;

/// Tombstone record for a failed pre-sign send attempt.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct FailedSendAttemptRecord {
    /// Unique attempt identifier
    pub attempt_id: Uuid,
    /// Intent identifier used by this attempt
    pub intent_id: Uuid,
    /// Quote ID linking to the melt quote
    pub quote_id: String,
    /// Human-readable failure reason
    pub reason: String,
    /// When the attempt failed (unix timestamp seconds)
    pub failed_at: u64,
}

/// Tombstone record for a finalized (confirmed) send intent.
///
/// Written when a confirmed intent is deleted, preserving the data needed
/// by `check_outgoing_payment` to return accurate `total_spent`.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct FinalizedSendIntentRecord {
    /// Unique intent identifier
    pub intent_id: Uuid,
    /// Quote ID linking to the melt quote
    pub quote_id: String,
    /// Total amount spent (payment + fee) in satoshis
    pub total_spent_sat: u64,
    /// Output point string (txid:vout)
    pub outpoint: String,
    /// When finalization occurred (unix timestamp seconds)
    pub finalized_at: u64,
}

/// Tombstone record for a finalized (confirmed) receive intent.
///
/// Written when a confirmed receive intent is deleted, preserving the
/// data needed by `check_incoming_payment_status` to return historical
/// payment information.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct FinalizedReceiveIntentRecord {
    /// Unique intent identifier
    pub intent_id: Uuid,
    /// Quote ID linking to the mint quote
    pub quote_id: String,
    /// Bitcoin address that received the payment
    pub address: String,
    /// Transaction ID of the payment
    pub txid: String,
    /// Output point string (txid:vout)
    pub outpoint: String,
    /// Payment amount in satoshis
    pub amount_sat: u64,
    /// When finalization occurred (unix timestamp seconds)
    pub finalized_at: u64,
}

/// Durable record of consecutive deterministic broadcast rejections.
///
/// Tracks how many times in a row a chain backend has definitively rejected
/// a `Broadcast` batch's transaction, so the rebroadcast loop can stop
/// hammering the backend and surface the batch for operator review instead
/// of retrying forever. Cleared on any accepted or already-known broadcast
/// outcome and when the batch record is deleted.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct BroadcastRejectionRecord {
    /// Batch whose transaction is being rejected
    pub batch_id: Uuid,
    /// Computed transaction ID that was rejected
    pub txid: String,
    /// Consecutive deterministic rejections since the last success
    pub consecutive_rejections: u32,
    /// Last rejection message from the backend
    pub last_error: String,
    /// When the last rejection was recorded (unix timestamp seconds)
    pub last_rejected_at: u64,
}
