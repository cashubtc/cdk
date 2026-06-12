//! Rate-converting [`MintPayment`](cdk_common::payment::MintPayment) decorator.

use std::collections::HashMap;
use std::pin::Pin;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use async_trait::async_trait;
use cdk_common::nuts::{CurrencyUnit, MeltQuoteState};
use cdk_common::payment::{
    CreateIncomingPaymentResponse, DynMintPayment, Event, IncomingPaymentOptions,
    MakePaymentResponse, MintPayment, OutgoingPaymentOptions, PaymentIdentifier,
    PaymentQuoteResponse, SettingsResponse, WaitPaymentResponse,
};
use cdk_common::Amount;
use futures::{Stream, StreamExt};
use serde::Serialize;
use tokio::sync::Mutex;

use crate::oracle::RateOracle;
use crate::store::{
    DynRateQuoteStore, ParkedPaymentRecord, RateQuoteRecord, RateQuoteSettlement,
    RateQuoteStoreError,
};
use crate::types::{fiat_subunit_scale, RateOracleError, RateSnapshot};

/// Number of basis points in 100%.
const BPS_DENOMINATOR: u64 = 10_000;

/// Default rate-quoted invoice TTL in seconds.
pub const DEFAULT_RATE_QUOTE_TTL_SECS: u64 = 90;

static PARKED_PAYMENT_EVENTS: AtomicU64 = AtomicU64::new(0);

/// Return the number of orphaned payment events parked by rate-converting processors.
pub fn parked_payment_event_count() -> u64 {
    PARKED_PAYMENT_EVENTS.load(Ordering::Relaxed)
}

static RESERVATION_SEQ: AtomicU64 = AtomicU64::new(0);

/// Process-unique key for a cap reservation taken before the inner backend
/// has assigned a payment lookup id.
fn provisional_reservation_key() -> String {
    format!(
        "provisional-{}",
        RESERVATION_SEQ.fetch_add(1, Ordering::Relaxed)
    )
}

/// Rate-converting payment decorator configuration.
#[derive(Debug, Clone)]
pub struct RateConvertingPaymentConfig {
    /// Fiat unit exposed by this processor.
    pub fiat_unit: CurrencyUnit,
    /// Extra basis points charged on the mint-favoring side.
    pub buffer_bps: u64,
    /// Quote TTL in seconds.
    pub ttl_secs: u64,
}

impl RateConvertingPaymentConfig {
    /// Create a config for one fiat unit.
    pub fn new(fiat_unit: CurrencyUnit, buffer_bps: u64, ttl_secs: u64) -> Self {
        Self {
            fiat_unit,
            buffer_bps,
            ttl_secs,
        }
    }
}

impl Default for RateConvertingPaymentConfig {
    fn default() -> Self {
        Self {
            fiat_unit: CurrencyUnit::Usd,
            buffer_bps: 100,
            ttl_secs: DEFAULT_RATE_QUOTE_TTL_SECS,
        }
    }
}

/// Errors returned by [`RateConvertingPayment`].
#[derive(Debug, thiserror::Error)]
pub enum RateConvertingPaymentError {
    /// Inner payment backend error.
    #[error(transparent)]
    Inner(#[from] cdk_common::payment::Error),
    /// Oracle error.
    #[error(transparent)]
    Oracle(#[from] RateOracleError),
    /// Quote-store error.
    #[error(transparent)]
    Store(#[from] RateQuoteStoreError),
    /// Unsupported payment option.
    #[error("unsupported payment option")]
    UnsupportedPaymentOption,
    /// Unsupported unit.
    #[error("unsupported unit {0}")]
    UnsupportedUnit(CurrencyUnit),
    /// Unsupported fiat subunit scale.
    #[error("unsupported fiat subunit scale for unit {0}")]
    UnsupportedFiatScale(CurrencyUnit),
    /// Invalid rate.
    #[error("invalid rate {0}")]
    InvalidRate(u64),
    /// Amount overflow.
    #[error("amount overflow")]
    AmountOverflow,
    /// Quote issuance is paused for this unit and side.
    #[error("{side} quotes are paused for unit {unit}")]
    UnitPaused {
        /// Paused unit.
        unit: CurrencyUnit,
        /// Paused quote side.
        side: &'static str,
    },
    /// Unit issuance cap would be exceeded.
    #[error("issuance cap exceeded for unit {unit}: requested {requested}, available {available}")]
    IssuanceCapExceeded {
        /// Capped unit.
        unit: CurrencyUnit,
        /// Amount requested.
        requested: u64,
        /// Amount still available under the cap.
        available: u64,
    },
}

impl From<RateConvertingPaymentError> for cdk_common::payment::Error {
    fn from(error: RateConvertingPaymentError) -> Self {
        match error {
            RateConvertingPaymentError::Inner(inner) => inner,
            RateConvertingPaymentError::UnsupportedPaymentOption => Self::UnsupportedPaymentOption,
            RateConvertingPaymentError::UnsupportedUnit(_)
            | RateConvertingPaymentError::UnsupportedFiatScale(_) => Self::UnsupportedUnit,
            RateConvertingPaymentError::UnitPaused { .. }
            | RateConvertingPaymentError::IssuanceCapExceeded { .. } => {
                Self::Custom(error.to_string())
            }
            other => Self::Custom(other.to_string()),
        }
    }
}

/// Runtime pause/cap settings for one exposed quote unit.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct UnitQuoteState {
    /// Refuse new mint quotes for this unit.
    pub mint_paused: bool,
    /// Refuse new melt quotes for this unit.
    pub melt_paused: bool,
}

/// One pending (unpaid, unexpired) cap reservation.
#[derive(Debug, Clone)]
struct Reservation {
    fiat_subunits: u64,
    expiry_unix: u64,
}

#[derive(Debug, Default)]
struct UnitControlState {
    quote_state: UnitQuoteState,
    cap: u64,
    outstanding: u64,
    buffer_surplus_sats: u64,
    reservations: HashMap<String, Reservation>,
}

impl UnitControlState {
    /// Drop expired reservations; expiry is enforced lazily on access so no
    /// background task is needed and reservations survive key rekeying.
    fn sweep_expired(&mut self, now: u64) {
        self.reservations
            .retain(|_, reservation| reservation.expiry_unix > now);
    }

    fn pending(&self) -> u64 {
        self.reservations
            .values()
            .map(|reservation| reservation.fiat_subunits)
            .fold(0_u64, u64::saturating_add)
    }
}

/// Shared control handle used by the decorator and management RPC.
///
/// Pause flags, the issuance cap, the outstanding issued counter, and the
/// buffer-surplus reserve are mirrored in memory for cheap cap checks and
/// written through to the store (when present) so they survive restarts.
#[derive(Clone, Default)]
pub struct RateQuoteControlHandle {
    units: Arc<Mutex<HashMap<CurrencyUnit, UnitControlState>>>,
    store: Option<DynRateQuoteStore>,
    buffer_bps: Arc<AtomicU64>,
}

impl std::fmt::Debug for RateQuoteControlHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RateQuoteControlHandle")
            .field("persisted", &self.store.is_some())
            .finish_non_exhaustive()
    }
}

