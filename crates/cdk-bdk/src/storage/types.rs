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

/// Persisted Payjoin v2 receive session.
#[cfg(feature = "payjoin")]
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct PayjoinReceiveSessionRecord {
    /// Quote ID linking this session to an onchain mint quote.
    pub quote_id: String,
    /// Fallback address tracked by the normal receive flow.
    pub fallback_address: String,
    /// Expected receive amount in satoshis.
    pub amount_sat: u64,
    /// Whether the payer required Payjoin for this quote.
    pub required: bool,
    /// Session expiry timestamp in unix seconds.
    pub expires_at: u64,
    /// Append-only Payjoin event history.
    pub events: Vec<payjoin::receive::v2::SessionEvent>,
    /// Whether the session reached a terminal state.
    pub closed: bool,
}

/// Persisted Payjoin v2 send session.
#[cfg(feature = "payjoin")]
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct PayjoinSendSessionRecord {
    /// Quote ID linking this session to an onchain melt quote.
    pub quote_id: String,
    /// Fallback address used if optional Payjoin negotiation fails.
    pub fallback_address: String,
    /// Payment amount in satoshis.
    pub amount_sat: u64,
    /// Maximum fee accepted by the melt quote.
    pub max_fee_sat: u64,
    /// Whether Payjoin was required by the payer.
    pub required: bool,
    /// Append-only Payjoin event history.
    pub events: Vec<payjoin::send::v2::SessionEvent>,
    /// Whether the session reached a terminal state.
    pub closed: bool,
}
