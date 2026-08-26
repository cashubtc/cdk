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
use self::state::{AwaitingConfirmation, Batched, Failed, Pending};
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
    /// Unique generation for the current payment attempt.
    pub attempt_id: Uuid,
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
        let attempt_id = Uuid::new_v4();
        let created_at = crate::util::unix_now();

        let record = SendIntentRecord {
            intent_id,
            attempt_id,
            quote_id: quote_id.clone(),
            address: address.clone(),
            amount_sat: amount,
            max_fee_amount_sat: max_fee_amount,
            tier,
            metadata: metadata.clone(),
            state: SendIntentState::Pending { created_at },
        };

        let record = storage.create_or_retry_failed_send_intent(&record).await?;
        let created_at = match record.state {
            SendIntentState::Pending { created_at } => created_at,
            _ => {
                return Err(Error::Wallet(
                    "send intent retry did not return Pending state".to_string(),
                ));
            }
        };

        Ok(Self {
            intent_id: record.intent_id,
            attempt_id: record.attempt_id,
            quote_id: record.quote_id,
            address: record.address,
            amount: record.amount_sat,
            max_fee_amount: record.max_fee_amount_sat,
            tier: record.tier,
            metadata: record.metadata,
            created_at,
            state: Pending,
        })
    }

    /// Transition to Batched state
    ///
    /// The transition locks the durable `Signed` batch and compare-and-sets
    /// the intent in one transaction. It fails when the batch changed or no
    /// longer lists the intent, or when the intent is no longer the expected
    /// `Pending` attempt.
    pub async fn assign_to_batch(
        self,
        storage: &BdkStorage,
        batch_id: Uuid,
    ) -> Result<SendIntent<Batched>, Error> {
        storage
            .claim_send_intent_for_signed_batch(
                &self.intent_id,
                &self.attempt_id,
                self.created_at,
                &batch_id,
            )
            .await?;

        Ok(SendIntent {
            intent_id: self.intent_id,
            attempt_id: self.attempt_id,
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

    /// Mark a pending intent as failed before a signed transaction was committed.
    ///
    /// The transition is a compare-and-set against the durable record: it
    /// fails with [`Error::SendIntentStateConflict`] when the intent is no
    /// longer stored as `Pending`, so a stale handle cannot fail an intent
    /// another worker already batched or broadcast. The failed-attempt
    /// tombstone is only written after the transition succeeds.
    pub async fn fail(
        self,
        storage: &BdkStorage,
        reason: String,
    ) -> Result<SendIntent<Failed>, Error> {
        let failed_at = crate::util::unix_now();
        let failed = storage
            .transition_send_intent(
                &self.intent_id,
                &self.attempt_id,
                &SendIntentState::Pending {
                    created_at: self.created_at,
                },
                &SendIntentState::Failed {
                    reason: reason.clone(),
                    created_at: self.created_at,
                    failed_at,
                },
            )
            .await?;
        if !failed {
            return Err(Error::SendIntentStateConflict {
                intent_id: self.intent_id,
                expected: "Pending",
            });
        }

        Ok(SendIntent {
            intent_id: self.intent_id,
            attempt_id: self.attempt_id,
            quote_id: self.quote_id,
            address: self.address,
            amount: self.amount,
            max_fee_amount: self.max_fee_amount,
            tier: self.tier,
            metadata: self.metadata,
            created_at: self.created_at,
            state: Failed,
        })
    }
}

impl SendIntent<Batched> {
    /// Transition to AwaitingConfirmation state after broadcast
    ///
    /// The transition is a compare-and-set against the durable record: it
    /// fails with [`Error::SendIntentStateConflict`] when the intent is no
    /// longer stored as `Batched` for this batch, so a stale handle cannot
    /// overwrite a compensated record or a replacement transaction.
    pub async fn mark_broadcast(
        self,
        storage: &BdkStorage,
        txid: String,
        outpoint: String,
        fee_contribution_sat: u64,
    ) -> Result<SendIntent<AwaitingConfirmation>, Error> {
        let broadcast = storage
            .transition_send_intent(
                &self.intent_id,
                &self.attempt_id,
                &SendIntentState::Batched {
                    batch_id: self.state.batch_id,
                    created_at: self.created_at,
                },
                &SendIntentState::AwaitingConfirmation {
                    batch_id: self.state.batch_id,
                    txid: txid.clone(),
                    outpoint: outpoint.clone(),
                    fee_contribution_sat,
                    created_at: self.created_at,
                },
            )
            .await?;
        if !broadcast {
            return Err(Error::SendIntentStateConflict {
                intent_id: self.intent_id,
                expected: "Batched",
            });
        }

        Ok(SendIntent {
            intent_id: self.intent_id,
            attempt_id: self.attempt_id,
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
    ///
    /// The transition is a compare-and-set against the durable record: it
    /// fails with [`Error::SendIntentStateConflict`] when the intent is no
    /// longer stored as `Batched` for this batch, so a stale handle cannot
    /// rewind an intent another worker already broadcast.
    pub async fn revert_to_pending(
        self,
        storage: &BdkStorage,
    ) -> Result<SendIntent<Pending>, Error> {
        let reverted = storage
            .transition_send_intent(
                &self.intent_id,
                &self.attempt_id,
                &SendIntentState::Batched {
                    batch_id: self.state.batch_id,
                    created_at: self.created_at,
                },
                &SendIntentState::Pending {
                    created_at: self.created_at,
                },
            )
            .await?;
        if !reverted {
            return Err(Error::SendIntentStateConflict {
                intent_id: self.intent_id,
                expected: "Batched",
            });
        }

        Ok(SendIntent {
            intent_id: self.intent_id,
            attempt_id: self.attempt_id,
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
    pub async fn finalize(self, storage: &BdkStorage) -> Result<bool, Error> {
        let total_spent_sat = self.amount + self.state.fee_contribution_sat;
        let expected = SendIntentRecord {
            intent_id: self.intent_id,
            attempt_id: self.attempt_id,
            quote_id: self.quote_id.clone(),
            address: self.address.clone(),
            amount_sat: self.amount,
            max_fee_amount_sat: self.max_fee_amount,
            tier: self.tier,
            metadata: self.metadata.clone(),
            state: SendIntentState::AwaitingConfirmation {
                batch_id: self.state.batch_id,
                txid: self.state.txid.clone(),
                outpoint: self.state.outpoint.clone(),
                fee_contribution_sat: self.state.fee_contribution_sat,
                created_at: self.created_at,
            },
        };

        let tombstone = FinalizedSendIntentRecord {
            intent_id: self.intent_id,
            quote_id: self.quote_id.clone(),
            total_spent_sat,
            outpoint: self.state.outpoint.clone(),
            finalized_at: crate::util::unix_now(),
        };

        storage.finalize_send_intent(&expected, &tombstone).await
    }
}

/// Reconstruct a `SendIntent` from a durable record for recovery
pub(crate) fn from_record(record: &SendIntentRecord) -> SendIntentAny {
    match &record.state {
        SendIntentState::Pending { created_at } => SendIntentAny::Pending(SendIntent {
            intent_id: record.intent_id,
            attempt_id: record.attempt_id,
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
            attempt_id: record.attempt_id,
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
            attempt_id: record.attempt_id,
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
        SendIntentState::Failed {
            reason,
            created_at,
            failed_at,
        } => {
            let _ = (reason, created_at, failed_at);
            SendIntentAny::Failed
        }
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
    /// Intent in Failed state
    Failed,
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::time::Duration;

    use cdk_common::payment::{MakePaymentResponse, PaymentIdentifier};
    use cdk_common::{Amount, CurrencyUnit, MeltQuoteState};

    use super::*;
    use crate::send::batch_transaction::record::{
        BatchOutputAssignment, SendBatchRecord, SendBatchState,
    };
    use crate::storage::{BdkStorage, BDK_NAMESPACE, SEND_INTENT_QUOTE_ID_NAMESPACE};
    use crate::testutil::{store_test_signed_batch, GatedKvStore, PausePoint, ReadPath};

    /// Helper: create an in-memory KVStore-backed BdkStorage for tests
    async fn test_storage() -> BdkStorage {
        let db = cdk_sqlite::mint::memory::empty()
            .await
            .expect("in-memory db");
        BdkStorage::new(Arc::new(db))
    }

    #[tokio::test]
    async fn concurrent_duplicate_quote_creates_only_one_intent() {
        let kv = GatedKvStore::default();
        let storage = BdkStorage::new(Arc::new(kv.clone()));
        let gate = kv.gate_read(
            ReadPath::Transaction,
            PausePoint::AfterRead,
            BDK_NAMESPACE,
            SEND_INTENT_QUOTE_ID_NAMESPACE,
            1,
        );

        let first_storage = storage.clone();
        let first = tokio::spawn(async move {
            SendIntent::new(
                &first_storage,
                "quote-race".to_string(),
                "bcrt1qw508d6qejxtdg4y5r3zarvary0c5xw7kygt080".to_string(),
                10_000,
                500,
                PaymentTier::Immediate,
                PaymentMetadata::default(),
            )
            .await
        });

        tokio::time::timeout(Duration::from_secs(5), gate.wait_entered())
            .await
            .expect("first create reached the gated quote-id read");

        let second = SendIntent::new(
            &storage,
            "quote-race".to_string(),
            "bcrt1qw508d6qejxtdg4y5r3zarvary0c5xw7kygt080".to_string(),
            10_000,
            500,
            PaymentTier::Immediate,
            PaymentMetadata::default(),
        )
        .await;

        gate.release();
        let first = tokio::time::timeout(Duration::from_secs(5), first)
            .await
            .expect("first create timed out")
            .expect("join first create");

        let success_count = usize::from(first.is_ok()) + usize::from(second.is_ok());
        assert_eq!(success_count, 1, "only one intent per quote id");

        let duplicate_count = [&first, &second]
            .iter()
            .filter(
                |result| matches!(result, Err(Error::DuplicateQuoteId(id)) if id == "quote-race"),
            )
            .count();
        assert_eq!(duplicate_count, 1, "loser must get DuplicateQuoteId");

        let records = storage
            .get_all_send_intents()
            .await
            .expect("list send intents");
        assert_eq!(records.len(), 1, "only one intent record may be persisted");
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
        store_test_signed_batch(&storage, batch_id, &[pending.intent_id]).await;
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
    async fn test_pending_to_failed() {
        let storage = test_storage().await;

        let pending = SendIntent::new(
            &storage,
            "quote-failed".to_string(),
            "bcrt1qw508d6qejxtdg4y5r3zarvary0c5xw7kygt080".to_string(),
            10_000,
            500,
            PaymentTier::Immediate,
            PaymentMetadata::default(),
        )
        .await
        .expect("new");

        let intent_id = pending.intent_id;
        let failed = pending
            .fail(&storage, "fee too high".to_string())
            .await
            .expect("fail");

        assert_eq!(failed.intent_id, intent_id);

        let persisted = storage
            .get_send_intent(&intent_id)
            .await
            .expect("get intent")
            .expect("intent should remain as failed terminal record");
        assert!(matches!(
            persisted.state,
            SendIntentState::Failed { ref reason, .. } if reason == "fee too high"
        ));

        let attempts = storage
            .get_failed_send_attempts_by_quote_id("quote-failed")
            .await
            .expect("failed attempts");
        assert_eq!(attempts.len(), 1);
        assert_eq!(attempts[0].intent_id, intent_id);
        assert_eq!(attempts[0].reason, "fee too high");
    }

    #[tokio::test]
    async fn test_failed_intent_can_be_requeued_with_same_quote_id() {
        let storage = test_storage().await;
        let quote_id = "quote-retry".to_string();

        let pending = SendIntent::new(
            &storage,
            quote_id.clone(),
            "bcrt1qw508d6qejxtdg4y5r3zarvary0c5xw7kygt080".to_string(),
            10_000,
            500,
            PaymentTier::Immediate,
            PaymentMetadata::default(),
        )
        .await
        .expect("new");
        let intent_id = pending.intent_id;
        let attempt_id = pending.attempt_id;

        pending
            .fail(&storage, "fee too high".to_string())
            .await
            .expect("fail");

        let retried = SendIntent::new(
            &storage,
            quote_id,
            "bcrt1qw508d6qejxtdg4y5r3zarvary0c5xw7kygt080".to_string(),
            10_000,
            750,
            PaymentTier::Immediate,
            PaymentMetadata::default(),
        )
        .await
        .expect("retry failed intent");

        assert_eq!(retried.intent_id, intent_id);
        assert_ne!(retried.attempt_id, attempt_id);
        assert_eq!(retried.max_fee_amount, 750);

        let persisted = storage
            .get_send_intent(&intent_id)
            .await
            .expect("get intent")
            .expect("intent should remain present");
        assert!(matches!(persisted.state, SendIntentState::Pending { .. }));
        assert_eq!(persisted.max_fee_amount_sat, 750);

        let attempts = storage
            .get_failed_send_attempts_by_quote_id("quote-retry")
            .await
            .expect("failed attempts");
        assert_eq!(attempts.len(), 1);
        assert_eq!(attempts[0].intent_id, intent_id);
    }

    #[tokio::test]
    async fn stale_pending_handle_cannot_advance_retried_attempt_with_same_timestamp() {
        let storage = test_storage().await;
        let quote_id = "quote-retry-aba".to_string();

        let pending = SendIntent::new(
            &storage,
            quote_id.clone(),
            "bcrt1qw508d6qejxtdg4y5r3zarvary0c5xw7kygt080".to_string(),
            10_000,
            500,
            PaymentTier::Immediate,
            PaymentMetadata::default(),
        )
        .await
        .expect("new");
        let stale = pending.clone();
        pending
            .fail(&storage, "first attempt failed".to_string())
            .await
            .expect("fail first attempt");

        let replacement_attempt_id = Uuid::new_v4();
        let replacement = storage
            .create_or_retry_failed_send_intent(&SendIntentRecord {
                intent_id: Uuid::new_v4(),
                attempt_id: replacement_attempt_id,
                quote_id,
                address: "bcrt1q6rhpng9evdsfnn833a4f4vej0asu6dk5srld6x".to_string(),
                amount_sat: 20_000,
                max_fee_amount_sat: 750,
                tier: PaymentTier::Standard,
                metadata: PaymentMetadata::default(),
                state: SendIntentState::Pending {
                    created_at: stale.created_at,
                },
            })
            .await
            .expect("retry with the same state timestamp");
        assert_eq!(replacement.attempt_id, replacement_attempt_id);

        let stale_batch_id = Uuid::new_v4();
        store_test_signed_batch(&storage, stale_batch_id, &[replacement.intent_id]).await;
        let error = stale
            .assign_to_batch(&storage, stale_batch_id)
            .await
            .expect_err("stale handle must not advance the replacement attempt");
        assert!(matches!(
            error,
            Error::SendBatchStateConflict {
                expected: "Signed with one assignment for exact intent attempt",
                ..
            }
        ));

        let persisted = storage
            .get_send_intent(&replacement.intent_id)
            .await
            .expect("get replacement intent")
            .expect("replacement intent remains active");
        assert_eq!(persisted.attempt_id, replacement_attempt_id);
        assert_eq!(persisted.address, replacement.address);
        assert!(matches!(persisted.state, SendIntentState::Pending { .. }));
    }

    #[tokio::test]
    async fn replacement_attempt_cannot_claim_original_attempts_signed_batch() {
        let storage = test_storage().await;
        let quote_id = "quote-signed-attempt-binding".to_string();
        let original = SendIntent::new(
            &storage,
            quote_id.clone(),
            "bcrt1qw508d6qejxtdg4y5r3zarvary0c5xw7kygt080".to_string(),
            10_000,
            500,
            PaymentTier::Immediate,
            PaymentMetadata::default(),
        )
        .await
        .expect("new original attempt");
        let intent_id = original.intent_id;
        let original_attempt_id = original.attempt_id;
        original
            .fail(&storage, "retry original attempt".to_string())
            .await
            .expect("fail original attempt");
        let replacement = SendIntent::new(
            &storage,
            quote_id,
            "bcrt1qw508d6qejxtdg4y5r3zarvary0c5xw7kygt080".to_string(),
            10_000,
            750,
            PaymentTier::Immediate,
            PaymentMetadata::default(),
        )
        .await
        .expect("new replacement attempt");
        assert_eq!(replacement.intent_id, intent_id);
        assert_ne!(replacement.attempt_id, original_attempt_id);

        let batch_id = Uuid::new_v4();
        storage
            .store_send_batch(&SendBatchRecord {
                batch_id,
                state: SendBatchState::Signed {
                    tx_bytes: vec![0x01],
                    assignments: vec![BatchOutputAssignment {
                        intent_id,
                        attempt_id: original_attempt_id,
                        vout: 0,
                        fee_contribution_sat: 0,
                    }],
                    fee_sat: 0,
                },
            })
            .await
            .expect("store original attempt's signed batch");

        let replacement_attempt_id = replacement.attempt_id;
        let error = replacement
            .assign_to_batch(&storage, batch_id)
            .await
            .expect_err("replacement must not claim original attempt's signed batch");
        assert!(matches!(
            error,
            Error::SendBatchStateConflict {
                expected: "Signed with one assignment for exact intent attempt",
                ..
            }
        ));

        let persisted = storage
            .get_send_intent(&intent_id)
            .await
            .expect("get replacement")
            .expect("replacement remains");
        assert_eq!(persisted.attempt_id, replacement_attempt_id);
        assert!(matches!(persisted.state, SendIntentState::Pending { .. }));
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

        let batch_id = Uuid::new_v4();
        store_test_signed_batch(&storage, batch_id, &[pending.intent_id]).await;
        let batched = pending
            .assign_to_batch(&storage, batch_id)
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

        assert!(
            awaiting.finalize(&storage).await.expect("finalize"),
            "active intent should be finalized"
        );

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

    #[tokio::test]
    async fn test_finalized_intent_quote_id_cannot_be_requeued() {
        let storage = test_storage().await;
        let quote_id = "quote-finalized-no-retry".to_string();

        let pending = SendIntent::new(
            &storage,
            quote_id.clone(),
            "bcrt1qw508d6qejxtdg4y5r3zarvary0c5xw7kygt080".to_string(),
            20_000,
            1_000,
            PaymentTier::Immediate,
            PaymentMetadata::default(),
        )
        .await
        .expect("new");

        let batch_id = Uuid::new_v4();
        store_test_signed_batch(&storage, batch_id, &[pending.intent_id]).await;
        let awaiting = pending
            .assign_to_batch(&storage, batch_id)
            .await
            .expect("assign")
            .mark_broadcast(
                &storage,
                "txid-finalized-no-retry".to_string(),
                "txid-finalized-no-retry:0".to_string(),
                250,
            )
            .await
            .expect("mark_broadcast");

        awaiting.finalize(&storage).await.expect("finalize");

        let result = SendIntent::new(
            &storage,
            quote_id.clone(),
            "bcrt1qw508d6qejxtdg4y5r3zarvary0c5xw7kygt080".to_string(),
            20_000,
            1_000,
            PaymentTier::Immediate,
            PaymentMetadata::default(),
        )
        .await;

        assert!(matches!(result, Err(Error::DuplicateQuoteId(id)) if id == quote_id));
    }

    #[tokio::test]
    async fn stale_pending_handle_cannot_fail_a_batched_intent() {
        let storage = test_storage().await;
        let quote_id = "quote-stale-fail".to_string();

        let pending = SendIntent::new(
            &storage,
            quote_id.clone(),
            "bcrt1qw508d6qejxtdg4y5r3zarvary0c5xw7kygt080".to_string(),
            10_000,
            500,
            PaymentTier::Immediate,
            PaymentMetadata::default(),
        )
        .await
        .expect("new");
        let stale = pending.clone();
        let intent_id = pending.intent_id;

        let batch_id = Uuid::new_v4();
        store_test_signed_batch(&storage, batch_id, &[intent_id]).await;
        pending
            .assign_to_batch(&storage, batch_id)
            .await
            .expect("assign");

        let err = stale
            .fail(&storage, "stale failure".to_string())
            .await
            .expect_err("stale handle must not fail a batched intent");
        assert!(matches!(
            err,
            Error::SendIntentStateConflict {
                intent_id: id,
                expected: "Pending",
            } if id == intent_id
        ));

        let persisted = storage
            .get_send_intent(&intent_id)
            .await
            .expect("get intent")
            .expect("intent present");
        assert!(
            matches!(persisted.state, SendIntentState::Batched { batch_id: b, .. } if b == batch_id),
            "durable state must remain Batched, got {:?}",
            persisted.state
        );

        let attempts = storage
            .get_failed_send_attempts_by_quote_id(&quote_id)
            .await
            .expect("failed attempts");
        assert!(attempts.is_empty());
    }

    #[tokio::test]
    async fn stale_pending_handle_cannot_batch_a_failed_intent() {
        let storage = test_storage().await;
        let quote_id = "quote-stale-batch".to_string();

        let pending = SendIntent::new(
            &storage,
            quote_id,
            "bcrt1qw508d6qejxtdg4y5r3zarvary0c5xw7kygt080".to_string(),
            10_000,
            500,
            PaymentTier::Immediate,
            PaymentMetadata::default(),
        )
        .await
        .expect("new");
        let stale = pending.clone();
        let intent_id = pending.intent_id;

        pending
            .fail(&storage, "fee too high".to_string())
            .await
            .expect("fail");

        let batch_id = Uuid::new_v4();
        store_test_signed_batch(&storage, batch_id, &[intent_id]).await;
        let err = stale
            .assign_to_batch(&storage, batch_id)
            .await
            .expect_err("stale handle must not batch a failed intent");
        assert!(matches!(
            err,
            Error::SendIntentStateConflict {
                intent_id: id,
                expected: "Pending",
            } if id == intent_id
        ));

        let persisted = storage
            .get_send_intent(&intent_id)
            .await
            .expect("get intent")
            .expect("intent present");
        assert!(
            matches!(persisted.state, SendIntentState::Failed { .. }),
            "durable state must remain Failed, got {:?}",
            persisted.state
        );
    }

    #[tokio::test]
    async fn stale_batched_handle_cannot_broadcast_a_reverted_intent() {
        let storage = test_storage().await;

        let pending = SendIntent::new(
            &storage,
            "quote-stale-broadcast".to_string(),
            "bcrt1qw508d6qejxtdg4y5r3zarvary0c5xw7kygt080".to_string(),
            10_000,
            500,
            PaymentTier::Immediate,
            PaymentMetadata::default(),
        )
        .await
        .expect("new");
        let batch_id = Uuid::new_v4();
        store_test_signed_batch(&storage, batch_id, &[pending.intent_id]).await;
        let batched = pending
            .assign_to_batch(&storage, batch_id)
            .await
            .expect("assign");
        let stale = batched.clone();
        let intent_id = batched.intent_id;

        batched
            .revert_to_pending(&storage)
            .await
            .expect("revert to pending");

        let err = stale
            .mark_broadcast(
                &storage,
                "txid-stale".to_string(),
                "txid-stale:0".to_string(),
                250,
            )
            .await
            .expect_err("stale handle must not broadcast a reverted intent");
        assert!(matches!(
            err,
            Error::SendIntentStateConflict {
                intent_id: id,
                expected: "Batched",
            } if id == intent_id
        ));

        let persisted = storage
            .get_send_intent(&intent_id)
            .await
            .expect("get intent")
            .expect("intent present");
        assert!(
            matches!(persisted.state, SendIntentState::Pending { .. }),
            "durable state must remain Pending, got {:?}",
            persisted.state
        );
        assert!(
            storage
                .get_quote_id_by_send_outpoint("txid-stale:0")
                .await
                .expect("outpoint lookup")
                .is_none(),
            "no outpoint index entry may be written for a rejected transition"
        );
    }

    #[tokio::test]
    async fn stale_batched_handle_cannot_revert_a_broadcast_intent() {
        let storage = test_storage().await;

        let pending = SendIntent::new(
            &storage,
            "quote-stale-revert".to_string(),
            "bcrt1qw508d6qejxtdg4y5r3zarvary0c5xw7kygt080".to_string(),
            10_000,
            500,
            PaymentTier::Immediate,
            PaymentMetadata::default(),
        )
        .await
        .expect("new");
        let batch_id = Uuid::new_v4();
        store_test_signed_batch(&storage, batch_id, &[pending.intent_id]).await;
        let batched = pending
            .assign_to_batch(&storage, batch_id)
            .await
            .expect("assign");
        let stale = batched.clone();
        let intent_id = batched.intent_id;

        batched
            .mark_broadcast(
                &storage,
                "txid-live".to_string(),
                "txid-live:0".to_string(),
                250,
            )
            .await
            .expect("mark broadcast");

        let err = stale
            .revert_to_pending(&storage)
            .await
            .expect_err("stale handle must not revert a broadcast intent");
        assert!(matches!(
            err,
            Error::SendIntentStateConflict {
                intent_id: id,
                expected: "Batched",
            } if id == intent_id
        ));

        let persisted = storage
            .get_send_intent(&intent_id)
            .await
            .expect("get intent")
            .expect("intent present");
        assert!(
            matches!(
                persisted.state,
                SendIntentState::AwaitingConfirmation {
                    batch_id: b,
                    ref txid,
                    ..
                } if b == batch_id && txid == "txid-live"
            ),
            "durable state must remain AwaitingConfirmation, got {:?}",
            persisted.state
        );
    }
}
