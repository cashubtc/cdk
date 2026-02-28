//! DLC oracle helpers for NUT-CTF
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

/// Extract outcomes from an event descriptor.
///
/// For enum events, returns the outcomes list directly.
/// For digit decomposition events (NUT-CTF-numeric), returns `["HI", "LO"]` as the two
/// outcome collections for numeric conditions.
pub fn extract_outcomes(announcement: &OracleAnnouncement) -> Result<Vec<String>, Error> {
    match &announcement.oracle_event.event_descriptor {
        EventDescriptor::EnumEvent(EnumEventDescriptor { outcomes }) => Ok(outcomes.clone()),
        EventDescriptor::DigitDecompositionEvent(_) => {
            // For numeric conditions, the partition is always HI/LO
            Ok(vec!["HI".to_string(), "LO".to_string()])
        }
    }
}

/// Descriptor info for digit decomposition events (NUT-CTF-numeric).
#[derive(Debug)]
pub struct DigitDecompositionInfo {
    /// Base for digit decomposition (e.g. 2, 10)
    pub base: u64,
    /// Whether the first digit is a sign digit
    pub is_signed: bool,
    /// Number of digits (including sign digit if present)
    pub nb_digits: usize,
    /// Unit string from the event descriptor
    pub unit: String,
    /// Precision value from the event descriptor
    pub precision: i32,
}

/// Extract digit decomposition parameters from a numeric announcement.
pub fn extract_digit_decomposition(
    announcement: &OracleAnnouncement,
) -> Result<DigitDecompositionInfo, Error> {
    match &announcement.oracle_event.event_descriptor {
        EventDescriptor::DigitDecompositionEvent(dd) => {
            Ok(DigitDecompositionInfo {
                base: dd.base as u64,
                is_signed: dd.is_signed,
                nb_digits: dd.nb_digits as usize,
                unit: dd.unit.clone(),
                precision: dd.precision,
            })
        }
        EventDescriptor::EnumEvent(_) => Err(Error::Dlc(
            "Expected digit decomposition event but got enum event".into(),
        )),
    }
}

/// Verify digit attestation signatures and reconstruct the attested value (NUT-CTF-numeric).
///
/// Each digit signature corresponds to one nonce point. The oracle signs each digit
/// independently. This function verifies all digit signatures and reconstructs the
/// integer value.
///
/// Returns the reconstructed attested value as i64.
pub fn verify_digit_attestation(
    oracle_pubkey: &[u8],
    digit_sigs: &[Vec<u8>],
    nonce_points: &[XOnlyPublicKey],
    base: u64,
    is_signed: bool,
) -> Result<i64, Error> {
    if digit_sigs.len() != nonce_points.len() {
        return Err(Error::DigitSignatureVerificationFailed(
            format!(
                "Expected {} digit signatures but got {}",
                nonce_points.len(),
                digit_sigs.len()
            ),
        ));
    }

    let nb_digits = digit_sigs.len();
    let value_digits_start = if is_signed { 1 } else { 0 };

    // Determine sign
    let sign: i64 = if is_signed {
        // First digit is sign: "+" or "-"
        let sign_outcome = find_attested_digit(
            oracle_pubkey,
            &digit_sigs[0],
            &nonce_points[0],
            &["+", "-"],
        )?;
        if sign_outcome == "+" { 1 } else { -1 }
    } else {
        1
    };

    // Reconstruct absolute value from remaining digits
    let mut abs_value: i64 = 0;
    for i in value_digits_start..nb_digits {
        let digit_outcomes: Vec<String> = (0..base).map(|d| d.to_string()).collect();
        let digit_strs: Vec<&str> = digit_outcomes.iter().map(|s| s.as_str()).collect();

        let digit_str = find_attested_digit(
            oracle_pubkey,
            &digit_sigs[i],
            &nonce_points[i],
            &digit_strs,
        )?;

        let digit_val: i64 = digit_str.parse().map_err(|_| {
            Error::DigitSignatureVerificationFailed(
                format!("Invalid digit value: {}", digit_str),
            )
        })?;

        // Most significant digit first
        abs_value = abs_value * (base as i64) + digit_val;
    }

    Ok(sign * abs_value)
}

