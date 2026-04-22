#![no_main]

//! Structured sibling bin for [`cashu::BlindSignature`].
//!
//! Builds a structurally-valid `BlindSignature` (with a DLEQ attached) from
//! fuzz-controlled scalars and curve points, then exercises
//! `BlindSignature::verify_dleq` and the JSON round-trip. Complements
//! `fuzz_blind_signature`, which stresses the raw-bytes parser.
//!
//! We construct via struct literals because `BlindSignature::new` also
//! *calculates* the DLEQ from scratch — here we want the fuzzer to pick
//! `(e, s)` freely so we reach the comparison inside `verify_dleq`.

use cashu::nuts::nut00::BlindSignature;
use cashu::nuts::nut12::BlindSignatureDleq;
use cashu::{Amount, SecretKey};
use cdk_fuzz::arbitrary_ext::{AmountArb, IdArb, PublicKeyArb, SecretKeyArb};
use libfuzzer_sys::arbitrary::{Arbitrary, Unstructured};
use libfuzzer_sys::fuzz_target;

#[derive(Debug)]
struct Input {
    amount: AmountArb,
    id: IdArb,
    c: PublicKeyArb,
    e: SecretKeyArb,
    s: SecretKeyArb,
    mint_pk: PublicKeyArb,
    blinded_message: PublicKeyArb,
}

impl<'a> Arbitrary<'a> for Input {
    fn arbitrary(u: &mut Unstructured<'a>) -> libfuzzer_sys::arbitrary::Result<Self> {
        Ok(Self {
            amount: AmountArb::arbitrary(u)?,
            id: IdArb::arbitrary(u)?,
            c: PublicKeyArb::arbitrary(u)?,
            e: SecretKeyArb::arbitrary(u)?,
            s: SecretKeyArb::arbitrary(u)?,
            mint_pk: PublicKeyArb::arbitrary(u)?,
            blinded_message: PublicKeyArb::arbitrary(u)?,
        })
    }
}

fuzz_target!(|input: Input| {
    let amount: Amount = input.amount.into_inner();
    let keyset_id = input.id.into_inner();
    let c = input.c.into_inner();
    let e: SecretKey = input.e.into_inner();
    let s: SecretKey = input.s.into_inner();
    let mint_pk = input.mint_pk.into_inner();
    let blinded_message = input.blinded_message.into_inner();

    let bsig = BlindSignature {
        amount,
        keyset_id,
        c,
        dleq: Some(BlindSignatureDleq { e, s }),
    };

    // ---------------------------------------------------------------------
    // 1. verify_dleq must not panic on arbitrary scalars/points.
    // ---------------------------------------------------------------------
    let _ = bsig.verify_dleq(mint_pk, blinded_message);

    // ---------------------------------------------------------------------
    // 2. JSON round-trip is lossless.
    // ---------------------------------------------------------------------
    let json = serde_json::to_string(&bsig).expect("blind signature serializes");
    let parsed: BlindSignature = serde_json::from_str(&json).expect("blind signature round-trips");
    assert_eq!(parsed, bsig, "blind signature json round-trip mismatch");
});
