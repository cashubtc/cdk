//! Integration tests for the rate-converting payment decorator.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use async_trait::async_trait;
use cdk_common::common::FeeReserve;
use cdk_common::nuts::{CurrencyUnit, MeltQuoteState};
use cdk_common::payment::{
    Bolt11IncomingPaymentOptions, Bolt11OutgoingPaymentOptions, IncomingPaymentOptions,
    MintPayment, OutgoingPaymentOptions,
};
use cdk_common::{Amount, Bolt11Invoice};
use cdk_exchange_rate::{
    parked_payment_event_count, DynRateQuoteStore, InMemoryRateQuoteStore, RateConvertingPayment,
    RateConvertingPaymentConfig, RateConvertingPaymentError, RateOracle, RateOracleError,
    RateQuoteControlHandle, RateQuoteStore, RateSnapshot,
};
use cdk_fake_wallet::{FakeInvoiceDescription, FakeWallet};
use futures::StreamExt;

#[derive(Debug)]
struct FixedOracle {
    sats_per_fiat_unit: u64,
}

#[async_trait]
impl RateOracle for FixedOracle {
    async fn snapshot(&self, fiat: &CurrencyUnit) -> Result<RateSnapshot, RateOracleError> {
        Ok(RateSnapshot {
            fiat: fiat.clone(),
            aggregated_rate: self.sats_per_fiat_unit,
            source_readings: Vec::new(),
            aggregation_meta: cdk_exchange_rate::types::AggregationMeta {
                sources_fetched: 1,
                sources_trimmed: 0,
                sources_survived: 1,
                median_before_trim: self.sats_per_fiat_unit,
                deviation_threshold_bps: 0,
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
            sats_per_fiat_unit: 1_000,
        }),
        Arc::new(store),
        RateConvertingPaymentConfig::new(CurrencyUnit::Usd, 100, ttl_secs),
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

/// Control handle with an open USD issuance cap. The default cap of 0 fails
/// closed, so every test that mints must opt in to headroom explicitly.
async fn open_control(cap: u64) -> RateQuoteControlHandle {
    let control = RateQuoteControlHandle::new();
    control
        .set_unit_issuance_cap(CurrencyUnit::Usd, cap)
        .await
        .expect("set cap");
    control
}

async fn next_payment_received(
    stream: &mut (impl futures::Stream<Item = cdk_common::payment::Event> + Unpin),
) -> cdk_common::payment::WaitPaymentResponse {
    let event = tokio::time::timeout(Duration::from_secs(2), stream.next())
        .await
        .expect("payment event in time")
        .expect("event");
    let cdk_common::payment::Event::PaymentReceived(payment) = event else {
        panic!("expected payment event");
    };
    payment
}

#[tokio::test]
async fn usd_mint_quote_uses_real_unit_semantics() {
    // Rate contract (B2): the oracle rate is sats per WHOLE fiat unit. With
    // BTC at $100,000, one USD is 1,000 sats. A 100-cent ($1.00) mint quote
    // with a 100 bps buffer must invoice ceil(1,000 × 1.01) = 1,010 sats and
    // credit exactly the quoted 100 cents when the invoice is paid.
    let store = InMemoryRateQuoteStore::new();
    let processor = processor(0, store.clone(), open_control(10_000).await, 120);
    let mut stream = processor
        .wait_payment_event()
        .await
        .expect("payment stream");

    let quote = processor
        .create_incoming_payment_request(mint_quote(100))
        .await
        .expect("mint quote");
    let record = store
        .get_by_lookup_id(&quote.request_lookup_id)
        .await
        .expect("store lookup")
        .expect("stored quote terms");

    assert_eq!(record.fiat_subunits, 100);
    assert_eq!(record.sats_invoiced, 1010);
    assert_eq!(
        record.snapshot_json["aggregated_rate_sats_per_fiat_unit"],
        1_000
    );

    let event = tokio::time::timeout(Duration::from_secs(1), stream.next())
        .await
        .expect("auto-paid event")
        .expect("event");
    let cdk_common::payment::Event::PaymentReceived(payment) = event else {
        panic!("expected payment event");
    };

    assert_eq!(payment.payment_amount, Amount::new(100, CurrencyUnit::Usd));
}

#[tokio::test]
async fn usd_mint_quote_persists_snapshot_and_credits_quoted_usd() {
    let store = InMemoryRateQuoteStore::new();
    let processor = processor(0, store.clone(), open_control(10_000).await, 120);
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
    assert_eq!(record.sats_invoiced, 1243);
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

async fn sat_invoice(sats: u64) -> Bolt11Invoice {
    sat_invoice_with_description(sats, None).await
}

async fn sat_invoice_with_description(sats: u64, description: Option<String>) -> Bolt11Invoice {
    // Large payment delay: this wallet only mints the invoice; its simulated
    // payment must never interfere with the test.
    let invoice_wallet = fake_wallet(86_400);
    let invoice = invoice_wallet
        .create_incoming_payment_request(IncomingPaymentOptions::Bolt11(
            Bolt11IncomingPaymentOptions {
                description,
                amount: Amount::new(sats, CurrencyUnit::Sat),
                unix_expiry: None,
            },
        ))
        .await
        .expect("sat invoice");
    invoice.request.parse().expect("bolt11")
}

fn melt_options(bolt11: Bolt11Invoice) -> OutgoingPaymentOptions {
    OutgoingPaymentOptions::Bolt11(Box::new(Bolt11OutgoingPaymentOptions {
        bolt11,
        max_fee_amount: None,
        timeout_secs: None,
        melt_options: None,
    }))
}

#[tokio::test]
async fn usd_melt_quote_converts_sats_to_fiat_mint_favoring() {
    // B3: melt converts the bolt11's SAT amount into fiat subunits in the
    // MINT-FAVORING direction: fiat = ceil(sats × scale × (1 + buffer) / rate).
    // 990 sats at 1,000 sats/USD with a 100 bps buffer:
    // ceil(990 × 100 × 1.01 / 1,000) = ceil(99.99) = 100 cents — strictly
    // more fiat than the unbuffered floor of 99 cents.
    let bolt11 = sat_invoice(990).await;
    let store = InMemoryRateQuoteStore::new();
    let processor = processor(0, store.clone(), RateQuoteControlHandle::new(), 120);

    let quote = processor
        .get_payment_quote(&CurrencyUnit::Usd, melt_options(bolt11))
        .await
        .expect("melt quote");

    assert_eq!(quote.amount, Amount::new(100, CurrencyUnit::Usd));
    assert_eq!(quote.fee, Amount::new(0, CurrencyUnit::Usd));
}

#[tokio::test]
async fn late_payment_uses_stored_terms_after_expiry() {
    let store = InMemoryRateQuoteStore::new();
    let processor = processor(2, store, open_control(10_000).await, 1);
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
    let processor = processor(0, store.clone(), open_control(10_000).await, 120);
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
    assert_eq!(parked[0].received_sats, 505);
    assert_eq!(parked_payment_event_count(), before + 1);
}

#[tokio::test]
async fn cap_reservation_rejects_then_releases_after_expiry() {
    let store = InMemoryRateQuoteStore::new();
    let control = open_control(100).await;
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
    let control = open_control(10_000).await;
    let processor = processor(30, store, control.clone(), 120);

    control
        .set_unit_quote_state(CurrencyUnit::Usd, true, false)
        .await
        .expect("pause mint side");
    processor
        .create_incoming_payment_request(mint_quote(1))
        .await
        .expect_err("paused unit should reject mint quote");

    control
        .set_unit_quote_state(CurrencyUnit::Usd, false, false)
        .await
        .expect("unpause mint side");
    processor
        .create_incoming_payment_request(mint_quote(1))
        .await
        .expect("unpaused unit should accept mint quote");
}

#[tokio::test]
async fn zero_cap_refuses_mint_quotes_fail_closed() {
    // B5: a never-configured (or explicitly zero) issuance cap refuses all
    // new mint quotes. Zero is fail-closed, never unlimited.
    let store = InMemoryRateQuoteStore::new();
    let processor = processor(30, store, RateQuoteControlHandle::new(), 120);

    let error = processor
        .create_incoming_payment_request(mint_quote(1))
        .await
        .expect_err("zero cap must refuse");
    assert!(matches!(
        error,
        RateConvertingPaymentError::IssuanceCapExceeded { available: 0, .. }
    ));
}

#[tokio::test]
async fn outstanding_issuance_counts_against_cap() {
    // B4: the cap covers persisted outstanding issuance (issued minus
    // melted) plus pending reservations plus the new request — paying a
    // quote consumes cap headroom permanently until the USD is melted.
    let store = InMemoryRateQuoteStore::new();
    let control = open_control(150).await;
    let processor = processor(0, store.clone(), control.clone(), 120);
    let mut stream = processor
        .wait_payment_event()
        .await
        .expect("payment stream");

    processor
        .create_incoming_payment_request(mint_quote(100))
        .await
        .expect("quote under cap");
    next_payment_received(&mut stream).await;

    assert_eq!(control.outstanding(&CurrencyUnit::Usd).await, 100);
    // M4: the buffer portion of the paid sats lands in the per-unit surplus
    // reserve: invoiced 1,010 sats, unbuffered 1,000 → 10 sats of reserve.
    assert_eq!(control.buffer_surplus_sats(&CurrencyUnit::Usd).await, 10);

    processor
        .create_incoming_payment_request(mint_quote(60))
        .await
        .expect_err("outstanding issuance must keep counting against the cap");
    processor
        .create_incoming_payment_request(mint_quote(50))
        .await
        .expect("headroom after outstanding remains usable");
}

#[tokio::test]
async fn failed_quote_persistence_releases_cap_reservation() {
    // M2: the cap is reserved BEFORE the inner invoice is created and
    // released on every failure path — a failed quote must not leak its
    // reservation and brick the cap until expiry.
    let store = InMemoryRateQuoteStore::new();
    let control = open_control(100).await;
    let processor = processor(30, store.clone(), control, 120);

    store.fail_next_insert().await;
    processor
        .create_incoming_payment_request(mint_quote(100))
        .await
        .expect_err("forced store failure");

    processor
        .create_incoming_payment_request(mint_quote(100))
        .await
        .expect("released reservation leaves the full cap available");
}

#[tokio::test]
async fn unit_control_state_survives_restart_via_store() {
    // M5: pause flags, the cap, outstanding issuance, and the buffer-surplus
    // reserve persist through the store and reload on a fresh control handle
    // (simulated restart).
    let store = InMemoryRateQuoteStore::new();
    let dyn_store: DynRateQuoteStore = Arc::new(store.clone());
    let control = RateQuoteControlHandle::with_store(dyn_store.clone());
    control
        .set_unit_issuance_cap(CurrencyUnit::Usd, 500)
        .await
        .expect("set cap");
    control
        .set_unit_quote_state(CurrencyUnit::Usd, false, true)
        .await
        .expect("pause melt side");

    let processor1 = processor(0, store.clone(), control.clone(), 120);
    let mut stream = processor1
        .wait_payment_event()
        .await
        .expect("payment stream");
    processor1
        .create_incoming_payment_request(mint_quote(200))
        .await
        .expect("quote under cap");
    next_payment_received(&mut stream).await;
    assert_eq!(control.outstanding(&CurrencyUnit::Usd).await, 200);

    // "Restart": a fresh handle over the same persisted store.
    let restarted = RateQuoteControlHandle::with_store(dyn_store);
    let loaded = restarted.load_persisted().await.expect("load persisted");
    assert!(loaded.contains(&CurrencyUnit::Usd));
    assert_eq!(restarted.outstanding(&CurrencyUnit::Usd).await, 200);
    assert_eq!(restarted.buffer_surplus_sats(&CurrencyUnit::Usd).await, 20);

    let processor2 = processor(30, store.clone(), restarted.clone(), 120);
    processor2
        .create_incoming_payment_request(mint_quote(301))
        .await
        .expect_err("persisted outstanding must restrict the reloaded cap");
    processor2
        .create_incoming_payment_request(mint_quote(300))
        .await
        .expect("remaining persisted headroom stays usable");

    let bolt11 = sat_invoice(1_000).await;
    processor2
        .get_payment_quote(&CurrencyUnit::Usd, melt_options(bolt11))
        .await
        .expect_err("melt pause must survive the restart");
}

#[tokio::test]
async fn forced_melt_failure_keeps_outstanding_and_returns_failed_status() {
    // ADR-023 Melt-Path Staleness (WS6 verification duty): a forced USD melt
    // LN payment failure must surface the failed status unchanged and leave
    // the outstanding issued counter untouched — only a Paid settlement may
    // reduce fiat liability. Proof release back to spendable is cdk's
    // PENDING→UNPAID machinery at the mint layer, covered by the ignored
    // full-stack test below.
    let store = InMemoryRateQuoteStore::new();
    let control = open_control(10_000).await;
    let processor = processor(0, store.clone(), control.clone(), 120);
    let mut stream = processor
        .wait_payment_event()
        .await
        .expect("payment stream");
    processor
        .create_incoming_payment_request(mint_quote(200))
        .await
        .expect("seed outstanding issuance");
    next_payment_received(&mut stream).await;
    assert_eq!(control.outstanding(&CurrencyUnit::Usd).await, 200);

    let failed_description = serde_json::to_string(&FakeInvoiceDescription {
        pay_invoice_state: MeltQuoteState::Failed,
        check_payment_state: MeltQuoteState::Failed,
        pay_err: false,
        check_err: false,
    })
    .expect("description json");
    let failing_bolt11 = sat_invoice_with_description(990, Some(failed_description)).await;
    processor
        .get_payment_quote(&CurrencyUnit::Usd, melt_options(failing_bolt11.clone()))
        .await
        .expect("melt quote");
    let response = processor
        .make_payment(&CurrencyUnit::Usd, melt_options(failing_bolt11))
        .await
        .expect("payment attempt resolves");
    assert_eq!(response.status, MeltQuoteState::Failed);
    assert_eq!(
        control.outstanding(&CurrencyUnit::Usd).await,
        200,
        "failed melt must not reduce outstanding"
    );

    // A successful melt then reduces the outstanding counter by the melted
    // amount: 990 sats at 1,000 sats/USD with 100 bps buffer → 100 cents.
    let paying_bolt11 = sat_invoice(990).await;
    processor
        .get_payment_quote(&CurrencyUnit::Usd, melt_options(paying_bolt11.clone()))
        .await
        .expect("melt quote");
    let response = processor
        .make_payment(&CurrencyUnit::Usd, melt_options(paying_bolt11))
        .await
        .expect("payment succeeds");
    assert_eq!(response.status, MeltQuoteState::Paid);
    assert_eq!(response.total_spent, Amount::new(100, CurrencyUnit::Usd));
    assert_eq!(control.outstanding(&CurrencyUnit::Usd).await, 100);
}

#[test]
#[ignore = "ADR-023 WS6 verification duty: asserting PENDING→UNPAID proof release after a \
forced USD melt LN failure needs a full mintd + regtest harness; the decorator-level \
liability accounting for the same failure is covered by \
forced_melt_failure_keeps_outstanding_and_returns_failed_status"]
fn forced_melt_failure_releases_proof_reservation() {}
