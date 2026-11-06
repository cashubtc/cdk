#![no_main]

//! Robustness + soundness fuzzing for the new BLS12-381 (v3) BDHKE surface.
//!
//! The pairing-based protocol added for `02` keysets is entirely new code with
//! its own point parsing, scalar handling, and (deterministic) batch-weight
//! derivation loop. This target stresses two things:
//!
//!   1. Panic-safety of parsing arbitrary bytes as BLS G1/G2 points and of the
//!      blind/sign/unblind/verify and batch-verify code paths.
//!   2. Soundness invariants: an honestly produced v3 proof must verify, and a
//!      batch of honest proofs must batch-verify; tampering must not verify.
//!
//! The batch path exercises `derive_batch_weights` (rejection-sampling loop) so
//! a regression that panics or loops forever there surfaces as a crash/timeout.

use cashu::dhke::{
    batch_verify_bls_messages, blind_message_for_version, sign_message, verify_bls_message,
};
use cashu::nuts::nut01::BlsG1PublicKey;
use cashu::nuts::nut02::KeySetVersion;
use cashu::{PublicKey, SecretKey};
use libfuzzer_sys::arbitrary::{Arbitrary, Unstructured};
use libfuzzer_sys::fuzz_target;

#[derive(Debug)]
struct Proofish {
    secret: Vec<u8>,
    r_bytes: [u8; 32],
    mint_bytes: [u8; 32],
    // Index of which mint key to use, to exercise the "group by key" batch path.
    mint_idx: u8,
}

#[derive(Debug)]
struct Input {
    proofs: Vec<Proofish>,
    // Arbitrary bytes thrown directly at the point/scalar parsers.
    raw_g1: Vec<u8>,
    raw_g2: Vec<u8>,
    raw_scalar: [u8; 32],
}

impl<'a> Arbitrary<'a> for Input {
    fn arbitrary(u: &mut Unstructured<'a>) -> libfuzzer_sys::arbitrary::Result<Self> {
        let n = u.int_in_range(0..=6)?;
        let proofs = (0..n)
            .map(|_| {
                Ok(Proofish {
                    secret: Vec::<u8>::arbitrary(u)?,
                    r_bytes: u.arbitrary()?,
                    mint_bytes: u.arbitrary()?,
                    mint_idx: u.arbitrary()?,
                })
            })
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Self {
            proofs,
            raw_g1: Vec::<u8>::arbitrary(u)?,
            raw_g2: Vec::<u8>::arbitrary(u)?,
            raw_scalar: u.arbitrary()?,
        })
    }
}

/// A valid, non-zero BLS scalar from fuzz bytes (falls back to a fixed scalar).
fn bls_scalar(bytes: &[u8; 32]) -> SecretKey {
    SecretKey::bls_from_slice(bytes)
        .ok()
        .filter(|_| bytes != &[0u8; 32])
        .unwrap_or_else(|| SecretKey::bls_from_slice(&[1u8; 32]).expect("valid scalar"))
}

fuzz_target!(|input: Input| {
    // --- Parsers must never panic on arbitrary bytes ---
    let _ = PublicKey::from_slice(&input.raw_g1);
    let _ = PublicKey::from_slice(&input.raw_g2);
    let _ = SecretKey::bls_from_slice(&input.raw_scalar);
    // hash_to_curve must accept any byte slice.
    let _ = BlsG1PublicKey::hash_to_curve(&input.raw_g1);

    if input.proofs.is_empty() {
        return;
    }

    // Pool of mint keys; `mint_idx` selects from it so several proofs can share
    // a key, exercising the batch "group by mint key" path.
    let key_pool: Vec<SecretKey> = input.proofs.iter().map(|p| bls_scalar(&p.mint_bytes)).collect();

    let mut mint_pubkeys: Vec<PublicKey> = Vec::new();
    let mut signatures: Vec<PublicKey> = Vec::new();
    let mut messages: Vec<Vec<u8>> = Vec::new();

    for p in &input.proofs {
        let r = bls_scalar(&p.r_bytes);
        let mint_secret = key_pool[p.mint_idx as usize % key_pool.len()].clone();

        // Honest v3 BDHKE round-trip: B_ = r·Y, C_ = a·B_, C = r^-1·C_.
        let (blinded, returned_r) =
            match blind_message_for_version(&p.secret, Some(r.clone()), KeySetVersion::Version02) {
                Ok(v) => v,
                Err(_) => return,
            };
        // Blinding with a provided factor must return it unchanged.
        assert_eq!(returned_r.to_secret_bytes(), r.to_secret_bytes());

        let blind_sig = match sign_message(&mint_secret, &blinded) {
            Ok(v) => v,
            Err(_) => return,
        };
        let c: PublicKey = match (|| {
            let inv = r.as_bls().ok()?.invert().ok()?;
            Some(blind_sig.as_bls_g1().ok()?.mul(&inv).into())
        })() {
            Some(c) => c,
            None => return,
        };

        // Single-proof pairing verification must accept the honest proof...
        verify_bls_message(mint_secret.public_key(), c, &p.secret)
            .expect("honest v3 proof must verify");
        // ...and reject a different message.
        let mut wrong = p.secret.clone();
        wrong.push(0xff);
        assert!(verify_bls_message(mint_secret.public_key(), c, &wrong).is_err());

        mint_pubkeys.push(mint_secret.public_key());
        signatures.push(c);
        messages.push(p.secret.clone());
    }

    // --- Batch verification: honest batch must verify ---
    let msg_refs: Vec<&[u8]> = messages.iter().map(|m| m.as_slice()).collect();
    batch_verify_bls_messages(&mint_pubkeys, &signatures, &msg_refs)
        .expect("honest batch must verify");

    // --- Tampering must break the batch (swap two distinct signatures) ---
    if signatures.len() >= 2 && signatures[0] != signatures[1] {
        let mut tampered = signatures.clone();
        tampered.swap(0, 1);
        assert!(batch_verify_bls_messages(&mint_pubkeys, &tampered, &msg_refs).is_err());
    }
});
