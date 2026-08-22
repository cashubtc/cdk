//! Identity signing
//!
//! The signatory signs arbitrary payloads with the key whose public half it
//! already publishes as `SignatoryKeysets::pubkey`. The payload is not signed
//! bare: it goes through the HMAC-SHA256 construction below first, so a
//! signature produced here cannot be replayed as a NUT-11 or NUT-20 signature
//! over the same bytes, nor those as one of these.
use bitcoin::secp256k1::hashes::{hmac, sha256, Hash, HashEngine, HmacEngine};
use bitcoin::secp256k1::schnorr::Signature;
use cdk_common::{Error, PublicKey, SecretKey};

/// Domain separation tag. It is the HMAC key rather than a message prefix so
/// any verifier can recompute the digest without holding a secret.
const SIGN_DOMAIN: &[u8] = b"Cashu_Signatory_Sign_v1";

/// Digest that is actually signed, HMAC-SHA256 over the payload keyed by the
/// domain tag.
fn signing_digest(payload: &[u8]) -> [u8; 32] {
    let mut engine = HmacEngine::<sha256::Hash>::new(SIGN_DOMAIN);
    engine.input(payload);
    hmac::Hmac::<sha256::Hash>::from_engine(engine).to_byte_array()
}

/// Sign an arbitrary payload with the signatory's identity key.
pub fn sign(secret_key: &SecretKey, payload: &[u8]) -> Result<Signature, Error> {
    Ok(secret_key.sign(&signing_digest(payload))?)
}

/// Verify a signature produced by [`sign`] against the mint's published pubkey.
pub fn verify(pubkey: &PublicKey, payload: &[u8], signature: &Signature) -> Result<(), Error> {
    Ok(pubkey.verify(&signing_digest(payload), signature)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key() -> SecretKey {
        SecretKey::from_hex("0000000000000000000000000000000000000000000000000000000000000001")
            .expect("valid secret key")
    }

    #[test]
    fn digest_is_stable() {
        assert_eq!(
            cdk_common::util::hex::encode(signing_digest(b"an arbitrary stream of bytes")),
            "a169782208c52b367550e7e123cc908508c82040c999c82a8920dfa64b666fd5"
        );
    }

    #[test]
    fn signature_round_trips() {
        let key = key();
        let payload = b"an arbitrary stream of bytes";
        let signature = sign(&key, payload).expect("sign");

        verify(&key.public_key(), payload, &signature).expect("signature should verify");
    }

    #[test]
    fn tampered_payload_does_not_verify() {
        let key = key();
        let signature = sign(&key, b"an arbitrary stream of bytes").expect("sign");

        assert!(verify(&key.public_key(), b"tampered", &signature).is_err());
    }

    #[test]
    fn domain_separation_rejects_a_bare_signature_over_the_same_bytes() {
        // A NUT-11 style signature over the raw payload must not pass as an
        // identity signature over the same bytes.
        let key = key();
        let payload = b"an arbitrary stream of bytes";
        let bare = key.sign(payload).expect("sign");

        assert!(verify(&key.public_key(), payload, &bare).is_err());
    }
}
