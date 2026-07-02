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

/// Durable state for a Payjoin cut-through settlement.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub enum CutThroughSettlementState {
    /// A compatible melt was reserved, but no proposal is known to be exposed.
    Reserved,
    /// A cut-through proposal may have reached the sender.
    ProposalExposed {
        /// Consensus-serialized proposal transaction.
        proposal_tx_bytes: Vec<u8>,
        /// Proposal transaction id.
        proposal_txid: String,
        /// Consensus-serialized original sender transaction.
        original_tx_bytes: Vec<u8>,
        /// Original transaction id.
        original_txid: String,
        /// Outpoint used as the receive payment id.
        receive_payment_id: String,
        /// Legacy receive outpoint field.
        receive_outpoint: String,
        /// Paid melt output point.
        melt_outpoint: String,
        /// Mint incremental spend beyond melt principal.
        fee_contribution_sat: u64,
    },
    /// Proposal confirmed and both receive and melt were finalized.
    Confirmed {
        /// When finalization occurred (unix timestamp seconds)
        finalized_at: u64,
    },
    /// Settlement was abandoned and the melt was released or finalized by another path.
    Abandoned {
        /// When abandonment occurred (unix timestamp seconds)
        abandoned_at: u64,
    },
}

/// Persisted Payjoin cut-through settlement record.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct CutThroughSettlementRecord {
    /// Unique settlement identifier.
    pub settlement_id: Uuid,
    /// Incoming mint quote id.
    pub receive_quote_id: String,
    /// Reserved outgoing send intent id.
    pub send_intent_id: Uuid,
    /// Outgoing melt quote id.
    pub send_quote_id: String,
    /// Original Payjoin receiver amount.
    pub original_receive_amount_sat: u64,
    /// Melt principal amount.
    pub melt_amount_sat: u64,
    /// Maximum fee accepted by the melt quote.
    pub max_fee_sat: u64,
    /// When the settlement was created (unix timestamp seconds).
    pub created_at: u64,
    /// Current settlement state.
    pub state: CutThroughSettlementState,
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
            #[serde(default)]
            proposal_receiver_outpoints: Vec<String>,
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
