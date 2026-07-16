//! Durable send intent record types

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::types::{PaymentMetadata, PaymentTier};

/// Durable send intent state
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum SendIntentState {
    /// Intent is waiting for batch assignment
    Pending {
        /// When the intent was created (unix timestamp seconds)
        created_at: u64,
    },
    /// Intent has been claimed by the normal batch builder before transaction
    /// construction.
    BatchClaimed {
        /// The batch this intent is claimed for
        batch_id: Uuid,
        /// When the intent was created (unix timestamp seconds)
        created_at: u64,
    },
    /// Intent has been reserved by an incoming Payjoin cut-through settlement.
    CutThroughReserved {
        /// Unique token protecting conditional state transitions.
        reservation_id: Uuid,
        /// Incoming mint quote id.
        receive_quote_id: String,
        /// Original receiver amount in satoshis.
        original_receive_amount_sat: u64,
        /// When the intent was created (unix timestamp seconds)
        created_at: u64,
    },
    /// Intent is committed to a cut-through proposal which may have been exposed.
    CutThroughExposed {
        /// Unique token protecting conditional state transitions.
        reservation_id: Uuid,
        /// Incoming mint quote id.
        receive_quote_id: String,
        /// Original receiver amount in satoshis.
        original_receive_amount_sat: u64,
        /// Consensus-serialized original sender transaction.
        original_tx_bytes: Vec<u8>,
        /// Proposal transaction id.
        proposal_txid: String,
        /// Receive outpoint, also used as the receive payment id.
        receive_outpoint: String,
        /// Paid melt output point.
        melt_outpoint: String,
        /// Mint incremental spend beyond melt principal.
        fee_contribution_sat: u64,
        /// Wallet tip where an unknown confirmed spend was first observed.
        conflict_observed_height: Option<u32>,
        /// When the intent was created (unix timestamp seconds)
        created_at: u64,
    },
    /// Intent is negotiating a Payjoin transaction before a final transaction
    /// has been selected and durably staged as a batch.
    PayjoinNegotiating {
        /// Consensus-serialized signed original transaction, used as fallback.
        original_tx_bytes: Vec<u8>,
        /// Fee of the signed original transaction in satoshis.
        original_fee_sat: u64,
        /// Append-only Payjoin sender event log.
        events: Vec<payjoin::send::v2::SessionEvent>,
        /// When the intent was created (unix timestamp seconds)
        created_at: u64,
    },
    /// Intent has been assigned to a batch
    Batched {
        /// The batch this intent belongs to
        batch_id: Uuid,
        /// When the intent was created (unix timestamp seconds)
        created_at: u64,
    },
    /// Intent is tied to a durably persisted post-build batch and is awaiting
    /// confirmation or recovery reconciliation.
    AwaitingConfirmation {
        /// The batch this intent belongs to
        batch_id: Uuid,
        /// Transaction ID
        txid: String,
        /// Output point (txid:vout)
        outpoint: String,
        /// Fee contribution in satoshis
        fee_contribution_sat: u64,
        /// When the intent was created (unix timestamp seconds)
        created_at: u64,
    },
    /// Intent failed before a signed transaction was durably committed.
    Failed {
        /// Human-readable failure reason
        reason: String,
        /// When the intent was created (unix timestamp seconds)
        created_at: u64,
        /// When the intent failed (unix timestamp seconds)
        failed_at: u64,
    },
}

/// Full durable record for a send intent
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SendIntentRecord {
    /// Unique intent identifier
    pub intent_id: Uuid,
    /// Quote ID linking this intent to a melt quote
    pub quote_id: String,
    /// Destination Bitcoin address
    pub address: String,
    /// Payment amount in satoshis
    pub amount_sat: u64,
    /// Maximum fee this intent will accept in satoshis
    pub max_fee_amount_sat: u64,
    /// Batching tier
    pub tier: PaymentTier,
    /// Opaque metadata
    pub metadata: PaymentMetadata,
    /// Current state
    pub state: SendIntentState,
}
