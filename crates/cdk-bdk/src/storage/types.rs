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
#[derive(Debug, Clone, serde::Serialize)]
pub struct PayjoinReceiveSessionRecord {
    /// Quote ID linking this session to an onchain mint quote.
    pub quote_id: String,
    /// Fallback address tracked by the normal receive flow.
    pub fallback_address: String,
    /// Expected receive amount in satoshis.
    pub amount_sat: u64,
    /// Session expiry timestamp in unix seconds.
    pub expires_at: u64,
    /// Append-only Payjoin event history.
    pub events: Vec<payjoin::receive::v2::SessionEvent>,
    /// Whether the session reached a terminal state.
    pub closed: bool,
}

impl<'de> serde::Deserialize<'de> for PayjoinReceiveSessionRecord {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(serde::Deserialize)]
        struct RawPayjoinReceiveSessionRecord {
            quote_id: String,
            fallback_address: String,
            #[serde(default)]
            amount_sat: u64,
            expires_at: u64,
            #[serde(default)]
            events: Vec<serde_json::Value>,
            #[serde(default)]
            closed: bool,
        }

        let raw = RawPayjoinReceiveSessionRecord::deserialize(deserializer)?;
        let mut malformed_events = false;
        let mut events = Vec::with_capacity(raw.events.len());

        for event in raw.events {
            match serde_json::from_value(event) {
                Ok(event) => events.push(event),
                Err(_) => malformed_events = true,
            }
        }

        Ok(Self {
            quote_id: raw.quote_id,
            fallback_address: raw.fallback_address,
            amount_sat: raw.amount_sat,
            expires_at: raw.expires_at,
            events,
            closed: raw.closed || malformed_events,
        })
    }
}

/// Persisted Payjoin v2 send session.
///
/// The background send poller drives this to completion: it posts the original
/// PSBT, polls for the Payjoin proposal, and broadcasts either the Payjoin
/// transaction (on a proposal) or the signed original (on expiry/failure). The
/// record is therefore self-contained enough to resume across a restart without
/// the in-memory sender: `tier` is needed to stage the resulting send intent,
/// and `original_tx_bytes`/`original_fee_sat` let the poller broadcast the
/// fallback once the Payjoin session has expired (at which point the event log
/// can no longer be replayed).
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
    /// Fee tier used to stage the resulting send intent.
    #[serde(default)]
    pub tier: crate::types::PaymentTier,
    /// Consensus-serialized, signed original transaction, broadcast as the
    /// Payjoin fallback if negotiation expires or fails.
    #[serde(default)]
    pub original_tx_bytes: Vec<u8>,
    /// Fee of the original transaction in satoshis (used when staging the
    /// fallback, since fee cannot be recomputed from the transaction alone).
    #[serde(default)]
    pub original_fee_sat: u64,
    /// Append-only Payjoin event history.
    pub events: Vec<payjoin::send::v2::SessionEvent>,
    /// Whether the session reached a terminal state.
    pub closed: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn payjoin_receive_session_keeps_empty_event_log_open() {
        let value = serde_json::json!({
            "quote_id": "b50e819c-c136-4af9-9123-9c1e8c1dd9d2",
            "fallback_address": "bcrt1qaddr",
            "amount_sat": 0,
            "expires_at": 1_780_848_540_u64,
            "events": [],
            "closed": false
        });

        let record: PayjoinReceiveSessionRecord =
            serde_json::from_value(value).expect("record should deserialize");

        assert!(record.events.is_empty());
        assert!(!record.closed);
    }
}
