#![no_main]

//! Deep fuzzing for NUT-14 HTLC proof verification.
//!
//! Builds a `Proof` whose secret is a `Nut10Secret` with `Kind::HTLC` and a
//! fuzz-controlled `HTLCWitness`, then calls `Proof::verify_htlc`. This
//! exercises:
//!   - sha256 hash parsing in the secret data field
//!   - preimage hex decoding and 32-byte length check
//!   - preimage-vs-hash comparison
//!   - optional signatures path (pubkeys / num_sigs / locktime refund path)

use std::str::FromStr;

use cashu::nuts::nut00::Witness;
use cashu::nuts::nut10::{Kind, SecretData};
use cashu::nuts::nut11::SigFlag;
use cashu::nuts::nut14::HTLCWitness;
use cashu::nuts::Conditions;
use cashu::secret::Secret as SecretString;
use cashu::{Amount, Id, Nut10Secret, Proof, PublicKey};
use cdk_fuzz::pubkey_from;
use libfuzzer_sys::arbitrary::{Arbitrary, Unstructured};
use libfuzzer_sys::fuzz_target;

#[derive(Debug)]
struct Input {
    // Hash string written into the Nut10Secret.data field. Intentionally a
    // String so fuzzer can drive malformed hex.
    hash_str: String,
    // Witness preimage (also String-typed to exercise hex parser).
    preimage: String,
    // Optional witness signatures.
    witness_sigs: Option<Vec<String>>,
    // Conditions.
    locktime: Option<u64>,
    pubkey_seeds: Vec<[u8; 32]>,
    refund_key_seeds: Vec<[u8; 32]>,
    num_sigs: Option<u64>,
    num_sigs_refund: Option<u64>,
    sig_flag_raw: u8,
    // Proof envelope.
    amount: u64,
    c_seed: [u8; 32],
    keyset_id_bytes: [u8; 8],
    extra_tag: Option<(String, String)>,
}

impl<'a> Arbitrary<'a> for Input {
    fn arbitrary(u: &mut Unstructured<'a>) -> libfuzzer_sys::arbitrary::Result<Self> {
        Ok(Self {
            hash_str: String::arbitrary(u)?,
            preimage: String::arbitrary(u)?,
            witness_sigs: {
                if bool::arbitrary(u)? {
                    let n = u.int_in_range(0..=3)?;
                    let v = (0..n)
                        .map(|_| String::arbitrary(u))
                        .collect::<Result<Vec<_>, _>>()?;
                    Some(v)
                } else {
                    None
                }
            },
            locktime: Option::<u64>::arbitrary(u)?,
            pubkey_seeds: {
                let n = u.int_in_range(0..=3)?;
                (0..n)
                    .map(|_| u.arbitrary::<[u8; 32]>())
                    .collect::<Result<_, _>>()?
            },
            refund_key_seeds: {
                let n = u.int_in_range(0..=2)?;
                (0..n)
                    .map(|_| u.arbitrary::<[u8; 32]>())
                    .collect::<Result<_, _>>()?
            },
            num_sigs: Option::<u64>::arbitrary(u)?,
            num_sigs_refund: Option::<u64>::arbitrary(u)?,
            sig_flag_raw: u.arbitrary()?,
            amount: u.arbitrary()?,
            c_seed: u.arbitrary()?,
            keyset_id_bytes: u.arbitrary()?,
            extra_tag: Option::<(String, String)>::arbitrary(u)?,
        })
    }
}

fn sig_flag_from(b: u8) -> SigFlag {
    if b & 1 == 0 {
        SigFlag::SigInputs
    } else {
        SigFlag::SigAll
    }
}

fuzz_target!(|input: Input| {
    let pubkeys: Vec<PublicKey> = input
        .pubkey_seeds
        .iter()
        .copied()
        .map(pubkey_from)
        .collect();
    let refund_keys: Vec<PublicKey> = input
        .refund_key_seeds
        .iter()
        .copied()
        .map(pubkey_from)
        .collect();

    let mut tags: Vec<Vec<String>> = Vec::new();
    // Emit a pubkeys tag set even if pubkeys is empty (exercises tag parser).
    if !pubkeys.is_empty() {
        let mut pk_tag = vec!["pubkeys".to_string()];
        for pk in &pubkeys {
            pk_tag.push(pk.to_hex());
        }
        tags.push(pk_tag);
    }
    if !refund_keys.is_empty() {
        let mut r_tag = vec!["refund".to_string()];
        for pk in &refund_keys {
            r_tag.push(pk.to_hex());
        }
        tags.push(r_tag);
    }
    if let Some(lt) = input.locktime {
        tags.push(vec!["locktime".to_string(), lt.to_string()]);
    }
    if let Some(n) = input.num_sigs {
        tags.push(vec!["n_sigs".to_string(), n.to_string()]);
    }
    if let Some(n) = input.num_sigs_refund {
        tags.push(vec!["n_sigs_refund".to_string(), n.to_string()]);
    }
    tags.push(vec![
        "sigflag".to_string(),
        match sig_flag_from(input.sig_flag_raw) {
            SigFlag::SigInputs => "SIG_INPUTS".to_string(),
            SigFlag::SigAll => "SIG_ALL".to_string(),
        },
    ]);
    if let Some((k, v)) = &input.extra_tag {
        tags.push(vec![k.clone(), v.clone()]);
    }

    let secret_data = SecretData::new(
        input.hash_str.clone(),
        if tags.is_empty() { None } else { Some(tags) },
    );
    let nut10 = Nut10Secret::new(Kind::HTLC, secret_data);

    let secret: SecretString = match nut10.try_into() {
        Ok(s) => s,
        Err(_) => return,
    };

    let c_point = pubkey_from(input.c_seed);
    let keyset_id = Id::from_bytes(&input.keyset_id_bytes)
        .unwrap_or_else(|_| Id::from_str("00deadbeef123456").expect("valid id"));

    let witness = Witness::HTLCWitness(HTLCWitness {
        preimage: input.preimage.clone(),
        signatures: input.witness_sigs.clone(),
    });

    let proof = Proof {
        amount: Amount::from(input.amount),
        keyset_id,
        secret,
        c: c_point,
        witness: Some(witness),
        dleq: None,
        p2pk_e: None,
    };

    // Silence unused-variant lint when SigFlag enum may grow.
    let _ = Conditions {
        locktime: input.locktime,
        pubkeys: None,
        refund_keys: None,
        num_sigs: input.num_sigs,
        sig_flag: sig_flag_from(input.sig_flag_raw),
        num_sigs_refund: input.num_sigs_refund,
    };

    let _ = proof.verify_htlc();
});
