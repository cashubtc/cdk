//! SendBatch typestate wrapper
//!
//! Represents a single Bitcoin transaction that batches one or more
//! [`SendIntent`]s. Progresses through: `Built` -> `Signed` -> `Broadcast`.
//!
//! The wrapper is internal to the crate.

pub(crate) mod compensation;
pub(crate) mod record;
pub(crate) mod state;

use uuid::Uuid;

use self::record::{BatchOutputAssignment, SendBatchRecord, SendBatchState};
use self::state::{Built, Signed};
use crate::error::Error;
use crate::send::payment_intent::state::Batched;
use crate::send::payment_intent::SendIntent;
use crate::storage::BdkStorage;

/// A send batch in a particular typestate
///
/// Each batch manages a single Bitcoin transaction that pays out one or
/// more send intents.
#[derive(Debug)]
pub(crate) struct SendBatch<S> {
    /// Unique identifier for this batch
    pub batch_id: Uuid,
    /// Intents included in this batch (in Batched state)
    pub intents: Vec<SendIntent<Batched>>,
    /// Current typestate marker enforcing valid transitions at compile time.
    _state: std::marker::PhantomData<S>,
}

/// Result of transitioning a batch to Broadcast state.
///
/// The batch releases ownership of its intents at this point,
/// since they will be transitioned independently to
/// `AwaitingConfirmation` after the actual broadcast.
pub(crate) struct BroadcastResult {
    /// The intents released from the batch, still in Batched state.
    /// The caller is responsible for transitioning each via
    /// [`SendIntent::mark_broadcast`].
    pub intents: Vec<SendIntent<Batched>>,
}

/// Allocate a batch fee across intents using equal-first distribution with
/// iterative capping.
pub(crate) fn allocate_batch_fee(
    actual_fee: u64,
    max_fees: &[u64],
    intent_ids: &[uuid::Uuid],
) -> Result<Vec<u64>, crate::error::Error> {
    let n = max_fees.len();
    if n == 0 {
        return if actual_fee == 0 {
            Ok(vec![])
        } else {
            Err(crate::error::Error::NoValidFeeAllocation)
        };
    }

    let total_max: u64 = max_fees.iter().sum();
    if actual_fee > total_max {
        return Err(crate::error::Error::BatchFeeTooHigh {
            actual_fee,
            max_fee: total_max,
        });
    }

    let mut allocations = vec![0u64; n];
    let mut remaining_fee = actual_fee;

    let mut indices: Vec<usize> = (0..n).collect();
    indices.sort_by_key(|&i| intent_ids[i]);

    let mut active: Vec<usize> = indices.clone();

    while remaining_fee > 0 && !active.is_empty() {
        let share = remaining_fee / active.len() as u64;
        let remainder = remaining_fee % active.len() as u64;

        let mut next_active = Vec::new();
        let mut used = 0u64;

        for (pos, &idx) in active.iter().enumerate() {
            let headroom = max_fees[idx].saturating_sub(allocations[idx]);
            let mut portion = share;
            if (pos as u64) < remainder {
                portion += 1;
            }
            let capped = portion.min(headroom);
            allocations[idx] += capped;
            used += capped;

            if allocations[idx] < max_fees[idx] {
                next_active.push(idx);
            }
        }

        remaining_fee -= used;
        if used == 0 {
            break;
        }
        active = next_active;
    }

    if remaining_fee > 0 {
        return Err(crate::error::Error::NoValidFeeAllocation);
    }

    Ok(allocations)
}

impl SendBatch<Built> {
    /// Create a new batch in the Built state and persist it atomically.
    pub async fn new(
        storage: &BdkStorage,
        batch_id: Uuid,
        psbt_bytes: Vec<u8>,
        intents: Vec<SendIntent<Batched>>,
    ) -> Result<Self, Error> {
        let intent_ids: Vec<Uuid> = intents.iter().map(|i| i.intent_id).collect();

        let record = SendBatchRecord {
            batch_id,
            state: SendBatchState::Built {
                psbt_bytes,
                intent_ids,
            },
        };
        storage.store_send_batch(&record).await?;

        Ok(Self {
            batch_id,
            intents,
            _state: std::marker::PhantomData,
        })
    }

    /// Reconstruct a `SendBatch<Built>` from a stored record for recovery.
    ///
    /// Does **not** persist anything — the batch already exists in storage.
    pub fn reconstruct(batch_id: Uuid, intents: Vec<SendIntent<Batched>>) -> Self {
        Self {
            batch_id,
            intents,
            _state: std::marker::PhantomData,
        }
    }

    /// Transition from Built to Signed after PSBT signing.
    ///
    /// `assignments` records the `intent_id -> vout` mapping plus per-intent
    /// fee contribution. It is computed once at build time and preserved
    /// through Broadcast so recovery never needs to re-derive it.
    pub async fn sign(
        self,
        storage: &BdkStorage,
        tx_bytes: Vec<u8>,
        assignments: Vec<BatchOutputAssignment>,
        fee_sat: u64,
    ) -> Result<SendBatch<Signed>, Error> {
        storage
            .update_send_batch(
                &self.batch_id,
                &SendBatchState::Signed {
                    tx_bytes: tx_bytes.clone(),
                    assignments,
                    fee_sat,
                },
            )
            .await?;

        Ok(SendBatch {
            batch_id: self.batch_id,
            intents: self.intents,
            _state: std::marker::PhantomData,
        })
    }
}

impl SendBatch<Signed> {
    /// Reconstruct a `SendBatch<Signed>` from a stored record for recovery.
    ///
    /// Does **not** persist anything — the batch already exists in storage.
    pub fn reconstruct(batch_id: Uuid, intents: Vec<SendIntent<Batched>>) -> Self {
        Self {
            batch_id,
            intents,
            _state: std::marker::PhantomData,
        }
    }