impl RateQuoteControlHandle {
    /// Create an in-memory-only rate-quote control handle.
    pub fn new() -> Self {
        Self::default()
    }

    /// Create an in-memory-only rate-quote control handle with the active
    /// mint-favoring buffer basis points.
    pub fn with_buffer_bps(buffer_bps: u64) -> Self {
        Self {
            units: Arc::default(),
            store: None,
            buffer_bps: Arc::new(AtomicU64::new(buffer_bps)),
        }
    }

    /// Create a control handle that writes pause/cap/outstanding state
    /// through to a durable store.
    pub fn with_store(store: DynRateQuoteStore) -> Self {
        Self::with_store_and_buffer_bps(store, 0)
    }

    /// Create a control handle that writes pause/cap/outstanding state
    /// through to a durable store and knows the active mint-favoring buffer.
    pub fn with_store_and_buffer_bps(store: DynRateQuoteStore, buffer_bps: u64) -> Self {
        Self {
            units: Arc::default(),
            store: Some(store),
            buffer_bps: Arc::new(AtomicU64::new(buffer_bps)),
        }
    }

    /// Set the active mint-favoring buffer basis points for later management
    /// control requests.
    pub fn set_buffer_bps(&self, buffer_bps: u64) {
        self.buffer_bps.store(buffer_bps, Ordering::Relaxed);
    }

    /// Return the active mint-favoring buffer basis points.
    pub fn running_buffer_bps(&self) -> u64 {
        self.buffer_bps.load(Ordering::Relaxed)
    }

    /// Load persisted unit-control state into memory. Returns the units that
    /// had a persisted record so callers can seed config defaults for the
    /// rest without overwriting operator-set values.
    pub async fn load_persisted(&self) -> Result<Vec<CurrencyUnit>, RateQuoteStoreError> {
        let Some(store) = &self.store else {
            return Ok(Vec::new());
        };
        let records = store.load_unit_controls().await?;
        let mut units = self.units.lock().await;
        let mut loaded = Vec::with_capacity(records.len());
        for record in records {
            let state = units.entry(record.unit.clone()).or_default();
            state.quote_state = UnitQuoteState {
                mint_paused: record.mint_paused,
                melt_paused: record.melt_paused,
            };
            state.cap = record.cap;
            state.outstanding = record.outstanding;
            state.buffer_surplus_sats = record.buffer_surplus_sats;
            loaded.push(record.unit);
        }
        Ok(loaded)
    }

    /// Set pause state for one exposed unit, persisting it when a store is
    /// attached.
    pub async fn set_unit_quote_state(
        &self,
        unit: CurrencyUnit,
        mint_paused: bool,
        melt_paused: bool,
    ) -> Result<(), RateQuoteStoreError> {
        if let Some(store) = &self.store {
            store
                .set_unit_quote_state(&unit, mint_paused, melt_paused)
                .await?;
        }
        let mut units = self.units.lock().await;
        units.entry(unit).or_default().quote_state = UnitQuoteState {
            mint_paused,
            melt_paused,
        };
        Ok(())
    }

    /// Set the issuance cap for one exposed unit, persisting it when a store
    /// is attached. A cap of `0` refuses all new mint quotes (fail-closed) —
    /// it never means unlimited.
    pub async fn set_unit_issuance_cap(
        &self,
        unit: CurrencyUnit,
        cap: u64,
    ) -> Result<(), RateQuoteStoreError> {
        if cap != 0 && self.running_buffer_bps() == 0 {
            return Err(RateQuoteStoreError::InvalidControl(
                "rate quote buffer_bps must be nonzero before setting a nonzero issuance cap"
                    .to_string(),
            ));
        }
        if let Some(store) = &self.store {
            store.set_unit_issuance_cap(&unit, cap).await?;
        }
        let mut units = self.units.lock().await;
        units.entry(unit).or_default().cap = cap;
        Ok(())
    }

    /// Configured issuance cap for one unit.
    pub async fn unit_issuance_cap(&self, unit: &CurrencyUnit) -> u64 {
        self.units
            .lock()
            .await
            .get(unit)
            .map(|state| state.cap)
            .unwrap_or_default()
    }

    /// Outstanding issued fiat subunits (issued minus melted) for one unit.
    pub async fn outstanding(&self, unit: &CurrencyUnit) -> u64 {
        self.units
            .lock()
            .await
            .get(unit)
            .map(|state| state.outstanding)
            .unwrap_or_default()
    }

    /// Accumulated buffer-surplus reserve in sats for one unit. Reserve, not
    /// revenue: observable separately so no code path books it as income.
    pub async fn buffer_surplus_sats(&self, unit: &CurrencyUnit) -> u64 {
        self.units
            .lock()
            .await
            .get(unit)
            .map(|state| state.buffer_surplus_sats)
            .unwrap_or_default()
    }

    async fn ensure_not_paused(
        &self,
        unit: &CurrencyUnit,
        side: QuoteSide,
    ) -> Result<(), RateConvertingPaymentError> {
        let units = self.units.lock().await;
        let paused = units
            .get(unit)
            .map(|state| match side {
                QuoteSide::Mint => state.quote_state.mint_paused,
                QuoteSide::Melt => state.quote_state.melt_paused,
            })
            .unwrap_or_default();
        if paused {
            return Err(RateConvertingPaymentError::UnitPaused {
                unit: unit.clone(),
                side: side.as_str(),
            });
        }
        Ok(())
    }

