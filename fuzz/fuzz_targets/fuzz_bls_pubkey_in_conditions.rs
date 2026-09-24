#![no_main]

//! Robustness fuzzing for BLS points smuggled into secp256k1-only positions.
//!
//! NUT-11 (P2PK) and NUT-14 (HTLC) spending conditions are secp256k1-only, but
//! `PublicKey::from_slice` accepts 48-byte (G1) and 96-byte (G2) BLS encodings.
//! A NUT-10 secret is fully attacker-controlled, so the `data` field and the
//! `pubkeys` / `refund_keys` tags can carry a *valid* compressed BLS point.
//!
//! Several `PublicKey` accessors (`x_only_public_key`, `negate`,
//! `to_uncompressed_bytes`) `panic!` on BLS variants. This target deliberately
//! injects valid BLS G1/G2 points into every pubkey-bearing position and then
//! runs the verification pipelines, asserting they return an error rather than
//! panic (libFuzzer treats any panic as a crash).
//!
//! This would have caught the `check_duplicate_pubkeys` panic: random hex
//! strings essentially never decode to a valid subgroup-correct BLS point, so
//! the existing `fuzz_p2pk_verify` `data_override` path could not reach it.

use std::str::FromStr;

use cashu::nuts::nut00::Witness;
use cashu::nuts::nut10::{Kind, SecretData};
use cashu::nuts::nut11::{P2PKWitness, SigFlag};
use cashu::nuts::Conditions;
use cashu::secret::Secret as SecretString;
use cashu::{Amount, BlindedMessage, Id, Nut10Secret, Proof, PublicKey};
use cdk_fuzz::{bls_g1_pubkey_from, bls_g2_pubkey_from, pubkey_from};
use libfuzzer_sys::arbitrary::{Arbitrary, Unstructured};
use libfuzzer_sys::fuzz_target;

/// How to encode a fuzz-chosen pubkey: as secp256k1, BLS G1, or BLS G2.
#[derive(Debug)]
enum KeyKind {
    Secp,
    BlsG1,
    BlsG2,
}

impl KeyKind {
    fn from_byte(b: u8) -> Self {
        match b % 3 {
            0 => KeyKind::Secp,
            1 => KeyKind::BlsG1,
            _ => KeyKind::BlsG2,
        }
    }

    fn pubkey(&self, seed: [u8; 32]) -> PublicKey {
        match self {
            KeyKind::Secp => pubkey_from(seed),
            KeyKind::BlsG1 => bls_g1_pubkey_from(&seed),
            KeyKind::BlsG2 => bls_g2_pubkey_from(seed),
        }
    }
}

#[derive(Debug)]
struct Input {
    // Kind tag + seed for each pubkey position. At least one will usually be BLS.
    data_kind: u8,
    data_seed: [u8; 32],
    extra: Vec<(u8, [u8; 32])>,
    refund: Vec<(u8, [u8; 32])>,
    is_htlc: bool,
    locktime: Option<u64>,
    num_sigs: Option<u64>,
    num_sigs_refund: Option<u64>,
    sig_flag_odd: bool,
    witness_sigs: Vec<String>,
    amount: u64,
    keyset_id_bytes: [u8; 8],
    required_sigs: u64,
}

