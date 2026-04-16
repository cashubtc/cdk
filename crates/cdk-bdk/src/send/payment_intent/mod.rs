//! SendIntent typestate wrapper
//!
//! Represents a single outgoing on-chain payment request. Each intent
//! progresses through: `Pending` -> `Batched` -> `AwaitingConfirmation`.
//!
//! The wrapper is internal to the crate. Durable record state is the source of
//! truth for recovery, while typestate enforces valid runtime transitions.

pub(crate) mod record;
pub(crate) mod state;

use uuid::Uuid;

use self::record::{SendIntentRecord, SendIntentState};
use self::state::{AwaitingConfirmation, Batched, Pending};
use crate::error::Error;
use crate::storage::{BdkStorage, FinalizedSendIntentRecord};
use crate::types::{PaymentMetadata, PaymentTier};

/// A send intent in a particular typestate
///
/// Each intent tracks a single outgoing on-chain payment request through
/// the send saga lifecycle.
#[derive(Debug, Clone)]
pub(crate) struct SendIntent<S> {
    /// Unique identifier for this intent
    pub intent_id: Uuid,
    /// Quote ID linking this intent to a melt quote
    pub quote_id: String,
    /// Destination Bitcoin address
    pub address: String,
    /// Payment amount in satoshis
    pub amount: u64,
    /// Maximum fee this intent will accept in satoshis
    pub max_fee_amount: u64,
    /// Batching tier
    pub tier: PaymentTier,
    /// Opaque metadata
    pub metadata: PaymentMetadata,
    /// When the intent was created (unix timestamp seconds)
    pub created_at: u64,
    /// Current typestate
    pub state: S,
}

impl SendIntent<Pending> {
    /// Create a new pending send intent and persist it immediately.
    ///
    /// This is called from `make_payment()` to enqueue a new payment request.
    pub async fn new(
        storage: &BdkStorage,
        quote_id: String,
        address: String,
        amount: u64,
        max_fee_amount: u64,
        tier: PaymentTier,
        metadata: PaymentMetadata,
    ) -> Result<Self, Error> {
        let intent_id = Uuid::new_v4();
        let created_at = crate::util::unix_now();

        let record = SendIntentRecord {
            intent_id,
            quote_id: quote_id.clone(),
            address: address.clone(),
            amount_sat: amount,
            max_fee_amount_sat: max_fee_amount,
            tier,
            metadata: metadata.clone(),
            state: SendIntentState::Pending { created_at },
        };

        storage.create_send_intent_if_absent(&record).await?;

        Ok(Self {
            intent_id,
            quote_id,
            address,
            amount,
            max_fee_amount,
            tier,
            metadata,
            created_at,
            state: Pending,
        })
    }

    /// Transition to Batched state
    pub async fn assign_to_batch(
        self,
        storage: &BdkStorage,
        batch_id: Uuid,
    ) -> Result<SendIntent<Batched>, Error> {
        storage
            .update_send_intent(
                &self.intent_id,
                &SendIntentState::Batched {
                    batch_id,
                    created_at: self.created_at,
                },
            )
            .await?;

        Ok(SendIntent {
            intent_id: self.intent_id,
            quote_id: self.quote_id,
            address: self.address,
            amount: self.amount,
            max_fee_amount: self.max_fee_amount,
            tier: self.tier,
            metadata: self.metadata,
            created_at: self.created_at,
            state: Batched { batch_id },
        })
    }
}

impl SendIntent<Batched> {
    /// Transition to AwaitingConfirmation state after broadcast
    pub async fn mark_broadcast(
        self,
        storage: &BdkStorage,
        txid: String,
        outpoint: String,
        fee_contribution_sat: u64,
    ) -> Result<SendIntent<AwaitingConfirmation>, Error> {
        storage
            .update_send_intent(
                &self.intent_id,
                &SendIntentState::AwaitingConfirmation {
                    batch_id: self.state.batch_id,
                    txid: txid.clone(),
                    outpoint: outpoint.clone(),
                    fee_contribution_sat,
                    created_at: self.created_at,
                },
            )
            .await?;

        Ok(SendIntent {
            intent_id: self.intent_id,
            quote_id: self.quote_id,
            address: self.address,
            amount: self.amount,
            max_fee_amount: self.max_fee_amount,
            tier: self.tier,
            metadata: self.metadata,
            created_at: self.created_at,
            state: AwaitingConfirmation {
                batch_id: self.state.batch_id,
                txid,
                outpoint,
                fee_contribution_sat,
            },
        })
    }

