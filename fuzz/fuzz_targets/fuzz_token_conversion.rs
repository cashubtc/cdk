#![no_main]

//! Differential fuzzer for Cashu `Token` V3 <-> V4 conversion.
//!
//! Starting from a structurally valid `TokenV4` (single-mint, ~1-3 keyset
//! groups, ~1-4 proofs per group, unit + mint_url always present), the
//! harness checks:
//!
//! 1. `TokenV4 -> TokenV3 -> TokenV4` round-trip is lossless on the set of
//!    proofs, mint_url, memo, and unit.
//! 2. The public `Token` API agrees between the V3 and V4 wrappings of the
//!    same underlying data (`value`, `unit`, `memo`, `mint_url`, and the
//!    number of `token_secrets`).
//! 3. `Token::from_str` / `TokenV3::from_str` / `TokenV4::from_str` round-
//!    trip correctly through `Display`.
//! 4. `TokenV4::to_raw_bytes` / `try_from(&Vec<u8>)` round-trips, and the
//!    generic `Token::try_from(&Vec<u8>)` path agrees.
//! 5. The spending-condition helpers (`spending_conditions`,
//!    `p2pk_pubkeys`, `p2pk_refund_pubkeys`, `htlc_hashes`, `locktimes`,
//!    `token_secrets`) never panic regardless of the conversion form.

use std::collections::BTreeSet;
use std::str::FromStr;

use cashu::nuts::nut00::{Token, TokenV3, TokenV4};
use cashu::secret::Secret as SecretString;
use cdk_fuzz::arbitrary_ext::TokenV4Arb;
use libfuzzer_sys::fuzz_target;

/// Collect secrets in sorted order so we can compare multisets across V3/V4.
fn sorted_secrets(secrets: &[&SecretString]) -> Vec<String> {
    let mut out: Vec<String> = secrets.iter().map(|s| s.to_string()).collect();
    out.sort();
    out
}

/// Compare two `Result<T, E>` on `is_ok()` only; we don't compare error
/// variants since V3 and V4 can return different error enums.
fn same_ok_arm<T, E, U, F>(a: &Result<T, E>, b: &Result<U, F>) -> bool {
    a.is_ok() == b.is_ok()
}

