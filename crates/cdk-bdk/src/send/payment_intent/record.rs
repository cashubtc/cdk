//! Durable send intent record types

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::types::{PaymentMetadata, PaymentTier};

/// Durable send intent state
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum SendIntentState {
    /// Intent is waiting for batch assignment
    Pending {
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
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SendIntentRecord {
    /// Unique intent identifier
    pub intent_id: Uuid,
    /// Unique generation for this quote's current payment attempt.
    ///
    /// Older records deserialize to the nil UUID. A retry always replaces it
    /// with a fresh value so handles from an earlier attempt cannot pass a
    /// `Pending -> Failed -> Pending` ABA cycle.
    #[serde(default)]
    pub attempt_id: Uuid,
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
