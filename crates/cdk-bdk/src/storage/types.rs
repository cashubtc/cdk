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
    /// Stable payment identifier returned to the mint.
    #[serde(default)]
    pub payment_id: Option<String>,
    /// Payment amount in satoshis
    pub amount_sat: u64,
    /// When finalization occurred (unix timestamp seconds)
    pub finalized_at: u64,
}

/// Replay marker for a Payjoin receive session which attempted cut-through.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub enum PayjoinCutThroughProgress {
    /// The proposal was durably exposed and is owned by the named send intent.
    Active {
        /// Unique token protecting conditional state transitions.
        reservation_id: Uuid,
        /// Reserved outgoing send intent.
        send_intent_id: Uuid,
        /// Proposal transaction id.
        proposal_txid: String,
    },
    /// The proposal confirmed and both sides were finalized.
    Confirmed {
        /// Proposal transaction id.
        proposal_txid: String,
    },
    /// The proposal lost and the outgoing intent was released.
    Abandoned {
        /// Proposal transaction id.
        proposal_txid: String,
    },
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
    /// Receiver outpoints from the finalized Payjoin proposal transaction.
    #[serde(default)]
    pub proposal_receiver_outpoints: Vec<String>,
    /// Signed proposal transaction, persisted before it is applied to BDK or posted.
    #[serde(default)]
    pub proposal_tx_bytes: Option<Vec<u8>>,
    /// Cut-through replay marker for this receive proposal.
    #[serde(default)]
    pub cut_through: Option<PayjoinCutThroughProgress>,
    /// Session expiry timestamp in unix seconds.
    pub expires_at: u64,
    /// Append-only Payjoin event history.
    pub events: Vec<payjoin::receive::v2::SessionEvent>,
    /// Whether the session reached a terminal state.
    pub closed: bool,
}

impl PayjoinReceiveSessionRecord {
    /// Whether an open session has expired.
    pub fn is_expired(&self, now: u64) -> bool {
        self.expires_at < now
    }

    /// Whether a closed session has aged past the retention window.
    pub fn should_prune(&self, now: u64, retention_secs: u64) -> bool {
        self.expires_at.saturating_add(retention_secs) < now
    }
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
            #[serde(default)]
            proposal_receiver_outpoints: Vec<String>,
            proposal_tx_bytes: Option<Vec<u8>>,
            cut_through: Option<PayjoinCutThroughProgress>,
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
            proposal_receiver_outpoints: raw.proposal_receiver_outpoints,
            proposal_tx_bytes: raw.proposal_tx_bytes,
            cut_through: raw.cut_through,
            expires_at: raw.expires_at,
            events,
            closed: raw.closed || malformed_events,
        })
    }
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
        assert!(record.proposal_receiver_outpoints.is_empty());
        assert!(!record.closed);
    }
}
