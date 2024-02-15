//! Diffie-Hellmann key exchange

use bitcoin::hashes::{sha256, Hash};
#[cfg(feature = "mint")]
pub use mint::{sign_message, verify_message};
#[cfg(feature = "wallet")]
pub use wallet::{blind_message, construct_proofs, unblind_message};

const DOMAIN_SEPARATOR: &[u8; 28] = b"Secp256k1_HashToCurve_Cashu_";

pub fn hash_to_curve(message: &[u8]) -> k256::PublicKey {
    let mut msg_to_hash = [DOMAIN_SEPARATOR, message].concat();

    let mut counter = 0;
    loop {
        let hash = sha256::Hash::hash(&[msg_to_hash, counter.to_string().into_bytes()].concat());
        match k256::PublicKey::from_sec1_bytes(
            &[0x02u8]
                .iter()
                .chain(&hash.to_byte_array())
                .cloned()
                .collect::<Vec<u8>>(),
        ) {
            Ok(pubkey) => return pubkey,
            Err(_) => {
                counter += 1;
                msg_to_hash = hash.to_byte_array().to_vec();
            }
        }
    }
}

#[cfg(feature = "wallet")]
mod wallet {
    use std::ops::Mul;

    use k256::{ProjectivePoint, Scalar, SecretKey};

    use super::hash_to_curve;
    use crate::error;
    use crate::nuts::{BlindedSignature, Keys, Proof, Proofs, PublicKey, *};
    use crate::secret::Secret;
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
                keyset_id: promise.keyset_id,
                amount: promise.amount,
                secret: secrets[i].clone(),
                c: unblinded_signature,
            };

            proofs.push(proof);
        }

        Ok(proofs)
    }
}

#[cfg(feature = "mint")]
mod mint {
    use std::ops::Mul;

    use k256::{Scalar, SecretKey};

    use super::hash_to_curve;
    use crate::error;
    use crate::secret::Secret;
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

    /// Verify Message
    pub fn verify_message(
        a: SecretKey,
        unblinded_message: k256::PublicKey,
        msg: &Secret,
    ) -> Result<(), error::mint::Error> {
        // Y
        let y = hash_to_curve(&msg.to_bytes()?);

        if unblinded_message
            == k256::PublicKey::try_from(*y.as_affine() * Scalar::from(a.as_scalar_primitive()))?
        {
            return Ok(());
        }

        Err(error::mint::Error::TokenNotVerifed)
    }
}

#[cfg(test)]
mod tests {
    use hex::decode;
    use k256::elliptic_curve::scalar::ScalarPrimitive;

    use super::*;

    #[cfg(feature = "wallet")]
    mod wallet_tests {
        use k256::SecretKey;

        use super::*;
        use crate::nuts::PublicKey;

        #[test]
        fn test_hash_to_curve() {
            let secret = "0000000000000000000000000000000000000000000000000000000000000000";
            let sec_hex = decode(secret).unwrap();

            let y = hash_to_curve(&sec_hex);
            let expected_y = k256::PublicKey::from_sec1_bytes(
                &hex::decode("02c03ade6f7345a213ea11acde3fda8514f2b7d836a32dfac38f9596c07258f9a9")
                    .unwrap(),
            )
            .unwrap();
            println!("{}", hex::encode(y.to_sec1_bytes()));
            assert_eq!(y, expected_y);

            let secret = "0000000000000000000000000000000000000000000000000000000000000001";
            let sec_hex = decode(secret).unwrap();
            let y = hash_to_curve(&sec_hex);
            let expected_y = k256::PublicKey::from_sec1_bytes(
                &hex::decode("02a5525df57a880f880f28903f32b421df848b3dc1d2cf0bf3d718d7bd772c2df9")
                    .unwrap(),
            )
            .unwrap();
            println!("{}", hex::encode(y.to_sec1_bytes()));
            assert_eq!(y, expected_y);

            // Note that this message will take a few iterations of the loop before finding
            // a valid point
            let secret = "0000000000000000000000000000000000000000000000000000000000000002";
            let sec_hex = decode(secret).unwrap();
            let y = hash_to_curve(&sec_hex);
            let expected_y = k256::PublicKey::from_sec1_bytes(
                &hex::decode("0277834447374a42908b34940dc2affc5f0fc4bbddb2e3b209c5c0b18438abf764")
                    .unwrap(),
            )
            .unwrap();
            println!("{}", hex::encode(y.to_sec1_bytes()));
            assert_eq!(y, expected_y);
        }

