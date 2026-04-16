//! BDK storage operations using KV store

use std::sync::Arc;

use cdk_common::database::KVStore;
use serde::de::DeserializeOwned;
use serde::Serialize;

use crate::error::Error;
use crate::receive::receive_intent::record::ReceiveIntentRecord;
use crate::send::batch_transaction::record::SendBatchRecord;
use crate::send::payment_intent::record::SendIntentRecord;

pub mod receive;
pub mod send;
mod types;

pub use types::{FinalizedReceiveIntentRecord, FinalizedSendIntentRecord};

/// Primary namespace for BDK KV store operations
pub const BDK_NAMESPACE: &str = "bdk";

/// Secondary namespace for send intents
pub const SEND_INTENT_NAMESPACE: &str = "send_intent";

/// Secondary namespace for send intent quote id index
pub const SEND_INTENT_QUOTE_ID_NAMESPACE: &str = "send_intent_quote_id";

/// Secondary namespace for send batches
pub const SEND_BATCH_NAMESPACE: &str = "send_batch";

/// Secondary namespace for finalized (confirmed) intents.
/// Stores tombstone records so `check_outgoing_payment` can return
/// correct `total_spent` after the active intent has been deleted.
pub const FINALIZED_INTENT_NAMESPACE: &str = "finalized_intent";

/// Secondary namespace for tracked receive address index (address -> quote_id)
pub const RECEIVE_ADDRESS_QUOTE_ID_NAMESPACE: &str = "receive_address_quote_id";

/// Secondary namespace for receive intents (keyed by intent_id)
pub const RECEIVE_INTENT_NAMESPACE: &str = "receive_intent";

/// Secondary namespace for receive intent outpoint index (outpoint -> intent_id)
pub const RECEIVE_INTENT_OUTPOINT_NAMESPACE: &str = "receive_intent_outpoint";

/// Secondary namespace for finalized (confirmed) receive intents.
/// Stores tombstone records so `check_incoming_payment_status` can
/// return historical data after the active intent has been deleted.
pub const FINALIZED_RECEIVE_INTENT_NAMESPACE: &str = "finalized_receive_intent";

/// Secondary namespace for finalized receive intent outpoint index (outpoint -> intent_id)
pub const FINALIZED_RECEIVE_INTENT_OUTPOINT_NAMESPACE: &str = "finalized_receive_intent_outpoint";

/// Secondary-namespace prefix for the finalized receive-intent quote-id index.
///
/// Full namespace: `finalized_receive_intent_by_quote__<quote_id>`, with
/// one key per finalized intent (`<intent_id>` → empty value). Storing
/// each intent under its own key lets `finalize_receive_intent` commit a
/// single idempotent `kv_write` instead of an RMW on a serialized list,
/// which would otherwise race under Postgres `READ COMMITTED`.
pub const FINALIZED_RECEIVE_INTENT_BY_QUOTE_NAMESPACE_PREFIX: &str =
    "finalized_receive_intent_by_quote";

/// Build the per-quote secondary namespace used to index finalized receive intents.
pub fn finalized_receive_intent_by_quote_namespace(quote_id: &str) -> String {
    format!("{FINALIZED_RECEIVE_INTENT_BY_QUOTE_NAMESPACE_PREFIX}__{quote_id}")
}

/// Secondary namespace for finalized send intent quote id index (quote_id -> intent_id)
pub const FINALIZED_SEND_INTENT_QUOTE_ID_NAMESPACE: &str = "finalized_send_intent_quote_id";

/// Encode an outpoint string for use as a KV store key.
///
/// The KV store only allows ASCII letters, numbers, underscore, and
/// hyphen. Outpoint strings contain `:` (e.g. `txid:vout`), so we
/// replace it with `-`.
fn outpoint_to_key(outpoint: &str) -> String {
    outpoint.replace(':', "-")
}

pub trait KvRecord: Serialize + DeserializeOwned + Sized {
    const NAMESPACE: &'static str;

    fn key(&self) -> String;
}

pub(crate) trait ReplaceState<S>: KvRecord {
    fn replace_state(&mut self, state: S);
}

/// BDK KV store operations
#[derive(Clone)]
pub struct BdkStorage {
    pub(crate) kv_store: Arc<dyn KVStore<Err = cdk_common::database::Error> + Send + Sync>,
}

impl BdkStorage {
    /// Create a new BdkStorage instance
    pub fn new(
        kv_store: Arc<dyn KVStore<Err = cdk_common::database::Error> + Send + Sync>,
    ) -> Self {
        Self { kv_store }
    }

    async fn put_record<T>(&self, record: &T) -> Result<(), Error>
    where
        T: KvRecord,
    {
        let serialized = serde_json::to_vec(record)?;
        let mut tx = self
            .kv_store
            .begin_transaction()
            .await
            .map_err(Error::from)?;
        tx.kv_write(BDK_NAMESPACE, T::NAMESPACE, &record.key(), &serialized)
            .await
            .map_err(Error::from)?;
        tx.commit().await.map_err(Error::from)?;
        Ok(())
    }

    async fn get_record<T>(&self, key: &str) -> Result<Option<T>, Error>
    where
        T: KvRecord,
    {
        let data = self
            .kv_store
            .kv_read(BDK_NAMESPACE, T::NAMESPACE, key)
            .await
            .map_err(Error::from)?;

        match data {
            Some(bytes) => Ok(Some(serde_json::from_slice(&bytes)?)),
            None => Ok(None),
        }
    }

    async fn list_records<T>(&self) -> Result<Vec<T>, Error>
    where
        T: KvRecord,
    {
        let keys = self
            .kv_store
            .kv_list(BDK_NAMESPACE, T::NAMESPACE)
            .await
            .map_err(Error::from)?;

        let mut records = Vec::new();
        for key in keys {
            if let Some(data) = self
                .kv_store
                .kv_read(BDK_NAMESPACE, T::NAMESPACE, &key)
                .await
                .map_err(Error::from)?
            {
                match serde_json::from_slice::<T>(&data) {
                    Ok(record) => records.push(record),
                    Err(e) => {
                        tracing::warn!("Failed to deserialize {} {}: {}", T::NAMESPACE, key, e);
                    }
                }
            }
        }

        Ok(records)
    }

    async fn delete_record<T>(&self, key: &str) -> Result<(), Error>
    where
        T: KvRecord,
    {
        let mut tx = self
            .kv_store
            .begin_transaction()
            .await
            .map_err(Error::from)?;
        tx.kv_remove(BDK_NAMESPACE, T::NAMESPACE, key)
            .await
            .map_err(Error::from)?;
        tx.commit().await.map_err(Error::from)?;
        Ok(())
    }

    async fn update_record_state<T, S>(&self, key: &str, new_state: &S) -> Result<(), Error>
    where
        T: ReplaceState<S>,
        S: Clone,
    {
        let data = self
            .kv_store
            .kv_read(BDK_NAMESPACE, T::NAMESPACE, key)
            .await
            .map_err(Error::from)?;

        let Some(bytes) = data else {
            return Err(Error::Wallet(format!(
                "Record not found in namespace {} for key {}",
                T::NAMESPACE,
                key
            )));
        };

        let mut record: T = serde_json::from_slice(&bytes)?;
        record.replace_state(new_state.clone());
        self.put_record(&record).await
    }
}

impl KvRecord for SendIntentRecord {
    const NAMESPACE: &'static str = SEND_INTENT_NAMESPACE;

    fn key(&self) -> String {
        self.intent_id.to_string()
    }
}

impl ReplaceState<crate::send::payment_intent::record::SendIntentState> for SendIntentRecord {
    fn replace_state(&mut self, state: crate::send::payment_intent::record::SendIntentState) {
        self.state = state;
    }
}