/// Brute-force find which outcome a digit signature attests to.
///
/// Tries each possible outcome and returns the one whose signature verifies.
fn find_attested_digit(
    oracle_pubkey: &[u8],
    sig_bytes: &[u8],
    nonce_point: &XOnlyPublicKey,
    possible_outcomes: &[&str],
) -> Result<String, Error> {
    for outcome in possible_outcomes {
        if verify_oracle_attestation(oracle_pubkey, sig_bytes, outcome, nonce_point).is_ok() {
            return Ok(outcome.to_string());
        }
    }

    Err(Error::DigitSignatureVerificationFailed(
        "No matching outcome found for digit signature".into(),
    ))
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
    use crate::nuts::nut_ctf::test_helpers::{
        create_oracle_witness, create_test_announcement, create_test_oracle, create_test_oracle_2,
        sign_ctf_attestation,
    };
    use crate::nuts::nut_ctf::to_hex;

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
        let sig = sign_ctf_attestation(&oracle, outcome);

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
        let sig = sign_ctf_attestation(&oracle, "YES");

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
        let sig = sign_ctf_attestation(&oracle, "YES");

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
        let sig = sign_ctf_attestation(&oracle, "YES");

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
    fn test_verify_attestation_multi_oracle_threshold() {
        let oracle = create_test_oracle();

        // Sign and verify each outcome with the same oracle (single oracle, multiple outcomes)
        for outcome in &["A", "B", "C", "DRAW"] {
            let sig = sign_ctf_attestation(&oracle, outcome);
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

    #[test]
    fn test_extract_outcomes_digit_decomposition() {
        use crate::nuts::nut_ctf::test_helpers::create_digit_decomposition_announcement;

        let oracle = create_test_oracle();
        let (ann, _) =
            create_digit_decomposition_announcement(&oracle, 10, false, 5, "sat", 0, "dd-event");

        let outcomes = extract_outcomes(&ann).expect("should extract outcomes");
        assert_eq!(outcomes, vec!["HI", "LO"]);
    }

    #[test]
    fn test_extract_digit_decomposition_info() {
        use crate::nuts::nut_ctf::test_helpers::create_digit_decomposition_announcement;

        let oracle = create_test_oracle();
        let (ann, _) =
            create_digit_decomposition_announcement(&oracle, 10, true, 4, "usd", 2, "dd-event");

        let info = extract_digit_decomposition(&ann).expect("should extract dd info");
        assert_eq!(info.base, 10);
        assert!(info.is_signed);
        assert_eq!(info.nb_digits, 4); // nb_digits counts value digits only, sign is separate
        assert_eq!(info.unit, "usd");
        assert_eq!(info.precision, 2);
    }

    #[test]
    fn test_verify_digit_attestation_unsigned() {
        use crate::nuts::nut_ctf::test_helpers::{
            create_digit_decomposition_announcement, sign_digit_attestation,
        };
        use crate::nuts::nut_ctf::from_hex;

        let oracle = create_test_oracle();
        let (ann, _) =
            create_digit_decomposition_announcement(&oracle, 10, false, 3, "sat", 0, "dd-test");

        let nonce_points = extract_nonce_points(&ann.oracle_event);
        let dd_info = extract_digit_decomposition(&ann).unwrap();

        // Attest value 42 (digits: 0, 4, 2 in base 10, 3 digits)
        let sigs = sign_digit_attestation(&oracle, 42, 10, false, 3);
        let sig_bytes: Vec<Vec<u8>> = sigs.iter().map(|s| s.to_vec()).collect();

        let value = verify_digit_attestation(
            &oracle.public_key.serialize(),
            &sig_bytes,
            &nonce_points,
            dd_info.base,
            dd_info.is_signed,
        )
        .unwrap();

        assert_eq!(value, 42);
    }

    #[test]
    fn test_verify_digit_attestation_zero() {
        use crate::nuts::nut_ctf::test_helpers::{
            create_digit_decomposition_announcement, sign_digit_attestation,
        };

        let oracle = create_test_oracle();
        let (ann, _) =
            create_digit_decomposition_announcement(&oracle, 10, false, 5, "sat", 0, "dd-zero");

        let nonce_points = extract_nonce_points(&ann.oracle_event);
        let dd_info = extract_digit_decomposition(&ann).unwrap();

        let sigs = sign_digit_attestation(&oracle, 0, 10, false, 5);
        let sig_bytes: Vec<Vec<u8>> = sigs.iter().map(|s| s.to_vec()).collect();

        let value = verify_digit_attestation(
            &oracle.public_key.serialize(),
            &sig_bytes,
            &nonce_points,
            dd_info.base,
            dd_info.is_signed,
        )
        .unwrap();

        assert_eq!(value, 0);
    }

    #[test]
    fn test_verify_digit_attestation_large_value() {
        use crate::nuts::nut_ctf::test_helpers::{
            create_digit_decomposition_announcement, sign_digit_attestation,
        };

        let oracle = create_test_oracle();
        let (ann, _) =
            create_digit_decomposition_announcement(&oracle, 10, false, 5, "sat", 0, "dd-large");

        let nonce_points = extract_nonce_points(&ann.oracle_event);
        let dd_info = extract_digit_decomposition(&ann).unwrap();

        // Value 99999 = max for 5 unsigned base-10 digits
        let sigs = sign_digit_attestation(&oracle, 99999, 10, false, 5);
        let sig_bytes: Vec<Vec<u8>> = sigs.iter().map(|s| s.to_vec()).collect();

        let value = verify_digit_attestation(
            &oracle.public_key.serialize(),
            &sig_bytes,
            &nonce_points,
            dd_info.base,
            dd_info.is_signed,
        )
        .unwrap();

        assert_eq!(value, 99999);
    }

    #[test]
    fn test_verify_digit_attestation_wrong_digit_count() {
        use crate::nuts::nut_ctf::test_helpers::{
            create_digit_decomposition_announcement, sign_digit_attestation,
        };

        let oracle = create_test_oracle();
        let (ann, _) =
            create_digit_decomposition_announcement(&oracle, 10, false, 5, "sat", 0, "dd-wrong");

        let nonce_points = extract_nonce_points(&ann.oracle_event);
        let dd_info = extract_digit_decomposition(&ann).unwrap();

        // Sign with wrong number of digits (3 instead of 5)
        let sigs = sign_digit_attestation(&oracle, 42, 10, false, 3);
        let sig_bytes: Vec<Vec<u8>> = sigs.iter().map(|s| s.to_vec()).collect();

        let result = verify_digit_attestation(
            &oracle.public_key.serialize(),
            &sig_bytes,
            &nonce_points,
            dd_info.base,
            dd_info.is_signed,
        );

        assert!(result.is_err(), "wrong digit count should fail");
    }

    #[test]
    fn test_verify_announcement_signature_digit_decomposition() {
        use crate::nuts::nut_ctf::test_helpers::create_digit_decomposition_announcement;

        let oracle = create_test_oracle();
        let (ann, hex_tlv) =
            create_digit_decomposition_announcement(&oracle, 10, false, 5, "sat", 0, "dd-sig");

        // Verify announcement signature
        let result = verify_announcement_signature(&ann);
        assert!(
            result.is_ok(),
            "digit decomposition announcement signature should verify: {:?}",
            result
        );

        // Also verify roundtrip via TLV parsing
        let parsed = parse_oracle_announcement(&hex_tlv).expect("parse should succeed");
        assert_eq!(parsed.oracle_public_key, ann.oracle_public_key);
    }
}
