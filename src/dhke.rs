//! Diffie-Hellmann key exchange

use std::str::FromStr;

use bitcoin_hashes::sha256;
use bitcoin_hashes::Hash;
use secp256k1::rand::rngs::OsRng;
use secp256k1::{PublicKey, Scalar, Secp256k1, SecretKey};

use crate::error::Error;
use crate::types::MintKeys;
use crate::types::Promise;
use crate::types::Proof;

/// Hash to Curve
pub fn hash_to_curve(secret_message: &[u8]) -> Result<PublicKey, Error> {
    let mut msg = secret_message.to_vec();
    loop {
        let hash = sha256::Hash::hash(&msg);
        let mut pubkey_bytes = vec![0x02];
        pubkey_bytes.extend_from_slice(&hash[..]);

        match PublicKey::from_slice(&pubkey_bytes) {
            Ok(pubkey) => return Ok(pubkey),
            Err(_) => {
                msg = hash.to_byte_array().to_vec();
            }
        }
    }
}

/// Blind Message
pub fn blind_message(
    secret: &[u8],
    blinding_factor: Option<SecretKey>,
) -> Result<(PublicKey, SecretKey), Error> {
    let y = hash_to_curve(secret)?;

    let secp = Secp256k1::new();
    let r: SecretKey = match blinding_factor {
        Some(sec_key) => sec_key,
        None => {
            let (secret_key, _public_key) = secp.generate_keypair(&mut OsRng);
            secret_key
        }
    };

    let b = y.combine(&r.public_key(&secp))?;

    Ok((b, r))
}

/// Unblind Message
pub fn unblind_message(
    blinded_key: PublicKey,
    r: SecretKey,
    a: PublicKey,
) -> Result<PublicKey, Error> {
    let secp = Secp256k1::new();
    let a_neg = a.negate(&secp);
    let blinded_key = blinded_key.combine(&a_neg).unwrap();
    let unblinded_key =
        blinded_key.mul_tweak(&secp, &Scalar::from_be_bytes(r.secret_bytes()).unwrap())?;
    Ok(unblinded_key)
}

/// Construct Proof
pub fn construct_proof(
    promises: Vec<Promise>,
    rs: Vec<SecretKey>,
    secrets: Vec<Vec<u8>>,
    keys: &MintKeys,
) -> Result<Vec<Proof>, Error> {
    let mut proofs = vec![];
    for (i, promise) in promises.into_iter().enumerate() {
        let blinded_c = PublicKey::from_str(&promise.c)?;
        let a: PublicKey = PublicKey::from_str(keys.0.get(&promise.amount.to_sat()).unwrap())?;
        let unblinded_signature = unblind_message(blinded_c, rs[i], a)?;

        let proof = Proof {
            id: Some(promise.id),
            amount: promise.amount,
            secret: hex::encode(&secrets[i]),
            c: unblinded_signature.to_string(),
            script: None,
        };

        proofs.push(proof);
    }

    Ok(proofs)
}

#[cfg(test)]
mod tests {
    use hex::decode;
    use std::str::FromStr;

    use super::*;

    #[test]
    fn test_hash_to_curve() {
        let secret = "0000000000000000000000000000000000000000000000000000000000000000";
        let sec_hex = decode(secret).unwrap();

        let y = hash_to_curve(&sec_hex).unwrap();
        let expected_y = PublicKey::from_str(
            "0266687aadf862bd776c8fc18b8e9f8e20089714856ee233b3902a591d0d5f2925",
        )
        .unwrap();
        assert_eq!(y, expected_y);

        let secret = "0000000000000000000000000000000000000000000000000000000000000001";
        let sec_hex = decode(secret).unwrap();
        let y = hash_to_curve(&sec_hex).unwrap();
        let expected_y = PublicKey::from_str(
            "02ec4916dd28fc4c10d78e287ca5d9cc51ee1ae73cbfde08c6b37324cbfaac8bc5",
        )
        .unwrap();
        assert_eq!(y, expected_y);
    }

    #[test]
    fn test_blind_message() {
        let message = "test_message";
        let blinding_factor = "0000000000000000000000000000000000000000000000000000000000000001";
        let sec = SecretKey::from_str(blinding_factor).unwrap();

        let (b, r) = blind_message(message.as_bytes(), Some(sec)).unwrap();

        assert_eq!(
            b.to_string(),
            "02a9acc1e48c25eeeb9289b5031cc57da9fe72f3fe2861d264bdc074209b107ba2".to_string()
        );

        assert_eq!(r, sec);
    }

    #[test]
    fn test_unblind_message() {
        let blinded_key = PublicKey::from_str(
            "02a9acc1e48c25eeeb9289b5031cc57da9fe72f3fe2861d264bdc074209b107ba2",
        )
        .unwrap();

        let r =
            SecretKey::from_str("0000000000000000000000000000000000000000000000000000000000000001")
                .unwrap();
        let a = PublicKey::from_str(
            "020000000000000000000000000000000000000000000000000000000000000001",
        )
        .unwrap();

        let unblinded = unblind_message(blinded_key, r, a).unwrap();

        assert_eq!(
            PublicKey::from_str(
                "03c724d7e6a5443b39ac8acf11f40420adc4f99a02e7cc1b57703d9391f6d129cd"
            )
            .unwrap(),
            unblinded
        );
    }
}
