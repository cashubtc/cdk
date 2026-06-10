//! Durable storage contracts for rate-quoted payment terms.

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use cdk_common::nuts::CurrencyUnit;
use cdk_common::payment::PaymentIdentifier;
use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;

/// Stored immutable terms for one rate-converted payment lookup id.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RateQuoteRecord {
    /// Inner payment backend lookup id.
    pub payment_lookup_id: PaymentIdentifier,
    /// Fiat unit credited or debited by the outer mint quote.
    pub fiat_unit: CurrencyUnit,
    /// Fiat amount in that unit's minor subunits.
    pub fiat_subunits: u64,
    /// Serialized oracle snapshot and quote metadata.
    pub snapshot_json: serde_json::Value,
    /// Sat amount requested from the inner backend.
    pub sats_invoiced: u64,
    /// Quote expiry as a Unix timestamp in seconds.
    pub expiry_unix: u64,
}

/// Parked payment row for fail-closed orphaned payment events.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ParkedPaymentRecord {
    /// Inner payment backend lookup id.
    pub payment_lookup_id: PaymentIdentifier,
    /// BOLT11 payment hash used as an operator reconciliation join key.
    pub bolt11_payment_hash: String,
    /// Received sat amount.
    pub received_sats: u64,
    /// Observation time as a Unix timestamp in seconds.
    pub observed_at: u64,
    /// Operator reconciliation status.
    pub resolution_status: String,
}

/// Storage failure returned by [`RateQuoteStore`] implementations.
#[derive(Debug, thiserror::Error)]
pub enum RateQuoteStoreError {
    /// Storage backend error.
    #[error("rate quote store error: {0}")]
    Storage(String),
}

/// Durable storage port for rate-converted quote terms and parked payments.
#[async_trait]
pub trait RateQuoteStore: Send + Sync {
    /// Persist immutable quoted terms before the quote is returned upstream.
    async fn insert(&self, record: RateQuoteRecord) -> Result<(), RateQuoteStoreError>;

    /// Load stored quoted terms by inner payment lookup id.
    async fn get_by_lookup_id(
        &self,
        payment_lookup_id: &PaymentIdentifier,
    ) -> Result<Option<RateQuoteRecord>, RateQuoteStoreError>;

    /// Persist an orphaned payment for operator reconciliation.
    async fn insert_parked(&self, record: ParkedPaymentRecord) -> Result<(), RateQuoteStoreError>;
}

/// Shared trait-object rate quote store.
pub type DynRateQuoteStore = Arc<dyn RateQuoteStore>;

/// In-memory [`RateQuoteStore`] for tests and ephemeral development.
#[derive(Debug, Clone, Default)]
pub struct InMemoryRateQuoteStore {
    records: Arc<Mutex<HashMap<String, RateQuoteRecord>>>,
    parked: Arc<Mutex<Vec<ParkedPaymentRecord>>>,
    fail_next_insert: Arc<Mutex<bool>>,
}

impl InMemoryRateQuoteStore {
    /// Create an empty in-memory store.
    pub fn new() -> Self {
        Self::default()
    }

    /// Cause the next quoted-terms insert to fail.
    pub async fn fail_next_insert(&self) {
        *self.fail_next_insert.lock().await = true;
    }

    /// Return all parked payment records.
    pub async fn parked_payments(&self) -> Vec<ParkedPaymentRecord> {
        self.parked.lock().await.clone()
    }
}

#[async_trait]
impl RateQuoteStore for InMemoryRateQuoteStore {
    async fn insert(&self, record: RateQuoteRecord) -> Result<(), RateQuoteStoreError> {
        let mut fail_next = self.fail_next_insert.lock().await;
        if *fail_next {
            *fail_next = false;
            return Err(RateQuoteStoreError::Storage(
                "forced in-memory insert failure".to_string(),
            ));
        }
        drop(fail_next);

        self.records
            .lock()
            .await
            .insert(record.payment_lookup_id.to_string(), record);
        Ok(())
    }

    async fn get_by_lookup_id(
        &self,
        payment_lookup_id: &PaymentIdentifier,
    ) -> Result<Option<RateQuoteRecord>, RateQuoteStoreError> {
        Ok(self
            .records
            .lock()
            .await
            .get(&payment_lookup_id.to_string())
            .cloned())
    }

    async fn insert_parked(&self, record: ParkedPaymentRecord) -> Result<(), RateQuoteStoreError> {
        self.parked.lock().await.push(record);
        Ok(())
    }
}
