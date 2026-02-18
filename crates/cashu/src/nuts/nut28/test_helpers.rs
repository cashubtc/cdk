//! Test helpers for DLC oracle signing (NUT-28 tests)
//!
//! Provides functions to create test oracles, sign announcements,
//! and produce attestation signatures for unit and integration tests.
#![allow(missing_docs)]

use bitcoin::hashes::sha256::Hash as Sha256Hash;
use bitcoin::hashes::Hash;
use bitcoin::secp256k1::{Keypair, Message, Scalar, Secp256k1, SecretKey, XOnlyPublicKey};
use dlc_messages::oracle_msgs::{
    EnumEventDescriptor, EventDescriptor, OracleAnnouncement, OracleEvent,
};
use dlc_messages::ser_impls::write_as_tlv;

use super::{tagged_hash, to_hex, OracleSig, OracleWitness};

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

    // Serialize the oracle event for signing
    let mut event_bytes = Vec::new();
    write_as_tlv(&oracle_event, &mut event_bytes).expect("serialize oracle event");

    // Sign the event hash (announcement signature)
    let event_hash = Sha256Hash::hash(&event_bytes).to_byte_array();
    let message = Message::from_digest(event_hash);
    let keypair = Keypair::from_secret_key(&secp, &oracle.secret_key);
    let announcement_sig = secp.sign_schnorr_no_aux_rand(&message, &keypair);

    let announcement = OracleAnnouncement {
        announcement_signature: announcement_sig,
        oracle_public_key: oracle.public_key,
        oracle_event,
    };

    // Serialize announcement to TLV hex
    let mut ann_bytes = Vec::new();
    write_as_tlv(&announcement, &mut ann_bytes).expect("serialize announcement");
    let hex_tlv = to_hex(&ann_bytes);

    (announcement, hex_tlv)
}

/// Sign a DLC oracle attestation for a given outcome.
///
/// Uses manual BIP-340 Schnorr signing with the pre-committed nonce,
/// producing a 64-byte signature `(R.x || s)` where `s = k + e*x mod n`.
pub fn sign_nut28_attestation(oracle: &TestOracle, outcome: &str) -> [u8; 64] {
    let secp = Secp256k1::new();

    // Get x-only keys and handle parity
    let (oracle_xonly, oracle_parity) = oracle.secret_key.x_only_public_key(&secp);
    let (nonce_xonly, nonce_parity) = oracle.nonce_secret.x_only_public_key(&secp);

    // Handle BIP-340 y-parity negation
    let mut k = oracle.nonce_secret;
    if nonce_parity == bitcoin::secp256k1::Parity::Odd {
        k = k.negate();
    }

    let mut x = oracle.secret_key;
    if oracle_parity == bitcoin::secp256k1::Parity::Odd {
        x = x.negate();
    }

    // Compute DLC attestation message:
    // msg = tagged_hash("DLC/oracle/attestation/v0", R || P || outcome)
    let mut msg_bytes = Vec::new();
    msg_bytes.extend_from_slice(&nonce_xonly.serialize());
    msg_bytes.extend_from_slice(&oracle_xonly.serialize());
    msg_bytes.extend_from_slice(outcome.as_bytes());
    let msg_hash = tagged_hash("DLC/oracle/attestation/v0", &msg_bytes);

    // Compute BIP-340 challenge:
    // e = tagged_hash("BIP0340/challenge", R || P || msg_hash)
    let mut challenge_input = Vec::new();
    challenge_input.extend_from_slice(&nonce_xonly.serialize());
    challenge_input.extend_from_slice(&oracle_xonly.serialize());
    challenge_input.extend_from_slice(&msg_hash);
    let e_bytes = tagged_hash("BIP0340/challenge", &challenge_input);

    // s = k + e * x mod n (secp256k1 curve order)
    let e_scalar = Scalar::from_be_bytes(e_bytes).expect("challenge hash fits in scalar");
    let ex = x.mul_tweak(&e_scalar).expect("mul_tweak");
    let ex_scalar = Scalar::from_be_bytes(ex.secret_bytes()).expect("product fits in scalar");
    let s = k.add_tweak(&ex_scalar).expect("add_tweak");

    // Build 64-byte signature: R.x || s
    let mut sig = [0u8; 64];
    sig[..32].copy_from_slice(&nonce_xonly.serialize());
    sig[32..].copy_from_slice(&s.secret_bytes());

    sig
}

/// Create an `OracleWitness` with a valid attestation signature.
pub fn create_oracle_witness(oracle: &TestOracle, outcome: &str) -> OracleWitness {
    let sig = sign_nut28_attestation(oracle, outcome);
    OracleWitness {
        oracle_sigs: vec![OracleSig {
            oracle_pubkey: to_hex(&oracle.public_key.serialize()),
            oracle_sig: Some(to_hex(&sig)),
            outcome: outcome.to_string(),
        }],
    }
}
