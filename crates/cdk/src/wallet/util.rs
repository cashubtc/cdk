//! Wallet Utility Functions

use std::collections::HashMap;
use std::str::FromStr;

use bitcoin::XOnlyPublicKey;

use crate::nuts::nut10::Kind;
use crate::nuts::{Conditions, Proofs, PublicKey, SecretKey};
use crate::{Error, SECP256K1};

/// Extract token from text
pub fn token_from_text(text: &str) -> Option<&str> {
    let text = text.trim();
    if let Some(start) = text.find("cashu") {
        match text[start..].find(' ') {
            Some(end) => return Some(&text[start..(end + start)]),
            None => return Some(&text[start..]),
        }
    }

    None
}

/// Sign P2PK-locked proofs using the provided signing keys.
///
/// For each proof with a recognized NUT-10 secret:
/// - P2PK: signs the data key (slot 0) and any condition keys (slots 1+)
/// - HTLC: signs condition keys (slots 1+) only; preimage injection is the caller's responsibility
///
/// Proofs without a NUT-10 secret, or with no matching signing key, are left unchanged.
pub(crate) fn sign_proofs(
    proofs: &mut Proofs,
    p2pk_signing_keys: &[SecretKey],
) -> Result<(), Error> {
    if p2pk_signing_keys.is_empty() {
        return Ok(());
    }

    let key_map: HashMap<XOnlyPublicKey, &SecretKey> = p2pk_signing_keys
        .iter()
        .map(|s| (s.x_only_public_key(&SECP256K1).0, s))
        .collect();

    for proof in proofs.iter_mut() {
        let Ok(secret) = <crate::secret::Secret as TryInto<crate::nuts::nut10::Secret>>::try_into(
            proof.secret.clone(),
        ) else {
            continue;
        };

        let conditions: Result<Conditions, _> = secret
            .secret_data()
            .tags()
            .cloned()
            .unwrap_or_default()
            .try_into();

        let Ok(conditions) = conditions else {
            continue;
        };

        let mut pubkeys = Vec::new();

        match secret.kind() {
            Kind::P2PK => {
                let data_key = PublicKey::from_str(secret.secret_data().data())?;
                pubkeys.push(data_key);
            }
            Kind::HTLC => {
                // HTLC slot 0 is a hash, not a pubkey.
                // Condition keys (slots 1+) may still need signing.
                // Preimage injection is handled separately by the caller.
            }
        }

        if let Some(mut cond_pubkeys) = conditions.pubkeys {
            pubkeys.append(&mut cond_pubkeys);
        }
        if let Some(mut refund_keys) = conditions.refund_keys {
            pubkeys.append(&mut refund_keys);
        }

        for (i, pubkey) in pubkeys.iter().enumerate() {
            let slot = match secret.kind() {
                Kind::P2PK => i as u8,
                Kind::HTLC => (i + 1) as u8,
            };
            if let Some(ephemeral_key) = proof.p2pk_e {
                for signing_key in key_map.values() {
                    if let Ok(r) = crate::nuts::nut28::ecdh_kdf(signing_key, &ephemeral_key, slot) {
                        if let Ok(derived_key) =
                            crate::nuts::nut28::derive_signing_key_bip340(signing_key, &r, pubkey)
                        {
                            proof.sign_p2pk(derived_key)?;
                            break;
                        }
                    }
                }
            } else if let Some(signing) = key_map.get(&pubkey.x_only_public_key()) {
                proof.sign_p2pk((*signing).clone())?;
            }
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use std::str::FromStr;

    use crate::nuts::{Id, Proof, SpendingConditions};
    use crate::Amount;

    use super::*;

    #[test]
    fn test_token_from_text() {
        let text = " Here is some ecash: cashuAeyJ0b2tlbiI6W3sicHJvb2ZzIjpbeyJhbW91bnQiOjIsInNlY3JldCI6ImI2Zjk1ODIxYmZlNjUyYjYwZGQ2ZjYwMDU4N2UyZjNhOTk4MzVhMGMyNWI4MTQzODNlYWIwY2QzOWFiNDFjNzUiLCJDIjoiMDI1YWU4ZGEyOTY2Y2E5OGVmYjA5ZDcwOGMxM2FiZmEwZDkxNGUwYTk3OTE4MmFjMzQ4MDllMjYxODY5YTBhNDJlIiwicmVzZXJ2ZWQiOmZhbHNlLCJpZCI6IjAwOWExZjI5MzI1M2U0MWUifSx7ImFtb3VudCI6Miwic2VjcmV0IjoiZjU0Y2JjNmNhZWZmYTY5MTUyOTgyM2M1MjU1MDkwYjRhMDZjNGQ3ZDRjNzNhNDFlZTFkNDBlM2ExY2EzZGZhNyIsIkMiOiIwMjMyMTIzN2JlYjcyMWU3NGI1NzcwNWE5MjJjNjUxMGQwOTYyYzAzNzlhZDM0OTJhMDYwMDliZTAyNjA5ZjA3NTAiLCJyZXNlcnZlZCI6ZmFsc2UsImlkIjoiMDA5YTFmMjkzMjUzZTQxZSJ9LHsiYW1vdW50IjoxLCJzZWNyZXQiOiJhNzdhM2NjODY4YWM4ZGU3YmNiOWMxMzJmZWI3YzEzMDY4Nzg3ODk5Yzk3YTk2NWE2ZThkZTFiMzliMmQ2NmQ3IiwiQyI6IjAzMTY0YTMxNWVhNjM0NGE5NWI2NzM1NzBkYzg0YmZlMTQ2NDhmMTQwM2EwMDJiZmJlMDhlNWFhMWE0NDQ0YWE0MCIsInJlc2VydmVkIjpmYWxzZSwiaWQiOiIwMDlhMWYyOTMyNTNlNDFlIn1dLCJtaW50IjoiaHR0cHM6Ly90ZXN0bnV0LmNhc2h1LnNwYWNlIn1dLCJ1bml0Ijoic2F0In0= fdfdfg
        sdfs";
        let token = token_from_text(text).unwrap();

        let token_str = "cashuAeyJ0b2tlbiI6W3sicHJvb2ZzIjpbeyJhbW91bnQiOjIsInNlY3JldCI6ImI2Zjk1ODIxYmZlNjUyYjYwZGQ2ZjYwMDU4N2UyZjNhOTk4MzVhMGMyNWI4MTQzODNlYWIwY2QzOWFiNDFjNzUiLCJDIjoiMDI1YWU4ZGEyOTY2Y2E5OGVmYjA5ZDcwOGMxM2FiZmEwZDkxNGUwYTk3OTE4MmFjMzQ4MDllMjYxODY5YTBhNDJlIiwicmVzZXJ2ZWQiOmZhbHNlLCJpZCI6IjAwOWExZjI5MzI1M2U0MWUifSx7ImFtb3VudCI6Miwic2VjcmV0IjoiZjU0Y2JjNmNhZWZmYTY5MTUyOTgyM2M1MjU1MDkwYjRhMDZjNGQ3ZDRjNzNhNDFlZTFkNDBlM2ExY2EzZGZhNyIsIkMiOiIwMjMyMTIzN2JlYjcyMWU3NGI1NzcwNWE5MjJjNjUxMGQwOTYyYzAzNzlhZDM0OTJhMDYwMDliZTAyNjA5ZjA3NTAiLCJyZXNlcnZlZCI6ZmFsc2UsImlkIjoiMDA5YTFmMjkzMjUzZTQxZSJ9LHsiYW1vdW50IjoxLCJzZWNyZXQiOiJhNzdhM2NjODY4YWM4ZGU3YmNiOWMxMzJmZWI3YzEzMDY4Nzg3ODk5Yzk3YTk2NWE2ZThkZTFiMzliMmQ2NmQ3IiwiQyI6IjAzMTY0YTMxNWVhNjM0NGE5NWI2NzM1NzBkYzg0YmZlMTQ2NDhmMTQwM2EwMDJiZmJlMDhlNWFhMWE0NDQ0YWE0MCIsInJlc2VydmVkIjpmYWxzZSwiaWQiOiIwMDlhMWYyOTMyNTNlNDFlIn1dLCJtaW50IjoiaHR0cHM6Ly90ZXN0bnV0LmNhc2h1LnNwYWNlIn1dLCJ1bml0Ijoic2F0In0=";

        assert_eq!(token, token_str)
    }

    fn make_p2pk_proof(pubkey: PublicKey) -> Proof {
        let spending_conditions = SpendingConditions::new_p2pk(pubkey, None);
        let nut10_secret: crate::nuts::nut10::Secret = spending_conditions.into();
        let secret: crate::secret::Secret = nut10_secret.try_into().unwrap();
        Proof::new(
            Amount::from(1),
            Id::from_str("00916bbf7ef91a36").unwrap(),
            secret,
            SecretKey::generate().public_key(),
        )
    }

    fn make_plain_proof() -> Proof {
        Proof::new(
            Amount::from(1),
            Id::from_str("00916bbf7ef91a36").unwrap(),
            crate::secret::Secret::generate(),
            SecretKey::generate().public_key(),
        )
    }

    #[test]
    fn sign_proofs_with_correct_key_adds_witness() {
        let secret_key = SecretKey::generate();
        let pubkey = secret_key.public_key();

        let mut proofs = vec![make_p2pk_proof(pubkey)];
        assert!(proofs[0].witness.is_none());

        let keys = vec![secret_key];
        sign_proofs(&mut proofs, &keys).unwrap();

        assert!(proofs[0].witness.is_some());
    }

    #[test]
    fn sign_proofs_with_wrong_key_leaves_proof_unchanged() {
        let pubkey = SecretKey::generate().public_key();
        let wrong_key = SecretKey::generate();

        let mut proofs = vec![make_p2pk_proof(pubkey)];

        let keys = vec![wrong_key];
        sign_proofs(&mut proofs, &keys).unwrap();

        assert!(proofs[0].witness.is_none());
    }

    #[test]
    fn sign_proofs_with_plain_proof_is_noop() {
        let signing_key = SecretKey::generate();
        let mut proofs = vec![make_plain_proof()];

        let keys = vec![signing_key];
        sign_proofs(&mut proofs, &keys).unwrap();

        assert!(proofs[0].witness.is_none());
    }

    #[test]
    fn sign_proofs_with_empty_keys_is_noop() {
        let pubkey = SecretKey::generate().public_key();
        let mut proofs = vec![make_p2pk_proof(pubkey)];

        let empty = Vec::<SecretKey>::new();
        sign_proofs(&mut proofs, &empty).unwrap();

        assert!(proofs[0].witness.is_none());
    }
}
