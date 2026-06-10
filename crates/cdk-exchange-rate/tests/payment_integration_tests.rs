//! Integration tests for the rate-converting payment decorator.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use async_trait::async_trait;
use cdk_common::common::FeeReserve;
use cdk_common::nuts::CurrencyUnit;
use cdk_common::payment::{Bolt11IncomingPaymentOptions, IncomingPaymentOptions, MintPayment};
use cdk_common::Amount;
use cdk_exchange_rate::{
    parked_payment_event_count, InMemoryRateQuoteStore, RateConvertingPayment,
    RateConvertingPaymentConfig, RateOracle, RateOracleError, RateQuoteControlHandle,
    RateQuoteStore, RateSnapshot,
};
use cdk_fake_wallet::FakeWallet;
use futures::StreamExt;

#[derive(Debug)]
struct FixedOracle {
    sats_per_fiat_subunit: f64,
}

#[async_trait]
impl RateOracle for FixedOracle {
    async fn snapshot(&self, fiat: &CurrencyUnit) -> Result<RateSnapshot, RateOracleError> {
        Ok(RateSnapshot {
            fiat: fiat.clone(),
            aggregated_rate: self.sats_per_fiat_subunit,
            source_readings: Vec::new(),
            aggregation_meta: cdk_exchange_rate::types::AggregationMeta {
                sources_fetched: 1,
                sources_trimmed: 0,
                sources_survived: 1,
                median_before_trim: self.sats_per_fiat_subunit,
                deviation_threshold_pct: 0.0,
            },
            created_at: SystemTime::now(),
        })
    }
}

fn fake_wallet(payment_delay: u64) -> FakeWallet {
    FakeWallet::new(
        FeeReserve {
            min_fee_reserve: Amount::new(0, CurrencyUnit::Sat).into(),
            percent_fee_reserve: 0.0,
        },
        HashMap::new(),
        HashSet::new(),
        payment_delay,
        CurrencyUnit::Sat,
    )
}

fn processor(
    payment_delay: u64,
    store: InMemoryRateQuoteStore,
    control: RateQuoteControlHandle,
    ttl_secs: u64,
) -> RateConvertingPayment<FakeWallet> {
    RateConvertingPayment::with_control(
        fake_wallet(payment_delay),
        Arc::new(FixedOracle {
            sats_per_fiat_subunit: 2.0,
        }),
        Arc::new(store),
        RateConvertingPaymentConfig::new(CurrencyUnit::Usd, 0, ttl_secs),
        control,
    )
}

fn mint_quote(amount: u64) -> IncomingPaymentOptions {
    IncomingPaymentOptions::Bolt11(Bolt11IncomingPaymentOptions {
        description: None,
        amount: Amount::new(amount, CurrencyUnit::Usd),
        unix_expiry: None,
    })
}

#[tokio::test]
async fn usd_mint_quote_persists_snapshot_and_credits_quoted_usd() {
    let store = InMemoryRateQuoteStore::new();
    let processor = processor(0, store.clone(), RateQuoteControlHandle::new(), 120);
    let mut stream = processor
        .wait_payment_event()
        .await
        .expect("payment stream");

    let quote = processor
        .create_incoming_payment_request(mint_quote(123))
        .await
        .expect("mint quote");
    let record = store
        .get_by_lookup_id(&quote.request_lookup_id)
        .await
        .expect("store lookup")
        .expect("stored quote terms");

    assert_eq!(record.fiat_subunits, 123);
    assert_eq!(record.sats_invoiced, 246);
    assert_eq!(record.snapshot_json["fiat_subunits"], 123);

    let event = tokio::time::timeout(Duration::from_secs(1), stream.next())
        .await
        .expect("auto-paid event")
        .expect("event");
    let cdk_common::payment::Event::PaymentReceived(payment) = event else {
        panic!("expected payment event");
    };

    assert_eq!(payment.payment_amount, Amount::new(123, CurrencyUnit::Usd));
}

#[tokio::test]
async fn late_payment_uses_stored_terms_after_expiry() {
    let store = InMemoryRateQuoteStore::new();
    let processor = processor(2, store, RateQuoteControlHandle::new(), 1);
    let mut stream = processor
        .wait_payment_event()
        .await
        .expect("payment stream");

    processor
        .create_incoming_payment_request(mint_quote(77))
        .await
        .expect("mint quote");

    let event = tokio::time::timeout(Duration::from_secs(3), stream.next())
        .await
        .expect("late payment event")
        .expect("event");
    let cdk_common::payment::Event::PaymentReceived(payment) = event else {
        panic!("expected payment event");
    };

    assert_eq!(payment.payment_amount, Amount::new(77, CurrencyUnit::Usd));
}

#[tokio::test]
async fn parked_payment_suppresses_upstream_event_after_quote_store_failure() {
    let store = InMemoryRateQuoteStore::new();
    store.fail_next_insert().await;
    let processor = processor(0, store.clone(), RateQuoteControlHandle::new(), 120);
    let mut stream = processor
        .wait_payment_event()
        .await
        .expect("payment stream");
    let before = parked_payment_event_count();

    processor
        .create_incoming_payment_request(mint_quote(50))
        .await
        .expect_err("forced quote-store failure");

    let suppressed = tokio::time::timeout(Duration::from_millis(50), stream.next()).await;
    assert!(suppressed.is_err(), "orphaned payment must be suppressed");
    let parked = store.parked_payments().await;
    assert_eq!(parked.len(), 1);
    assert_eq!(parked[0].received_sats, 100);
    assert_eq!(parked_payment_event_count(), before + 1);
}

#[tokio::test]
async fn cap_reservation_rejects_then_releases_after_expiry() {
    let store = InMemoryRateQuoteStore::new();
    let control = RateQuoteControlHandle::new();
    control.set_unit_issuance_cap(CurrencyUnit::Usd, 100).await;
    let processor = processor(30, store, control, 1);

    processor
        .create_incoming_payment_request(mint_quote(100))
        .await
        .expect("quote up to cap");
    processor
        .create_incoming_payment_request(mint_quote(1))
        .await
        .expect_err("cap should reject next quote");

    tokio::time::sleep(Duration::from_millis(1100)).await;

    processor
        .create_incoming_payment_request(mint_quote(1))
        .await
        .expect("expired reservation should release cap");
}

#[tokio::test]
async fn pause_state_blocks_and_unblocks_mint_quotes() {
    let store = InMemoryRateQuoteStore::new();
    let control = RateQuoteControlHandle::new();
    let processor = processor(30, store, control.clone(), 120);

    control
        .set_unit_quote_state(CurrencyUnit::Usd, true, false)
        .await;
    processor
        .create_incoming_payment_request(mint_quote(1))
        .await
        .expect_err("paused unit should reject mint quote");

    control
        .set_unit_quote_state(CurrencyUnit::Usd, false, false)
        .await;
    processor
        .create_incoming_payment_request(mint_quote(1))
        .await
        .expect("unpaused unit should accept mint quote");
}

#[test]
#[ignore = "WS6.6: needs full mintd"]
fn forced_melt_failure_releases_proof_reservation() {
    // WS6.6: needs full mintd
}
