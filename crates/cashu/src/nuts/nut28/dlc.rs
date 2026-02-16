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
