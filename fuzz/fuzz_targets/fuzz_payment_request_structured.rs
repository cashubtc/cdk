#![no_main]

//! Structured sibling bin for [`cashu::PaymentRequest`].
//!
//! Drives the CBOR (`Display` / `FromStr` via `creqA` prefix), bech32
//! (`to_bech32_string` / `from_bech32_string` via `creqb` HRP), and JSON
//! round-trips using a fuzz-controlled `PaymentRequestArb`. Complements
//! `fuzz_payment_request` and `fuzz_payment_request_bech32`, which stress
//! the parsers on raw bytes.

use std::str::FromStr;

use cashu::PaymentRequest;
use cdk_fuzz::arbitrary_ext::PaymentRequestArb;
use libfuzzer_sys::fuzz_target;

fuzz_target!(|arb: PaymentRequestArb| {
    let pr: PaymentRequest = arb.into_inner();

    // ---------------------------------------------------------------------
    // 1. `AsRef<Option<String>>` accessor must not panic.
    // ---------------------------------------------------------------------
    let _: &Option<String> = pr.as_ref();

    // ---------------------------------------------------------------------
    // 2. CBOR (creqA) Display / FromStr round-trip.
    // ---------------------------------------------------------------------
    let s = pr.to_string();
    match PaymentRequest::from_str(&s) {
        Ok(parsed) => assert_eq!(parsed, pr, "payment request creqA round-trip mismatch"),
        Err(e) => panic!("PaymentRequest::to_string should be round-trippable: {e:?}"),
    }

    // ---------------------------------------------------------------------
    // 3. bech32 (creqB) round-trip — must also parse via FromStr.
    // ---------------------------------------------------------------------
    match pr.to_bech32_string() {
        Ok(bech) => match PaymentRequest::from_str(&bech) {
            Ok(parsed) => assert_eq!(parsed, pr, "payment request bech32 round-trip mismatch"),
            Err(e) => panic!("bech32-encoded payment request must re-parse: {e:?}"),
        },
        // A generator quirk (e.g. a field that cannot be bech32-encoded) is
        // acceptable; we're primarily proving the encoder does not panic.
        Err(_) => {}
    }

    // ---------------------------------------------------------------------
    // 4. JSON round-trip is lossless.
    // ---------------------------------------------------------------------
    let json = serde_json::to_string(&pr).expect("payment request serializes");
    let parsed: PaymentRequest =
        serde_json::from_str(&json).expect("payment request json round-trips");
    assert_eq!(parsed, pr, "payment request json round-trip mismatch");
});
