//! ReceiveIntent typestate wrapper
//!
//! Represents a single detected incoming UTXO to a tracked address.
//! Each intent progresses through: `Detected` -> finalized (tombstone).
//!
//! The wrapper is internal to the crate. Durable record state is the source of
//! truth for recovery, while typestate enforces valid runtime transitions.

pub(crate) mod record;
pub(crate) mod state;

use uuid::Uuid;

use self::record::{ReceiveIntentRecord, ReceiveIntentState};
use self::state::Detected;
use crate::error::Error;
use crate::storage::{BdkStorage, FinalizedReceiveIntentRecord};

/// A receive intent in a particular typestate
///
/// Each intent tracks a single detected incoming UTXO through the
/// receive saga lifecycle.
#[derive(Debug, Clone)]
pub(crate) struct ReceiveIntent<S> {
    /// Unique identifier for this intent
    pub intent_id: Uuid,
    /// Current typestate
    pub state: S,
}

impl ReceiveIntent<Detected> {
    /// Create a new detected receive intent and persist it immediately.
    pub async fn new(
        storage: &BdkStorage,
        address: String,
        txid: String,
        outpoint: String,
        amount_sat: u64,
        block_height: u32,
    ) -> Result<Option<Self>, Error> {
        let intent_id = Uuid::new_v4();
        let created_at = crate::util::unix_now();

        let request = storage
            .get_quote_id_by_receive_address(&address)
            .await?
            .ok_or_else(|| {
                Error::Wallet(format!(
                    "No tracked receive address for address {}",
                    address
                ))
            })?;

        let quote_id = request;

        let record = ReceiveIntentRecord {
            intent_id,
            quote_id: quote_id.clone(),
            state: ReceiveIntentState::Detected {
                address: address.clone(),
                txid: txid.clone(),
                outpoint: outpoint.clone(),
                amount_sat,
                block_height,
                created_at,
            },
        };

        let was_created = storage.create_receive_intent_if_absent(&record).await?;

        if !was_created {
            // Duplicate outpoint — another intent already tracks this UTXO
            return Ok(None);
        }

        Ok(Some(Self {
            intent_id,
            state: Detected {
                quote_id,
                address,
                txid,
                outpoint,
                amount_sat,
                block_height,
            },
        }))
    }

    /// Finalize a confirmed receive intent: write a tombstone and delete the
    /// active record.
    pub async fn finalize(self, storage: &BdkStorage) -> Result<(), Error> {
        let tombstone = FinalizedReceiveIntentRecord {
            intent_id: self.intent_id,
            quote_id: self.state.quote_id.clone(),
            address: self.state.address.clone(),
            txid: self.state.txid.clone(),
            outpoint: self.state.outpoint.clone(),
            amount_sat: self.state.amount_sat,
            finalized_at: crate::util::unix_now(),
        };

        storage
            .finalize_receive_intent(&self.intent_id, &tombstone)
            .await?;
        Ok(())
    }
}

/// Reconstruct a `ReceiveIntent` from a durable record for recovery
pub(crate) fn from_record(record: &ReceiveIntentRecord) -> ReceiveIntentAny {
    match &record.state {
        ReceiveIntentState::Detected {
            address,
            txid,
            outpoint,
            amount_sat,
            block_height,
            ..
        } => ReceiveIntentAny::Detected(ReceiveIntent {
            intent_id: record.intent_id,
            state: Detected {
                quote_id: record.quote_id.clone(),
                address: address.clone(),
                txid: txid.clone(),
                outpoint: outpoint.clone(),
                amount_sat: *amount_sat,
                block_height: *block_height,
            },
        }),
    }
}

/// Type-erased receive intent for recovery and querying
pub(crate) enum ReceiveIntentAny {
    /// Intent in Detected state
    Detected(ReceiveIntent<Detected>),
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use cdk_common::payment::{PaymentIdentifier, WaitPaymentResponse};
    use cdk_common::{Amount, CurrencyUnit};

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
    async fn test_detected_creation() {
        let storage = test_storage().await;

        let address = "bcrt1qaddr".to_string();
        let quote_id = Uuid::new_v4().to_string();
        storage
            .track_receive_address(&address, &quote_id)
            .await
            .expect("track address");

        let intent = ReceiveIntent::new(
            &storage,
            address,
            "txid_abc".to_string(),
            "txid_abc:0".to_string(),
            50_000,
            100,
        )
        .await
        .expect("create detected intent")
        .expect("should not be duplicate");

        assert_eq!(intent.state.address, "bcrt1qaddr");
        assert_eq!(intent.state.quote_id, quote_id);
    }

    #[tokio::test]
    async fn test_finalize_receive_intent_creates_tombstone() {
        let storage = test_storage().await;

        let address = "bcrt1qreceive".to_string();
        let quote_id = Uuid::new_v4().to_string();
        storage
            .track_receive_address(&address, &quote_id)
            .await
            .expect("track address");

        let intent = ReceiveIntent::new(
            &storage,
            address.clone(),
            "txid_receive".to_string(),
            "txid_receive:0".to_string(),
            75_000,
            150,
        )
        .await
        .expect("create detected intent")
        .expect("should not be duplicate");

        let intent_id = intent.intent_id;
        let outpoint = intent.state.outpoint.clone();
        let amount_sat = intent.state.amount_sat;

        intent.finalize(&storage).await.expect("finalize");

        let active = storage
            .get_receive_intent(&intent_id)
            .await
            .expect("get active receive intent");
        assert!(
            active.is_none(),
            "active receive intent should be deleted after finalization"
        );

        let tombstone = storage
            .get_finalized_receive_intent(&intent_id)
            .await
            .expect("get finalized receive intent")
            .expect("tombstone should exist");

        assert_eq!(tombstone.address, address);
        assert_eq!(tombstone.quote_id, quote_id);
        assert_eq!(tombstone.outpoint, outpoint.clone());
        assert_eq!(tombstone.amount_sat, amount_sat);

        let response = WaitPaymentResponse {
            payment_identifier: PaymentIdentifier::QuoteId(quote_id.parse().unwrap()),
            payment_amount: Amount::new(tombstone.amount_sat, CurrencyUnit::Sat),
            payment_id: tombstone.outpoint,
        };

        assert_eq!(
            response.payment_identifier,
            PaymentIdentifier::QuoteId(quote_id.parse().unwrap())
        );
        assert_eq!(
            response.payment_amount,
            Amount::new(75_000, CurrencyUnit::Sat)
        );
        assert_eq!(response.payment_id, outpoint);
    }
}
