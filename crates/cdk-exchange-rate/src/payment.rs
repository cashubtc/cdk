//! Rate-converting [`MintPayment`](cdk_common::payment::MintPayment) decorator.

use std::pin::Pin;
use std::sync::Arc;

use async_trait::async_trait;
use cdk_common::nuts::CurrencyUnit;
use cdk_common::payment::{
    CreateIncomingPaymentResponse, Event, IncomingPaymentOptions, MakePaymentResponse, MintPayment,
    OutgoingPaymentOptions, PaymentIdentifier, PaymentQuoteResponse, SettingsResponse,
    WaitPaymentResponse,
};
use cdk_common::Amount;
use futures::{Stream, StreamExt};
use serde::Serialize;

use crate::oracle::RateOracle;
use crate::store::{DynRateQuoteStore, ParkedPaymentRecord, RateQuoteRecord, RateQuoteStoreError};
use crate::types::{RateOracleError, RateSnapshot};

/// Number of basis points in 100%.
const BPS_DENOMINATOR: u64 = 10_000;

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
}

impl From<RateConvertingPaymentError> for cdk_common::payment::Error {
    fn from(error: RateConvertingPaymentError) -> Self {
        match error {
            RateConvertingPaymentError::Inner(inner) => inner,
            RateConvertingPaymentError::UnsupportedPaymentOption => Self::UnsupportedPaymentOption,
            RateConvertingPaymentError::UnsupportedUnit(_) => Self::UnsupportedUnit,
            other => Self::Custom(other.to_string()),
        }
    }
}

/// Decorates a sat payment backend as a fiat-denominated payment processor.
#[derive(Clone)]
pub struct RateConvertingPayment<T> {
    inner: T,
    oracle: Arc<dyn RateOracle>,
    store: DynRateQuoteStore,
    config: RateConvertingPaymentConfig,
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
        }
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

        let fiat_subunits = options.amount.value();
        let (snapshot, sats_invoiced) = self.quote_terms(fiat_subunits).await?;
        let expiry_unix = effective_expiry(options.unix_expiry, self.config.ttl_secs);
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
                    payment_lookup_id,
                    fiat_unit: self.config.fiat_unit.clone(),
                    fiat_subunits,
                    snapshot_json: snapshot_json.clone(),
                    sats_invoiced: sats_reserved,
                    expiry_unix,
                })
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
        let fiat_unit = self.config.fiat_unit.clone();

        Ok(Box::pin(stream.filter_map(move |event| {
            let store = Arc::clone(&store);
            let fiat_unit = fiat_unit.clone();
            async move {
                match event {
                    Event::PaymentReceived(payment) => {
                        convert_payment_event(store, fiat_unit, payment).await
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

fn effective_expiry(requested_expiry: Option<u64>, ttl_secs: u64) -> u64 {
    let decorator_expiry = unix_time().saturating_add(ttl_secs);
    requested_expiry
        .map(|requested| requested.min(decorator_expiry))
        .unwrap_or(decorator_expiry)
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
}
