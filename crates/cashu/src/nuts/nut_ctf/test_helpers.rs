//! Test helpers for DLC oracle signing (NUT-CTF tests)
//!
//! Provides functions to create test oracles, sign announcements,
//! and produce attestation signatures for unit and integration tests.
#![allow(missing_docs)]

use dlc::secp256k1_zkp::{Keypair, Secp256k1, SecretKey, XOnlyPublicKey};
use dlc::secp_utils::schnorrsig_sign_with_nonce;
use dlc_messages::oracle_msgs::{
    tagged_announcement_msg, tagged_attestation_msg, EnumEventDescriptor, EventDescriptor,
    OracleAnnouncement, OracleEvent,
};
use dlc_messages::ser_impls::write_as_tlv;

use super::{to_hex, OracleSig, OracleWitness};

/// Test oracle with keypair and nonce
#[derive(Debug)]
pub struct TestOracle {
    pub secret_key: SecretKey,
    pub public_key: XOnlyPublicKey,
    pub nonce_secret: SecretKey,
    pub nonce_public: XOnlyPublicKey,
}

/// Create a deterministic test oracle (for reproducible tests)
pub fn create_test_oracle() -> TestOracle {
    let secp = Secp256k1::new();

    // Use deterministic keys for reproducibility
    let oracle_secret = SecretKey::from_slice(&[0x11; 32]).expect("valid secret");
    let (oracle_pk, _) = oracle_secret.x_only_public_key(&secp);

    let nonce_secret = SecretKey::from_slice(&[0x22; 32]).expect("valid secret");
    let (nonce_pk, _) = nonce_secret.x_only_public_key(&secp);

    TestOracle {
        secret_key: oracle_secret,
        public_key: oracle_pk,
        nonce_secret,
        nonce_public: nonce_pk,
    }
}

/// Create a second test oracle (for multi-oracle / threshold tests)
pub fn create_test_oracle_2() -> TestOracle {
    let secp = Secp256k1::new();

    let oracle_secret = SecretKey::from_slice(&[0x33; 32]).expect("valid secret");
    let (oracle_pk, _) = oracle_secret.x_only_public_key(&secp);

    let nonce_secret = SecretKey::from_slice(&[0x44; 32]).expect("valid secret");
    let (nonce_pk, _) = nonce_secret.x_only_public_key(&secp);

    TestOracle {
        secret_key: oracle_secret,
        public_key: oracle_pk,
        nonce_secret,
        nonce_public: nonce_pk,
    }
}

/// Create a valid oracle announcement with a proper signature.
///
/// Returns the parsed `OracleAnnouncement` and its hex-encoded TLV representation.
pub fn create_test_announcement(
    oracle: &TestOracle,
    outcomes: &[&str],
    event_id: &str,
) -> (OracleAnnouncement, String) {
    let secp = Secp256k1::new();

    let oracle_event = OracleEvent {
        oracle_nonces: vec![oracle.nonce_public],
        event_maturity_epoch: 1_000_000,
        event_descriptor: EventDescriptor::EnumEvent(EnumEventDescriptor {
            outcomes: outcomes.iter().map(|s| s.to_string()).collect(),
        }),
        event_id: event_id.to_string(),
    };

    // Sign the current DLC spec announcement message.
    let message = tagged_announcement_msg(&oracle_event);
    let keypair = Keypair::from_secret_key(&secp, &oracle.secret_key);
    let announcement_sig = secp.sign_schnorr_no_aux_rand(&message, &keypair);

    let announcement = OracleAnnouncement {
        announcement_signature: announcement_sig,
        oracle_public_key: oracle.public_key,
        oracle_event,
    };

    let mut ann_bytes = Vec::new();
    write_as_tlv(&announcement, &mut ann_bytes).expect("serialize announcement");
    let hex_tlv = to_hex(&ann_bytes);

    (announcement, hex_tlv)
}

