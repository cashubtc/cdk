//! Diffie-Hellmann key exchange

use std::ops::Mul;

use bitcoin::hashes::{sha256, Hash};
#[cfg(feature = "wallet")]
use k256::ProjectivePoint;
use k256::{Scalar, SecretKey};

use crate::error;
#[cfg(feature = "wallet")]
use crate::nuts::nut00::{BlindedSignature, Proof, Proofs};
#[cfg(feature = "wallet")]
use crate::nuts::nut01::{Keys, PublicKey};
#[cfg(feature = "wallet")]
use crate::nuts::*;
use crate::secret::Secret;

fn hash_to_curve(message: &[u8]) -> k256::PublicKey {
    let mut msg_to_hash = message.to_vec();

    loop {
        let hash = sha256::Hash::hash(&msg_to_hash);
        match k256::PublicKey::from_sec1_bytes(
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

#[cfg(feature = "wallet")]
/// Blind Message Alice Step one
pub fn blind_message(
    secret: &[u8],
    blinding_factor: Option<SecretKey>,
) -> Result<(PublicKey, SecretKey), error::wallet::Error> {
    let y = hash_to_curve(secret);

    let r: SecretKey = match blinding_factor {
        Some(sec_key) => sec_key,
        None => SecretKey::random(&mut rand::thread_rng()),
    };

    let b = ProjectivePoint::from(y) + ProjectivePoint::from(&r.public_key());

    Ok((k256::PublicKey::try_from(b)?.into(), r))
}

#[cfg(feature = "wallet")]
/// Unblind Message (Alice Step 3)
pub fn unblind_message(
    // C_
    blinded_key: PublicKey,
    r: SecretKey,
    // A
    mint_pubkey: PublicKey,
) -> Result<PublicKey, error::wallet::Error> {
    // C
    // Unblinded message
    let c = ProjectivePoint::from(Into::<k256::PublicKey>::into(blinded_key).as_affine())
        - Into::<k256::PublicKey>::into(mint_pubkey)
            .as_affine()
            .mul(Scalar::from(r.as_scalar_primitive()));

    Ok(k256::PublicKey::try_from(c)?.into())
}

#[cfg(feature = "wallet")]
/// Construct Proof
pub fn construct_proofs(
    promises: Vec<BlindedSignature>,
    rs: Vec<nut01::SecretKey>,
    secrets: Vec<Secret>,
    keys: &Keys,
) -> Result<Proofs, error::wallet::Error> {
    let mut proofs = vec![];
    for (i, promise) in promises.into_iter().enumerate() {
        let blinded_c = promise.c;
        let a: PublicKey = keys
            .amount_key(promise.amount)
            .ok_or(error::wallet::Error::CustomError(
                "Could not get proofs".to_string(),
            ))?
            .to_owned();

        let unblinded_signature = unblind_message(blinded_c, rs[i].clone().into(), a)?;

        let proof = Proof {
            id: Some(promise.id),
            amount: promise.amount,
            secret: secrets[i].clone(),
            c: unblinded_signature,
        };

        proofs.push(proof);
    }

    Ok(proofs)
}

#[cfg(feature = "mint")]
/// Sign Blinded Message (Step2 bob)
pub fn sign_message(
    a: SecretKey,
    blinded_message: k256::PublicKey,
) -> Result<k256::PublicKey, error::mint::Error> {
    Ok(k256::PublicKey::try_from(
        blinded_message
            .as_affine()
            .mul(Scalar::from(a.as_scalar_primitive())),
    )?)
}

#[cfg(feature = "mint")]
/// Verify Message
pub fn verify_message(
    a: SecretKey,
    unblinded_message: k256::PublicKey,
    msg: &Secret,
) -> Result<(), error::mint::Error> {
    // Y
    let y = hash_to_curve(msg.as_bytes());

    if unblinded_message
        == k256::PublicKey::try_from(*y.as_affine() * Scalar::from(a.as_scalar_primitive()))?
    {
        return Ok(());
    }

    Err(error::mint::Error::TokenNotVerifed)
}

#[cfg(test)]
mod tests {
    use hex::decode;
    use k256::elliptic_curve::scalar::ScalarPrimitive;

    use super::*;

    #[test]
    fn test_hash_to_curve() {
        let secret = "0000000000000000000000000000000000000000000000000000000000000000";
        let sec_hex = decode(secret).unwrap();

        let y = hash_to_curve(&sec_hex);
        let expected_y = k256::PublicKey::from_sec1_bytes(
            &hex::decode("0266687aadf862bd776c8fc18b8e9f8e20089714856ee233b3902a591d0d5f2925")
                .unwrap(),
        )
        .unwrap();
        assert_eq!(y, expected_y);

        let secret = "0000000000000000000000000000000000000000000000000000000000000001";
        let sec_hex = decode(secret).unwrap();
        let y = hash_to_curve(&sec_hex);
        let expected_y = k256::PublicKey::from_sec1_bytes(
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
            k256::PublicKey::from_sec1_bytes(
                &hex::decode("02a9acc1e48c25eeeb9289b5031cc57da9fe72f3fe2861d264bdc074209b107ba2")
                    .unwrap()
            )
            .unwrap()
            .into()
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
        let signed = sign_message(bob_sec, blinded_message.into()).unwrap();

        assert_eq!(
            signed,
            k256::PublicKey::from_sec1_bytes(
                &hex::decode("02a9acc1e48c25eeeb9289b5031cc57da9fe72f3fe2861d264bdc074209b107ba2")
                    .unwrap()
            )
            .unwrap()
        );
    }

    #[test]
    fn test_unblind_message() {
        let blinded_key = k256::PublicKey::from_sec1_bytes(
            &hex::decode("02a9acc1e48c25eeeb9289b5031cc57da9fe72f3fe2861d264bdc074209b107ba2")
                .unwrap(),
        )
        .unwrap();

        let r = SecretKey::new(ScalarPrimitive::ONE);
        let a = k256::PublicKey::from_sec1_bytes(
            &hex::decode("020000000000000000000000000000000000000000000000000000000000000001")
                .unwrap(),
        )
        .unwrap();

        let unblinded = unblind_message(blinded_key.into(), r, a.into()).unwrap();

        assert_eq!(
            Into::<PublicKey>::into(
                k256::PublicKey::from_sec1_bytes(
                    &hex::decode(
                        "03c724d7e6a5443b39ac8acf11f40420adc4f99a02e7cc1b57703d9391f6d129cd"
                    )
                    .unwrap()
                )
                .unwrap()
            ),
            unblinded
        );
    }

    #[ignore]
    #[test]
    fn test_blinded_dhke() {
        // a
        let bob_sec = SecretKey::random(&mut rand::thread_rng());

        // A
        let bob_pub = bob_sec.public_key();

        // let alice_sec = SecretKey::random(&mut rand::thread_rng());

        let x = Secret::new();

        // Y
        let y = hash_to_curve(x.as_bytes());

        // B_
        let blinded = blind_message(&y.to_sec1_bytes(), None).unwrap();

        // C_
        let signed = sign_message(bob_sec.clone(), blinded.0.into()).unwrap();

        // C
        let c = unblind_message(signed.into(), blinded.1, bob_pub.into()).unwrap();

        assert!(verify_message(bob_sec, c.into(), &x).is_ok());
    }
}