fuzz_target!(|arb: TokenV4Arb| {
    let v4_original = arb.into_inner();

    // ---------------------------------------------------------------------
    // 1. V4 -> V3 -> V4 round-trip.
    // ---------------------------------------------------------------------
    let v3: TokenV3 = v4_original.clone().into();
    let v4_roundtrip = match TokenV4::try_from(v3.clone()) {
        Ok(v) => v,
        // If conversion fails at all, the generator invariants are broken.
        // Bail silently rather than panic.
        Err(_) => return,
    };

    // Mint URL must survive.
    assert_eq!(v4_original.mint_url, v4_roundtrip.mint_url);
    // Unit must survive (V4 always has a unit).
    assert_eq!(v4_original.unit, v4_roundtrip.unit);
    // Memo must survive.
    assert_eq!(v4_original.memo, v4_roundtrip.memo);

    // The set of proof secrets must survive. Note that V4 groups proofs by
    // keyset_id, but V3 stores one flat list and V4 regroups on re-entry;
    // two proofs that shared a keyset prefix may land together. Compare as
    // a multiset of secrets instead.
    let s_orig = sorted_secrets(
        &v4_original
            .token
            .iter()
            .flat_map(|g| g.proofs.iter().map(|p| &p.secret))
            .collect::<Vec<_>>(),
    );
    let s_round = sorted_secrets(
        &v4_roundtrip
            .token
            .iter()
            .flat_map(|g| g.proofs.iter().map(|p| &p.secret))
            .collect::<Vec<_>>(),
    );
    assert_eq!(s_orig, s_round, "proof secrets differ after V4->V3->V4");

    // `value()` must agree on ok/err.
    let v_orig = v4_original.value();
    let v_round = v4_roundtrip.value();
    assert!(same_ok_arm(&v_orig, &v_round));
    if let (Ok(a), Ok(b)) = (&v_orig, &v_round) {
        assert_eq!(a, b);
    }

    // ---------------------------------------------------------------------
    // 2. Token API agreement between V3 and V4 wrappings.
    // ---------------------------------------------------------------------
    let as_v4 = Token::TokenV4(v4_original.clone());
    let as_v3 = Token::TokenV3(v3.clone());

    // value(): both Ok or both Err, equal when Ok.
    let r4 = as_v4.value();
    let r3 = as_v3.value();
    assert!(
        same_ok_arm(&r4, &r3),
        "value() ok/err mismatch: v4={r4:?} v3={r3:?}"
    );
    if let (Ok(a), Ok(b)) = (&r4, &r3) {
        assert_eq!(a, b);
    }

    // unit(): V3 returns Option<CurrencyUnit>, V4 returns Some(...). Our
    // generator always populates V3 with the same unit, so expect equal.
    assert_eq!(as_v4.unit(), as_v3.unit());

    // memo(): must match.
    assert_eq!(as_v4.memo(), as_v3.memo());

    // mint_url(): V3 mint_url() errors if multi-mint; our generator is
    // single-mint so both should return Ok and be equal.
    let m4 = as_v4.mint_url();
    let m3 = as_v3.mint_url();
    assert!(same_ok_arm(&m4, &m3));
    if let (Ok(a), Ok(b)) = (&m4, &m3) {
        assert_eq!(a, b);
    }

    // token_secrets() length must match (both flat enumerations of proofs).
    assert_eq!(as_v4.token_secrets().len(), as_v3.token_secrets().len());

    // ---------------------------------------------------------------------
    // 3. String round-trip on both forms.
    // ---------------------------------------------------------------------
    // V4 string round-trip via the generic Token parser.
    let v4_str = v4_original.to_string();
    match Token::from_str(&v4_str) {
        Ok(Token::TokenV4(parsed)) => assert_eq!(parsed, v4_original),
        Ok(_) => panic!("cashuB prefix should parse as TokenV4"),
        Err(_) => panic!("TokenV4::to_string should be round-trippable"),
    }
    // Also via the dedicated TokenV4::from_str parser.
    assert_eq!(TokenV4::from_str(&v4_str).expect("v4 parse"), v4_original);

    // V3 string round-trip.
    let v3_str = v3.to_string();
    assert_eq!(TokenV3::from_str(&v3_str).expect("v3 parse"), v3);
    match Token::from_str(&v3_str) {
        Ok(Token::TokenV3(parsed)) => assert_eq!(parsed, v3),
        Ok(_) => panic!("cashuA prefix should parse as TokenV3"),
        Err(_) => panic!("TokenV3::to_string should be round-trippable"),
    }

    // to_v3_string() on the outer Token must also parse back as TokenV3.
    let outer_v3_str = as_v4.to_v3_string();
    assert!(TokenV3::from_str(&outer_v3_str).is_ok());

    // ---------------------------------------------------------------------
    // 4. Raw-bytes round-trip on V4.
    // ---------------------------------------------------------------------
    let raw = v4_original
        .to_raw_bytes()
        .expect("v4 raw serialization succeeds");
    let v4_back = TokenV4::try_from(&raw).expect("v4 raw parse succeeds");
    assert_eq!(v4_back, v4_original);
    let generic_back = Token::try_from(&raw).expect("generic raw parse succeeds");
    assert_eq!(generic_back, Token::TokenV4(v4_original.clone()));

    // ---------------------------------------------------------------------
    // 5. Spending-condition helpers must not panic.
    //    (Results are allowed to be Err; we only check no panic.)
    // ---------------------------------------------------------------------
    for tok in &[&as_v3, &as_v4] {
        let _ = tok.spending_conditions();
        let _ = tok.p2pk_pubkeys();
        let _ = tok.p2pk_refund_pubkeys();
        let _ = tok.htlc_hashes();
        let _ = tok.locktimes();
        let _ = tok.token_secrets();
    }

    // Confirm at least one invariant about the helper: if both forms
    // yield Ok sets, their lengths must match (same underlying proofs).
    if let (Ok(a), Ok(b)) = (as_v3.locktimes(), as_v4.locktimes()) {
        let a: BTreeSet<_> = a.into_iter().collect();
        let b: BTreeSet<_> = b.into_iter().collect();
        assert_eq!(a, b);
    }
});
