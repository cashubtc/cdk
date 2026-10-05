//! Mint identity key
//!
//! Derivation and signing for the mint's identity key, the key published as
//! `SignatoryKeysets::pubkey` and, downstream, as `MintInfo.pubkey`. Both
//! follow NUT-06 so any wallet holding the published pubkey can verify without
//! a signatory round trip.
use bitcoin::secp256k1::hashes::{hmac, sha256, Hash, HashEngine, HmacEngine};
use bitcoin::secp256k1::schnorr::Signature;
use bitcoin::secp256k1::{Keypair, Message};
use cdk_common::{Error, PublicKey, SecretKey, SECP256K1};

/// NUT-06 domain separator for the identity key derivation.
const MINT_IDENTITY_DOMAIN_SEPARATOR: &[u8] = b"Cashu_Mint_Identity_v1";

/// Derive the mint identity key from the mint seed, per NUT-06.
///
/// `HMAC-SHA256(key = seed, msg = domain_separator || ctr)`, with `ctr`
/// incrementing while the result falls outside `1..n-1`.
pub fn derive_identity_key(seed: &[u8]) -> Result<SecretKey, Error> {
    for ctr in 0..=u8::MAX {
        let mut engine = HmacEngine::<sha256::Hash>::new(seed);
        engine.input(MINT_IDENTITY_DOMAIN_SEPARATOR);
        engine.input(&[ctr]);
        let candidate = hmac::Hmac::<sha256::Hash>::from_engine(engine).to_byte_array();

        match SecretKey::from_slice(&candidate) {
            Ok(secret_key) => return Ok(secret_key),
            Err(error) => {
                tracing::debug!(%error, ctr, "identity key candidate out of range, retrying")
            }
        }
    }

    Err(Error::IdentityKeyDerivation)
}

/// Sign a payload with the mint identity key, per NUT-06.
///
/// BIP-340 over `SHA256(payload)`. The auxiliary randomness is zeroed so the
/// signature over a given payload is stable across calls and reproduces the
/// NUT-06 example vector.
pub fn sign(secret_key: &SecretKey, payload: &[u8]) -> Result<Signature, Error> {
    let digest = sha256::Hash::hash(payload);
    let message = Message::from_digest(digest.to_byte_array());
    let keypair = Keypair::from_secret_key(&SECP256K1, secret_key);

    Ok(SECP256K1.sign_schnorr_no_aux_rand(&message, &keypair))
}

/// Verify a signature produced by [`sign`] against the mint's published pubkey.
pub fn verify(pubkey: &PublicKey, payload: &[u8], signature: &Signature) -> Result<(), Error> {
    Ok(pubkey.verify(payload, signature)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn derivation_matches_the_nut06_vector() {
        let secret_key = derive_identity_key(b"NUT-06 example mint seed").expect("derive");

        assert_eq!(
            secret_key.public_key().to_hex(),
            "0338596797cef0627f653cd6568387361b00314add55d9f1ea9c94f46ae421e3da"
        );
    }

    #[test]
    fn derivation_is_deterministic() {
        let first = derive_identity_key(b"seed").expect("derive");
        let second = derive_identity_key(b"seed").expect("derive");

        assert_eq!(first.public_key(), second.public_key());
    }

    #[test]
    fn signature_is_stable_across_calls() {
        let secret_key = derive_identity_key(b"seed").expect("derive");
        let payload = b"an arbitrary stream of bytes";

        assert_eq!(
            sign(&secret_key, payload).expect("sign").serialize(),
            sign(&secret_key, payload).expect("sign").serialize()
        );
    }

    #[test]
    fn signature_verifies_with_the_plain_public_key_api() {
        let secret_key = derive_identity_key(b"seed").expect("derive");
        let payload = b"an arbitrary stream of bytes";
        let signature = sign(&secret_key, payload).expect("sign");

        secret_key
            .public_key()
            .verify(payload, &signature)
            .expect("a verifier needs nothing from this crate");
        verify(&secret_key.public_key(), payload, &signature).expect("verify");
    }

    #[test]
    fn tampered_payload_does_not_verify() {
        let secret_key = derive_identity_key(b"seed").expect("derive");
        let signature = sign(&secret_key, b"an arbitrary stream of bytes").expect("sign");

        assert!(verify(&secret_key.public_key(), b"tampered", &signature).is_err());
    }
}
