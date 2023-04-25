//! Diffie-Hellmann key exchange

use std::ops::{Add, Mul, Neg};

use bitcoin_hashes::sha256;
use bitcoin_hashes::Hash;
// use secp256k1::rand::rngs::OsRng;
// use secp256k1::{PublicKey, Scalar, Secp256k1, SecretKey};

use k256::Scalar;
use k256::{AffinePoint, ProjectivePoint, PublicKey, Secp256k1, SecretKey};

use rand::rngs::OsRng;

use crate::error::Error;
use crate::types::MintKeys;
use crate::types::Promise;
use crate::types::Proof;

fn hash_to_curve(message: &[u8]) -> PublicKey {
    let mut msg_to_hash = message.to_vec();

    loop {
        let hash = sha256::Hash::hash(&msg_to_hash);
        match PublicKey::from_sec1_bytes(
            &[0x02u8]
                .iter()
                .chain(&hash.to_byte_array())
                .cloned()
                .collect::<Vec<u8>>(),
        ) {
            Ok(pubkey) => return pubkey,
            Err(_) => msg_to_hash = hash.to_byte_array().to_vec(),
        }
    }
}

/// Blind Message Alice Step one
pub fn blind_message(
    secret: &[u8],
    blinding_factor: Option<SecretKey>,
) -> Result<(PublicKey, SecretKey), Error> {
    let y = hash_to_curve(secret);

    let r: SecretKey = match blinding_factor {
        Some(sec_key) => sec_key,
        None => SecretKey::random(&mut rand::thread_rng()),
    };

    let b = ProjectivePoint::from(y) + ProjectivePoint::from(&r.public_key());

    Ok((PublicKey::try_from(b).unwrap(), r))
}

/// Unblind Message (Alice Step 3)
pub fn unblind_message(
    // C_
    blinded_key: PublicKey,
    r: SecretKey,
    // A
    mint_pubkey: PublicKey,
) -> Result<PublicKey, Error> {
    // C
    // Unblinded message
    let c = ProjectivePoint::from(blinded_key.as_affine())
        - mint_pubkey
            .as_affine()
            .mul(Scalar::from(r.as_scalar_primitive()));

    Ok(PublicKey::try_from(c).unwrap())
}

/// Sign Blinded Message (Step2 bob)
// Really only needed for mint
// Used here for testing
fn _sign_message(a: SecretKey, blinded_message: PublicKey) -> Result<PublicKey, Error> {
    Ok(PublicKey::try_from(
        blinded_message
            .as_affine()
            .mul(Scalar::from(a.as_scalar_primitive())),
    )
    .unwrap())
}

/// Verify Message
// Really only needed for mint
// used for testing
fn _verify_message(a: SecretKey, unblinded_message: PublicKey, msg: &str) -> Result<bool, Error> {
    // Y
    let y = hash_to_curve(msg.as_bytes());

    Ok(unblinded_message
        == PublicKey::try_from(*y.as_affine() * Scalar::from(a.as_scalar_primitive())).unwrap())
}