        #[test]
        fn test_blind_message() {
            let message = "d341ee4871f1f889041e63cf0d3823c713eea6aff01e80f1719f08f9e5be98f6";
            let sec: crate::nuts::SecretKey = crate::nuts::SecretKey::from_hex(
                "99fce58439fc37412ab3468b73db0569322588f62fb3a49182d67e23d877824a",
            )
            .unwrap();

            println!("{}", sec.to_hex());

            let (b, r) =
                blind_message(&hex::decode(message).unwrap(), Some(sec.clone().into())).unwrap();

            assert_eq!(sec, r.into());

            assert_eq!(
                b.to_hex(),
                PublicKey::from(
                    k256::PublicKey::from_sec1_bytes(
                        &hex::decode(
                            "03039eb7fb76a0db827d7b978a508e3319db03cde6ca8744ef32d0b4e4f455f5dc"
                        )
                        .unwrap()
                    )
                    .unwrap()
                )
                .to_hex()
            );

            let message = "f1aaf16c2239746f369572c0784d9dd3d032d952c2d992175873fb58fae31a60";
            let sec: crate::nuts::SecretKey = crate::nuts::SecretKey::from_hex(
                "f78476ea7cc9ade20f9e05e58a804cf19533f03ea805ece5fee88c8e2874ba50",
            )
            .unwrap();

            println!("{}", sec.to_hex());

            let (b, r) =
                blind_message(&hex::decode(message).unwrap(), Some(sec.clone().into())).unwrap();

            assert_eq!(sec, r.into());

            assert_eq!(
                b.to_hex(),
                PublicKey::from(
                    k256::PublicKey::from_sec1_bytes(
                        &hex::decode(
                            "036498fe9280b09e071c6f838a185d9f0caa1bf84fe9b5cafe595f1898c8c23f9e"
                        )
                        .unwrap()
                    )
                    .unwrap()
                )
                .to_hex()
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
    }

    #[cfg(feature = "mint")]
    mod mint_test {

        use k256::SecretKey;

        use super::{hash_to_curve, *};
        use crate::secret::Secret;

        #[test]
        fn test_sign_message() {
            use super::*;
            let message = "test_message";
            let sec = SecretKey::new(ScalarPrimitive::ONE);

            let (blinded_message, _r) = blind_message(message.as_bytes(), Some(sec)).unwrap();

            // A
            let bob_sec = SecretKey::new(ScalarPrimitive::ONE);

            // C_
            let signed = sign_message(bob_sec, blinded_message.clone().into()).unwrap();

            assert_eq!(
                signed,
                k256::PublicKey::from_sec1_bytes(
                    &hex::decode(
                        "03342e7f3dd691e1e82ede680f51a826991fb9b261a051860dd493a713ae61a84b"
                    )
                    .unwrap()
                )
                .unwrap()
            );

            // A
            let bob_sec = crate::nuts::SecretKey::from_hex(
                "7f7f7f7f7f7f7f7f7f7f7f7f7f7f7f7f7f7f7f7f7f7f7f7f7f7f7f7f7f7f7f7f",
            )
            .unwrap();

            // C_
            let signed = sign_message(bob_sec.into(), blinded_message.into()).unwrap();

            println!("{}", hex::encode(signed.to_sec1_bytes()));
            assert_eq!(
                signed,
                k256::PublicKey::from_sec1_bytes(
                    &hex::decode(
                        "039387dbf13b55919606ba42b5302bd97895bf0ee6bfcff5c0fe8efe5eb2ce50da"
                    )
                    .unwrap()
                )
                .unwrap()
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
            let y = hash_to_curve(&x.to_bytes().unwrap());

            // B_
            let blinded = blind_message(&y.to_sec1_bytes(), None).unwrap();

            // C_
            let signed = sign_message(bob_sec.clone(), blinded.0.into()).unwrap();

            // C
            let c = unblind_message(signed.into(), blinded.1, bob_pub.into()).unwrap();

            assert!(verify_message(bob_sec, c.into(), &x).is_ok());
        }
    }
}