    /// Reserve cap headroom for a new mint quote.
    ///
    /// The cap covers persisted outstanding issuance plus pending (unpaid,
    /// unexpired) reservations plus the new request. A cap of `0` refuses
    /// every request (fail-closed) — it never means unlimited.
    async fn reserve(
        &self,
        unit: &CurrencyUnit,
        key: &str,
        fiat_subunits: u64,
        expiry_unix: u64,
    ) -> Result<(), RateConvertingPaymentError> {
        let mut units = self.units.lock().await;
        let state = units.entry(unit.clone()).or_default();
        state.sweep_expired(unix_time());
        let used = state.outstanding.saturating_add(state.pending());
        let available = state.cap.saturating_sub(used);
        if state.cap == 0 || fiat_subunits > available {
            return Err(RateConvertingPaymentError::IssuanceCapExceeded {
                unit: unit.clone(),
                requested: fiat_subunits,
                available,
            });
        }
        state.reservations.insert(
            key.to_string(),
            Reservation {
                fiat_subunits,
                expiry_unix,
            },
        );
        Ok(())
    }

    async fn release(&self, unit: &CurrencyUnit, key: &str) {
        let mut units = self.units.lock().await;
        if let Some(state) = units.get_mut(unit) {
            state.reservations.remove(key);
        }
    }

    async fn rekey_reservation(&self, unit: &CurrencyUnit, old_key: &str, new_key: &str) {
        let mut units = self.units.lock().await;
        if let Some(state) = units.get_mut(unit) {
            if let Some(reservation) = state.reservations.remove(old_key) {
                state.reservations.insert(new_key.to_string(), reservation);
            }
        }
    }

    /// Mirror a durably settled mint credit in memory: vacate the pending
    /// reservation, add the issued subunits to outstanding, and book the
    /// buffer surplus to the reserve counter.
    async fn commit_issued(
        &self,
        unit: &CurrencyUnit,
        key: &str,
        fiat_subunits: u64,
        surplus_sats: u64,
    ) {
        let mut units = self.units.lock().await;
        let state = units.entry(unit.clone()).or_default();
        state.reservations.remove(key);
        state.outstanding = state.outstanding.saturating_add(fiat_subunits);
        state.buffer_surplus_sats = state.buffer_surplus_sats.saturating_add(surplus_sats);
    }

    /// Mirror a durably settled melt in memory.
    async fn commit_melted(&self, unit: &CurrencyUnit, fiat_subunits: u64) {
        let mut units = self.units.lock().await;
        let state = units.entry(unit.clone()).or_default();
        state.outstanding = state.outstanding.saturating_sub(fiat_subunits);
    }
}

#[derive(Debug, Clone, Copy)]
enum QuoteSide {
    Mint,
    Melt,
}

impl QuoteSide {
    fn as_str(self) -> &'static str {
        match self {
            Self::Mint => "mint",
            Self::Melt => "melt",
        }
    }
}

/// Adapter that exposes any payment processor error as `cdk_common::payment::Error`.
#[derive(Debug, Clone)]
pub struct PaymentErrorAdapter<T> {
    inner: T,
}

impl<T> PaymentErrorAdapter<T> {
    /// Create a new payment error adapter.
    pub fn new(inner: T) -> Self {
        Self { inner }
    }
}

/// Shared payment processor adapter for trait-object backends.
#[derive(Clone)]
pub struct SharedMintPayment {
    inner: DynMintPayment,
}

impl std::fmt::Debug for SharedMintPayment {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SharedMintPayment").finish_non_exhaustive()
    }
}

impl SharedMintPayment {
    /// Wrap a shared payment processor.
    pub fn new(inner: DynMintPayment) -> Self {
        Self { inner }
    }
}

#[async_trait]
impl MintPayment for SharedMintPayment {
    type Err = cdk_common::payment::Error;

    async fn start(&self) -> Result<(), Self::Err> {
        self.inner.start().await
    }

    async fn stop(&self) -> Result<(), Self::Err> {
        self.inner.stop().await
    }

    async fn get_settings(&self) -> Result<SettingsResponse, Self::Err> {
        self.inner.get_settings().await
    }

    async fn create_incoming_payment_request(
        &self,
        options: IncomingPaymentOptions,
    ) -> Result<CreateIncomingPaymentResponse, Self::Err> {
        self.inner.create_incoming_payment_request(options).await
    }

    async fn get_payment_quote(
        &self,
        unit: &CurrencyUnit,
        options: OutgoingPaymentOptions,
    ) -> Result<PaymentQuoteResponse, Self::Err> {
        self.inner.get_payment_quote(unit, options).await
    }

    async fn make_payment(
        &self,
        unit: &CurrencyUnit,
        options: OutgoingPaymentOptions,
    ) -> Result<MakePaymentResponse, Self::Err> {
        self.inner.make_payment(unit, options).await
    }

    async fn wait_payment_event(
        &self,
    ) -> Result<Pin<Box<dyn Stream<Item = Event> + Send>>, Self::Err> {
        self.inner.wait_payment_event().await
    }

    fn is_payment_event_stream_active(&self) -> bool {
        self.inner.is_payment_event_stream_active()
    }

    fn cancel_payment_event_stream(&self) {
        self.inner.cancel_payment_event_stream();
    }

    async fn check_incoming_payment_status(
        &self,
        payment_identifier: &PaymentIdentifier,
    ) -> Result<Vec<WaitPaymentResponse>, Self::Err> {
        self.inner
            .check_incoming_payment_status(payment_identifier)
            .await
    }

    async fn check_outgoing_payment(
        &self,
        payment_identifier: &PaymentIdentifier,
    ) -> Result<MakePaymentResponse, Self::Err> {
        self.inner.check_outgoing_payment(payment_identifier).await
    }
}