    /// Transition from Signed to Broadcast.
    ///
    /// Persists the Broadcast state **before** the actual network broadcast
    /// (crash safety). Returns a [`BroadcastResult`] containing the released
    /// intents.
    ///
    /// `assignments` is carried forward verbatim from the Signed state so
    /// recovery can attribute vouts and fees unambiguously.
    pub async fn mark_broadcast(
        self,
        storage: &BdkStorage,
        txid: String,
        tx_bytes: Vec<u8>,
        assignments: Vec<BatchOutputAssignment>,
        fee_sat: u64,
    ) -> Result<BroadcastResult, Error> {
        // Persist Broadcast state BEFORE actually broadcasting (crash safety)
        storage
            .update_send_batch(
                &self.batch_id,
                &SendBatchState::Broadcast {
                    txid: txid.clone(),
                    tx_bytes,
                    assignments,
                    fee_sat,
                },
            )
            .await?;

        Ok(BroadcastResult {
            intents: self.intents,
        })
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use uuid::Uuid;

    use super::*;
    use crate::send::payment_intent::state::Batched as IntentBatched;
    use crate::send::payment_intent::SendIntent;
    use crate::storage::BdkStorage;
    use crate::types::{PaymentMetadata, PaymentTier};

    /// Helper: create an in-memory KVStore-backed BdkStorage for tests
    async fn test_storage() -> BdkStorage {
        let db = cdk_sqlite::mint::memory::empty()
            .await
            .expect("in-memory db");
        BdkStorage::new(Arc::new(db))
    }

    /// Helper: create a pending intent and assign it to a batch, returning a Batched intent
    async fn create_batched_intent(
        storage: &BdkStorage,
        batch_id: Uuid,
        quote_id: &str,
        amount: u64,
        max_fee: u64,
    ) -> SendIntent<IntentBatched> {
        let pending = SendIntent::new(
            storage,
            quote_id.to_string(),
            "bcrt1qw508d6qejxtdg4y5r3zarvary0c5xw7kygt080".to_string(),
            amount,
            max_fee,
            PaymentTier::Immediate,
            PaymentMetadata::default(),
        )
        .await
        .expect("create pending intent");

        pending
            .assign_to_batch(storage, batch_id)
            .await
            .expect("assign to batch")
    }

    #[tokio::test]
    async fn test_built_to_signed_to_broadcast() {
        let storage = test_storage().await;
        let batch_id = Uuid::new_v4();

        // Create two batched intents
        let intent1 = create_batched_intent(&storage, batch_id, "q1", 10_000, 500).await;
        let intent2 = create_batched_intent(&storage, batch_id, "q2", 20_000, 800).await;

        // Create batch via constructor (persists atomically)
        let psbt_bytes = vec![0x01, 0x02, 0x03, 0x04];
        let built_batch = SendBatch::new(
            &storage,
            batch_id,
            psbt_bytes.clone(),
            vec![intent1, intent2],
        )
        .await
        .expect("new batch");

        assert_eq!(built_batch.batch_id, batch_id);
        assert_eq!(built_batch.intents.len(), 2);

        // Sign
        let tx_bytes = vec![0xAA, 0xBB, 0xCC];
        let assignments = vec![
            BatchOutputAssignment {
                intent_id: built_batch.intents[0].intent_id,
                vout: 0,
                fee_contribution_sat: 250,
            },
            BatchOutputAssignment {
                intent_id: built_batch.intents[1].intent_id,
                vout: 1,
                fee_contribution_sat: 250,
            },
        ];
        let signed_batch = built_batch
            .sign(&storage, tx_bytes.clone(), assignments.clone(), 500)
            .await
            .expect("sign");

        assert_eq!(signed_batch.intents.len(), 2);

        // Verify assignments persisted in Signed state
        let signed_record = storage
            .get_send_batch(&batch_id)
            .await
            .expect("get batch")
            .expect("batch present");
        match &signed_record.state {
            SendBatchState::Signed {
                assignments: stored,
                fee_sat,
                ..
            } => {
                assert_eq!(stored, &assignments);
                assert_eq!(*fee_sat, 500);
            }
            other => panic!("expected Signed state, got {:?}", other),
        }

        // Broadcast
        let txid = "deadbeef".to_string();
        let result = signed_batch
            .mark_broadcast(
                &storage,
                txid.clone(),
                tx_bytes.clone(),
                assignments.clone(),
                500,
            )
            .await
            .expect("mark_broadcast");

        assert_eq!(result.intents.len(), 2);

        // Verify assignments carried forward into Broadcast state
        let broadcast_record = storage
            .get_send_batch(&batch_id)
            .await
            .expect("get batch")
            .expect("batch present");
        match &broadcast_record.state {
            SendBatchState::Broadcast {
                assignments: stored,
                txid: stored_txid,
                fee_sat,
                ..
            } => {
                assert_eq!(stored, &assignments);
                assert_eq!(stored_txid, &txid);
                assert_eq!(*fee_sat, 500);
            }
            other => panic!("expected Broadcast state, got {:?}", other),
        }
    }

    #[tokio::test]
    async fn test_assignments_roundtrip_serde() {
        let assignment = BatchOutputAssignment {
            intent_id: Uuid::new_v4(),
            vout: 7,
            fee_contribution_sat: 1234,
        };
        let encoded = serde_json::to_vec(&assignment).expect("encode");
        let decoded: BatchOutputAssignment = serde_json::from_slice(&encoded).expect("decode");
        assert_eq!(decoded, assignment);
    }
}
