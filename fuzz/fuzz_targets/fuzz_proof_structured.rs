#![no_main]

//! Structured sibling bin for [`cashu::Proof`].
//!
//! Consumes a [`ProofArb`] (fuzz-controlled `Proof` with NUT-10 P2PK / HTLC
//! secrets ~2/3 of the time, an optional P2PK/HTLC witness, and an optional
//! `ProofDleq`) and exercises the protocol-level verify paths as well as the
//! JSON round-trip. Complements `fuzz_proof`, which stresses the parser on
//! raw bytes.
//!
//! The fuzz bits decide:
//!   * classic vs NUT-10 secret shape
//!   * P2PKWitness vs HTLCWitness vs no witness
//!   * DLEQ present / absent
//!   * mint pubkey seed (for `verify_dleq`)
//!
//! All verification calls are allowed to return `Err`; we only assert they
//! do not panic, and that the JSON round-trip is lossless.
//!
//! We intentionally do **not** feed the same `ProofArb` through
//! `TokenV4::try_from`-style aggregation here — that lives in
//! `fuzz_token_conversion` and `fuzz_token_structured`.

use cashu::Proof;
use cdk_fuzz::arbitrary_ext::{ProofArb, PublicKeyArb};
use libfuzzer_sys::arbitrary::{Arbitrary, Unstructured};
use libfuzzer_sys::fuzz_target;

#[derive(Debug)]
struct Input {
    proof: ProofArb,
    mint_pubkey: PublicKeyArb,
}

impl<'a> Arbitrary<'a> for Input {
    fn arbitrary(u: &mut Unstructured<'a>) -> libfuzzer_sys::arbitrary::Result<Self> {
        Ok(Self {
            proof: ProofArb::arbitrary(u)?,
            mint_pubkey: PublicKeyArb::arbitrary(u)?,
        })
    }
}

fuzz_target!(|input: Input| {
    let proof: Proof = input.proof.into_inner();
    let mint_pk = input.mint_pubkey.into_inner();

    // ---------------------------------------------------------------------
    // 1. Verification methods must not panic.
    // ---------------------------------------------------------------------
    let _ = proof.verify_p2pk();
    let _ = proof.verify_htlc();
    // Only exercise verify_dleq when a dleq is attached; the None path is
    // trivial and already covered by `fuzz_dleq_verify`.
    if proof.dleq.is_some() {
        let _ = proof.verify_dleq(mint_pk);
    }

    // ---------------------------------------------------------------------
    // 2. JSON round-trip must be lossless.
    // ---------------------------------------------------------------------
    let json = serde_json::to_string(&proof).expect("proof serializes");
    let parsed: Proof = serde_json::from_str(&json).expect("proof round-trips");
    assert_eq!(parsed, proof, "proof json round-trip mismatch");
});