#[async_trait]
impl<T> MintPayment for PaymentErrorAdapter<T>
where
    T: MintPayment + Send + Sync,
    T::Err: Into<cdk_common::payment::Error>,
{
    type Err = cdk_common::payment::Error;

    async fn start(&self) -> Result<(), Self::Err> {
        self.inner.start().await.map_err(Into::into)
    }

    async fn stop(&self) -> Result<(), Self::Err> {
        self.inner.stop().await.map_err(Into::into)
    }

    async fn get_settings(&self) -> Result<SettingsResponse, Self::Err> {
        self.inner.get_settings().await.map_err(Into::into)
    }

    async fn create_incoming_payment_request(
        &self,
        options: IncomingPaymentOptions,
    ) -> Result<CreateIncomingPaymentResponse, Self::Err> {
        self.inner
            .create_incoming_payment_request(options)
            .await
            .map_err(Into::into)
    }

    async fn get_payment_quote(
        &self,
        unit: &CurrencyUnit,
        options: OutgoingPaymentOptions,
    ) -> Result<PaymentQuoteResponse, Self::Err> {
        self.inner
            .get_payment_quote(unit, options)
            .await
            .map_err(Into::into)
    }

    async fn make_payment(
        &self,
        unit: &CurrencyUnit,
        options: OutgoingPaymentOptions,
    ) -> Result<MakePaymentResponse, Self::Err> {
        self.inner
            .make_payment(unit, options)
            .await
            .map_err(Into::into)
    }

    async fn wait_payment_event(
        &self,
    ) -> Result<Pin<Box<dyn Stream<Item = Event> + Send>>, Self::Err> {
        self.inner.wait_payment_event().await.map_err(Into::into)
    }

    fn is_payment_event_stream_active(&self) -> bool {
        self.inner.is_payment_event_stream_active()
    }

    fn cancel_payment_event_stream(&self) {
        self.inner.cancel_payment_event_stream();
    }

    async fn check_incoming_payment_status(
        &self,
        payment_identifier: &PaymentIdentifier,
    ) -> Result<Vec<WaitPaymentResponse>, Self::Err> {
        self.inner
            .check_incoming_payment_status(payment_identifier)
            .await
            .map_err(Into::into)
    }

    async fn check_outgoing_payment(
        &self,
        payment_identifier: &PaymentIdentifier,
    ) -> Result<MakePaymentResponse, Self::Err> {
        self.inner
            .check_outgoing_payment(payment_identifier)
            .await
            .map_err(Into::into)
    }
}

/// Decorates a sat payment backend as a fiat-denominated payment processor.
#[derive(Clone)]
pub struct RateConvertingPayment<T> {
    inner: T,
    oracle: Arc<dyn RateOracle>,
    store: DynRateQuoteStore,
    config: RateConvertingPaymentConfig,
    control: RateQuoteControlHandle,
}

impl<T> std::fmt::Debug for RateConvertingPayment<T>
where
    T: std::fmt::Debug,
{
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RateConvertingPayment")
            .field("inner", &self.inner)
            .field("config", &self.config)
            .finish_non_exhaustive()
    }
}

impl<T> RateConvertingPayment<T> {
    /// Create a new rate-converting decorator.
    pub fn new(
        inner: T,
        oracle: Arc<dyn RateOracle>,
        store: DynRateQuoteStore,
        config: RateConvertingPaymentConfig,
    ) -> Self {
        Self {
            inner,
            oracle,
            store,
            control: RateQuoteControlHandle::with_buffer_bps(config.buffer_bps),
            config,
        }
    }

    /// Create a new rate-converting decorator with shared management control.
    pub fn with_control(
        inner: T,
        oracle: Arc<dyn RateOracle>,
        store: DynRateQuoteStore,
        config: RateConvertingPaymentConfig,
        control: RateQuoteControlHandle,
    ) -> Self {
        control.set_buffer_bps(config.buffer_bps);
        Self {
            inner,
            oracle,
            store,
            config,
            control,
        }
    }

    /// Return the shared pause/cap control handle.
    pub fn control_handle(&self) -> RateQuoteControlHandle {
        self.control.clone()
    }

    /// Access the configured fiat unit.
    pub fn fiat_unit(&self) -> &CurrencyUnit {
        &self.config.fiat_unit
    }

    async fn mint_quote_terms(&self, fiat_subunits: u64) -> Result<(RateSnapshot, u64), SelfError> {
        let snapshot = self.oracle.snapshot(&self.config.fiat_unit).await?;
        let sats = sats_for_fiat_subunits(
            &self.config.fiat_unit,
            fiat_subunits,
            snapshot.aggregated_rate,
            self.config.buffer_bps,
        )?;
        Ok((snapshot, sats))
    }

    /// Book a completed melt against the outstanding issued counter exactly
    /// once. Settlement persistence is fail-closed: if the settled flag and
    /// counter update cannot commit together, the payment is parked for
    /// operator reconciliation and the in-memory counter is left unchanged.
    async fn settle_melt(&self, record: &RateQuoteRecord, response: &MakePaymentResponse) {
        if response.status != MeltQuoteState::Paid {
            return;
        }
        let settlement = RateQuoteSettlement::Melt {
            fiat_subunits: record_total_fiat_subunits(record),
        };
        match self
            .store
            .settle_quote_and_commit_unit_control(
                &response.payment_lookup_id,
                &record.fiat_unit,
                settlement,
            )
            .await
        {
            Ok(true) => {
                self.control
                    .commit_melted(&record.fiat_unit, record_total_fiat_subunits(record))
                    .await;
            }
            Ok(false) => {}
            Err(error) => {
                park_settlement_failure(
                    &self.store,
                    ParkedPaymentRecord {
                        payment_lookup_id: response.payment_lookup_id.clone(),
                        bolt11_payment_hash: response.payment_lookup_id.to_string(),
                        received_sats: record.sats_invoiced,
                        observed_at: unix_time(),
                        resolution_status: "settlement_failed".to_string(),
                    },
                    error,
                    "melt",
                )
                .await;
            }
        }
    }
}

type SelfError = RateConvertingPaymentError;