impl KvRecord for SendBatchRecord {
    const NAMESPACE: &'static str = SEND_BATCH_NAMESPACE;

    fn key(&self) -> String {
        self.batch_id.to_string()
    }
}

impl ReplaceState<crate::send::batch_transaction::record::SendBatchState> for SendBatchRecord {
    fn replace_state(&mut self, state: crate::send::batch_transaction::record::SendBatchState) {
        self.state = state;
    }
}

impl KvRecord for ReceiveIntentRecord {
    const NAMESPACE: &'static str = RECEIVE_INTENT_NAMESPACE;

    fn key(&self) -> String {
        self.intent_id.to_string()
    }
}

impl KvRecord for FinalizedSendIntentRecord {
    const NAMESPACE: &'static str = FINALIZED_INTENT_NAMESPACE;

    fn key(&self) -> String {
        self.intent_id.to_string()
    }
}

impl KvRecord for FinalizedReceiveIntentRecord {
    const NAMESPACE: &'static str = FINALIZED_RECEIVE_INTENT_NAMESPACE;

    fn key(&self) -> String {
        self.intent_id.to_string()
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use uuid::Uuid;

    use super::*;
    use crate::send::batch_transaction::record::{
        BatchOutputAssignment, SendBatchRecord, SendBatchState,
    };
    use crate::send::payment_intent::record::{SendIntentRecord, SendIntentState};
    use crate::types::{PaymentMetadata, PaymentTier};

    /// Helper: create an in-memory KVStore-backed BdkStorage for tests
    async fn test_storage() -> BdkStorage {
        let db = cdk_sqlite::mint::memory::empty()
            .await
            .expect("in-memory db");
        BdkStorage::new(Arc::new(db))
    }

    /// Helper: build a test SendIntentRecord in Pending state
    fn make_pending_intent(intent_id: Uuid) -> SendIntentRecord {
        SendIntentRecord {
            intent_id,
            quote_id: "test-quote-1".to_string(),
            address: "bcrt1qw508d6qejxtdg4y5r3zarvary0c5xw7kygt080".to_string(),
            amount_sat: 50_000,
            max_fee_amount_sat: 1_000,
            tier: PaymentTier::Immediate,
            metadata: PaymentMetadata::default(),
            state: SendIntentState::Pending {
                created_at: 1_700_000_000,
            },
        }
    }

    // ── Serialization round-trip tests ─────────────────────────────

    #[test]
    fn test_send_intent_record_state_roundtrip() {
        let batch_id = Uuid::new_v4();

        let states = vec![
            SendIntentState::Pending {
                created_at: 1_700_000_000,
            },
            SendIntentState::Batched {
                batch_id,
                created_at: 1_700_000_000,
            },
            SendIntentState::AwaitingConfirmation {
                batch_id,
                txid: "abc123def456".to_string(),
                outpoint: "abc123def456:0".to_string(),
                fee_contribution_sat: 250,
                created_at: 1_700_000_000,
            },
        ];

        for state in states {
            let json = serde_json::to_string(&state).expect("serialize state");
            let deserialized: SendIntentState =
                serde_json::from_str(&json).expect("deserialize state");

            // Re-serialize and compare JSON to verify round-trip
            let json2 = serde_json::to_string(&deserialized).expect("re-serialize state");
            assert_eq!(json, json2, "Round-trip failed for state variant");
        }

        // Also test full SendIntentRecord round-trip
        let intent = SendIntentRecord {
            intent_id: Uuid::new_v4(),
            quote_id: "quote-123".to_string(),
            address: "bcrt1qw508d6qejxtdg4y5r3zarvary0c5xw7kygt080".to_string(),
            amount_sat: 100_000,
            max_fee_amount_sat: 5_000,
            tier: PaymentTier::Standard,
            metadata: PaymentMetadata::from_optional_json(Some(r#"{"key": "value"}"#)),
            state: SendIntentState::Pending {
                created_at: 1_700_000_000,
            },
        };
        let json = serde_json::to_string(&intent).expect("serialize intent");
        let deserialized: SendIntentRecord =
            serde_json::from_str(&json).expect("deserialize intent");
        assert_eq!(intent.intent_id, deserialized.intent_id);
        assert_eq!(intent.quote_id, deserialized.quote_id);
        assert_eq!(intent.address, deserialized.address);
        assert_eq!(intent.amount_sat, deserialized.amount_sat);
        assert_eq!(intent.max_fee_amount_sat, deserialized.max_fee_amount_sat);
    }

    #[test]
    fn test_send_batch_record_state_roundtrip() {
        let intent_ids = vec![Uuid::new_v4(), Uuid::new_v4()];
        let assignments: Vec<BatchOutputAssignment> = intent_ids
            .iter()
            .enumerate()
            .map(|(idx, id)| BatchOutputAssignment {
                intent_id: *id,
                vout: idx as u32,
                fee_contribution_sat: 125,
            })
            .collect();

        let states = vec![
            SendBatchState::Built {
                psbt_bytes: vec![0x01, 0x02, 0x03, 0x04],
                intent_ids: intent_ids.clone(),
            },
            SendBatchState::Signed {
                tx_bytes: vec![0x05, 0x06, 0x07, 0x08],
                assignments: assignments.clone(),
                fee_sat: 250,
            },
            SendBatchState::Broadcast {
                txid: "deadbeef1234".to_string(),
                tx_bytes: vec![0x05, 0x06, 0x07, 0x08],
                assignments: assignments.clone(),
                fee_sat: 1000,
            },
        ];

        for state in states {
            let json = serde_json::to_string(&state).expect("serialize state");
            let deserialized: SendBatchState =
                serde_json::from_str(&json).expect("deserialize state");

            let json2 = serde_json::to_string(&deserialized).expect("re-serialize state");
            assert_eq!(json, json2, "Round-trip failed for batch state variant");
        }

        // Also test full SendBatchRecord round-trip
        let batch = SendBatchRecord {
            batch_id: Uuid::new_v4(),
            state: SendBatchState::Broadcast {
                txid: "abc".to_string(),
                tx_bytes: vec![1, 2, 3],
                assignments,
                fee_sat: 500,
            },
        };
        let json = serde_json::to_string(&batch).expect("serialize batch");
        let deserialized: SendBatchRecord = serde_json::from_str(&json).expect("deserialize batch");
        assert_eq!(batch.batch_id, deserialized.batch_id);
    }

    // ── CRUD tests for send intent storage ────────────────

    #[tokio::test]
    async fn test_send_intent_crud() {
        let storage = test_storage().await;
        let intent_id = Uuid::new_v4();
        let intent = make_pending_intent(intent_id);

        // Store
        storage
            .create_send_intent_if_absent(&intent)
            .await
            .expect("store");

        // Get
        let fetched = storage
            .get_send_intent(&intent_id)
            .await
            .expect("get")
            .expect("should exist");
        assert_eq!(fetched.intent_id, intent_id);
        assert_eq!(fetched.quote_id, "test-quote-1");
        assert_eq!(fetched.amount_sat, 50_000);
        assert!(matches!(
            fetched.state,
            SendIntentState::Pending {
                created_at: 1_700_000_000
            }
        ));

        // Update state to Batched
        let batch_id = Uuid::new_v4();
        storage
            .update_send_intent(
                &intent_id,
                &SendIntentState::Batched {
                    batch_id,
                    created_at: 1_700_000_000,
                },
            )
            .await
            .expect("update");

        let updated = storage
            .get_send_intent(&intent_id)
            .await
            .expect("get")
            .expect("should exist");
        match &updated.state {
            SendIntentState::Batched {
                batch_id: bid,
                created_at,
            } => {
                assert_eq!(*bid, batch_id);
                assert_eq!(*created_at, 1_700_000_000);
            }
            other => panic!("Expected Batched, got {:?}", other),
        }

        // get_all_send_intents
        let all = storage.get_all_send_intents().await.expect("get_all");
        assert_eq!(all.len(), 1);

        // get_pending_send_intents should now be empty (intent is Batched)
        let pending = storage
            .get_pending_send_intents()
            .await
            .expect("get_pending");
        assert!(
            pending.is_empty(),
            "Batched intent should not appear in pending"
        );

        // Revert to Pending and check pending filter
        storage
            .update_send_intent(
                &intent_id,
                &SendIntentState::Pending {
                    created_at: 1_700_000_000,
                },
            )
            .await
            .expect("revert");
        let pending = storage
            .get_pending_send_intents()
            .await
            .expect("get_pending");
        assert_eq!(pending.len(), 1);

        // Delete
        storage
            .delete_send_intent(&intent_id)
            .await
            .expect("delete");
        let gone = storage.get_send_intent(&intent_id).await.expect("get");
        assert!(gone.is_none(), "Intent should be deleted");

        // get_all should now be empty
        let all = storage.get_all_send_intents().await.expect("get_all");
        assert!(all.is_empty());
    }

    #[tokio::test]
    async fn test_create_send_intent_if_absent_rejects_duplicate_quote_id() {
        let storage = test_storage().await;
        let first = make_pending_intent(Uuid::new_v4());
        storage
            .create_send_intent_if_absent(&first)
            .await
            .expect("store first");

        let mut second = make_pending_intent(Uuid::new_v4());
        second.address = "bcrt1qother".to_string();

        let err = storage
            .create_send_intent_if_absent(&second)
            .await
            .expect_err("duplicate quote id should fail");
        assert!(matches!(err, Error::DuplicateQuoteId(_)));
    }

    /// Regression test for un-rolled-back transaction on the active-duplicate
    /// path. Prior to the fix, `create_send_intent_if_absent` returned
    /// `DuplicateQuoteId` without rolling back the open transaction, violating
    /// the `DbTransactionFinalizer` contract. This test hits the duplicate
    /// branch many times and then performs follow-up storage operations to
    /// prove the backend isn't starved of connections/locks and subsequent
    /// writes still succeed.
    #[tokio::test]
    async fn test_create_send_intent_if_absent_active_duplicate_rolls_back_tx() {
        let storage = test_storage().await;
        let first = make_pending_intent(Uuid::new_v4());
        storage
            .create_send_intent_if_absent(&first)
            .await
            .expect("store first");

        // Repeatedly trigger the active-duplicate branch. If the transaction
        // were leaked (never committed or rolled back), a pool-backed KV store
        // could eventually deadlock or return an error here.
        for _ in 0..16 {
            let mut dup = make_pending_intent(Uuid::new_v4());
            dup.address = "bcrt1qother".to_string();
            let err = storage
                .create_send_intent_if_absent(&dup)
                .await
                .expect_err("duplicate quote id should fail");
            assert!(matches!(err, Error::DuplicateQuoteId(_)));
        }

        // A follow-up write with a fresh quote id must still succeed,
        // proving the store is not wedged by a leaked transaction.
        let mut follow_up = make_pending_intent(Uuid::new_v4());
        follow_up.quote_id = "test-quote-follow-up".to_string();
        storage
            .create_send_intent_if_absent(&follow_up)
            .await
            .expect("follow-up write must succeed after duplicate rejection");

        // Sanity: the original intent is untouched.
        let original = storage
            .get_send_intent(&first.intent_id)
            .await
            .expect("get first")
            .expect("first intent should still exist");
        assert_eq!(original.intent_id, first.intent_id);
        assert_eq!(original.quote_id, first.quote_id);
    }

    /// Regression test for un-rolled-back transaction on the finalized-duplicate
    /// path. Mirrors the active-duplicate test but exercises the second
    /// early-return branch (finalized tombstone present).
    #[tokio::test]
    async fn test_create_send_intent_if_absent_finalized_duplicate_rolls_back_tx() {
        let storage = test_storage().await;
        let intent = make_pending_intent(Uuid::new_v4());
        let intent_id = intent.intent_id;
        let quote_id = intent.quote_id.clone();
        storage
            .create_send_intent_if_absent(&intent)
            .await
            .expect("store intent");

        let tombstone = FinalizedSendIntentRecord {
            intent_id,
            quote_id: quote_id.clone(),
            total_spent_sat: 50_500,
            outpoint: "txid:0".to_string(),
            finalized_at: 1_700_000_001,
        };
        storage
            .finalize_send_intent(&intent_id, &tombstone)
            .await
            .expect("finalize intent");

        // Repeatedly trigger the finalized-duplicate branch.
        for _ in 0..16 {
            let mut dup = make_pending_intent(Uuid::new_v4());
            dup.quote_id = quote_id.clone();
            let err = storage
                .create_send_intent_if_absent(&dup)
                .await
                .expect_err("finalized quote id should be rejected");
            assert!(matches!(err, Error::DuplicateQuoteId(_)));
        }

        // A follow-up write with a fresh quote id must still succeed.
        let mut follow_up = make_pending_intent(Uuid::new_v4());
        follow_up.quote_id = "test-quote-follow-up-finalized".to_string();
        storage
            .create_send_intent_if_absent(&follow_up)
            .await
            .expect("follow-up write must succeed after duplicate rejection");
    }

    #[tokio::test]
    async fn test_finalize_send_intent_removes_quote_id_index() {
        let storage = test_storage().await;
        let intent = make_pending_intent(Uuid::new_v4());
        let intent_id = intent.intent_id;
        let quote_id = intent.quote_id.clone();
        storage
            .create_send_intent_if_absent(&intent)
            .await
            .expect("store intent");

        let tombstone = FinalizedSendIntentRecord {
            intent_id,
            quote_id: quote_id.clone(),
            total_spent_sat: 50_500,
            outpoint: "txid:0".to_string(),
            finalized_at: 1_700_000_001,
        };
        storage
            .finalize_send_intent(&intent_id, &tombstone)
            .await
            .expect("finalize intent");

        assert!(storage
            .get_send_intent(&intent_id)
            .await
            .expect("get intent")
            .is_none());
        assert!(storage
            .get_send_intent_by_quote_id(&quote_id)
            .await
            .expect("lookup quote id")
            .is_none());
        assert!(storage
            .get_finalized_intent(&intent_id)
            .await
            .expect("get tombstone")
            .is_some());

        // A new intent for the SAME quote ID should NOT be allowed
        let mut second = make_pending_intent(Uuid::new_v4());
        second.quote_id = quote_id.clone();
        let err = storage
            .create_send_intent_if_absent(&second)
            .await
            .expect_err("should reject already finalized quote id");
        assert!(matches!(err, Error::DuplicateQuoteId(_)));
    }

    // ── CRUD tests for send batch storage ─────────────────

    #[tokio::test]
    async fn test_send_batch_crud() {
        let storage = test_storage().await;
        let batch_id = Uuid::new_v4();
        let intent_ids = vec![Uuid::new_v4(), Uuid::new_v4()];

        let batch = SendBatchRecord {
            batch_id,
            state: SendBatchState::Built {
                psbt_bytes: vec![0xAA, 0xBB],
                intent_ids: intent_ids.clone(),
            },
        };

        // Store
        storage.store_send_batch(&batch).await.expect("store");

        // Get
        let fetched = storage
            .get_send_batch(&batch_id)
            .await
            .expect("get")
            .expect("should exist");
        assert_eq!(fetched.batch_id, batch_id);
        match &fetched.state {
            SendBatchState::Built {
                psbt_bytes,
                intent_ids: ids,
            } => {
                assert_eq!(psbt_bytes, &vec![0xAA, 0xBB]);
                assert_eq!(ids, &intent_ids);
            }
            other => panic!("Expected Built, got {:?}", other),
        }

        // Update to Signed
        let tx_bytes = vec![0xCC, 0xDD, 0xEE];
        let assignments: Vec<BatchOutputAssignment> = intent_ids
            .iter()
            .enumerate()
            .map(|(idx, intent_id)| BatchOutputAssignment {
                intent_id: *intent_id,
                vout: idx as u32,
                fee_contribution_sat: 125,
            })
            .collect();
        storage
            .update_send_batch(
                &batch_id,
                &SendBatchState::Signed {
                    tx_bytes: tx_bytes.clone(),
                    assignments: assignments.clone(),
                    fee_sat: 250,
                },
            )
            .await
            .expect("update");

        let updated = storage
            .get_send_batch(&batch_id)
            .await
            .expect("get")
            .expect("should exist");
        match &updated.state {
            SendBatchState::Signed {
                tx_bytes: tb,
                assignments: a,
                fee_sat,
            } => {
                assert_eq!(tb, &tx_bytes);
                assert_eq!(a, &assignments);
                assert_eq!(*fee_sat, 250);
            }
            other => panic!("Expected Signed, got {:?}", other),
        }

        // Update to Broadcast
        storage
            .update_send_batch(
                &batch_id,
                &SendBatchState::Broadcast {
                    txid: "txid123".to_string(),
                    tx_bytes: tx_bytes.clone(),
                    assignments: assignments.clone(),
                    fee_sat: 400,
                },
            )
            .await
            .expect("update to broadcast");

        let broadcast = storage
            .get_send_batch(&batch_id)
            .await
            .expect("get")
            .expect("should exist");
        match &broadcast.state {
            SendBatchState::Broadcast {
                txid,
                tx_bytes: tb,
                assignments: a,
                fee_sat,
            } => {
                assert_eq!(txid, "txid123");
                assert_eq!(tb, &tx_bytes);
                assert_eq!(a, &assignments);
                assert_eq!(*fee_sat, 400);
            }
            other => panic!("Expected Broadcast, got {:?}", other),
        }

        // get_all
        let all = storage.get_all_send_batches().await.expect("get_all");
        assert_eq!(all.len(), 1);

        // Delete
        storage.delete_send_batch(&batch_id).await.expect("delete");
        let gone = storage.get_send_batch(&batch_id).await.expect("get");
        assert!(gone.is_none(), "Batch should be deleted");

        let all = storage.get_all_send_batches().await.expect("get_all");
        assert!(all.is_empty());
    }

    // ── Update non-existent records ───────────────────────

    #[tokio::test]
    async fn test_update_nonexistent_intent_returns_error() {
        let storage = test_storage().await;
        let result = storage
            .update_send_intent(
                &Uuid::new_v4(),
                &SendIntentState::Pending {
                    created_at: 1_700_000_000,
                },
            )
            .await;
        assert!(result.is_err(), "Updating nonexistent intent should fail");
    }

    #[tokio::test]
    async fn test_update_nonexistent_batch_returns_error() {
        let storage = test_storage().await;
        let result = storage
            .update_send_batch(
                &Uuid::new_v4(),
                &SendBatchState::Built {
                    psbt_bytes: vec![],
                    intent_ids: vec![],
                },
            )
            .await;
        assert!(result.is_err(), "Updating nonexistent batch should fail");
    }

    // ── Confirmation storage-level tests ──────────────────

    #[tokio::test]
    async fn test_awaiting_confirmation_intent_lookup() {
        let storage = test_storage().await;
        let batch_id = Uuid::new_v4();

        // Store one Pending and one AwaitingConfirmation intent
        let pending_id = Uuid::new_v4();
        let pending = make_pending_intent(pending_id);
        storage
            .create_send_intent_if_absent(&pending)
            .await
            .expect("store pending");

        let confirming_id = Uuid::new_v4();
        let confirming = SendIntentRecord {
            intent_id: confirming_id,
            quote_id: "quote-confirm".to_string(),
            address: "bcrt1qw508d6qejxtdg4y5r3zarvary0c5xw7kygt080".to_string(),
            amount_sat: 75_000,
            max_fee_amount_sat: 2_000,
            tier: PaymentTier::Standard,
            metadata: PaymentMetadata::default(),
            state: SendIntentState::AwaitingConfirmation {
                batch_id,
                txid: "abc123".to_string(),
                outpoint: "abc123:0".to_string(),
                fee_contribution_sat: 300,
                created_at: 1_700_000_000,
            },
        };
        storage
            .create_send_intent_if_absent(&confirming)
            .await
            .expect("store confirming");

        // get_all returns both
        let all = storage.get_all_send_intents().await.expect("get_all");
        assert_eq!(all.len(), 2);

        // get_pending only returns the Pending one
        let pending = storage
            .get_pending_send_intents()
            .await
            .expect("get_pending");
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].intent_id, pending_id);

        // Can look up AwaitingConfirmation by ID and read its fields
        let fetched = storage
            .get_send_intent(&confirming_id)
            .await
            .expect("get")
            .expect("should exist");
        match &fetched.state {
            SendIntentState::AwaitingConfirmation {
                txid,
                outpoint,
                fee_contribution_sat,
                ..
            } => {
                assert_eq!(txid, "abc123");
                assert_eq!(outpoint, "abc123:0");
                assert_eq!(*fee_contribution_sat, 300);
            }
            other => panic!("Expected AwaitingConfirmation, got {:?}", other),
        }
    }

    #[tokio::test]
    async fn test_finalize_confirmed_intent_and_cleanup_batch() {
        let storage = test_storage().await;
        let batch_id = Uuid::new_v4();
        let intent_id_1 = Uuid::new_v4();
        let intent_id_2 = Uuid::new_v4();

        // Store a Broadcast batch referencing two intents
        let batch = SendBatchRecord {
            batch_id,
            state: SendBatchState::Broadcast {
                txid: "tx123".to_string(),
                tx_bytes: vec![0x01],
                assignments: vec![
                    BatchOutputAssignment {
                        intent_id: intent_id_1,
                        vout: 0,
                        fee_contribution_sat: 250,
                    },
                    BatchOutputAssignment {
                        intent_id: intent_id_2,
                        vout: 1,
                        fee_contribution_sat: 250,
                    },
                ],
                fee_sat: 500,
            },
        };
        storage.store_send_batch(&batch).await.expect("store batch");

        // Store both intents as AwaitingConfirmation
        for (id, quote) in [(intent_id_1, "q1"), (intent_id_2, "q2")] {
            let intent = SendIntentRecord {
                intent_id: id,
                quote_id: quote.to_string(),
                address: "bcrt1qaddr".to_string(),
                amount_sat: 10_000,
                max_fee_amount_sat: 500,
                tier: PaymentTier::Immediate,
                metadata: PaymentMetadata::default(),
                state: SendIntentState::AwaitingConfirmation {
                    batch_id,
                    txid: "tx123".to_string(),
                    outpoint: format!("tx123:{}", if id == intent_id_1 { 0 } else { 1 }),
                    fee_contribution_sat: 250,
                    created_at: 1_700_000_000,
                },
            };
            storage
                .create_send_intent_if_absent(&intent)
                .await
                .expect("store intent");
        }

        // "Finalize" first intent (simulating confirmation handler)
        storage
            .delete_send_intent(&intent_id_1)
            .await
            .expect("delete first");

        // Batch should still exist -- second intent is still active
        let remaining_intents = storage.get_all_send_intents().await.expect("all intents");
        assert_eq!(remaining_intents.len(), 1);
        assert_eq!(remaining_intents[0].intent_id, intent_id_2);

        // "Finalize" second intent
        storage
            .delete_send_intent(&intent_id_2)
            .await
            .expect("delete second");

        // Now simulate cleanup_completed_batches logic:
        // Check if any intents still reference this batch
        let all_intents = storage.get_all_send_intents().await.expect("all intents");
        let batches = storage.get_all_send_batches().await.expect("all batches");
        assert_eq!(batches.len(), 1);

        let batch_intent_ids: Vec<Uuid> = match &batches[0].state {
            SendBatchState::Broadcast { assignments, .. } => {
                assignments.iter().map(|a| a.intent_id).collect()
            }
            _ => panic!("Expected Broadcast"),
        };
        let has_remaining = batch_intent_ids
            .iter()
            .any(|bid| all_intents.iter().any(|i| i.intent_id == *bid));
        assert!(!has_remaining, "All intents finalized");

        // Clean up batch
        storage
            .delete_send_batch(&batch_id)
            .await
            .expect("delete batch");
        let batches = storage.get_all_send_batches().await.expect("all batches");
        assert!(batches.is_empty());
    }

    // ── Recovery storage-level tests ─────────────────────

    #[tokio::test]
    async fn test_pre_broadcast_recovery_reverts_intents() {
        let storage = test_storage().await;
        let signed_intent_id = Uuid::new_v4();
        for state in [
            SendBatchState::Built {
                psbt_bytes: vec![0x01, 0x02],
                intent_ids: vec![Uuid::new_v4(), Uuid::new_v4()],
            },
            SendBatchState::Signed {
                tx_bytes: vec![0xAA, 0xBB],
                assignments: vec![BatchOutputAssignment {
                    intent_id: signed_intent_id,
                    vout: 0,
                    fee_contribution_sat: 100,
                }],
                fee_sat: 100,
            },
        ] {
            let batch_id = Uuid::new_v4();
            let intent_ids: Vec<Uuid> = match &state {
                SendBatchState::Built { intent_ids, .. } => intent_ids.clone(),
                SendBatchState::Signed { assignments, .. } => {
                    assignments.iter().map(|a| a.intent_id).collect()
                }
                SendBatchState::Broadcast { .. } => unreachable!(),
            };

            let batch = SendBatchRecord { batch_id, state };
            storage.store_send_batch(&batch).await.expect("store batch");

            for intent_id in &intent_ids {
                let intent = SendIntentRecord {
                    intent_id: *intent_id,
                    quote_id: format!("q-{}", intent_id),
                    address: "bcrt1qaddr".to_string(),
                    amount_sat: 25_000,
                    max_fee_amount_sat: 500,
                    tier: PaymentTier::Immediate,
                    metadata: PaymentMetadata::default(),
                    state: SendIntentState::Batched {
                        batch_id,
                        created_at: 1_700_000_000,
                    },
                };
                storage
                    .create_send_intent_if_absent(&intent)
                    .await
                    .expect("store intent");
            }

            for intent_id in &intent_ids {
                storage
                    .update_send_intent(
                        intent_id,
                        &SendIntentState::Pending {
                            created_at: 1_700_000_000,
                        },
                    )
                    .await
                    .expect("revert intent");
            }

            storage
                .delete_send_batch(&batch_id)
                .await
                .expect("delete batch");

            let batches = storage.get_all_send_batches().await.expect("all batches");
            assert!(batches.iter().all(|b| b.batch_id != batch_id));

            for intent_id in intent_ids {
                let intent = storage
                    .get_send_intent(&intent_id)
                    .await
                    .expect("get intent")
                    .expect("intent exists");
                assert!(matches!(intent.state, SendIntentState::Pending { .. }));
                storage
                    .delete_send_intent(&intent_id)
                    .await
                    .expect("cleanup intent");
            }
        }
    }

    #[tokio::test]
    async fn test_post_broadcast_and_orphaned_recovery_storage_shapes() {
        let storage = test_storage().await;
        let broadcast_batch_id = Uuid::new_v4();
        let broadcast_intent_id = Uuid::new_v4();
        let orphan_batch_id = Uuid::new_v4();
        let orphan_intent_id = Uuid::new_v4();

        let batch = SendBatchRecord {
            batch_id: broadcast_batch_id,
            state: SendBatchState::Broadcast {
                txid: "txid_broadcast".to_string(),
                tx_bytes: vec![0x01, 0x02, 0x03],
                assignments: vec![BatchOutputAssignment {
                    intent_id: broadcast_intent_id,
                    vout: 0,
                    fee_contribution_sat: 200,
                }],
                fee_sat: 200,
            },
        };
        storage.store_send_batch(&batch).await.expect("store batch");

        let awaiting_intent = SendIntentRecord {
            intent_id: broadcast_intent_id,
            quote_id: "q-broadcast".to_string(),
            address: "bcrt1qaddr".to_string(),
            amount_sat: 40_000,
            max_fee_amount_sat: 800,
            tier: PaymentTier::Economy,
            metadata: PaymentMetadata::default(),
            state: SendIntentState::AwaitingConfirmation {
                batch_id: broadcast_batch_id,
                txid: "txid_broadcast".to_string(),
                outpoint: "txid_broadcast:0".to_string(),
                fee_contribution_sat: 200,
                created_at: 1_700_000_000,
            },
        };
        storage
            .create_send_intent_if_absent(&awaiting_intent)
            .await
            .expect("store awaiting intent");

        let orphan_intent = SendIntentRecord {
            intent_id: orphan_intent_id,
            quote_id: "q-orphan".to_string(),
            address: "bcrt1qaddr".to_string(),
            amount_sat: 20_000,
            max_fee_amount_sat: 400,
            tier: PaymentTier::Immediate,
            metadata: PaymentMetadata::default(),
            state: SendIntentState::Batched {
                batch_id: orphan_batch_id,
                created_at: 1_700_000_000,
            },
        };
        storage
            .create_send_intent_if_absent(&orphan_intent)
            .await
            .expect("store orphan intent");

        let batches = storage.get_all_send_batches().await.expect("all batches");
        assert_eq!(batches.len(), 1);
        assert!(matches!(batches[0].state, SendBatchState::Broadcast { .. }));

        let awaiting = storage
            .get_send_intent(&broadcast_intent_id)
            .await
            .expect("get awaiting")
            .expect("awaiting exists");
        assert!(matches!(
            awaiting.state,
            SendIntentState::AwaitingConfirmation { .. }
        ));

        storage
            .update_send_intent(
                &orphan_intent_id,
                &SendIntentState::Pending {
                    created_at: 1_700_000_000,
                },
            )
            .await
            .expect("revert orphan");

        let orphan = storage
            .get_send_intent(&orphan_intent_id)
            .await
            .expect("get orphan")
            .expect("orphan exists");
        assert!(matches!(orphan.state, SendIntentState::Pending { .. }));
    }

    #[tokio::test]
    async fn test_recovery_shape_batch_can_reference_missing_intent() {
        let storage = test_storage().await;
        let batch_id = Uuid::new_v4();
        let present_intent_id = Uuid::new_v4();
        let missing_intent_id = Uuid::new_v4();

        let batch = SendBatchRecord {
            batch_id,
            state: SendBatchState::Built {
                psbt_bytes: vec![0x01, 0x02],
                intent_ids: vec![present_intent_id, missing_intent_id],
            },
        };
        storage.store_send_batch(&batch).await.expect("store batch");

        let intent = SendIntentRecord {
            intent_id: present_intent_id,
            quote_id: "q-present".to_string(),
            address: "bcrt1qaddr".to_string(),
            amount_sat: 25_000,
            max_fee_amount_sat: 500,
            tier: PaymentTier::Immediate,
            metadata: PaymentMetadata::default(),
            state: SendIntentState::Batched {
                batch_id,
                created_at: 1_700_000_000,
            },
        };
        storage
            .create_send_intent_if_absent(&intent)
            .await
            .expect("store present intent");

        let stored_batch = storage
            .get_send_batch(&batch_id)
            .await
            .expect("get batch")
            .expect("batch exists");
        match stored_batch.state {
            SendBatchState::Built { intent_ids, .. } => {
                assert_eq!(intent_ids.len(), 2);
                assert!(intent_ids.contains(&missing_intent_id));
            }
            _ => panic!("expected built batch"),
        }
    }

    #[tokio::test]
    async fn test_recovery_shape_intent_can_reference_missing_batch() {
        let storage = test_storage().await;
        let batch_id = Uuid::new_v4();
        let intent_id = Uuid::new_v4();

        let intent = SendIntentRecord {
            intent_id,
            quote_id: "q-missing-batch".to_string(),
            address: "bcrt1qaddr".to_string(),
            amount_sat: 15_000,
            max_fee_amount_sat: 300,
            tier: PaymentTier::Immediate,
            metadata: PaymentMetadata::default(),
            state: SendIntentState::Batched {
                batch_id,
                created_at: 1_700_000_000,
            },
        };
        storage
            .create_send_intent_if_absent(&intent)
            .await
            .expect("store intent");

        let stored = storage
            .get_send_intent(&intent_id)
            .await
            .expect("get intent")
            .expect("intent exists");
        match stored.state {
            SendIntentState::Batched {
                batch_id: stored_batch_id,
                ..
            } => {
                assert_eq!(stored_batch_id, batch_id);
            }
            _ => panic!("expected batched intent"),
        }

        assert!(
            storage
                .get_send_batch(&batch_id)
                .await
                .expect("get batch")
                .is_none(),
            "batch should be missing for orphan intent scenario"
        );
    }

    #[tokio::test]
    async fn test_recovery_shape_batch_and_intent_can_disagree_on_membership() {
        let storage = test_storage().await;
        let referenced_batch_id = Uuid::new_v4();
        let actual_batch_id = Uuid::new_v4();
        let intent_id = Uuid::new_v4();

        let batch = SendBatchRecord {
            batch_id: actual_batch_id,
            state: SendBatchState::Broadcast {
                txid: "txid_membership".to_string(),
                tx_bytes: vec![0x01, 0x02, 0x03],
                assignments: Vec::new(),
                fee_sat: 200,
            },
        };
        storage.store_send_batch(&batch).await.expect("store batch");

        let intent = SendIntentRecord {
            intent_id,
            quote_id: "q-membership".to_string(),
            address: "bcrt1qaddr".to_string(),
            amount_sat: 30_000,
            max_fee_amount_sat: 700,
            tier: PaymentTier::Standard,
            metadata: PaymentMetadata::default(),
            state: SendIntentState::AwaitingConfirmation {
                batch_id: referenced_batch_id,
                txid: "txid_membership".to_string(),
                outpoint: "txid_membership:0".to_string(),
                fee_contribution_sat: 200,
                created_at: 1_700_000_000,
            },
        };
        storage
            .create_send_intent_if_absent(&intent)
            .await
            .expect("store intent");

        let stored_batch = storage
            .get_send_batch(&actual_batch_id)
            .await
            .expect("get batch")
            .expect("batch exists");
        match stored_batch.state {
            SendBatchState::Broadcast { assignments, .. } => {
                assert!(
                    assignments.is_empty(),
                    "batch intentionally excludes the intent"
                );
            }
            _ => panic!("expected broadcast batch"),
        }

        let stored_intent = storage
            .get_send_intent(&intent_id)
            .await
            .expect("get intent")
            .expect("intent exists");
        match stored_intent.state {
            SendIntentState::AwaitingConfirmation { batch_id, .. } => {
                assert_eq!(batch_id, referenced_batch_id);
            }
            _ => panic!("expected awaiting confirmation intent"),
        }
    }

    #[tokio::test]
    async fn test_recovery_shape_batch_can_have_mixed_intent_states() {
        let storage = test_storage().await;
        let batch_id = Uuid::new_v4();
        let batched_intent_id = Uuid::new_v4();
        let awaiting_intent_id = Uuid::new_v4();

        let batch = SendBatchRecord {
            batch_id,
            state: SendBatchState::Broadcast {
                txid: "txid_mixed".to_string(),
                tx_bytes: vec![0x01, 0x02, 0x03],
                assignments: vec![
                    BatchOutputAssignment {
                        intent_id: batched_intent_id,
                        vout: 0,
                        fee_contribution_sat: 200,
                    },
                    BatchOutputAssignment {
                        intent_id: awaiting_intent_id,
                        vout: 1,
                        fee_contribution_sat: 200,
                    },
                ],
                fee_sat: 400,
            },
        };
        storage.store_send_batch(&batch).await.expect("store batch");

        for (intent_id, state) in [
            (
                batched_intent_id,
                SendIntentState::Batched {
                    batch_id,
                    created_at: 1_700_000_000,
                },
            ),
            (
                awaiting_intent_id,
                SendIntentState::AwaitingConfirmation {
                    batch_id,
                    txid: "txid_mixed".to_string(),
                    outpoint: "txid_mixed:1".to_string(),
                    fee_contribution_sat: 200,
                    created_at: 1_700_000_000,
                },
            ),
        ] {
            let intent = SendIntentRecord {
                intent_id,
                quote_id: format!("q-{}", intent_id),
                address: "bcrt1qaddr".to_string(),
                amount_sat: 10_000,
                max_fee_amount_sat: 500,
                tier: PaymentTier::Immediate,
                metadata: PaymentMetadata::default(),
                state,
            };
            storage
                .create_send_intent_if_absent(&intent)
                .await
                .expect("store intent");
        }

        let intents = storage.get_all_send_intents().await.expect("all intents");
        assert_eq!(intents.len(), 2);
        assert!(intents
            .iter()
            .any(|intent| matches!(intent.state, SendIntentState::Batched { .. })));
        assert!(intents
            .iter()
            .any(|intent| matches!(intent.state, SendIntentState::AwaitingConfirmation { .. })));
    }

    // ── Receive saga: serialization round-trip tests ─────────────────

    #[test]
    fn test_receive_intent_record_state_roundtrip() {
        use crate::receive::receive_intent::record::{ReceiveIntentRecord, ReceiveIntentState};

        let state = ReceiveIntentState::Detected {
            address: "bcrt1qaddr".to_string(),
            txid: "abc123".to_string(),
            outpoint: "abc123:0".to_string(),
            amount_sat: 50_000,
            block_height: 100,
            created_at: 1_700_000_000,
        };
        let json = serde_json::to_string(&state).expect("serialize state");
        let deserialized: ReceiveIntentState =
            serde_json::from_str(&json).expect("deserialize state");
        let json2 = serde_json::to_string(&deserialized).expect("re-serialize");
        assert_eq!(json, json2, "Round-trip failed for receive intent state");

        // Full intent round-trip
        let intent = ReceiveIntentRecord {
            intent_id: Uuid::new_v4(),
            quote_id: Uuid::new_v4().to_string(),
            state,
        };
        let json = serde_json::to_string(&intent).expect("serialize intent");
        let deserialized: ReceiveIntentRecord =
            serde_json::from_str(&json).expect("deserialize intent");
        assert_eq!(intent.intent_id, deserialized.intent_id);
        assert_eq!(intent.quote_id, deserialized.quote_id);
    }

    #[test]
    fn test_finalized_receive_intent_roundtrip() {
        let tombstone = FinalizedReceiveIntentRecord {
            intent_id: Uuid::new_v4(),
            quote_id: Uuid::new_v4().to_string(),
            address: "bcrt1qaddr".to_string(),
            txid: "abc123".to_string(),
            outpoint: "abc123:0".to_string(),
            amount_sat: 50_000,
            finalized_at: 1_700_000_001,
        };
        let json = serde_json::to_string(&tombstone).expect("serialize");
        let deserialized: FinalizedReceiveIntentRecord =
            serde_json::from_str(&json).expect("deserialize");
        assert_eq!(tombstone.intent_id, deserialized.intent_id);
        assert_eq!(tombstone.quote_id, deserialized.quote_id);
        assert_eq!(tombstone.address, deserialized.address);
        assert_eq!(tombstone.txid, deserialized.txid);
        assert_eq!(tombstone.outpoint, deserialized.outpoint);
        assert_eq!(tombstone.amount_sat, deserialized.amount_sat);
        assert_eq!(tombstone.finalized_at, deserialized.finalized_at);
    }

    // ── Receive saga: address index tests ────────────────────────────

    #[tokio::test]
    async fn test_receive_address_quote_id_index() {
        let storage = test_storage().await;

        let q1 = Uuid::new_v4().to_string();
        let q2 = Uuid::new_v4().to_string();

        storage
            .track_receive_address("bcrt1qaddr1", &q1)
            .await
            .expect("track addr1");
        storage
            .track_receive_address("bcrt1qaddr2", &q2)
            .await
            .expect("track addr2");

        let fetched = storage
            .get_quote_id_by_receive_address("bcrt1qaddr1")
            .await
            .expect("get by address")
            .expect("should exist");
        assert_eq!(fetched, q1);

        let fetched2 = storage
            .get_quote_id_by_receive_address("bcrt1qaddr2")
            .await
            .expect("get by address")
            .expect("should exist");
        assert_eq!(fetched2, q2);

        let missing = storage
            .get_quote_id_by_receive_address("unknown")
            .await
            .expect("get by address");
        assert!(missing.is_none());
    }

    // ── Receive saga: intent CRUD tests ──────────────────────────────

    #[tokio::test]
    async fn test_receive_intent_crud() {
        use crate::receive::receive_intent::record::{ReceiveIntentRecord, ReceiveIntentState};

        let storage = test_storage().await;
        let intent_id = Uuid::new_v4();
        let quote_id = Uuid::new_v4().to_string();
        let intent = ReceiveIntentRecord {
            intent_id,
            quote_id: quote_id.clone(),
            state: ReceiveIntentState::Detected {
                address: "bcrt1qaddr".to_string(),
                txid: "txid_abc".to_string(),
                outpoint: "txid_abc:0".to_string(),
                amount_sat: 50_000,
                block_height: 100,
                created_at: 1_700_000_000,
            },
        };

        // Create
        let created = storage
            .create_receive_intent_if_absent(&intent)
            .await
            .expect("create");
        assert!(created);

        // Get
        let fetched = storage
            .get_receive_intent(&intent_id)
            .await
            .expect("get")
            .expect("should exist");
        assert_eq!(fetched.intent_id, intent_id);
        assert_eq!(fetched.quote_id, quote_id);

        // Get all
        let all = storage.get_all_receive_intents().await.expect("get all");
        assert_eq!(all.len(), 1);

        // Delete
        storage
            .delete_receive_intent(&intent_id)
            .await
            .expect("delete");
        let gone = storage.get_receive_intent(&intent_id).await.expect("get");
        assert!(gone.is_none());
    }

    #[tokio::test]
    async fn test_receive_intent_duplicate_outpoint_rejection() {
        use crate::receive::receive_intent::record::{ReceiveIntentRecord, ReceiveIntentState};

        let storage = test_storage().await;

        let intent1 = ReceiveIntentRecord {
            intent_id: Uuid::new_v4(),
            quote_id: Uuid::new_v4().to_string(),
            state: ReceiveIntentState::Detected {
                address: "bcrt1qaddr".to_string(),
                txid: "txid_abc".to_string(),
                outpoint: "txid_abc:0".to_string(),
                amount_sat: 50_000,
                block_height: 100,
                created_at: 1_700_000_000,
            },
        };

        let intent2 = ReceiveIntentRecord {
            intent_id: Uuid::new_v4(),
            quote_id: Uuid::new_v4().to_string(),
            state: ReceiveIntentState::Detected {
                address: "bcrt1qaddr".to_string(),
                txid: "txid_abc".to_string(),
                outpoint: "txid_abc:0".to_string(), // same outpoint
                amount_sat: 50_000,
                block_height: 100,
                created_at: 1_700_000_001,
            },
        };

        let created1 = storage
            .create_receive_intent_if_absent(&intent1)
            .await
            .expect("create first");
        assert!(created1);

        let created2 = storage
            .create_receive_intent_if_absent(&intent2)
            .await
            .expect("create second (should not error)");
        assert!(!created2, "Duplicate outpoint should be rejected");

        // Only one intent should exist
        let all = storage.get_all_receive_intents().await.expect("get all");
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].intent_id, intent1.intent_id);
    }

    #[tokio::test]
    async fn test_finalize_receive_intent_atomicity() {
        use crate::receive::receive_intent::record::{ReceiveIntentRecord, ReceiveIntentState};

        let storage = test_storage().await;
        let intent_id = Uuid::new_v4();
        let quote_id = Uuid::new_v4().to_string();

        let intent = ReceiveIntentRecord {
            intent_id,
            quote_id: quote_id.clone(),
            state: ReceiveIntentState::Detected {
                address: "bcrt1qaddr".to_string(),
                txid: "txid_abc".to_string(),
                outpoint: "txid_abc:0".to_string(),
                amount_sat: 50_000,
                block_height: 100,
                created_at: 1_700_000_000,
            },
        };

        storage
            .create_receive_intent_if_absent(&intent)
            .await
            .expect("create");

        let tombstone = FinalizedReceiveIntentRecord {
            intent_id,
            quote_id: quote_id.clone(),
            address: "bcrt1qaddr".to_string(),
            txid: "txid_abc".to_string(),
            outpoint: "txid_abc:0".to_string(),
            amount_sat: 50_000,
            finalized_at: 1_700_000_001,
        };

        storage
            .finalize_receive_intent(&intent_id, &tombstone)
            .await
            .expect("finalize");

        // Active record should be gone
        assert!(storage
            .get_receive_intent(&intent_id)
            .await
            .expect("get")
            .is_none());

        // Tombstone should exist
        let fetched_tombstone = storage
            .get_finalized_receive_intent(&intent_id)
            .await
            .expect("get tombstone")
            .expect("tombstone should exist");
        assert_eq!(fetched_tombstone.intent_id, intent_id);
        assert_eq!(fetched_tombstone.amount_sat, 50_000);

        // Outpoint should NOT be freed (cannot create a new intent with same outpoint)
        let intent2 = ReceiveIntentRecord {
            intent_id: Uuid::new_v4(),
            quote_id: Uuid::new_v4().to_string(),
            state: ReceiveIntentState::Detected {
                address: "bcrt1qaddr".to_string(),
                txid: "txid_abc".to_string(),
                outpoint: "txid_abc:0".to_string(),
                amount_sat: 60_000,
                block_height: 200,
                created_at: 1_700_000_002,
            },
        };
        let created = storage
            .create_receive_intent_if_absent(&intent2)
            .await
            .expect("create after finalization");
        assert!(
            !created,
            "Should NOT be able to create intent after outpoint is finalized"
        );

        // A DIFFERENT outpoint for the SAME quote ID should be allowed
        let intent3 = ReceiveIntentRecord {
            intent_id: Uuid::new_v4(),
            quote_id: quote_id.clone(),
            state: ReceiveIntentState::Detected {
                address: "bcrt1qaddr".to_string(),
                txid: "txid_abc".to_string(),
                outpoint: "txid_abc:1".to_string(), // different vout
                amount_sat: 50_000,
                block_height: 100,
                created_at: 1_700_000_003,
            },
        };
        let created3 = storage
            .create_receive_intent_if_absent(&intent3)
            .await
            .expect("create different outpoint same quote");
        assert!(
            created3,
            "Should be able to create intent for a different outpoint even if same quote ID"
        );
    }

    #[tokio::test]
    async fn test_tombstone_query_by_quote_id() {
        use crate::receive::receive_intent::record::{ReceiveIntentRecord, ReceiveIntentState};

        let storage = test_storage().await;

        // Create and finalize two intents for the same quote id
        let shared_quote_id = Uuid::new_v4().to_string();
        for (i, outpoint) in ["txid_a:0", "txid_b:1"].iter().enumerate() {
            let intent_id = Uuid::new_v4();
            let intent = ReceiveIntentRecord {
                intent_id,
                quote_id: shared_quote_id.to_string(),
                state: ReceiveIntentState::Detected {
                    address: "bcrt1qshared".to_string(),
                    txid: format!("txid_{}", i),
                    outpoint: outpoint.to_string(),
                    amount_sat: 10_000 * (i as u64 + 1),
                    block_height: 100 + i as u32,
                    created_at: 1_700_000_000 + i as u64,
                },
            };
            storage
                .create_receive_intent_if_absent(&intent)
                .await
                .expect("create");

            let tombstone = FinalizedReceiveIntentRecord {
                intent_id,
                quote_id: shared_quote_id.to_string(),
                address: "bcrt1qshared".to_string(),
                txid: format!("txid_{}", i),
                outpoint: outpoint.to_string(),
                amount_sat: 10_000 * (i as u64 + 1),
                finalized_at: 1_700_000_010 + i as u64,
            };
            storage
                .finalize_receive_intent(&intent_id, &tombstone)
                .await
                .expect("finalize");
        }

        // Also create and finalize one for a different quote id
        let other_id = Uuid::new_v4();
        let other_quote_id = Uuid::new_v4().to_string();
        let other = ReceiveIntentRecord {
            intent_id: other_id,
            quote_id: other_quote_id.to_string(),
            state: ReceiveIntentState::Detected {
                address: "bcrt1qother".to_string(),
                txid: "txid_c".to_string(),
                outpoint: "txid_c:0".to_string(),
                amount_sat: 99_000,
                block_height: 300,
                created_at: 1_700_000_100,
            },
        };
        storage
            .create_receive_intent_if_absent(&other)
            .await
            .expect("create other");
        storage
            .finalize_receive_intent(
                &other_id,
                &FinalizedReceiveIntentRecord {
                    intent_id: other_id,
                    quote_id: other_quote_id.to_string(),
                    address: "bcrt1qother".to_string(),
                    txid: "txid_c".to_string(),
                    outpoint: "txid_c:0".to_string(),
                    amount_sat: 99_000,
                    finalized_at: 1_700_000_200,
                },
            )
            .await
            .expect("finalize other");

        // Query by quote id should return only matching ones
        let shared = storage
            .get_finalized_receive_intents_by_quote_id(&shared_quote_id)
            .await
            .expect("query shared");
        assert_eq!(shared.len(), 2);
        assert!(shared.iter().all(|t| t.quote_id == shared_quote_id));

        let other_results = storage
            .get_finalized_receive_intents_by_quote_id(&other_quote_id)
            .await
            .expect("query other");
        assert_eq!(other_results.len(), 1);
        assert_eq!(other_results[0].amount_sat, 99_000);

        // Unknown quote id returns empty
        let unknown = storage
            .get_finalized_receive_intents_by_quote_id("unknown")
            .await
            .expect("query unknown");
        assert!(unknown.is_empty());
    }

    /// Regression test for the quote-id index write-skew race that affected
    /// `finalize_receive_intent` on Postgres `READ COMMITTED`. SQLite
    /// serializes writers via `BEGIN IMMEDIATE`, so this test exercises the
    /// new code path rather than reproducing the bug; the structural
    /// guarantee (one key per intent, no RMW) is what actually fixes
    /// Postgres.
    #[tokio::test]
    async fn test_finalize_receive_intent_concurrent_same_quote_id() {
        use crate::receive::receive_intent::record::{ReceiveIntentRecord, ReceiveIntentState};

        let storage = test_storage().await;
        let shared_quote_id = Uuid::new_v4().to_string();

        // Pre-create two active intents under the same quote id.
        let mut intent_ids = Vec::new();
        for (i, outpoint) in ["txid_a:0", "txid_b:1"].iter().enumerate() {
            let intent_id = Uuid::new_v4();
            intent_ids.push((intent_id, outpoint.to_string(), i));
            let intent = ReceiveIntentRecord {
                intent_id,
                quote_id: shared_quote_id.to_string(),
                state: ReceiveIntentState::Detected {
                    address: "bcrt1qshared".to_string(),
                    txid: format!("txid_{}", i),
                    outpoint: outpoint.to_string(),
                    amount_sat: 10_000 * (i as u64 + 1),
                    block_height: 100 + i as u32,
                    created_at: 1_700_000_000 + i as u64,
                },
            };
            storage
                .create_receive_intent_if_absent(&intent)
                .await
                .expect("create");
        }

        // Finalize both concurrently.
        let storage_a = storage.clone();
        let quote_a = shared_quote_id.clone();
        let (intent_a, outpoint_a, i_a) = intent_ids[0].clone();
        let task_a = tokio::spawn(async move {
            let record = FinalizedReceiveIntentRecord {
                intent_id: intent_a,
                quote_id: quote_a,
                address: "bcrt1qshared".to_string(),
                txid: format!("txid_{}", i_a),
                outpoint: outpoint_a,
                amount_sat: 10_000 * (i_a as u64 + 1),
                finalized_at: 1_700_000_010 + i_a as u64,
            };
            storage_a.finalize_receive_intent(&intent_a, &record).await
        });

        let storage_b = storage.clone();
        let quote_b = shared_quote_id.clone();
        let (intent_b, outpoint_b, i_b) = intent_ids[1].clone();
        let task_b = tokio::spawn(async move {
            let record = FinalizedReceiveIntentRecord {
                intent_id: intent_b,
                quote_id: quote_b,
                address: "bcrt1qshared".to_string(),
                txid: format!("txid_{}", i_b),
                outpoint: outpoint_b,
                amount_sat: 10_000 * (i_b as u64 + 1),
                finalized_at: 1_700_000_010 + i_b as u64,
            };
            storage_b.finalize_receive_intent(&intent_b, &record).await
        });

        task_a.await.expect("join a").expect("finalize a");
        task_b.await.expect("join b").expect("finalize b");

        // Both intent_ids must be recoverable from the quote-id index.
        let results = storage
            .get_finalized_receive_intents_by_quote_id(&shared_quote_id)
            .await
            .expect("query shared");
        assert_eq!(
            results.len(),
            2,
            "both concurrently finalized intents must appear in the quote-id index"
        );
        let returned_ids: std::collections::HashSet<Uuid> =
            results.iter().map(|r| r.intent_id).collect();
        assert!(returned_ids.contains(&intent_ids[0].0));
        assert!(returned_ids.contains(&intent_ids[1].0));
    }
}
