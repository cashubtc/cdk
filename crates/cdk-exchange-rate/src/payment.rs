//! Rate-converting [`MintPayment`](cdk_common::payment::MintPayment) decorator.

use std::collections::{HashMap, HashSet};
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use cdk_common::nuts::CurrencyUnit;
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
use crate::store::{DynRateQuoteStore, ParkedPaymentRecord, RateQuoteRecord, RateQuoteStoreError};
use crate::types::{RateOracleError, RateSnapshot};

/// Number of basis points in 100%.
const BPS_DENOMINATOR: u64 = 10_000;

/// Default rate-quoted invoice TTL in seconds.
pub const DEFAULT_RATE_QUOTE_TTL_SECS: u64 = 120;

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
            buffer_bps: 0,
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
    /// Invalid rate.
    #[error("invalid rate {0}")]
    InvalidRate(f64),
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
            RateConvertingPaymentError::UnsupportedUnit(_) => Self::UnsupportedUnit,
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

#[derive(Debug, Default)]
struct UnitReservationState {
    quote_state: UnitQuoteState,
    cap: u64,
    reserved: u64,
    reservations: HashSet<String>,
}

/// Shared control handle used by the decorator and management RPC.
#[derive(Debug, Clone, Default)]
pub struct RateQuoteControlHandle {
    units: Arc<Mutex<HashMap<CurrencyUnit, UnitReservationState>>>,
}

impl RateQuoteControlHandle {
    /// Create an empty rate-quote control handle.
    pub fn new() -> Self {
        Self::default()
    }

    /// Set pause state for one exposed unit.
    pub async fn set_unit_quote_state(
        &self,
        unit: CurrencyUnit,
        mint_paused: bool,
        melt_paused: bool,
    ) {
        let mut units = self.units.lock().await;
        let state = units.entry(unit).or_default();
        state.quote_state = UnitQuoteState {
            mint_paused,
            melt_paused,
        };
    }

