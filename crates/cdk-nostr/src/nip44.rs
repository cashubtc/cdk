//! NIP-44 v2 authenticated encryption between Nostr keys
//!
//! The conversation key is derived with secp256k1 ECDH; payloads are padded,
//! ChaCha20-encrypted, HMAC-SHA256 authenticated and base64-encoded.

use nostr_sdk::prelude::nip44 as nostr_nip44;
use nostr_sdk::prelude::{PublicKey, SecretKey};

use crate::error::{Error, Result};

/// Encrypt `plaintext` for `recipient_pubkey` with NIP-44 v2
///
/// Returns the base64-encoded payload.
///
/// # Errors
///
/// Returns [`Error::Nip44`] if the conversation key cannot be derived or the
/// plaintext cannot be encrypted (e.g. empty or over the 65535-byte limit).
pub fn encrypt(
    secret_key: &SecretKey,
    recipient_pubkey: &PublicKey,
    plaintext: &str,
) -> Result<String> {
    nostr_nip44::encrypt(
        secret_key,
        recipient_pubkey,
        plaintext,
        nostr_nip44::Version::V2,
    )
    .map_err(|e| Error::Nip44(e.to_string()))
}

/// Decrypt a base64-encoded NIP-44 v2 `payload` from `sender_pubkey`
///
/// # Errors
///
/// Returns [`Error::Nip44`] if the payload is malformed, the MAC does not
/// verify, or the plaintext is not valid UTF-8.
pub fn decrypt(secret_key: &SecretKey, sender_pubkey: &PublicKey, payload: &str) -> Result<String> {
    nostr_nip44::decrypt(secret_key, sender_pubkey, payload)
        .map_err(|e| Error::Nip44(e.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::keys;

    #[test]
    fn roundtrip() {
        let alice = keys::generate_secret_key();
        let bob = keys::generate_secret_key();

        let payload = encrypt(&alice, &keys::public_key(&bob), "hello cashu").expect("encrypt");
        let decrypted = decrypt(&bob, &keys::public_key(&alice), &payload).expect("decrypt");

        assert_eq!(decrypted, "hello cashu");
    }

    /// Official NIP-44 v2 test vectors
    /// (<https://github.com/nostr-protocol/nips/blob/master/44.md>)
    #[test]
    fn decrypt_official_vectors() {
        let vectors = [
            (
                "0000000000000000000000000000000000000000000000000000000000000001",
                "0000000000000000000000000000000000000000000000000000000000000002",
                "AgAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAABee0G5VSK0/9YypIObAtDKfYEAjD35uVkHyB0F4DwrcNaCXlCWZKaArsGrY6M9wnuTMxWfp1RTN9Xga8no+kF5Vsb",
                "a",
            ),
            (
                "0000000000000000000000000000000000000000000000000000000000000002",
                "0000000000000000000000000000000000000000000000000000000000000001",
                "AvAAAAAAAAAAAAAAAAAAAPAAAAAAAAAAAAAAAAAAAAAPSKSK6is9ngkX2+cSq85Th16oRTISAOfhStnixqZziKMDvB0QQzgFZdjLTPicCJaV8nDITO+QfaQ61+KbWQIOO2Yj",
                "\u{1f355}\u{1fac3}",
            ),
            (
                "5c0c523f52a5b6fad39ed2403092df8cebc36318b39383bca6c00808626fab3a",
                "4b22aa260e4acb7021e32f38a6cdf4b673c6a277755bfce287e370c924dc936d",
                "ArY1I2xC2yDwIbuNHN/1ynXdGgzHLqdCrXUPMwELJPc7s7JqlCMJBAIIjfkpHReBPXeoMCyuClwgbT419jUWU1PwaNl4FEQYKCDKVJz+97Mp3K+Q2YGa77B6gpxB/lr1QgoqpDf7wDVrDmOqGoiPjWDqy8KzLueKDcm9BVP8xeTJIxs=",
                "\u{8868}\u{30dd}\u{3042}A\u{9dd7}\u{152}\u{e9}\u{ff22}\u{900d}\u{dc}\u{df}\u{aa}\u{105}\u{f1}\u{4e02}\u{3400}\u{20000}",
            ),
        ];

        for (sec1, sec2, payload, expected) in vectors {
            let secret = keys::parse_secret_key(sec1).expect("vector secret key");
            let peer = keys::parse_secret_key(sec2).expect("vector peer key");
            let decrypted = decrypt(&secret, &keys::public_key(&peer), payload)
                .expect("official vector decrypts");
            assert_eq!(decrypted, expected);
        }
    }

    #[test]
    fn decrypt_rejects_tampered_mac() {
        let alice = keys::generate_secret_key();
        let bob = keys::generate_secret_key();

        let mut payload = encrypt(&alice, &keys::public_key(&bob), "hello").expect("encrypt");
        let last = payload.len() - 1;
        let replacement = if payload.as_bytes()[last] == b'A' {
            "B"
        } else {
            "A"
        };
        payload.replace_range(last.., replacement);

        assert!(decrypt(&bob, &keys::public_key(&alice), &payload).is_err());
    }
}