    /// Revert to Pending state (compensation)
    pub async fn revert_to_pending(
        self,
        storage: &BdkStorage,
    ) -> Result<SendIntent<Pending>, Error> {
        storage
            .update_send_intent(
                &self.intent_id,
                &SendIntentState::Pending {
                    created_at: self.created_at,
                },
            )
            .await?;

        Ok(SendIntent {
            intent_id: self.intent_id,
            quote_id: self.quote_id,
            address: self.address,
            amount: self.amount,
            max_fee_amount: self.max_fee_amount,
            tier: self.tier,
            metadata: self.metadata,
            created_at: self.created_at,
            state: Pending,
        })
    }
}

impl SendIntent<AwaitingConfirmation> {
    /// Finalize a confirmed intent: write a tombstone and delete the active record.
    ///
    /// Called after the transaction reaches the required confirmation depth.
    /// The tombstone preserves `total_spent` and `outpoint` so that
    /// `check_outgoing_payment` returns correct data after the intent is gone.
    pub async fn finalize(self, storage: &BdkStorage) -> Result<(), Error> {
        let total_spent_sat = self.amount + self.state.fee_contribution_sat;

        let tombstone = FinalizedSendIntentRecord {
            intent_id: self.intent_id,
            quote_id: self.quote_id.clone(),
            total_spent_sat,
            outpoint: self.state.outpoint.clone(),
            finalized_at: crate::util::unix_now(),
        };

        storage
            .finalize_send_intent(&self.intent_id, &tombstone)
            .await?;
        Ok(())
    }
}

/// Reconstruct a `SendIntent` from a durable record for recovery
pub(crate) fn from_record(record: &SendIntentRecord) -> SendIntentAny {
    match &record.state {
        SendIntentState::Pending { created_at } => SendIntentAny::Pending(SendIntent {
            intent_id: record.intent_id,
            quote_id: record.quote_id.clone(),
            address: record.address.clone(),
            amount: record.amount_sat,
            max_fee_amount: record.max_fee_amount_sat,
            tier: record.tier,
            metadata: record.metadata.clone(),
            created_at: *created_at,
            state: Pending,
        }),
        SendIntentState::Batched {
            batch_id,
            created_at,
        } => SendIntentAny::Batched(SendIntent {
            intent_id: record.intent_id,
            quote_id: record.quote_id.clone(),
            address: record.address.clone(),
            amount: record.amount_sat,
            max_fee_amount: record.max_fee_amount_sat,
            tier: record.tier,
            metadata: record.metadata.clone(),
            created_at: *created_at,
            state: Batched {
                batch_id: *batch_id,
            },
        }),
        SendIntentState::AwaitingConfirmation {
            batch_id,
            txid,
            outpoint,
            fee_contribution_sat,
            created_at,
        } => SendIntentAny::AwaitingConfirmation(SendIntent {
            intent_id: record.intent_id,
            quote_id: record.quote_id.clone(),
            address: record.address.clone(),
            amount: record.amount_sat,
            max_fee_amount: record.max_fee_amount_sat,
            tier: record.tier,
            metadata: record.metadata.clone(),
            created_at: *created_at,
            state: AwaitingConfirmation {
                batch_id: *batch_id,
                txid: txid.clone(),
                outpoint: outpoint.clone(),
                fee_contribution_sat: *fee_contribution_sat,
            },
        }),
    }
}

/// Type-erased send intent for recovery and querying
pub(crate) enum SendIntentAny {
    /// Intent in Pending state
    Pending(SendIntent<Pending>),
    /// Intent in Batched state
    Batched(SendIntent<Batched>),
    /// Intent in AwaitingConfirmation state
    AwaitingConfirmation(SendIntent<AwaitingConfirmation>),
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use cdk_common::payment::{MakePaymentResponse, PaymentIdentifier};
    use cdk_common::{Amount, CurrencyUnit, MeltQuoteState};

