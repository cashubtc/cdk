//! DLC oracle helpers for NUT-28
//!
//! Uses the `dlc-messages` crate for announcement TLV parsing
//! and verification of oracle attestation signatures.

use bitcoin::hashes::sha256::Hash as Sha256Hash;
use bitcoin::hashes::Hash;
use bitcoin::secp256k1::{schnorr::Signature, Message, Secp256k1, XOnlyPublicKey};
use dlc_messages::oracle_msgs::{
    EnumEventDescriptor, EventDescriptor, OracleAnnouncement, OracleEvent,
};
use dlc_messages::ser_impls::read_as_tlv;

use super::Error;

/// Parse a hex-encoded oracle announcement TLV into an OracleAnnouncement struct.
pub fn parse_oracle_announcement(hex_tlv: &str) -> Result<OracleAnnouncement, Error> {
    let bytes = super::from_hex(hex_tlv)?;
    let mut cursor = std::io::Cursor::new(&bytes);
    read_as_tlv::<OracleAnnouncement, _>(&mut cursor)
        .map_err(|e| Error::OracleAnnouncementVerificationFailed(format!("TLV parse: {}", e)))
}

/// Extract the event_id from a parsed announcement.
pub fn extract_event_id(announcement: &OracleAnnouncement) -> String {
    announcement.oracle_event.event_id.clone()
}

/// Extract the oracle's x-only public key bytes from a parsed announcement.
pub fn extract_oracle_pubkey(announcement: &OracleAnnouncement) -> Vec<u8> {
    announcement
        .oracle_public_key
        .serialize()
        .to_vec()
}

/// Extract outcomes from an enum event descriptor.
///
/// Returns `Err` if the event descriptor is not an enum type.
pub fn extract_outcomes(announcement: &OracleAnnouncement) -> Result<Vec<String>, Error> {
    match &announcement.oracle_event.event_descriptor {
        EventDescriptor::EnumEvent(EnumEventDescriptor { outcomes }) => Ok(outcomes.clone()),
        EventDescriptor::DigitDecompositionEvent(_) => {
            // For numeric conditions (NUT-30), outcomes are computed differently
            Err(Error::Dlc(
                "Digit decomposition events not supported in NUT-28 enum mode".into(),
            ))
        }
    }
}

/// Extract the nonce points (R-values) from the oracle event.
pub fn extract_nonce_points(event: &OracleEvent) -> Vec<XOnlyPublicKey> {
    event
        .oracle_nonces
        .clone()
}

/// Verify a DLC oracle attestation signature.
///
/// The oracle signs the outcome using tagged hash `"DLC/oracle/attestation/v0"`:
/// ```text
/// s = k + e * x
/// where e = tagged_hash("DLC/oracle/attestation/v0", R || P || msg)
/// ```
///
/// This verifies `oracle_sig` as `R + e*P` where R is the nonce point.
pub fn verify_oracle_attestation(
    oracle_pubkey: &[u8],
    oracle_sig: &[u8],
    outcome: &str,
    nonce_point: &XOnlyPublicKey,
) -> Result<(), Error> {
    let secp = Secp256k1::verification_only();

    // Parse oracle public key
    let pk = XOnlyPublicKey::from_slice(oracle_pubkey)
        .map_err(|_| Error::InvalidOracleSignature)?;

    // Parse the 64-byte signature
    let sig = Signature::from_slice(oracle_sig)
        .map_err(|_| Error::InvalidOracleSignature)?;

    // Compute the tagged hash message for DLC attestation verification
    // e = tagged_hash("DLC/oracle/attestation/v0", R || P || msg)
    let mut msg_bytes = Vec::new();
    msg_bytes.extend_from_slice(&nonce_point.serialize());
    msg_bytes.extend_from_slice(&pk.serialize());
    msg_bytes.extend_from_slice(outcome.as_bytes());
    let msg_hash = super::tagged_hash("DLC/oracle/attestation/v0", &msg_bytes);
    let message = Message::from_digest(msg_hash);

    // Verify using standard Schnorr verification on the oracle pubkey
    // The DLC attestation sig is s = k + e*x, verified as s*G = R + e*P
    secp.verify_schnorr(&sig, &message, &pk)
        .map_err(|_| Error::InvalidOracleSignature)
}

