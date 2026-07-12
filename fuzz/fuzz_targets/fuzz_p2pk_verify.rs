#![no_main]

//! Deep fuzzing for NUT-11 P2PK proof verification.
//!
//! Goes beyond serde parsing: assembles a structurally valid `Proof` whose
//! `secret` is a `Nut10Secret` with `Kind::P2PK` and a fuzz-controlled
//! `P2PKWitness`, then exercises `Proof::verify_p2pk`. This stresses
//! tag parsing (pubkeys, refund keys, locktime, sig_flag, num_sigs*),
//! signature hex decoding, and the multi-sig threshold logic.
//!
//! A secondary path builds a `BlindedMessage` and fuzzes
//! `BlindedMessage::verify_p2pk` with arbitrary pubkey lists + required sigs.

use std::str::FromStr;

use cashu::nuts::nut00::Witness;
use cashu::nuts::nut10::SecretData;
use cashu::nuts::nut11::{P2PKWitness, SigFlag};
use cashu::nuts::Conditions;
use cashu::secret::Secret as SecretString;
use cashu::{
    Amount, BlindedMessage, Id, Nut10Secret, Proof, PublicKey, SecretKey, SpendingConditions,
};
use cdk_fuzz::{pubkey_from, secret_key_from};
use libfuzzer_sys::arbitrary::{Arbitrary, Unstructured};
use libfuzzer_sys::fuzz_target;

/// Structured fuzz input. Manually implemented `Arbitrary` keeps all
/// production types untouched.
#[derive(Debug)]
struct Input {
    // Receiver + optional extra signer pubkeys.
    receiver_sk_bytes: [u8; 32],
    extra_pubkey_seeds: Vec<[u8; 32]>,
    refund_key_seeds: Vec<[u8; 32]>,
    // Conditions shape.
    locktime: Option<u64>,
    num_sigs: Option<u64>,
    num_sigs_refund: Option<u64>,
    sig_flag_raw: u8,
    // Nonce/secret data fuzz.
    data_override: Option<String>,
    nonce_tag: Option<String>,
    // Witness contents.
    witness_sigs: Vec<String>,
    // Amount + keyset id.
    amount: u64,
    keyset_id_bytes: [u8; 8],
    // Blinded-message path inputs.
    required_sigs: u64,
    // Alternate between generated-valid and arbitrary-invalid verification.
    valid_case: bool,
}

