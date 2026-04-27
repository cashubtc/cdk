#![no_main]

//! Structured sibling bin for [`cashu::nuts::nut00::Token`].
//!
//! Consumes a [`TokenArb`] (randomly V3 or V4) and drives the public
//! aggregate helpers plus `Display`/`FromStr` round-trip. Complements
//! `fuzz_token`, which exercises the parser on raw bytes.
//!
//! All helpers are allowed to return `Err`; we only assert no panics and
//! that round-trips via `to_string` -> `from_str` are lossless on the outer
//! `Token`.

use std::str::FromStr;

use cashu::nuts::nut00::Token;
use cdk_fuzz::arbitrary_ext::TokenArb;
use libfuzzer_sys::fuzz_target;

fuzz_target!(|arb: TokenArb| {
    let token: Token = arb.into_inner();

    // ---------------------------------------------------------------------
    // 1. No-panic sweep of the public aggregate helpers.
    // ---------------------------------------------------------------------
    let _ = token.value();
    let _ = token.unit();
    let _ = token.memo();
    let _ = token.mint_url();
    let _ = token.spending_conditions();
    let _ = token.p2pk_pubkeys();
    let _ = token.p2pk_refund_pubkeys();
    let _ = token.htlc_hashes();
    let _ = token.locktimes();
    let _ = token.token_secrets();
    // `to_v3_string` must never panic even when the source is already V3.
    let v3_str = token.to_v3_string();
    // And that rendered form must at least parse back as something.
    let _ = Token::from_str(&v3_str);

    // ---------------------------------------------------------------------
    // 2. Outer Token Display / FromStr round-trip is lossless.
    // ---------------------------------------------------------------------
    let s = token.to_string();
    match Token::from_str(&s) {
        Ok(parsed) => assert_eq!(parsed, token, "Token round-trip mismatch"),
        Err(e) => panic!("Token::to_string should be round-trippable: {e:?}"),
    }
});