#[async_trait]
impl<T> MintPayment for RateConvertingPayment<T>
where
    T: MintPayment<Err = cdk_common::payment::Error> + Send + Sync,
{
    type Err = RateConvertingPaymentError;

    async fn start(&self) -> Result<(), Self::Err> {
        self.inner.start().await?;
        Ok(())
    }

    async fn stop(&self) -> Result<(), Self::Err> {
        self.inner.stop().await?;
        Ok(())
    }

    async fn get_settings(&self) -> Result<SettingsResponse, Self::Err> {
        let inner = self.inner.get_settings().await?;
        Ok(SettingsResponse {
            unit: self.config.fiat_unit.to_string(),
            bolt11: inner.bolt11,
            bolt12: None,
            custom: Default::default(),
        })
    }

    async fn create_incoming_payment_request(
        &self,
        options: IncomingPaymentOptions,
    ) -> Result<CreateIncomingPaymentResponse, Self::Err> {
        let IncomingPaymentOptions::Bolt11(mut options) = options else {
            return Err(SelfError::UnsupportedPaymentOption);
        };

        if options.amount.unit() != &self.config.fiat_unit {
            return Err(SelfError::UnsupportedUnit(options.amount.unit().clone()));
        }
        self.control
            .ensure_not_paused(&self.config.fiat_unit, QuoteSide::Mint)
            .await?;

        let fiat_subunits = options.amount.value();
        let (snapshot, sats_invoiced) = self.mint_quote_terms(fiat_subunits).await?;
        let sats_unbuffered = sats_for_fiat_subunits(
            &self.config.fiat_unit,
            fiat_subunits,
            snapshot.aggregated_rate,
            0,
        )?;
        let expiry_unix = effective_expiry(self.config.ttl_secs);
        options.amount = Amount::new(sats_invoiced, CurrencyUnit::Sat);
        options.unix_expiry = Some(expiry_unix);

        let snapshot_json = snapshot_json(
            &self.config.fiat_unit,
            fiat_subunits,
            &snapshot,
            self.config.buffer_bps,
            sats_invoiced,
            expiry_unix,
        )?;

        // Reserve cap headroom BEFORE the inner invoice exists, under a
        // provisional key: an invoice must never be handed out for exposure
        // the cap cannot absorb. The reservation is rekeyed to the inner
        // lookup id on success and released on every failure path.
        let provisional_key = provisional_reservation_key();
        self.control
            .reserve(
                &self.config.fiat_unit,
                &provisional_key,
                fiat_subunits,
                expiry_unix,
            )
            .await?;

        let mut response = match self
            .inner
            .create_incoming_payment_request(IncomingPaymentOptions::Bolt11(options))
            .await
        {
            Ok(response) => response,
            Err(error) => {
                self.control
                    .release(&self.config.fiat_unit, &provisional_key)
                    .await;
                return Err(error.into());
            }
        };

        let record = RateQuoteRecord {
            payment_lookup_id: response.request_lookup_id.clone(),
            fiat_unit: self.config.fiat_unit.clone(),
            fiat_subunits,
            fiat_fee_subunits: 0,
            snapshot_json: snapshot_json.clone(),
            sats_invoiced,
            sats_unbuffered,
            expiry_unix,
        };
        // Quoted terms persist before the quote is returned upstream; a
        // payment for an unpersisted invoice parks fail-closed.
        if let Err(error) = self.store.insert(record).await {
            self.control
                .release(&self.config.fiat_unit, &provisional_key)
                .await;
            return Err(error.into());
        }
        self.control
            .rekey_reservation(
                &self.config.fiat_unit,
                &provisional_key,
                &response.request_lookup_id.to_string(),
            )
            .await;

        response.expiry = Some(expiry_unix);
        response.extra_json = merge_extra(response.extra_json, snapshot_json);
        Ok(response)
    }

    async fn get_payment_quote(
        &self,
        unit: &CurrencyUnit,
        options: OutgoingPaymentOptions,
    ) -> Result<PaymentQuoteResponse, Self::Err> {
        if unit != &self.config.fiat_unit {
            return Err(SelfError::UnsupportedUnit(unit.clone()));
        }
        self.control
            .ensure_not_paused(&self.config.fiat_unit, QuoteSide::Melt)
            .await?;

        let inner_quote = self
            .inner
            .get_payment_quote(&CurrencyUnit::Sat, options)
            .await?;
        let snapshot = self.oracle.snapshot(&self.config.fiat_unit).await?;
        let fiat_subunits = fiat_subunits_for_sats(
            &self.config.fiat_unit,
            inner_quote.amount.value(),
            snapshot.aggregated_rate,
            self.config.buffer_bps,
        )?;
        let fiat_fee_subunits = fiat_subunits_for_sats(
            &self.config.fiat_unit,
            inner_quote.fee.value(),
            snapshot.aggregated_rate,
            self.config.buffer_bps,
        )?;
        let expiry_unix = unix_time().saturating_add(self.config.ttl_secs);
        let snapshot_json = snapshot_json(
            &self.config.fiat_unit,
            fiat_subunits,
            &snapshot,
            self.config.buffer_bps,
            inner_quote.amount.value(),
            expiry_unix,
        )?;

        if let Some(payment_lookup_id) = inner_quote.request_lookup_id.clone() {
            self.store
                .insert(RateQuoteRecord {
                    payment_lookup_id: payment_lookup_id.clone(),
                    fiat_unit: self.config.fiat_unit.clone(),
                    fiat_subunits,
                    fiat_fee_subunits,
                    snapshot_json: snapshot_json.clone(),
                    sats_invoiced: inner_quote.amount.value(),
                    sats_unbuffered: inner_quote.amount.value(),
                    expiry_unix,
                })
                .await?;
        }

        Ok(PaymentQuoteResponse {
            request_lookup_id: inner_quote.request_lookup_id,
            amount: Amount::new(fiat_subunits, self.config.fiat_unit.clone()),
            fee: Amount::new(fiat_fee_subunits, self.config.fiat_unit.clone()),
            state: inner_quote.state,
            extra_json: merge_extra(inner_quote.extra_json, snapshot_json),
        })
    }

    async fn make_payment(
        &self,
        unit: &CurrencyUnit,
        options: OutgoingPaymentOptions,
    ) -> Result<MakePaymentResponse, Self::Err> {
        if unit != &self.config.fiat_unit {
            return Err(SelfError::UnsupportedUnit(unit.clone()));
        }

        let response = self.inner.make_payment(&CurrencyUnit::Sat, options).await?;
        let Some(record) = self
            .store
            .get_by_lookup_id(&response.payment_lookup_id)
            .await?
        else {
            return Ok(response);
        };
        self.settle_melt(&record, &response).await;

        Ok(MakePaymentResponse {
            payment_lookup_id: response.payment_lookup_id,
            payment_proof: response.payment_proof,
            status: response.status,
            total_spent: Amount::new(record_total_fiat_subunits(&record), record.fiat_unit),
        })
    }

    async fn wait_payment_event(
        &self,
    ) -> Result<Pin<Box<dyn Stream<Item = Event> + Send>>, Self::Err> {
        let stream = self.inner.wait_payment_event().await?;
        let store = Arc::clone(&self.store);
        let control = self.control.clone();
        let fiat_unit = self.config.fiat_unit.clone();

        Ok(Box::pin(stream.filter_map(move |event| {
            let store = Arc::clone(&store);
            let control = control.clone();
            let fiat_unit = fiat_unit.clone();
            async move {
                match event {
                    Event::PaymentReceived(payment) => {
                        convert_payment_event(store, control, fiat_unit, payment).await
                    }
                    other => Some(other),
                }
            }
        })))
    }

    fn is_payment_event_stream_active(&self) -> bool {
        self.inner.is_payment_event_stream_active()
    }

    fn cancel_payment_event_stream(&self) {
        self.inner.cancel_payment_event_stream();
    }

    async fn check_incoming_payment_status(
        &self,
        payment_identifier: &PaymentIdentifier,
    ) -> Result<Vec<WaitPaymentResponse>, Self::Err> {
        let payments = self
            .inner
            .check_incoming_payment_status(payment_identifier)
            .await?;
        let mut converted = Vec::new();
        for payment in payments {
            if let Some(Event::PaymentReceived(payment)) = convert_payment_event(
                Arc::clone(&self.store),
                self.control.clone(),
                self.config.fiat_unit.clone(),
                payment,
            )
            .await
            {
                converted.push(payment);
            }
        }
        Ok(converted)
    }

    async fn check_outgoing_payment(
        &self,
        payment_identifier: &PaymentIdentifier,
    ) -> Result<MakePaymentResponse, Self::Err> {
        let response = self
            .inner
            .check_outgoing_payment(payment_identifier)
            .await?;
        let Some(record) = self
            .store
            .get_by_lookup_id(&response.payment_lookup_id)
            .await?
        else {
            return Ok(response);
        };
        self.settle_melt(&record, &response).await;

        Ok(MakePaymentResponse {
            payment_lookup_id: response.payment_lookup_id,
            payment_proof: response.payment_proof,
            status: response.status,
            total_spent: Amount::new(record_total_fiat_subunits(&record), record.fiat_unit),
        })
    }
}