/// Sign a DLC oracle attestation for a given outcome.
///
/// Uses the DLC crate's Schnorr nonce-signing helper with the pre-committed
/// oracle nonce, producing a 64-byte BIP-340 signature `(R.x || s)`.
pub fn sign_ctf_attestation(oracle: &TestOracle, outcome: &str) -> [u8; 64] {
    let secp = Secp256k1::new();

    let message = tagged_attestation_msg(outcome);
    let keypair = Keypair::from_secret_key(&secp, &oracle.secret_key);
    let dlc_sig = schnorrsig_sign_with_nonce(
        &secp,
        &message,
        &keypair,
        &oracle.nonce_secret.secret_bytes(),
    );

    let mut sig = [0u8; 64];
    sig.copy_from_slice(dlc_sig.as_ref());
    sig
}

/// Return the hex-encoded x-only public key of a test oracle.
pub fn to_hex_pubkey(oracle: &TestOracle) -> String {
    to_hex(&oracle.public_key.serialize())
}

/// Sign an outcome with a test oracle and return the hex-encoded 64-byte signature.
pub fn sign_hex(oracle: &TestOracle, outcome: &str) -> String {
    to_hex(&sign_ctf_attestation(oracle, outcome))
}

/// Create an `OracleWitness` combining signatures from multiple oracles (for threshold tests).
pub fn create_multi_oracle_witness(oracle_outcomes: &[(&TestOracle, &str)]) -> OracleWitness {
    let oracle_sigs = oracle_outcomes
        .iter()
        .map(|(oracle, outcome)| {
            let sig = sign_ctf_attestation(oracle, outcome);
            OracleSig {
                oracle_pubkey: to_hex(&oracle.public_key.serialize()),
                oracle_sig: Some(to_hex(&sig)),
                outcome: outcome.to_string(),
                digit_sigs: None,
            }
        })
        .collect();
    OracleWitness { oracle_sigs }
}

/// Create an `OracleWitness` with a valid attestation signature (enum mode).
pub fn create_oracle_witness(oracle: &TestOracle, outcome: &str) -> OracleWitness {
    let sig = sign_ctf_attestation(oracle, outcome);
    OracleWitness {
        oracle_sigs: vec![OracleSig {
            oracle_pubkey: to_hex(&oracle.public_key.serialize()),
            oracle_sig: Some(to_hex(&sig)),
            outcome: outcome.to_string(),
            digit_sigs: None,
        }],
    }
}

/// Create a digit decomposition oracle announcement for numeric conditions (NUT-CTF-numeric).
///
/// Returns the parsed `OracleAnnouncement` and its hex-encoded TLV representation.
pub fn create_digit_decomposition_announcement(
    oracle: &TestOracle,
    base: u16,
    is_signed: bool,
    nb_digits: u16,
    unit: &str,
    precision: i32,
    event_id: &str,
) -> (dlc_messages::oracle_msgs::OracleAnnouncement, String) {
    use bitcoin::secp256k1::{Keypair, Secp256k1};
    use dlc_messages::oracle_msgs::{
        tagged_announcement_msg, DigitDecompositionEventDescriptor, OracleAnnouncement, OracleEvent,
    };
    use dlc_messages::ser_impls::write_as_tlv;

    let secp = Secp256k1::new();

    // Create nonce points: one per digit
    let total_nonces = if is_signed { nb_digits + 1 } else { nb_digits };
    let mut nonces = Vec::new();
    // Use the oracle's single nonce for the first, generate deterministic ones for the rest
    nonces.push(oracle.nonce_public);
    for i in 1..total_nonces {
        let mut seed = [0u8; 32];
        seed[0] = 0x55;
        seed[1] = i as u8;
        let nonce_sk = bitcoin::secp256k1::SecretKey::from_slice(&seed).expect("valid secret");
        let (nonce_pk, _) = nonce_sk.x_only_public_key(&secp);
        nonces.push(nonce_pk);
    }

    let oracle_event = OracleEvent {
        oracle_nonces: nonces,
        event_maturity_epoch: 1_000_000,
        event_descriptor: EventDescriptor::DigitDecompositionEvent(
            DigitDecompositionEventDescriptor {
                base,
                is_signed,
                unit: unit.to_string(),
                precision,
                nb_digits,
            },
        ),
        event_id: event_id.to_string(),
    };

    let message = tagged_announcement_msg(&oracle_event);
    let keypair = Keypair::from_secret_key(&secp, &oracle.secret_key);
    let announcement_sig = secp.sign_schnorr_no_aux_rand(&message, &keypair);

    let announcement = OracleAnnouncement {
        announcement_signature: announcement_sig,
        oracle_public_key: oracle.public_key,
        oracle_event,
    };

    let mut ann_bytes = Vec::new();
    write_as_tlv(&announcement, &mut ann_bytes).expect("serialize announcement");
    let hex_tlv = to_hex(&ann_bytes);

    (announcement, hex_tlv)
}

