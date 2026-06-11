//! Durable storage contracts for rate-quoted payment terms.

use std::collections::{HashMap, HashSet};
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
    /// Fiat fee in that unit's minor subunits.
    #[serde(default)]
    pub fiat_fee_subunits: u64,
    /// Serialized oracle snapshot and quote metadata.
    pub snapshot_json: serde_json::Value,
    /// Sat amount requested from the inner backend.
    pub sats_invoiced: u64,
    /// Sat amount the quote would have required without the buffer. The
    /// difference between the received sats and this value is booked to the
    /// per-unit buffer-surplus reserve when the quote settles.
    #[serde(default)]
    pub sats_unbuffered: u64,
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

/// Persisted runtime control state for one rate-quoted unit.
///
/// Pause flags, the issuance cap, the outstanding issued counter
/// (issued minus melted), and the buffer-surplus reserve all survive
/// process restarts through this record.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct UnitControlRecord {
    /// Controlled unit.
    pub unit: CurrencyUnit,
    /// Refuse new mint quotes for this unit.
    pub mint_paused: bool,
    /// Refuse new melt quotes for this unit.
    pub melt_paused: bool,
    /// Issuance cap in fiat subunits. `0` refuses all new mint quotes
    /// (fail-closed) — it never means unlimited.
    pub cap: u64,
    /// Outstanding issued fiat subunits (issued minus melted).
    pub outstanding: u64,
    /// Accumulated buffer-surplus reserve in sats. Reserve, not revenue.
    pub buffer_surplus_sats: u64,
}

impl UnitControlRecord {
    /// Create an empty control record for one unit.
    pub fn new(unit: CurrencyUnit) -> Self {
        Self {
            unit,
            ..Self::default()
        }
    }
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

    /// Atomically look up quoted terms for a received payment, parking the
    /// payment in the same storage operation when no terms exist.
    ///
    /// Returns the stored terms when the payment can be credited, or `None`
    /// when the payment was parked. The detection of the missing record and
    /// the parked-row write happen in one transaction so no orphaned payment
    /// is silently lost.
    async fn park_or_credit(
        &self,
        parked: ParkedPaymentRecord,
    ) -> Result<Option<RateQuoteRecord>, RateQuoteStoreError>;

    /// Atomically mark a quote settled. Returns `true` exactly once per
    /// lookup id — callers gate one-shot counter adjustments (outstanding,
    /// buffer surplus) on the `true` result.
    async fn mark_settled(
        &self,
        payment_lookup_id: &PaymentIdentifier,
    ) -> Result<bool, RateQuoteStoreError>;

    /// Load all persisted per-unit control records.
    async fn load_unit_controls(&self) -> Result<Vec<UnitControlRecord>, RateQuoteStoreError>;

    /// Persist pause state for one unit.
    async fn set_unit_quote_state(
        &self,
        unit: &CurrencyUnit,
        mint_paused: bool,
        melt_paused: bool,
    ) -> Result<(), RateQuoteStoreError>;

    /// Persist the issuance cap for one unit.
    async fn set_unit_issuance_cap(
        &self,
        unit: &CurrencyUnit,
        cap: u64,
    ) -> Result<(), RateQuoteStoreError>;

    /// Atomically add issued fiat subunits to the unit's outstanding counter.
    async fn add_unit_outstanding(
        &self,
        unit: &CurrencyUnit,
        fiat_subunits: u64,
    ) -> Result<(), RateQuoteStoreError>;

    /// Atomically subtract melted fiat subunits from the unit's outstanding
    /// counter, flooring at zero.
    async fn subtract_unit_outstanding(
        &self,
        unit: &CurrencyUnit,
        fiat_subunits: u64,
    ) -> Result<(), RateQuoteStoreError>;

    /// Atomically add sats to the unit's buffer-surplus reserve counter.
    async fn add_unit_buffer_surplus(
        &self,
        unit: &CurrencyUnit,
        sats: u64,
    ) -> Result<(), RateQuoteStoreError>;
}