async fn convert_payment_event(
    store: DynRateQuoteStore,
    control: RateQuoteControlHandle,
    fiat_unit: CurrencyUnit,
    payment: WaitPaymentResponse,
) -> Option<Event> {
    let parked = ParkedPaymentRecord {
        payment_lookup_id: payment.payment_identifier.clone(),
        bolt11_payment_hash: payment.payment_id.clone(),
        received_sats: payment.payment_amount.value(),
        observed_at: unix_time(),
        resolution_status: "parked".to_string(),
    };
    // The missing-record detection and the parked write are one atomic store
    // operation, so an orphaned payment can never be silently lost between
    // the lookup and the park.
    match store.park_or_credit(parked.clone()).await {
        Ok(Some(record)) => {
            if payment.payment_amount.value() < record.sats_invoiced {
                tracing::warn!(
                    payment_lookup_id = %payment.payment_identifier,
                    received_sats = payment.payment_amount.value(),
                    sats_invoiced = record.sats_invoiced,
                    "suppressing underpaid rate-converted payment"
                );
                return None;
            }
            if !settle_mint_credit(&store, &control, &record, &payment, parked).await {
                return None;
            }

            Some(Event::PaymentReceived(WaitPaymentResponse {
                payment_identifier: payment.payment_identifier,
                payment_amount: Amount::new(record.fiat_subunits, record.fiat_unit),
                payment_id: payment.payment_id,
            }))
        }
        Ok(None) => {
            PARKED_PAYMENT_EVENTS.fetch_add(1, Ordering::Relaxed);
            tracing::warn!(
                payment_lookup_id = %payment.payment_identifier,
                bolt11_payment_hash = payment.payment_id,
                fiat_unit = %fiat_unit,
                "parked orphaned rate-converted payment"
            );
            None
        }
        Err(error) => {
            tracing::warn!(
                payment_lookup_id = %payment.payment_identifier,
                error = %error,
                "suppressing rate-converted payment after quote-store failure"
            );
            None
        }
    }
}

/// Apply the one-shot counter effects of a credited mint payment: vacate the
/// pending cap reservation, grow the outstanding issued counter, and book the
/// buffer portion of the received sats to the per-unit surplus reserve.
///
/// The event is re-emitted only after the settled flag and counter updates
/// commit in one store operation. Store failures park the payment and suppress
/// the upstream event so the mint cannot credit liabilities without persisted
/// accounting.
async fn settle_mint_credit(
    store: &DynRateQuoteStore,
    control: &RateQuoteControlHandle,
    record: &RateQuoteRecord,
    payment: &WaitPaymentResponse,
    parked: ParkedPaymentRecord,
) -> bool {
    let key = payment.payment_identifier.to_string();
    let unbuffered = if record.sats_unbuffered > 0 {
        record.sats_unbuffered
    } else {
        record.sats_invoiced
    };
    let surplus_sats = payment.payment_amount.value().saturating_sub(unbuffered);
    let settlement = RateQuoteSettlement::MintCredit {
        fiat_subunits: record.fiat_subunits,
        buffer_surplus_sats: surplus_sats,
    };
    match store
        .settle_quote_and_commit_unit_control(
            &payment.payment_identifier,
            &record.fiat_unit,
            settlement,
        )
        .await
    {
        Ok(true) => {
            control
                .commit_issued(&record.fiat_unit, &key, record.fiat_subunits, surplus_sats)
                .await;
            true
        }
        Ok(false) => {
            // Already settled (event stream and status check can both fire):
            // make sure the reservation is vacated, but never re-count.
            control.release(&record.fiat_unit, &key).await;
            true
        }
        Err(error) => {
            let mut parked = parked;
            parked.resolution_status = "settlement_failed".to_string();
            park_settlement_failure(store, parked, error, "mint").await;
            false
        }
    }
}