impl<'a> Arbitrary<'a> for Input {
    fn arbitrary(u: &mut Unstructured<'a>) -> libfuzzer_sys::arbitrary::Result<Self> {
        Ok(Self {
            receiver_sk_bytes: u.arbitrary()?,
            extra_pubkey_seeds: {
                let n = u.int_in_range(0..=4)?;
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
            locktime: Option::<u64>::arbitrary(u)?,
            num_sigs: Option::<u64>::arbitrary(u)?,
            num_sigs_refund: Option::<u64>::arbitrary(u)?,
            sig_flag_raw: u.arbitrary()?,
            data_override: Option::<String>::arbitrary(u)?,
            nonce_tag: Option::<String>::arbitrary(u)?,
            witness_sigs: {
                let n = u.int_in_range(0..=4)?;
                (0..n)
                    .map(|_| String::arbitrary(u))
                    .collect::<Result<_, _>>()?
            },
            amount: u.arbitrary()?,
            keyset_id_bytes: u.arbitrary()?,
            required_sigs: u.arbitrary()?,
            valid_case: u.arbitrary()?,
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
    let receiver_sk = secret_key_from(input.receiver_sk_bytes);
    let receiver_pk = receiver_sk.public_key();

    let mut signing_keys: Vec<SecretKey> = vec![receiver_sk];
    for seed in input.extra_pubkey_seeds.iter().copied() {
        let secret_key = secret_key_from(seed);
        if signing_keys
            .iter()
            .all(|existing| existing.public_key() != secret_key.public_key())
        {
            signing_keys.push(secret_key);
        }
    }
    let extra_pubkeys: Vec<PublicKey> = signing_keys
        .iter()
        .skip(1)
        .map(SecretKey::public_key)
        .collect();
    let refund_keys: Vec<PublicKey> = input
        .refund_key_seeds
        .iter()
        .copied()
        .map(pubkey_from)
        .collect();

    let keyset_id = Id::from_bytes(&input.keyset_id_bytes)
        .unwrap_or_else(|_| Id::from_str("00deadbeef123456").expect("valid id"));

    if input.valid_case {
        let required_sigs = 1 + input.required_sigs % signing_keys.len() as u64;
        let conditions = Conditions {
            locktime: None,
            pubkeys: if extra_pubkeys.is_empty() {
                None
            } else {
                Some(extra_pubkeys.clone())
            },
            refund_keys: None,
            num_sigs: Some(required_sigs),
            sig_flag: SigFlag::SigInputs,
            num_sigs_refund: None,
        };
        let spending = SpendingConditions::new_p2pk(receiver_pk, Some(conditions));
        let nut10: Nut10Secret = cdk_fuzz::nut10_secret_unchecked(spending);
        let secret: SecretString = nut10
            .try_into()
            .expect("generated valid P2PK conditions must serialize");
        let mut proof = Proof {
            amount: Amount::from(input.amount),
            keyset_id,
            secret,
            c: receiver_pk,
            witness: None,
            dleq: None,
            p2pk_e: None,
        };
        for key in signing_keys.iter().take(required_sigs as usize).cloned() {
            proof
                .sign_p2pk(key)
                .expect("valid signing key must sign a P2PK proof");
        }
        assert!(
            proof.verify_p2pk().is_ok(),
            "generated P2PK proof must satisfy its signature threshold"
        );

        let mut corrupted = proof.clone();
        corrupted.witness = Some(Witness::P2PKWitness(P2PKWitness {
            signatures: vec!["00".to_string()],
        }));
        assert!(
            corrupted.verify_p2pk().is_err(),
            "malformed signatures must not satisfy P2PK conditions"
        );

        let mut blinded = BlindedMessage::new(
            Amount::from(input.amount),
            keyset_id,
            pubkey_from(input.receiver_sk_bytes),
        );
        for key in signing_keys.iter().take(required_sigs as usize).cloned() {
            blinded
                .sign_p2pk(key)
                .expect("valid signing key must sign a blinded message");
        }
        let pubkeys = signing_keys
            .iter()
            .map(SecretKey::public_key)
            .collect::<Vec<_>>();
        assert!(
            blinded.verify_p2pk(&pubkeys, required_sigs).is_ok(),
            "generated blinded-message signatures must verify"
        );
        return;
    }

    // Build Conditions directly via struct literal so we can exercise past
    // locktimes and weird thresholds that `Conditions::new` would reject.
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
        sig_flag: sig_flag_from(input.sig_flag_raw),
        num_sigs_refund: input.num_sigs_refund,
    };

    let spending = SpendingConditions::new_p2pk(receiver_pk, Some(conditions));

    // Convert to a Nut10Secret, then to the JSON-encoded plaintext `Secret`.
    let mut nut10: Nut10Secret = cdk_fuzz::nut10_secret_unchecked(spending);

    // Optionally mutate the data field to a fuzz-provided string so we can
    // feed malformed hex into the pubkey parser inside `verify_p2pk`.
    if let Some(ref raw) = input.data_override {
        let tags = nut10.secret_data().tags().cloned();
        let new_sd = SecretData::new(raw.clone(), tags);
        nut10 = Nut10Secret::new(nut10.kind(), new_sd);
    }

    // Optionally inject a bogus tag key to exercise unknown-tag handling.
    if let Some(ref tag_val) = input.nonce_tag {
        let mut tags: Vec<Vec<String>> = nut10.secret_data().tags().cloned().unwrap_or_default();
        tags.push(vec!["unknown_tag".to_string(), tag_val.clone()]);
        let new_sd = SecretData::new(nut10.secret_data().data().to_string(), Some(tags));
        nut10 = Nut10Secret::new(nut10.kind(), new_sd);
    }

    let secret: SecretString = match nut10.try_into() {
        Ok(s) => s,
        Err(_) => return,
    };

    // Any well-formed 33-byte compressed pubkey for `C`. Reuse receiver_pk.
    let c_point = receiver_pk;

    let witness = Witness::P2PKWitness(P2PKWitness {
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

    // Primary target: the full P2PK verification pipeline.
    let _ = proof.verify_p2pk();

    // Secondary target: BlindedMessage::verify_p2pk with arbitrary pubkey
    // list and required-sig count.
    let blinded = BlindedMessage::new(Amount::from(input.amount), keyset_id, c_point);
    let _ = blinded.verify_p2pk(&extra_pubkeys, input.required_sigs);
});
