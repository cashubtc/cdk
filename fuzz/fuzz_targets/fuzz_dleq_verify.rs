#![no_main]

//! Deep fuzzing for NUT-12 DLEQ verification paths.
//!
//! Two targets in one harness:
//!  1. `BlindSignature::verify_dleq(mint_pubkey, blinded_message)` with a
//!     fuzz-controlled `BlindSignatureDleq { e, s }`.
//!  2. `Proof::verify_dleq(mint_pubkey)` with a fuzz-controlled
//!     `ProofDleq { e, s, r }` and a fuzz-controlled `secret`.
//!
//! These exercise the internal hash/curve reconstruction and final point
//! comparison. We feed valid secp256k1 scalars so we reach the actual
//! verification math rather than bailing out at deserialization.

use std::str::FromStr;

use cashu::dhke::{blind_message, unblind_message};
use cashu::nuts::nut12::{BlindSignatureDleq, ProofDleq};
use cashu::secret::Secret as SecretString;
use cashu::{Amount, BlindSignature, Id, Proof, SecretKey, SECP256K1};
use cdk_fuzz::{pubkey_from, secret_key_from};
use libfuzzer_sys::arbitrary::{Arbitrary, Unstructured};
use libfuzzer_sys::fuzz_target;

#[derive(Debug)]
struct Input {
    // Scalars for the DLEQ proofs.
    e_bytes: [u8; 32],
    s_bytes: [u8; 32],
    r_bytes: [u8; 32],
    // Mint pubkey + blinded message + blind signature point.
    mint_sk_bytes: [u8; 32],
    bmsg_sk_bytes: [u8; 32],
    bsig_sk_bytes: [u8; 32],
    // Proof side.
    proof_secret: String,
    proof_c_seed: [u8; 32],
    amount: u64,
    keyset_id_bytes: [u8; 8],
    valid_case: bool,
}

impl<'a> Arbitrary<'a> for Input {
    fn arbitrary(u: &mut Unstructured<'a>) -> libfuzzer_sys::arbitrary::Result<Self> {
        Ok(Self {
            e_bytes: u.arbitrary()?,
            s_bytes: u.arbitrary()?,
            r_bytes: u.arbitrary()?,
            mint_sk_bytes: u.arbitrary()?,
            bmsg_sk_bytes: u.arbitrary()?,
            bsig_sk_bytes: u.arbitrary()?,
            proof_secret: String::arbitrary(u)?,
            proof_c_seed: u.arbitrary()?,
            amount: u.arbitrary()?,
            keyset_id_bytes: u.arbitrary()?,
            valid_case: u.arbitrary()?,
        })
    }
}

fuzz_target!(|input: Input| {
    let e = secret_key_from(input.e_bytes);
    let s = secret_key_from(input.s_bytes);
    let r = secret_key_from(input.r_bytes);

    let mint_pubkey = pubkey_from(input.mint_sk_bytes);
    let blinded_message_point = pubkey_from(input.bmsg_sk_bytes);
    let blinded_signature_point = pubkey_from(input.bsig_sk_bytes);

    let keyset_id = Id::from_bytes(&input.keyset_id_bytes)
        .unwrap_or_else(|_| Id::from_str("00deadbeef123456").expect("valid id"));

    if input.valid_case {
        let mint_secret_key = secret_key_from(input.mint_sk_bytes);
        let mint_pubkey = mint_secret_key.public_key();
        let secret = SecretString::new(input.proof_secret);
        let (blinded_message, r) =
            blind_message(secret.as_bytes(), Some(secret_key_from(input.r_bytes)))
                .expect("valid secret and scalar must produce a blinded message");
        let blinded_signature_point = blinded_message
            .mul_tweak(&SECP256K1, &mint_secret_key.as_scalar())
            .expect("valid mint scalar must sign a blinded message")
            .into();

        let blind_signature = BlindSignature::new(
            Amount::from(input.amount),
            blinded_signature_point,
            keyset_id,
            &blinded_message,
            &mint_secret_key,
        )
        .expect("valid blinded signature must produce a DLEQ proof");
        assert!(
            blind_signature
                .verify_dleq(mint_pubkey, blinded_message)
                .is_ok(),
            "generated blind-signature DLEQ proof must verify"
        );

        let blind_dleq = blind_signature
            .dleq
            .clone()
            .expect("BlindSignature::new attaches a DLEQ proof");
        let c = unblind_message(&blinded_signature_point, &r, &mint_pubkey)
            .expect("valid blinded signature must unblind");
        let proof = Proof {
            amount: Amount::from(input.amount),
            keyset_id,
            secret,
            c,
            witness: None,
            dleq: Some(ProofDleq::new(blind_dleq.e, blind_dleq.s, r)),
            p2pk_e: None,
        };
        assert!(
            proof.verify_dleq(mint_pubkey).is_ok(),
            "generated proof DLEQ must verify"
        );

        let mut wrong_mint_key = secret_key_from(input.bsig_sk_bytes);
        if wrong_mint_key.public_key() == mint_pubkey {
            wrong_mint_key = secret_key_from([2_u8; 32]);
        }
        if wrong_mint_key.public_key() == mint_pubkey {
            wrong_mint_key = secret_key_from([3_u8; 32]);
        }
        assert!(
            proof.verify_dleq(wrong_mint_key.public_key()).is_err(),
            "a valid DLEQ proof must reject a different mint key"
        );
        return;
    }

    // --- Target 1: BlindSignature::verify_dleq ---
    // Build a BlindSignature with a DLEQ attached. `verify_dleq` consumes
    // the dleq field, so construct via struct literal.
    let dleq_bs = BlindSignatureDleq {
        e: SecretKey::from_slice(e.as_secret_bytes()).expect("roundtrip"),
        s: SecretKey::from_slice(s.as_secret_bytes()).expect("roundtrip"),
    };
    let bsig = BlindSignature {
        amount: Amount::from(input.amount),
        keyset_id,
        c: blinded_signature_point,
        dleq: Some(dleq_bs),
    };
    let _ = bsig.verify_dleq(mint_pubkey, blinded_message_point);

    // --- Target 2: Proof::verify_dleq ---
    let dleq_proof = ProofDleq::new(
        SecretKey::from_slice(e.as_secret_bytes()).expect("roundtrip"),
        SecretKey::from_slice(s.as_secret_bytes()).expect("roundtrip"),
        r,
    );
    // `Proof::verify_dleq` hashes `self.secret.as_bytes()` onto the curve,
    // so we feed an arbitrary String directly (via Secret::new).
    let secret = SecretString::new(input.proof_secret);
    let proof = Proof {
        amount: Amount::from(input.amount),
        keyset_id,
        secret,
        c: pubkey_from(input.proof_c_seed),
        witness: None,
        dleq: Some(dleq_proof),
        p2pk_e: None,
    };
    let _ = proof.verify_dleq(mint_pubkey);
});