async fn park_settlement_failure(
    store: &DynRateQuoteStore,
    parked: ParkedPaymentRecord,
    error: RateQuoteStoreError,
    side: &'static str,
) {
    let payment_lookup_id = parked.payment_lookup_id.clone();
    let bolt11_payment_hash = parked.bolt11_payment_hash.clone();
    match store.insert_parked(parked).await {
        Ok(()) => {
            PARKED_PAYMENT_EVENTS.fetch_add(1, Ordering::Relaxed);
            tracing::warn!(
                payment_lookup_id = %payment_lookup_id,
                bolt11_payment_hash = %bolt11_payment_hash,
                side,
                error = %error,
                "parked rate-converted payment after atomic settlement failure"
            );
        }
        Err(park_error) => {
            tracing::warn!(
                payment_lookup_id = %payment_lookup_id,
                bolt11_payment_hash = %bolt11_payment_hash,
                side,
                error = %error,
                park_error = %park_error,
                "suppressing rate-converted payment after settlement and parking failure"
            );
        }
    }
}

fn sats_for_fiat_subunits(
    fiat_unit: &CurrencyUnit,
    fiat_subunits: u64,
    sats_per_fiat_unit: u64,
    buffer_bps: u64,
) -> Result<u64, SelfError> {
    if sats_per_fiat_unit == 0 {
        return Err(SelfError::InvalidRate(sats_per_fiat_unit));
    }

    let scale = fiat_subunit_scale(fiat_unit)
        .ok_or_else(|| SelfError::UnsupportedFiatScale(fiat_unit.clone()))?;
    let buffered_bps = BPS_DENOMINATOR
        .checked_add(buffer_bps)
        .ok_or(SelfError::AmountOverflow)?;
    let numerator = (fiat_subunits as u128)
        .checked_mul(sats_per_fiat_unit as u128)
        .and_then(|value| value.checked_mul(buffered_bps as u128))
        .ok_or(SelfError::AmountOverflow)?;
    u64::try_from(div_ceil(
        numerator,
        (scale as u128).saturating_mul(BPS_DENOMINATOR as u128),
    ))
    .map_err(|_| SelfError::AmountOverflow)
}

fn fiat_subunits_for_sats(
    fiat_unit: &CurrencyUnit,
    sats: u64,
    sats_per_fiat_unit: u64,
    buffer_bps: u64,
) -> Result<u64, SelfError> {
    if sats_per_fiat_unit == 0 {
        return Err(SelfError::InvalidRate(sats_per_fiat_unit));
    }

    let scale = fiat_subunit_scale(fiat_unit)
        .ok_or_else(|| SelfError::UnsupportedFiatScale(fiat_unit.clone()))?;
    let buffered_bps = BPS_DENOMINATOR
        .checked_add(buffer_bps)
        .ok_or(SelfError::AmountOverflow)?;
    let numerator = (sats as u128)
        .checked_mul(scale as u128)
        .and_then(|value| value.checked_mul(buffered_bps as u128))
        .ok_or(SelfError::AmountOverflow)?;
    u64::try_from(div_ceil(
        numerator,
        (sats_per_fiat_unit as u128).saturating_mul(BPS_DENOMINATOR as u128),
    ))
    .map_err(|_| SelfError::AmountOverflow)
}

fn div_ceil(numerator: u128, denominator: u128) -> u128 {
    numerator / denominator + u128::from(numerator % denominator != 0)
}

fn record_total_fiat_subunits(record: &RateQuoteRecord) -> u64 {
    record
        .fiat_subunits
        .saturating_add(record.fiat_fee_subunits)
}

fn effective_expiry(ttl_secs: u64) -> u64 {
    unix_time().saturating_add(ttl_secs)
}

fn unix_time() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or_default()
}

#[derive(Debug, Serialize)]
struct StoredSnapshot<'a> {
    fiat_unit: &'a CurrencyUnit,
    collateral_unit: CurrencyUnit,
    fiat_subunits: u64,
    aggregated_rate_sats_per_fiat_unit: u64,
    source_readings: &'a [crate::types::SourceReading],
    aggregation_meta: &'a crate::types::AggregationMeta,
    buffer_bps: u64,
    sats_invoiced: u64,
    expiry_unix: u64,
}

fn snapshot_json(
    fiat_unit: &CurrencyUnit,
    fiat_subunits: u64,
    snapshot: &RateSnapshot,
    buffer_bps: u64,
    sats_invoiced: u64,
    expiry_unix: u64,
) -> Result<serde_json::Value, SelfError> {
    serde_json::to_value(StoredSnapshot {
        fiat_unit,
        collateral_unit: CurrencyUnit::Sat,
        fiat_subunits,
        aggregated_rate_sats_per_fiat_unit: snapshot.aggregated_rate,
        source_readings: &snapshot.source_readings,
        aggregation_meta: &snapshot.aggregation_meta,
        buffer_bps,
        sats_invoiced,
        expiry_unix,
    })
    .map_err(cdk_common::payment::Error::from)
    .map_err(SelfError::from)
}

fn merge_extra(
    existing: Option<serde_json::Value>,
    rate_snapshot: serde_json::Value,
) -> Option<serde_json::Value> {
    let mut base = existing.unwrap_or_else(|| serde_json::json!({}));
    match base.as_object_mut() {
        Some(object) => {
            object.insert("rate_snapshot".to_string(), rate_snapshot);
            Some(base)
        }
        None => Some(serde_json::json!({ "rate_snapshot": rate_snapshot })),
    }
}

#[cfg(test)]
mod tests {
    use std::time::SystemTime;

    use super::*;
    use crate::types::{AggregationMeta, SourceReading};

    #[test]
    fn sats_for_fiat_subunits_uses_whole_unit_rate() {
        let sats =
            sats_for_fiat_subunits(&CurrencyUnit::Usd, 100, 1_000, 100).expect("valid amount");
        assert_eq!(sats, 1010);
    }

    #[test]
    fn fiat_subunits_for_sats_applies_mint_favoring_buffer() {
        let cents =
            fiat_subunits_for_sats(&CurrencyUnit::Usd, 1_000, 1_000, 100).expect("valid amount");
        assert_eq!(cents, 101);
    }