impl<'a> Arbitrary<'a> for Input {
    fn arbitrary(u: &mut Unstructured<'a>) -> libfuzzer_sys::arbitrary::Result<Self> {
        let take_keys = |u: &mut Unstructured<'a>,
                         max: usize|
         -> libfuzzer_sys::arbitrary::Result<Vec<(u8, [u8; 32])>> {
            let n = u.int_in_range(0..=max)?;
            (0..n)
                .map(|_| Ok((u.arbitrary::<u8>()?, u.arbitrary::<[u8; 32]>()?)))
                .collect()
        };
        Ok(Self {
            data_kind: u.arbitrary()?,
            data_seed: u.arbitrary()?,
            extra: take_keys(u, 4)?,
            refund: take_keys(u, 3)?,
            is_htlc: u.arbitrary()?,
            locktime: Option::<u64>::arbitrary(u)?,
            num_sigs: Option::<u64>::arbitrary(u)?,
            num_sigs_refund: Option::<u64>::arbitrary(u)?,
            sig_flag_odd: u.arbitrary()?,
            witness_sigs: {
                let n = u.int_in_range(0..=4)?;
                (0..n).map(|_| String::arbitrary(u)).collect::<Result<_, _>>()?
            },
            amount: u.arbitrary()?,
            keyset_id_bytes: u.arbitrary()?,
            required_sigs: u.arbitrary()?,
        })
    }
}

fuzz_target!(|input: Input| {
    let data_pubkey = KeyKind::from_byte(input.data_kind).pubkey(input.data_seed);
    let extra_pubkeys: Vec<PublicKey> = input
        .extra
        .iter()
        .map(|(k, s)| KeyKind::from_byte(*k).pubkey(*s))
        .collect();
    let refund_keys: Vec<PublicKey> = input
        .refund
        .iter()
        .map(|(k, s)| KeyKind::from_byte(*k).pubkey(*s))
        .collect();

    let conditions = Conditions {
        locktime: input.locktime,
        pubkeys: if extra_pubkeys.is_empty() {
            None
        } else {
            Some(extra_pubkeys.clone())
        },
        refund_keys: if refund_keys.is_empty() {
            None
        } else {
            Some(refund_keys.clone())
        },
        num_sigs: input.num_sigs,
        sig_flag: if input.sig_flag_odd {
            SigFlag::SigAll
        } else {
            SigFlag::SigInputs
        },
        num_sigs_refund: input.num_sigs_refund,
    };

    let kind = if input.is_htlc { Kind::HTLC } else { Kind::P2PK };

    // Build the data field: a HTLC needs a sha256-shaped hash, but we want to
    // drive a (possibly BLS) pubkey into the P2PK `data` slot. For HTLC, put a
    // valid hash; for P2PK, put the (possibly BLS) data pubkey hex.
    let data_string = if input.is_htlc {
        // 32-byte zero hash hex; pubkeys live in the tags for HTLC.
        "00".repeat(32)
    } else {
        data_pubkey.to_hex()
    };

    let tags: Vec<Vec<String>> = conditions.into();
    let secret_data = SecretData::new(data_string, Some(tags));
    let nut10 = Nut10Secret::new(kind, secret_data);

    let secret: SecretString = match nut10.try_into() {
        Ok(s) => s,
        Err(_) => return,
    };

    let keyset_id = Id::from_bytes(&input.keyset_id_bytes)
        .unwrap_or_else(|_| Id::from_str("00deadbeef123456").expect("valid id"));

    // `c` just needs to be a well-formed point; reuse a secp key.
    let c_point = pubkey_from([2u8; 32]);

    let witness = if input.is_htlc {
        Witness::HTLCWitness(cashu::nuts::nut14::HTLCWitness {
            preimage: "00".repeat(32),
            signatures: Some(input.witness_sigs.clone()),
        })
    } else {
        Witness::P2PKWitness(P2PKWitness {
            signatures: input.witness_sigs.clone(),
        })
    };

    let proof = Proof {
        amount: Amount::from(input.amount),
        keyset_id,
        secret,
        c: c_point,
        witness: Some(witness),
        dleq: None,
        p2pk_e: None,
    };

    // Primary targets: the full verification pipelines must not panic.
    let _ = proof.verify_p2pk();
    let _ = proof.verify_htlc();

    // Secondary: BlindedMessage::verify_p2pk with a (possibly BLS) pubkey list.
    let blinded = BlindedMessage::new(Amount::from(input.amount), keyset_id, c_point);
    let _ = blinded.verify_p2pk(&extra_pubkeys, input.required_sigs);
});