/// Construct Proof
pub fn construct_proof(
    promises: Vec<Promise>,
    rs: Vec<SecretKey>,
    secrets: Vec<String>,
    keys: &MintKeys,
) -> Result<Vec<Proof>, Error> {
    let mut proofs = vec![];
    for (i, promise) in promises.into_iter().enumerate() {
        let blinded_c = promise.c;
        let a: PublicKey = PublicKey::from_sec1_bytes(
            keys.0
                .get(&promise.amount.to_sat())
                .unwrap()
                .to_owned()
                .as_bytes(),
        )
        .unwrap();
        // println!("Construct proof Pub {:?}", serde_json::to_string(&a));
        todo!();
        let unblinded_signature = unblind_message(blinded_c, rs[i], a)?;

        let proof = Proof {
            id: Some(promise.id),
            amount: promise.amount,
            secret: secrets[i].clone(),
            c: unblinded_signature,
            script: None,
        };

        proofs.push(proof);
    }

    println!("proofs: {:?}", proofs);

    Ok(proofs)
}
pub fn verify_proof(proof: Proof, keys: &MintKeys) -> Result<(), Error> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use hex::decode;

    use k256::elliptic_curve::scalar::ScalarPrimitive;

    use super::*;
    use crate::utils::generate_secret;

    #[test]
    fn test_hash_to_curve() {
        let secret = "0000000000000000000000000000000000000000000000000000000000000000";
        let sec_hex = decode(secret).unwrap();

        let y = hash_to_curve(&sec_hex);
        let expected_y = PublicKey::from_sec1_bytes(
            &hex::decode("0266687aadf862bd776c8fc18b8e9f8e20089714856ee233b3902a591d0d5f2925")
                .unwrap(),
        )
        .unwrap();
        assert_eq!(y, expected_y);

        let secret = "0000000000000000000000000000000000000000000000000000000000000001";
        let sec_hex = decode(secret).unwrap();
        let y = hash_to_curve(&sec_hex);
        let expected_y = PublicKey::from_sec1_bytes(
            &hex::decode("02ec4916dd28fc4c10d78e287ca5d9cc51ee1ae73cbfde08c6b37324cbfaac8bc5")
                .unwrap(),
        )
        .unwrap();
        assert_eq!(y, expected_y);
    }

    #[test]
    fn test_blind_message() {
        let message = "test_message";
        let sec = SecretKey::new(ScalarPrimitive::ONE);

        let (b, r) = blind_message(message.as_bytes(), Some(sec.clone())).unwrap();

        assert_eq!(
            b,
            PublicKey::from_sec1_bytes(
                &hex::decode("02a9acc1e48c25eeeb9289b5031cc57da9fe72f3fe2861d264bdc074209b107ba2")
                    .unwrap()
            )
            .unwrap()
        );

        assert_eq!(r, sec);
    }

    #[test]
    fn test_sign_message() {
        let message = "test_message";
        let sec = SecretKey::new(ScalarPrimitive::ONE);

        let (blinded_message, _r) = blind_message(message.as_bytes(), Some(sec)).unwrap();

        // A
        let bob_sec = SecretKey::new(ScalarPrimitive::ONE);

        // C_
        let signed = _sign_message(bob_sec, blinded_message).unwrap();

        assert_eq!(
            signed,
            PublicKey::from_sec1_bytes(
                &hex::decode("02a9acc1e48c25eeeb9289b5031cc57da9fe72f3fe2861d264bdc074209b107ba2")
                    .unwrap()
            )
            .unwrap()
        );
    }

    #[test]
    fn test_unblind_message() {
        let blinded_key = PublicKey::from_sec1_bytes(
            &hex::decode("02a9acc1e48c25eeeb9289b5031cc57da9fe72f3fe2861d264bdc074209b107ba2")
                .unwrap(),
        )
        .unwrap();

        let r = SecretKey::new(ScalarPrimitive::ONE);
        let a = PublicKey::from_sec1_bytes(
            &hex::decode("020000000000000000000000000000000000000000000000000000000000000001")
                .unwrap(),
        )
        .unwrap();

        let unblinded = unblind_message(blinded_key, r, a).unwrap();

        assert_eq!(
            PublicKey::from_sec1_bytes(
                &hex::decode("03c724d7e6a5443b39ac8acf11f40420adc4f99a02e7cc1b57703d9391f6d129cd")
                    .unwrap()
            )
            .unwrap(),
            unblinded
        );
    }

    #[test]
    fn test_blinded_dhke() {
        // a
        let bob_sec = SecretKey::random(&mut rand::thread_rng());

        // A
        let bob_pub = bob_sec.public_key();

        // let alice_sec = SecretKey::random(&mut rand::thread_rng());

        let x = generate_secret();

        // Y
        let y = hash_to_curve(x.as_bytes());

        // B_
        let blinded = blind_message(&y.to_sec1_bytes(), None).unwrap();

        // C_
        let signed = _sign_message(bob_sec.clone(), blinded.0).unwrap();

        // C
        let c = unblind_message(signed, blinded.1, bob_pub).unwrap();

        assert!(_verify_message(bob_sec, c, &x).unwrap());
    }
}