/// Sign digit attestation for a numeric value (NUT-CTF-numeric).
///
/// Produces per-digit Schnorr signatures. Each digit is signed with its own nonce.
/// Returns a Vec of 64-byte signatures (one per digit/nonce).
pub fn sign_digit_attestation(
    oracle: &TestOracle,
    value: i64,
    base: u16,
    is_signed: bool,
    nb_digits: u16,
) -> Vec<[u8; 64]> {
    let secp = Secp256k1::new();

    // Decompose value into digits
    let mut digits: Vec<String> = Vec::new();
    if is_signed {
        digits.push(if value >= 0 {
            "+".to_string()
        } else {
            "-".to_string()
        });
    }

    let abs_val = value.unsigned_abs();
    let mut digit_values = Vec::new();
    let mut remaining = abs_val;
    for _ in 0..nb_digits {
        digit_values.push((remaining % base as u64) as u16);
        remaining /= base as u64;
    }
    digit_values.reverse();
    for d in &digit_values {
        digits.push(d.to_string());
    }

    // Generate nonces matching create_digit_decomposition_announcement
    let total_nonces = if is_signed { nb_digits + 1 } else { nb_digits };
    let mut nonce_secrets = Vec::new();
    nonce_secrets.push(oracle.nonce_secret);
    for i in 1..total_nonces {
        let mut seed = [0u8; 32];
        seed[0] = 0x55;
        seed[1] = i as u8;
        let nonce_sk = SecretKey::from_slice(&seed).expect("valid secret");
        nonce_secrets.push(nonce_sk);
    }

    // Sign each digit
    let mut sigs = Vec::new();
    for (i, digit_str) in digits.iter().enumerate() {
        let nonce_sk = nonce_secrets[i];
        let temp_oracle = TestOracle {
            secret_key: oracle.secret_key,
            public_key: oracle.public_key,
            nonce_secret: nonce_sk,
            nonce_public: {
                let (pk, _) = nonce_sk.x_only_public_key(&secp);
                pk
            },
        };
        sigs.push(sign_ctf_attestation(&temp_oracle, digit_str));
    }

    sigs
}

/// Create an `OracleWitness` with digit signatures for numeric conditions (NUT-CTF-numeric).
pub fn create_numeric_oracle_witness(
    oracle: &TestOracle,
    value: i64,
    base: u16,
    is_signed: bool,
    nb_digits: u16,
) -> OracleWitness {
    let digit_sigs = sign_digit_attestation(oracle, value, base, is_signed, nb_digits);
    let digit_sigs_hex: Vec<String> = digit_sigs.iter().map(|s| to_hex(s)).collect();

    OracleWitness {
        oracle_sigs: vec![OracleSig {
            oracle_pubkey: to_hex(&oracle.public_key.serialize()),
            oracle_sig: None,
            outcome: value.to_string(),
            digit_sigs: Some(digit_sigs_hex),
        }],
    }
}