/// Verify that an oracle announcement's TLV signature is valid.
///
/// The announcement contains a Schnorr signature over the serialized oracle event.
pub fn verify_announcement_signature(
    announcement: &OracleAnnouncement,
) -> Result<(), Error> {
    let secp = Secp256k1::verification_only();

    // Serialize the oracle event for signature verification
    let mut event_bytes = Vec::new();
    dlc_messages::ser_impls::write_as_tlv(
        &announcement.oracle_event,
        &mut event_bytes,
    )
    .map_err(|e| {
        Error::OracleAnnouncementVerificationFailed(format!("Event serialization: {}", e))
    })?;

    // Hash the serialized event
    let event_hash = Sha256Hash::hash(&event_bytes).to_byte_array();
    let message = Message::from_digest(event_hash);

    // Verify the announcement signature
    secp.verify_schnorr(
        &announcement.announcement_signature,
        &message,
        &announcement.oracle_public_key,
    )
    .map_err(|_| {
        Error::OracleAnnouncementVerificationFailed("Signature verification failed".into())
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::nuts::nut28::test_helpers::{
        create_oracle_witness, create_test_announcement, create_test_oracle, create_test_oracle_2,
        sign_nut28_attestation,
    };
    use crate::nuts::nut28::to_hex;

    #[test]
    fn test_parse_announcement_roundtrip() {
        let oracle = create_test_oracle();
        let (original, hex_tlv) = create_test_announcement(&oracle, &["YES", "NO"], "test-event");

        let parsed = parse_oracle_announcement(&hex_tlv).expect("parse should succeed");
        assert_eq!(parsed.oracle_public_key, original.oracle_public_key);
        assert_eq!(
            parsed.oracle_event.event_id,
            original.oracle_event.event_id
        );
    }

    #[test]
    fn test_parse_announcement_invalid_hex() {
        assert!(parse_oracle_announcement("not_valid_hex!!").is_err());
        assert!(parse_oracle_announcement("").is_err());
        assert!(parse_oracle_announcement("deadbeef").is_err());
    }

    #[test]
    fn test_extract_event_id() {
        let oracle = create_test_oracle();
        let (ann, _) = create_test_announcement(&oracle, &["YES", "NO"], "my-event-123");
        assert_eq!(extract_event_id(&ann), "my-event-123");
    }

    #[test]
    fn test_extract_oracle_pubkey() {
        let oracle = create_test_oracle();
        let (ann, _) = create_test_announcement(&oracle, &["YES", "NO"], "evt");

        let pubkey_bytes = extract_oracle_pubkey(&ann);
        assert_eq!(pubkey_bytes.len(), 32);
        assert_eq!(pubkey_bytes, oracle.public_key.serialize().to_vec());
    }

    #[test]
    fn test_extract_outcomes() {
        let oracle = create_test_oracle();
        let (ann, _) = create_test_announcement(&oracle, &["WIN", "LOSE", "DRAW"], "game");

        let outcomes = extract_outcomes(&ann).expect("should extract outcomes");
        assert_eq!(outcomes, vec!["WIN", "LOSE", "DRAW"]);
    }

    #[test]
    fn test_extract_nonce_points() {
        let oracle = create_test_oracle();
        let (ann, _) = create_test_announcement(&oracle, &["YES", "NO"], "evt");

        let nonces = extract_nonce_points(&ann.oracle_event);
        assert_eq!(nonces.len(), 1);
        assert_eq!(nonces[0], oracle.nonce_public);
    }

    #[test]
    fn test_verify_attestation_valid() {
        let oracle = create_test_oracle();
        let outcome = "YES";
        let sig = sign_nut28_attestation(&oracle, outcome);

        let result = verify_oracle_attestation(
            &oracle.public_key.serialize(),
            &sig,
            outcome,
            &oracle.nonce_public,
        );
        assert!(result.is_ok(), "valid attestation should verify: {:?}", result);
    }

    #[test]
    fn test_verify_attestation_wrong_outcome() {
        let oracle = create_test_oracle();
        let sig = sign_nut28_attestation(&oracle, "YES");

        let result = verify_oracle_attestation(
            &oracle.public_key.serialize(),
            &sig,
            "NO", // wrong outcome
            &oracle.nonce_public,
        );
        assert!(result.is_err(), "wrong outcome should fail verification");
    }

    #[test]
    fn test_verify_attestation_wrong_pubkey() {
        let oracle = create_test_oracle();
        let oracle2 = create_test_oracle_2();
        let sig = sign_nut28_attestation(&oracle, "YES");

        let result = verify_oracle_attestation(
            &oracle2.public_key.serialize(), // wrong pubkey
            &sig,
            "YES",
            &oracle.nonce_public,
        );
        assert!(result.is_err(), "wrong pubkey should fail verification");
    }

    #[test]
    fn test_verify_attestation_wrong_nonce() {
        let oracle = create_test_oracle();
        let oracle2 = create_test_oracle_2();
        let sig = sign_nut28_attestation(&oracle, "YES");

        let result = verify_oracle_attestation(
            &oracle.public_key.serialize(),
            &sig,
            "YES",
            &oracle2.nonce_public, // wrong nonce
        );
        assert!(result.is_err(), "wrong nonce should fail verification");
    }

    #[test]
    fn test_verify_announcement_signature_valid() {
        let oracle = create_test_oracle();
        let (ann, _) = create_test_announcement(&oracle, &["YES", "NO"], "evt");

        let result = verify_announcement_signature(&ann);
        assert!(
            result.is_ok(),
            "valid announcement signature should verify: {:?}",
            result
        );
    }

    #[test]
    fn test_verify_announcement_signature_corrupted() {
        let oracle = create_test_oracle();
        let (mut ann, _) = create_test_announcement(&oracle, &["YES", "NO"], "evt");

        // Corrupt the event_id after signing
        ann.oracle_event.event_id = "tampered".to_string();

        let result = verify_announcement_signature(&ann);
        assert!(
            result.is_err(),
            "corrupted announcement should fail verification"
        );
    }

    #[test]
    fn test_create_oracle_witness_roundtrip() {
        let oracle = create_test_oracle();
        let witness = create_oracle_witness(&oracle, "YES");

        assert_eq!(witness.oracle_sigs.len(), 1);
        assert_eq!(witness.oracle_sigs[0].outcome, "YES");
        assert_eq!(
            witness.oracle_sigs[0].oracle_pubkey,
            to_hex(&oracle.public_key.serialize())
        );
        assert!(witness.oracle_sigs[0].oracle_sig.is_some());
    }

    #[test]
    fn test_verify_attestation_multiple_outcomes() {
        let oracle = create_test_oracle();

        // Sign and verify each outcome
        for outcome in &["A", "B", "C", "DRAW"] {
            let sig = sign_nut28_attestation(&oracle, outcome);
            let result = verify_oracle_attestation(
                &oracle.public_key.serialize(),
                &sig,
                outcome,
                &oracle.nonce_public,
            );
            assert!(
                result.is_ok(),
                "attestation for outcome '{}' should verify: {:?}",
                outcome,
                result
            );
        }
    }
}