    #[test]
    fn snapshot_json_contains_stored_terms() {
        let snapshot = RateSnapshot {
            fiat: CurrencyUnit::Usd,
            aggregated_rate: 17,
            source_readings: vec![SourceReading {
                source_name: "test".to_string(),
                rate: 17,
                fetched_at_age_secs: 0,
                source_reported_timestamp: None,
                included_in_aggregation: true,
            }],
            aggregation_meta: AggregationMeta {
                sources_fetched: 1,
                sources_trimmed: 0,
                sources_survived: 1,
                median_before_trim: 17,
                deviation_threshold_bps: 100,
            },
            created_at: SystemTime::now(),
        };

        let json = snapshot_json(&CurrencyUnit::Usd, 100, &snapshot, 100, 1717, 42)
            .expect("snapshot json");
        assert_eq!(json["fiat_subunits"], 100);
        assert_eq!(json["sats_invoiced"], 1717);
        assert_eq!(json["buffer_bps"], 100);
        assert_eq!(json["aggregated_rate_sats_per_fiat_unit"], 17);
    }

    #[test]
    fn config_defaults_to_90_second_ttl_and_100_bps_buffer() {
        let config = RateConvertingPaymentConfig::default();
        assert_eq!(config.ttl_secs, DEFAULT_RATE_QUOTE_TTL_SECS);
        assert_eq!(config.buffer_bps, 100);
    }

    #[test]
    fn effective_expiry_uses_decorator_ttl() {
        let before = unix_time();
        let expiry = effective_expiry(77);
        assert!(expiry >= before.saturating_add(77));
        assert!(expiry <= unix_time().saturating_add(77));
    }

    #[tokio::test]
    async fn quote_control_rejects_paused_mint_unit() {
        let control = RateQuoteControlHandle::new();
        control
            .set_unit_quote_state(CurrencyUnit::Usd, true, false)
            .await
            .expect("set pause state");

        let error = control
            .ensure_not_paused(&CurrencyUnit::Usd, QuoteSide::Mint)
            .await
            .expect_err("mint side should be paused");
        assert!(matches!(
            error,
            RateConvertingPaymentError::UnitPaused { side: "mint", .. }
        ));
        control
            .ensure_not_paused(&CurrencyUnit::Usd, QuoteSide::Melt)
            .await
            .expect("melt side should stay open");
    }

    #[tokio::test]
    async fn quote_control_reserves_against_cap() {
        let control = RateQuoteControlHandle::with_buffer_bps(100);
        control
            .set_unit_issuance_cap(CurrencyUnit::Usd, 100)
            .await
            .expect("set cap");

        control
            .reserve(&CurrencyUnit::Usd, "first", 80, unix_time() + 60)
            .await
            .expect("first reservation");
        let error = control
            .reserve(&CurrencyUnit::Usd, "second", 21, unix_time() + 60)
            .await
            .expect_err("cap should reject over-reservation");

        assert!(matches!(
            error,
            RateConvertingPaymentError::IssuanceCapExceeded {
                requested: 21,
                available: 20,
                ..
            }
        ));
    }

    #[tokio::test]
    async fn quote_control_rejects_nonzero_cap_without_buffer() {
        let control = RateQuoteControlHandle::new();
        let error = control
            .set_unit_issuance_cap(CurrencyUnit::Usd, 100)
            .await
            .expect_err("nonzero cap without buffer must fail");
        assert!(matches!(error, RateQuoteStoreError::InvalidControl(_)));
    }

    #[tokio::test]
    async fn quote_control_zero_cap_fails_closed() {
        // A cap of 0 (including the never-configured default) refuses every
        // request — it never means unlimited.
        let control = RateQuoteControlHandle::new();

        let error = control
            .reserve(&CurrencyUnit::Usd, "any", 1, unix_time() + 60)
            .await
            .expect_err("unset cap must refuse");
        assert!(matches!(
            error,
            RateConvertingPaymentError::IssuanceCapExceeded { available: 0, .. }
        ));

        control
            .set_unit_issuance_cap(CurrencyUnit::Usd, 0)
            .await
            .expect("set cap");
        control
            .reserve(&CurrencyUnit::Usd, "any", 0, unix_time() + 60)
            .await
            .expect_err("explicit zero cap must refuse even zero-amount requests");
    }

    #[tokio::test]
    async fn quote_control_releases_reservation_after_expiry() {
        let control = RateQuoteControlHandle::with_buffer_bps(100);
        control
            .set_unit_issuance_cap(CurrencyUnit::Usd, 100)
            .await
            .expect("set cap");

        // An already-expired reservation occupies the full cap until the
        // lazy sweep on the next reserve call vacates it.
        control
            .reserve(&CurrencyUnit::Usd, "first", 100, unix_time())
            .await
            .expect("first reservation");

        control
            .reserve(&CurrencyUnit::Usd, "second", 100, unix_time() + 60)
            .await
            .expect("expired reservation should be swept and released");
    }

    #[tokio::test]
    async fn quote_control_counts_outstanding_against_cap() {
        let control = RateQuoteControlHandle::with_buffer_bps(100);
        control
            .set_unit_issuance_cap(CurrencyUnit::Usd, 150)
            .await
            .expect("set cap");

        control
            .reserve(&CurrencyUnit::Usd, "first", 100, unix_time() + 60)
            .await
            .expect("reservation under cap");
        control
            .commit_issued(&CurrencyUnit::Usd, "first", 100, 7)
            .await;

        assert_eq!(control.outstanding(&CurrencyUnit::Usd).await, 100);
        assert_eq!(control.buffer_surplus_sats(&CurrencyUnit::Usd).await, 7);

        control
            .reserve(&CurrencyUnit::Usd, "second", 60, unix_time() + 60)
            .await
            .expect_err("outstanding issuance must count against the cap");
        control
            .reserve(&CurrencyUnit::Usd, "second", 50, unix_time() + 60)
            .await
            .expect("headroom after outstanding remains usable");

        control.commit_melted(&CurrencyUnit::Usd, 30).await;
        assert_eq!(control.outstanding(&CurrencyUnit::Usd).await, 70);
    }
}