/// Shared trait-object rate quote store.
pub type DynRateQuoteStore = Arc<dyn RateQuoteStore>;

/// In-memory [`RateQuoteStore`] for tests and ephemeral development.
#[derive(Debug, Clone, Default)]
pub struct InMemoryRateQuoteStore {
    records: Arc<Mutex<HashMap<String, RateQuoteRecord>>>,
    parked: Arc<Mutex<Vec<ParkedPaymentRecord>>>,
    settled: Arc<Mutex<HashSet<String>>>,
    unit_controls: Arc<Mutex<HashMap<CurrencyUnit, UnitControlRecord>>>,
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

    async fn park_or_credit(
        &self,
        parked: ParkedPaymentRecord,
    ) -> Result<Option<RateQuoteRecord>, RateQuoteStoreError> {
        // The records lock is held across the park write so the missing-record
        // detection and the parked-row insert are one atomic store operation.
        let records = self.records.lock().await;
        match records.get(&parked.payment_lookup_id.to_string()) {
            Some(record) => Ok(Some(record.clone())),
            None => {
                self.parked.lock().await.push(parked);
                Ok(None)
            }
        }
    }

    async fn mark_settled(
        &self,
        payment_lookup_id: &PaymentIdentifier,
    ) -> Result<bool, RateQuoteStoreError> {
        Ok(self
            .settled
            .lock()
            .await
            .insert(payment_lookup_id.to_string()))
    }

    async fn load_unit_controls(&self) -> Result<Vec<UnitControlRecord>, RateQuoteStoreError> {
        Ok(self.unit_controls.lock().await.values().cloned().collect())
    }

    async fn set_unit_quote_state(
        &self,
        unit: &CurrencyUnit,
        mint_paused: bool,
        melt_paused: bool,
    ) -> Result<(), RateQuoteStoreError> {
        let mut controls = self.unit_controls.lock().await;
        let control = controls
            .entry(unit.clone())
            .or_insert_with(|| UnitControlRecord::new(unit.clone()));
        control.mint_paused = mint_paused;
        control.melt_paused = melt_paused;
        Ok(())
    }

    async fn set_unit_issuance_cap(
        &self,
        unit: &CurrencyUnit,
        cap: u64,
    ) -> Result<(), RateQuoteStoreError> {
        let mut controls = self.unit_controls.lock().await;
        let control = controls
            .entry(unit.clone())
            .or_insert_with(|| UnitControlRecord::new(unit.clone()));
        control.cap = cap;
        Ok(())
    }

    async fn add_unit_outstanding(
        &self,
        unit: &CurrencyUnit,
        fiat_subunits: u64,
    ) -> Result<(), RateQuoteStoreError> {
        let mut controls = self.unit_controls.lock().await;
        let control = controls
            .entry(unit.clone())
            .or_insert_with(|| UnitControlRecord::new(unit.clone()));
        control.outstanding = control.outstanding.saturating_add(fiat_subunits);
        Ok(())
    }

    async fn subtract_unit_outstanding(
        &self,
        unit: &CurrencyUnit,
        fiat_subunits: u64,
    ) -> Result<(), RateQuoteStoreError> {
        let mut controls = self.unit_controls.lock().await;
        let control = controls
            .entry(unit.clone())
            .or_insert_with(|| UnitControlRecord::new(unit.clone()));
        control.outstanding = control.outstanding.saturating_sub(fiat_subunits);
        Ok(())
    }

    async fn add_unit_buffer_surplus(
        &self,
        unit: &CurrencyUnit,
        sats: u64,
    ) -> Result<(), RateQuoteStoreError> {
        let mut controls = self.unit_controls.lock().await;
        let control = controls
            .entry(unit.clone())
            .or_insert_with(|| UnitControlRecord::new(unit.clone()));
        control.buffer_surplus_sats = control.buffer_surplus_sats.saturating_add(sats);
        Ok(())
    }
}
