use uuid::Uuid;

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
