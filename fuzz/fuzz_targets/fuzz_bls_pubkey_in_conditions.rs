#![no_main]

//! Robustness fuzzing for BLS points placed in secp256k1-only positions.
//!
//! NUT-11 (P2PK) and NUT-14 (HTLC) spending conditions are secp256k1-only. A
//! NUT-10 secret is fully attacker-controlled, so its `data` field and its
//! `pubkeys` / `refund` tags can carry the hex of a *valid* compressed BLS
//! G1 (48-byte) or G2 (96-byte) point. Random hex essentially never decodes
//! to a subgroup-correct BLS point, so existing P2PK/HTLC targets cannot reach
//! these encodings.
//!
//! This target injects valid BLS encodings into every key position, runs the
//! verification pipelines, and checks that:
//!   - verification never panics, and
//!   - a lock containing any BLS key is never accepted.
//!
//! Proofs are also built for v3 keysets with BLS `C` points, since spending
//! conditions are independent of the curve that signed the proof.

use std::str::FromStr;

use cashu::nuts::nut00::Witness;
use cashu::nuts::nut01::DhkePoint;
use cashu::nuts::nut02::KeySetVersion;
use cashu::nuts::nut10::{Kind, SecretData};
use cashu::nuts::nut11::P2PKWitness;
use cashu::nuts::nut14::HTLCWitness;
use cashu::nuts::SpendingConditions;
use cashu::secret::Secret as SecretString;
use cashu::{Amount, Id, Nut10Secret, Proof, PublicKey};
use cdk_fuzz::{bls_g1_point_from, bls_g2_key_from, pubkey_from};
use libfuzzer_sys::arbitrary::{Arbitrary, Unstructured};
use libfuzzer_sys::fuzz_target;

/// How to encode a fuzz-chosen key: as secp256k1, BLS G1, or BLS G2.
#[derive(Debug, Clone, Copy)]
enum KeyKind {
    Secp,
    BlsG1,
    BlsG2,
}

impl KeyKind {
    fn from_byte(b: u8) -> Self {
        match b % 3 {
            0 => Self::Secp,
            1 => Self::BlsG1,
            _ => Self::BlsG2,
        }
    }

    fn is_bls(self) -> bool {
        !matches!(self, Self::Secp)
    }

    /// Hex encoding of a valid key of this kind.
    fn key_hex(self, seed: [u8; 32]) -> String {
        match self {
            Self::Secp => pubkey_from(seed).to_hex(),
            Self::BlsG1 => DhkePoint::from(bls_g1_point_from(&seed)).to_hex(),
            Self::BlsG2 => cashu::MintPublicKey::from(bls_g2_key_from(seed)).to_hex(),
        }
    }
}

#[derive(Debug)]
struct Input {
    // Kind tag + seed for each key position. At least one will usually be BLS.
    data_kind: u8,
    data_seed: [u8; 32],
    extra: Vec<(u8, [u8; 32])>,
    refund: Vec<(u8, [u8; 32])>,
    is_htlc: bool,
    locktime: Option<u64>,
    num_sigs: Option<u64>,
    num_sigs_refund: Option<u64>,
    sig_all: bool,
    witness_sigs: Vec<String>,
    amount: u64,
    v3_proof: bool,
    keyset_id_bytes: [u8; 32],
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
            sig_all: u.arbitrary()?,
            witness_sigs: {
                let n = u.int_in_range(0..=4)?;
                (0..n)
                    .map(|_| String::arbitrary(u))
                    .collect::<Result<_, _>>()?
            },
            amount: u.arbitrary()?,
            v3_proof: u.arbitrary()?,
            keyset_id_bytes: u.arbitrary()?,
        })
    }
}

/// Build a key-list tag such as `["pubkeys", k1, k2]`, recording BLS use.
fn key_tag(name: &str, keys: &[(u8, [u8; 32])], has_bls: &mut bool) -> Option<Vec<String>> {
    if keys.is_empty() {
        return None;
    }
    let mut tag = vec![name.to_string()];
    for (kind, seed) in keys {
        let kind = KeyKind::from_byte(*kind);
        *has_bls |= kind.is_bls();
        tag.push(kind.key_hex(*seed));
    }
    Some(tag)
}

fuzz_target!(|input: Input| {
    let mut has_bls = false;

    // HTLC `data` is a hash; P2PK `data` is a (possibly BLS) key.
    let data = if input.is_htlc {
        "00".repeat(32)
    } else {
        let kind = KeyKind::from_byte(input.data_kind);
        has_bls |= kind.is_bls();
        kind.key_hex(input.data_seed)
    };

    // Build the tags by hand: `Conditions` only holds secp256k1 keys, so it
    // cannot carry the BLS encodings this target needs to inject.
    let mut tags: Vec<Vec<String>> = Vec::new();
    tags.extend(key_tag("pubkeys", &input.extra, &mut has_bls));
    tags.extend(key_tag("refund", &input.refund, &mut has_bls));
    if let Some(locktime) = input.locktime {
        tags.push(vec!["locktime".to_string(), locktime.to_string()]);
    }
    if let Some(n) = input.num_sigs {
        tags.push(vec!["n_sigs".to_string(), n.to_string()]);
    }
    if let Some(n) = input.num_sigs_refund {
        tags.push(vec!["n_sigs_refund".to_string(), n.to_string()]);
    }
    let sig_flag = if input.sig_all {
        "SIG_ALL"
    } else {
        "SIG_INPUTS"
    };
    tags.push(vec!["sigflag".to_string(), sig_flag.to_string()]);

    let kind = if input.is_htlc {
        Kind::HTLC
    } else {
        Kind::P2PK
    };
    let nut10 = Nut10Secret::new(kind, SecretData::new(data, Some(tags)));

    // Parsing the lock must reject every BLS key encoding.
    let parsed = SpendingConditions::try_from(nut10.clone());
    if has_bls {
        assert!(
            parsed.is_err(),
            "spending conditions with a BLS key must not parse"
        );
    }

    let secret: SecretString = match nut10.try_into() {
        Ok(s) => s,
        Err(_) => return,
    };

    // `C` only needs to be a well-formed point on the keyset's curve.
    let (keyset_id, c) = if input.v3_proof {
        let mut id_bytes = [0u8; 33];
        id_bytes[0] = KeySetVersion::Version02.to_byte();
        id_bytes[1..].copy_from_slice(&input.keyset_id_bytes);
        let id = Id::from_bytes(&id_bytes).expect("33-byte v3 id");
        (
            id,
            DhkePoint::from(bls_g1_point_from(&input.keyset_id_bytes)),
        )
    } else {
        let id = Id::from_str("00deadbeef123456").expect("valid id");
        (id, DhkePoint::from(pubkey_from([2u8; 32])))
    };

    let witness = if input.is_htlc {
        Witness::HTLCWitness(HTLCWitness {
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
        c,
        witness: Some(witness),
        dleq: None,
        p2pk_e: None,
    };

    // Neither pipeline may panic, and neither may accept a BLS-keyed lock.
    let p2pk = proof.verify_p2pk();
    let htlc = proof.verify_htlc();
    if has_bls {
        assert!(p2pk.is_err(), "P2PK verification accepted a BLS key");
        assert!(htlc.is_err(), "HTLC verification accepted a BLS key");
    }

    // The secp256k1 key parser must reject both BLS encodings outright.
    for kind in [KeyKind::BlsG1, KeyKind::BlsG2] {
        assert!(PublicKey::from_hex(kind.key_hex(input.data_seed)).is_err());
    }
});