    /// Set the issuance cap for one exposed unit. A cap of `0` means unlimited.
    pub async fn set_unit_issuance_cap(&self, unit: CurrencyUnit, cap: u64) {
        let mut units = self.units.lock().await;
        let state = units.entry(unit).or_default();
        state.cap = cap;
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

    async fn reserve(
        &self,
        unit: &CurrencyUnit,
        payment_lookup_id: &PaymentIdentifier,
        fiat_subunits: u64,
        expiry_unix: u64,
    ) -> Result<(), RateConvertingPaymentError> {
        let mut units = self.units.lock().await;
        let state = units.entry(unit.clone()).or_default();
        if state.cap > 0 {
            let available = state.cap.saturating_sub(state.reserved);
            if fiat_subunits > available {
                return Err(RateConvertingPaymentError::IssuanceCapExceeded {
                    unit: unit.clone(),
                    requested: fiat_subunits,
                    available,
                });
            }
        }
        let key = payment_lookup_id.to_string();
        if state.reservations.insert(key.clone()) {
            state.reserved = state.reserved.saturating_add(fiat_subunits);
        }
        drop(units);

        self.release_after_expiry(unit.clone(), key, fiat_subunits, expiry_unix);
        Ok(())
    }

    async fn release(
        &self,
        unit: &CurrencyUnit,
        payment_lookup_id: &PaymentIdentifier,
        fiat_subunits: u64,
    ) {
        let mut units = self.units.lock().await;
        let Some(state) = units.get_mut(unit) else {
            return;
        };
        if state.reservations.remove(&payment_lookup_id.to_string()) {
            state.reserved = state.reserved.saturating_sub(fiat_subunits);
        }
    }

    fn release_after_expiry(
        &self,
        unit: CurrencyUnit,
        payment_lookup_id: String,
        fiat_subunits: u64,
        expiry_unix: u64,
    ) {
        let handle = self.clone();
        tokio::spawn(async move {
            let now = unix_time();
            tokio::time::sleep(Duration::from_secs(expiry_unix.saturating_sub(now))).await;
            let mut units = handle.units.lock().await;
            let Some(state) = units.get_mut(&unit) else {
                return;
            };
            if state.reservations.remove(&payment_lookup_id) {
                state.reserved = state.reserved.saturating_sub(fiat_subunits);
            }
        });
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
            config,
            control: RateQuoteControlHandle::new(),
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

    async fn quote_terms(&self, fiat_subunits: u64) -> Result<(RateSnapshot, u64), SelfError> {
        let snapshot = self.oracle.snapshot(&self.config.fiat_unit).await?;
        let sats = sats_for_fiat(
            fiat_subunits,
            snapshot.aggregated_rate,
            self.config.buffer_bps,
        )?;
        Ok((snapshot, sats))
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
        let (snapshot, sats_invoiced) = self.quote_terms(fiat_subunits).await?;
        let expiry_unix = effective_expiry(self.config.ttl_secs);
        options.amount = Amount::new(sats_invoiced, CurrencyUnit::Sat);
        options.unix_expiry = Some(expiry_unix);

        let mut response = self
            .inner
            .create_incoming_payment_request(IncomingPaymentOptions::Bolt11(options))
            .await?;

        let snapshot_json = snapshot_json(
            &self.config.fiat_unit,
            fiat_subunits,
            &snapshot,
            self.config.buffer_bps,
            sats_invoiced,
            expiry_unix,
        )?;

        let record = RateQuoteRecord {
            payment_lookup_id: response.request_lookup_id.clone(),
            fiat_unit: self.config.fiat_unit.clone(),
            fiat_subunits,
            snapshot_json: snapshot_json.clone(),
            sats_invoiced,
            expiry_unix,
        };
        self.store.insert(record).await?;
        self.control
            .reserve(
                &self.config.fiat_unit,
                &response.request_lookup_id,
                fiat_subunits,
                expiry_unix,
            )
            .await?;
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
        let fiat_subunits = inner_quote.amount.value();
        let (snapshot, sats_reserved) = self.quote_terms(fiat_subunits).await?;
        let expiry_unix = unix_time().saturating_add(self.config.ttl_secs);
        let snapshot_json = snapshot_json(
            &self.config.fiat_unit,
            fiat_subunits,
            &snapshot,
            self.config.buffer_bps,
            sats_reserved,
            expiry_unix,
        )?;

        if let Some(payment_lookup_id) = inner_quote.request_lookup_id.clone() {
            self.store
                .insert(RateQuoteRecord {
                    payment_lookup_id: payment_lookup_id.clone(),
                    fiat_unit: self.config.fiat_unit.clone(),
                    fiat_subunits,
                    snapshot_json: snapshot_json.clone(),
                    sats_invoiced: sats_reserved,
                    expiry_unix,
                })
                .await?;
            self.control
                .reserve(
                    &self.config.fiat_unit,
                    &payment_lookup_id,
                    fiat_subunits,
                    expiry_unix,
                )
                .await?;
        }

        Ok(PaymentQuoteResponse {
            request_lookup_id: inner_quote.request_lookup_id,
            amount: Amount::new(fiat_subunits, self.config.fiat_unit.clone()),
            fee: Amount::new(inner_quote.fee.value(), self.config.fiat_unit.clone()),
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
        self.control
            .release(
                &record.fiat_unit,
                &response.payment_lookup_id,
                record.fiat_subunits,
            )
            .await;

        Ok(MakePaymentResponse {
            payment_lookup_id: response.payment_lookup_id,
            payment_proof: response.payment_proof,
            status: response.status,
            total_spent: Amount::new(record.fiat_subunits, record.fiat_unit),
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
        self.control
            .release(
                &record.fiat_unit,
                &response.payment_lookup_id,
                record.fiat_subunits,
            )
            .await;

        Ok(MakePaymentResponse {
            payment_lookup_id: response.payment_lookup_id,
            payment_proof: response.payment_proof,
            status: response.status,
            total_spent: Amount::new(record.fiat_subunits, record.fiat_unit),
        })
    }
}

async fn convert_payment_event(
    store: DynRateQuoteStore,
    control: RateQuoteControlHandle,
    fiat_unit: CurrencyUnit,
    payment: WaitPaymentResponse,
) -> Option<Event> {
    match store.get_by_lookup_id(&payment.payment_identifier).await {
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
            control
                .release(
                    &record.fiat_unit,
                    &payment.payment_identifier,
                    record.fiat_subunits,
                )
                .await;

            Some(Event::PaymentReceived(WaitPaymentResponse {
                payment_identifier: payment.payment_identifier,
                payment_amount: Amount::new(record.fiat_subunits, record.fiat_unit),
                payment_id: payment.payment_id,
            }))
        }
        Ok(None) => {
            let parked = ParkedPaymentRecord {
                payment_lookup_id: payment.payment_identifier.clone(),
                bolt11_payment_hash: payment.payment_id.clone(),
                received_sats: payment.payment_amount.value(),
                observed_at: unix_time(),
                resolution_status: "parked".to_string(),
            };
            if let Err(error) = store.insert_parked(parked).await {
                tracing::error!(
                    payment_lookup_id = %payment.payment_identifier,
                    bolt11_payment_hash = payment.payment_id,
                    error = %error,
                    "failed to park orphaned rate-converted payment"
                );
            } else {
                tracing::warn!(
                    payment_lookup_id = %payment.payment_identifier,
                    bolt11_payment_hash = payment.payment_id,
                    fiat_unit = %fiat_unit,
                    "parked orphaned rate-converted payment"
                );
            }
            None
        }
        Err(error) => {
            tracing::warn!(
                payment_lookup_id = %payment.payment_identifier,
                error = %error,
                "suppressing rate-converted payment after quote-store lookup failure"
            );
            None
        }
    }
}

fn sats_for_fiat(
    fiat_subunits: u64,
    sats_per_fiat_subunit: f64,
    buffer_bps: u64,
) -> Result<u64, SelfError> {
    if !sats_per_fiat_subunit.is_finite() || sats_per_fiat_subunit <= 0.0 {
        return Err(SelfError::InvalidRate(sats_per_fiat_subunit));
    }

    let buffered = sats_per_fiat_subunit
        * fiat_subunits as f64
        * (BPS_DENOMINATOR
            .checked_add(buffer_bps)
            .ok_or(SelfError::AmountOverflow)? as f64
            / BPS_DENOMINATOR as f64);
    if !buffered.is_finite() || buffered > u64::MAX as f64 {
        return Err(SelfError::AmountOverflow);
    }
    Ok(buffered.ceil() as u64)
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
    aggregated_rate_sats_per_fiat_subunit: f64,
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
        aggregated_rate_sats_per_fiat_subunit: snapshot.aggregated_rate,
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
    fn sats_for_fiat_applies_ceil_and_buffer() {
        let sats = sats_for_fiat(100, 15.2, 100).expect("valid amount");
        assert_eq!(sats, 1536);
    }

    #[test]
    fn snapshot_json_contains_stored_terms() {
        let snapshot = RateSnapshot {
            fiat: CurrencyUnit::Usd,
            aggregated_rate: 17.0,
            source_readings: vec![SourceReading {
                source_name: "test".to_string(),
                rate: 17.0,
                fetched_at_age_secs: 0,
                source_reported_timestamp: None,
                included_in_aggregation: true,
            }],
            aggregation_meta: AggregationMeta {
                sources_fetched: 1,
                sources_trimmed: 0,
                sources_survived: 1,
                median_before_trim: 17.0,
                deviation_threshold_pct: 1.0,
            },
            created_at: SystemTime::now(),
        };

        let json = snapshot_json(&CurrencyUnit::Usd, 100, &snapshot, 100, 1717, 42)
            .expect("snapshot json");
        assert_eq!(json["fiat_subunits"], 100);
        assert_eq!(json["sats_invoiced"], 1717);
        assert_eq!(json["buffer_bps"], 100);
    }

    #[test]
    fn config_defaults_to_120_second_ttl() {
        let config = RateConvertingPaymentConfig::default();
        assert_eq!(config.ttl_secs, DEFAULT_RATE_QUOTE_TTL_SECS);
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
            .await;

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
        let control = RateQuoteControlHandle::new();
        control.set_unit_issuance_cap(CurrencyUnit::Usd, 100).await;
        let first = PaymentIdentifier::CustomId("first".to_string());
        let second = PaymentIdentifier::CustomId("second".to_string());

        control
            .reserve(
                &CurrencyUnit::Usd,
                &first,
                80,
                unix_time().saturating_add(60),
            )
            .await
            .expect("first reservation");
        let error = control
            .reserve(
                &CurrencyUnit::Usd,
                &second,
                21,
                unix_time().saturating_add(60),
            )
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

    #[tokio::test(start_paused = true)]
    async fn quote_control_releases_reservation_after_expiry() {
        let control = RateQuoteControlHandle::new();
        control.set_unit_issuance_cap(CurrencyUnit::Usd, 100).await;
        let first = PaymentIdentifier::CustomId("first".to_string());
        let second = PaymentIdentifier::CustomId("second".to_string());

        control
            .reserve(
                &CurrencyUnit::Usd,
                &first,
                100,
                unix_time().saturating_add(2),
            )
            .await
            .expect("first reservation");
        control
            .reserve(
                &CurrencyUnit::Usd,
                &second,
                1,
                unix_time().saturating_add(2),
            )
            .await
            .expect_err("cap should initially reject");

        tokio::task::yield_now().await;
        tokio::time::advance(Duration::from_secs(3)).await;
        tokio::task::yield_now().await;

        control
            .reserve(
                &CurrencyUnit::Usd,
                &second,
                1,
                unix_time().saturating_add(60),
            )
            .await
            .expect("expired reservation should be released");
    }
}