    use super::*;
    use crate::storage::BdkStorage;

    /// Helper: create an in-memory KVStore-backed BdkStorage for tests
    async fn test_storage() -> BdkStorage {
        let db = cdk_sqlite::mint::memory::empty()
            .await
            .expect("in-memory db");
        BdkStorage::new(Arc::new(db))
    }

    #[tokio::test]
    async fn test_pending_to_batched_to_awaiting() {
        let storage = test_storage().await;

        let quote_id = "quote123".to_string();
        let address = "bcrt1qw508d6qejxtdg4y5r3zarvary0c5xw7kygt080".to_string();
        let amount = 10_000;
        let max_fee = 500;

        // 1. Create Pending
        let pending = SendIntent::new(
            &storage,
            quote_id.clone(),
            address.clone(),
            amount,
            max_fee,
            PaymentTier::Immediate,
            PaymentMetadata::default(),
        )
        .await
        .expect("new");

        assert_eq!(pending.amount, amount);

        // 2. Transition to Batched
        let batch_id = Uuid::new_v4();
        let batched = pending
            .assign_to_batch(&storage, batch_id)
            .await
            .expect("assign");
        assert_eq!(batched.state.batch_id, batch_id);

        // 3. Transition to AwaitingConfirmation
        let txid = "tx123".to_string();
        let outpoint = "tx123:0".to_string();
        let fee_contrib = 250;
        let awaiting = batched
            .mark_broadcast(&storage, txid.clone(), outpoint.clone(), fee_contrib)
            .await
            .expect("mark_broadcast");

        assert_eq!(awaiting.state.txid, txid);
        assert_eq!(awaiting.state.outpoint, outpoint);
        assert_eq!(awaiting.state.fee_contribution_sat, fee_contrib);
    }

    #[tokio::test]
    async fn test_finalize_send_intent_creates_tombstone_and_preserves_total_spent() {
        let storage = test_storage().await;

        let pending = SendIntent::new(
            &storage,
            "quote-finalize".to_string(),
            "bcrt1qw508d6qejxtdg4y5r3zarvary0c5xw7kygt080".to_string(),
            20_000,
            1_000,
            PaymentTier::Immediate,
            PaymentMetadata::default(),
        )
        .await
        .expect("new");

        let batched = pending
            .assign_to_batch(&storage, Uuid::new_v4())
            .await
            .expect("assign");

        let awaiting = batched
            .mark_broadcast(
                &storage,
                "txid-finalize".to_string(),
                "txid-finalize:1".to_string(),
                321,
            )
            .await
            .expect("mark_broadcast");

        let intent_id = awaiting.intent_id;
        let quote_id = awaiting.quote_id.clone();
        let outpoint = awaiting.state.outpoint.clone();

        awaiting.finalize(&storage).await.expect("finalize");

        let active = storage
            .get_send_intent(&intent_id)
            .await
            .expect("get active");
        assert!(
            active.is_none(),
            "active intent should be deleted after finalization"
        );

        let tombstone = storage
            .get_finalized_intent(&intent_id)
            .await
            .expect("get tombstone")
            .expect("tombstone should exist");

        assert_eq!(tombstone.quote_id, quote_id);
        assert_eq!(tombstone.outpoint, outpoint);
        assert_eq!(tombstone.total_spent_sat, 20_321);

        let payment_lookup_id = PaymentIdentifier::CustomId(tombstone.quote_id.clone());
        let response = MakePaymentResponse {
            payment_lookup_id,
            payment_proof: Some(tombstone.outpoint.clone()),
            status: MeltQuoteState::Paid,
            total_spent: Amount::new(tombstone.total_spent_sat, CurrencyUnit::Sat),
        };

        assert_eq!(response.status, MeltQuoteState::Paid);
        assert_eq!(response.total_spent, Amount::new(20_321, CurrencyUnit::Sat));
    }
}
