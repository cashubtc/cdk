//! Diffie-Hellmann key exchange

use bitcoin::hashes::sha256::Hash as Sha256Hash;
use bitcoin::hashes::Hash;
use bitcoin::secp256k1::{Parity, PublicKey as NormalizedPublicKey, Secp256k1, XOnlyPublicKey};
use thiserror::Error;

use crate::nuts::nut01::{bls, BlindingFactor, DhkePoint, MintPublicKey, MintSecretKey, PublicKey};
use crate::nuts::nut02::KeySetVersion;
use crate::nuts::nut12::ProofDleq;
use crate::nuts::{BlindSignature, Keys, Proof, Proofs};
use crate::secret::Secret;
use crate::util::hex;
use crate::SECP256K1;

const DOMAIN_SEPARATOR: &[u8; 28] = b"Secp256k1_HashToCurve_Cashu_";

/// NUT00 Error
#[derive(Debug, Error)]
pub enum Error {
    /// Token could not be validated
    #[error("Token not verified")]
    TokenNotVerified,
    /// No valid point on curve
    #[error("No valid point found")]
    NoValidPoint,
    /// Secp256k1 error
    #[error(transparent)]
    Secp256k1(#[from] bitcoin::secp256k1::Error),
    /// Key error
    #[error(transparent)]
    Key(#[from] crate::nuts::nut01::Error),
    /// Secret error
    #[error(transparent)]
    Secret(#[from] crate::secret::Error),
    // TODO: Remove use anyhow
    /// Custom Error
    #[error("`{0}`")]
    Custom(String),
}

/// Deterministically maps a message to a public key point on the secp256k1
/// curve, utilizing a domain separator to ensure uniqueness.
///
/// For definationn in NUT see [NUT-00](https://github.com/cashubtc/nuts/blob/main/00.md)
pub fn hash_to_curve(message: &[u8]) -> Result<PublicKey, Error> {
    let msg_to_hash: Vec<u8> = [DOMAIN_SEPARATOR, message].concat();

    let msg_hash: [u8; 32] = Sha256Hash::hash(&msg_to_hash).to_byte_array();

    let mut counter: u32 = 0;
    while counter < 2_u32.pow(16) {
        let mut bytes_to_hash: Vec<u8> = Vec::with_capacity(36);
        bytes_to_hash.extend_from_slice(&msg_hash);
        bytes_to_hash.extend_from_slice(&counter.to_le_bytes());
        let hash: [u8; 32] = Sha256Hash::hash(&bytes_to_hash).to_byte_array();

        // Try to parse public key
        match XOnlyPublicKey::from_slice(&hash) {
            Ok(pk) => {
                return Ok(NormalizedPublicKey::from_x_only_public_key(pk, Parity::Even).into())
            }
            Err(_) => {
                counter += 1;
            }
        }
    }

    Err(Error::NoValidPoint)
}

/// Hash a proof secret to the curve used by a keyset version.
pub fn hash_to_curve_for_version(
    message: &[u8],
    version: KeySetVersion,
) -> Result<DhkePoint, Error> {
    match version {
        KeySetVersion::Version00 | KeySetVersion::Version01 => Ok(hash_to_curve(message)?.into()),
        KeySetVersion::Version02 => Ok(bls::BlsG1PublicKey::hash_to_curve(message).into()),
    }
}

/// Hash a proof secret to the curve of a keyset version
///
/// v3 secrets hash their 33 decoded point bytes; see [`Secret::hash_input`].
pub fn hash_secret_to_curve(secret: &Secret, version: KeySetVersion) -> Result<DhkePoint, Error> {
    hash_to_curve_for_version(&secret.hash_input(version)?, version)
}

/// Blind a proof secret for a keyset version
///
/// v3 secrets blind their 33 decoded point bytes; see [`Secret::hash_input`].
pub fn blind_secret(
    secret: &Secret,
    blinding_factor: Option<BlindingFactor>,
    version: KeySetVersion,
) -> Result<(DhkePoint, BlindingFactor), Error> {
    blind_message_for_version(&secret.hash_input(version)?, blinding_factor, version)
}

/// Convert iterator of [`PublicKey`] to byte array
pub fn hash_e<I>(public_keys: I) -> [u8; 32]
where
    I: IntoIterator<Item = PublicKey>,
{
    let mut e: String = String::new();

    for public_key in public_keys.into_iter() {
        let uncompressed: [u8; 65] = public_key.to_uncompressed_bytes();
        e.push_str(&hex::encode(uncompressed));
    }

    Sha256Hash::hash(e.as_bytes()).to_byte_array()
}

/// Blind Message
///
/// `B_ = Y + rG`
pub fn blind_message(
    secret: &[u8],
    blinding_factor: Option<BlindingFactor>,
) -> Result<(DhkePoint, BlindingFactor), Error> {
    blind_message_for_version(secret, blinding_factor, KeySetVersion::Version00)
}

/// Blind Message for a specific keyset version.
pub fn blind_message_for_version(
    secret: &[u8],
    blinding_factor: Option<BlindingFactor>,
    version: KeySetVersion,
) -> Result<(DhkePoint, BlindingFactor), Error> {
    match version {
        KeySetVersion::Version00 | KeySetVersion::Version01 => {
            let y: PublicKey = hash_to_curve(secret)?;
            let r = blinding_factor.unwrap_or_else(BlindingFactor::generate);
            Ok((y.combine(&r.secp_public_key()?)?.into(), r))
        }
        KeySetVersion::Version02 => {
            let r = match blinding_factor {
                Some(r) => r,
                None => BlindingFactor::generate_bls()?,
            };
            let b = bls::BlsG1PublicKey::hash_to_curve(secret).mul(r.as_bls()?);
            Ok((b.into(), r))
        }
    }
}

/// Unblind Message
///
/// `C_ - rK`
pub fn unblind_message(
    // C_
    blinded_key: &DhkePoint,
    r: &BlindingFactor,
    // K
    mint_pubkey: &MintPublicKey,
) -> Result<DhkePoint, Error> {
    let r = r.as_secp256k1()?.as_scalar();

    // a = r * K
    let a = mint_pubkey.as_secp256k1()?.mul_tweak(&SECP256K1, &r)?;

    // C_ - a
    let a: PublicKey = a.negate(&SECP256K1);
    Ok(blinded_key.as_secp256k1()?.combine(&a)?.into()) // C_ + (-a)
}

/// Construct Proof
pub fn construct_proofs(
    promises: Vec<BlindSignature>,
    rs: Vec<BlindingFactor>,
    secrets: Vec<Secret>,
    keys: &Keys,
) -> Result<Proofs, Error> {
    if (promises.len() != rs.len()) || (promises.len() != secrets.len()) {
        tracing::error!(
            "Promises: {}, RS: {}, secrets:{}",
            promises.len(),
            rs.len(),
            secrets.len()
        );
        return Err(Error::Custom(
            "Lengths of promises, rs, and secrets must be equal".to_string(),
        ));
    }
    let mut proofs = vec![];
    for ((blinded_signature, r), secret) in promises.into_iter().zip(rs).zip(secrets) {
        let blinded_c = blinded_signature.c;
        let a = keys
            .amount_key(blinded_signature.amount)
            .ok_or(Error::Custom("Could not get proofs".to_string()))?;

        let unblinded_signature = match blinded_signature.keyset_id.get_version() {
            KeySetVersion::Version00 | KeySetVersion::Version01 => {
                unblind_message(&blinded_c, &r, &a)?
            }
            KeySetVersion::Version02 => {
                let c = blinded_c.as_bls()?.mul(&r.as_bls()?.invert()?);
                c.into()
            }
        };

        blinded_signature
            .ensure_dleq_allowed()
            .map_err(|_| Error::TokenNotVerified)?;
        let dleq = blinded_signature.dleq.map(|d| ProofDleq::new(d.e, d.s, r));

        let proof = Proof {
            amount: blinded_signature.amount,
            keyset_id: blinded_signature.keyset_id,
            secret,
            c: unblinded_signature,
            witness: None,
            dleq,
            p2pk_e: None,
        };

        proofs.push(proof);
    }

    Ok(proofs)
}

/// Sign Blinded Message
///
/// `C_ = k * B_`, where:
/// * `k` is the private key of mint (one for each amount)
/// * `B_` is the blinded message
#[inline]
pub fn sign_message(k: &MintSecretKey, blinded_message: &DhkePoint) -> Result<DhkePoint, Error> {
    match k {
        MintSecretKey::Secp256k1(inner) => {
            let k = inner.as_scalar();
            Ok(blinded_message
                .as_secp256k1()?
                .mul_tweak(&SECP256K1, &k)?
                .into())
        }
        MintSecretKey::Bls(inner) => Ok(blinded_message.as_bls()?.mul(inner).into()),
    }
}

/// Verify Message
pub fn verify_message(
    a: &MintSecretKey,
    unblinded_message: DhkePoint,
    msg: &[u8],
) -> Result<(), Error> {
    // Y
    let y: PublicKey = hash_to_curve(msg)?;

    // Compute the expected unblinded message
    let expected_unblinded_message: PublicKey =
        y.mul_tweak(&Secp256k1::new(), &a.as_secp256k1()?.as_scalar())?;

    // Compare the unblinded_message with the expected value
    if unblinded_message.as_secp256k1()? == expected_unblinded_message {
        return Ok(());
    }

    Err(Error::TokenNotVerified)
}

/// Verify a proof's signature with the mint's secret key.
///
/// The secret key lets the mint check `C == a·Y` directly, without DLEQ
/// proofs or pairings. Any DLEQ proof attached to the proof is ignored, for
/// every keyset version: it carries nothing the mint needs. NUT-12 only says
/// receivers SHOULD treat a v3 proof with a DLEQ as malformed, and rejecting
/// it would fail a valid spend.
pub fn verify_proof_with_mint_key(
    proof: &Proof,
    mint_secret_key: &MintSecretKey,
) -> Result<(), Error> {
    match proof.keyset_id.get_version() {
        KeySetVersion::Version00 | KeySetVersion::Version01 => {
            verify_message(mint_secret_key, proof.c, proof.secret.as_bytes())
        }
        KeySetVersion::Version02 => verify_bls_message_keyed(
            mint_secret_key,
            proof.c,
            &proof.secret.hash_input(KeySetVersion::Version02)?,
        ),
    }
}

/// Verify BLS proof using pairings.
pub fn verify_bls_message(
    mint_pubkey: MintPublicKey,
    unblinded_message: DhkePoint,
    msg: &[u8],
) -> Result<(), Error> {
    if bls::verify_pairing(&unblinded_message.as_bls()?, msg, &mint_pubkey.as_bls()?) {
        return Ok(());
    }

    Err(Error::TokenNotVerified)
}

/// Verify BLS blind signature using pairings.
pub fn verify_bls_blind_signature(
    mint_pubkey: MintPublicKey,
    blinded_signature: DhkePoint,
    blinded_message: DhkePoint,
) -> Result<(), Error> {
    if bls::verify_blind_signature_pairing(
        &blinded_signature.as_bls()?,
        &blinded_message.as_bls()?,
        &mint_pubkey.as_bls()?,
    ) {
        return Ok(());
    }

    Err(Error::TokenNotVerified)
}

/// Verify BLS proof using the mint secret key.
pub fn verify_bls_message_keyed(
    mint_secretkey: &MintSecretKey,
    unblinded_message: DhkePoint,
    msg: &[u8],
) -> Result<(), Error> {
    let y = bls::BlsG1PublicKey::hash_to_curve(msg);
    let expected = y.mul(mint_secretkey.as_bls()?);

    if unblinded_message.as_bls()? == expected {
        return Ok(());
    }

    Err(Error::TokenNotVerified)
}

/// Batch verify BLS proofs using pairings.
pub fn batch_verify_bls_messages(
    mint_pubkeys: &[MintPublicKey],
    unblinded_messages: &[DhkePoint],
    messages: &[&[u8]],
) -> Result<(), Error> {
    let mint_pubkeys = mint_pubkeys
        .iter()
        .map(MintPublicKey::as_bls)
        .collect::<Result<Vec<_>, _>>()?;
    let unblinded_messages = unblinded_messages
        .iter()
        .map(DhkePoint::as_bls)
        .collect::<Result<Vec<_>, _>>()?;

    if bls::batch_verify_pairing(&mint_pubkeys, &unblinded_messages, messages) {
        return Ok(());
    }

    Err(Error::TokenNotVerified)
}

#[cfg(test)]
mod tests {
    use std::str::FromStr;

    use super::*;
    use crate::nuts::{BlindingFactor, MintPublicKey, MintSecretKey, SecretKey};

    #[test]
    fn test_hash_to_curve() {
        let secret = "0000000000000000000000000000000000000000000000000000000000000000";
        let sec_hex = hex::decode(secret).unwrap();

        let y = hash_to_curve(&sec_hex).unwrap();
        let expected_y = PublicKey::from_hex(
            "024cce997d3b518f739663b757deaec95bcd9473c30a14ac2fd04023a739d1a725",
        )
        .unwrap();
        assert_eq!(y, expected_y);

        let secret = "0000000000000000000000000000000000000000000000000000000000000001";
        let sec_hex = hex::decode(secret).unwrap();
        let y = hash_to_curve(&sec_hex).unwrap();
        let expected_y = PublicKey::from_hex(
            "022e7158e11c9506f1aa4248bf531298daa7febd6194f003edcd9b93ade6253acf",
        )
        .unwrap();
        assert_eq!(y, expected_y);
        // Note that this message will take a few iterations of the loop before finding
        // a valid point
        let secret = "0000000000000000000000000000000000000000000000000000000000000002";
        let sec_hex = hex::decode(secret).unwrap();
        let y = hash_to_curve(&sec_hex).unwrap();
        let expected_y = PublicKey::from_hex(
            "026cdbe15362df59cd1dd3c9c11de8aedac2106eca69236ecd9fbe117af897be4f",
        )
        .unwrap();
        assert_eq!(y, expected_y);
    }

    #[test]
    fn test_hash_e() {
        let c = PublicKey::from_str(
            "02a9acc1e48c25eeeb9289b5031cc57da9fe72f3fe2861d264bdc074209b107ba2",
        )
        .unwrap();

        let k = PublicKey::from_str(
            "020000000000000000000000000000000000000000000000000000000000000001",
        )
        .unwrap();

        let r1 = PublicKey::from_str(
            "020000000000000000000000000000000000000000000000000000000000000001",
        )
        .unwrap();

        let r2 = PublicKey::from_str(
            "020000000000000000000000000000000000000000000000000000000000000001",
        )
        .unwrap();

        let e = hash_e(vec![r1, r2, k, c]);
        let e_hex = hex::encode(e);

        assert_eq!(
            "a4dc034b74338c28c6bc3ea49731f2a24440fc7c4affc08b31a93fc9fbe6401e",
            e_hex
        )
    }

    #[test]
    fn test_blind_message() {
        let message =
            hex::decode("d341ee4871f1f889041e63cf0d3823c713eea6aff01e80f1719f08f9e5be98f6")
                .unwrap();
        let sec = BlindingFactor::from_hex(
            "99fce58439fc37412ab3468b73db0569322588f62fb3a49182d67e23d877824a",
        )
        .unwrap();

        let (b, r) = blind_message(&message, Some(sec.clone())).unwrap();

        assert_eq!(sec, r);
        assert_eq!(
            b,
            DhkePoint::from_hex_any_curve(
                "033b1a9737a40cc3fd9b6af4b723632b76a67a36782596304612a6c2bfb5197e6d"
            )
            .unwrap()
        );

        let message =
            hex::decode("f1aaf16c2239746f369572c0784d9dd3d032d952c2d992175873fb58fae31a60")
                .unwrap();
        let sec = BlindingFactor::from_hex(
            "f78476ea7cc9ade20f9e05e58a804cf19533f03ea805ece5fee88c8e2874ba50",
        )
        .unwrap();

        let (b, r) = blind_message(&message, Some(sec.clone())).unwrap();

        assert_eq!(sec, r);
        assert_eq!(
            b,
            DhkePoint::from_hex_any_curve(
                "029bdf2d716ee366eddf599ba252786c1033f47e230248a4612a5670ab931f1763"
            )
            .unwrap()
        );
    }

    #[test]
    fn test_unblind_message() {
        let blinded_key = DhkePoint::from_hex_any_curve(
            "02a9acc1e48c25eeeb9289b5031cc57da9fe72f3fe2861d264bdc074209b107ba2",
        )
        .unwrap();

        let r = BlindingFactor::from_hex(
            "0000000000000000000000000000000000000000000000000000000000000001",
        )
        .unwrap();
        let a = MintPublicKey::from_hex(
            "020000000000000000000000000000000000000000000000000000000000000001",
        )
        .unwrap();

        let unblinded = unblind_message(&blinded_key, &r, &a).unwrap();

        assert_eq!(
            DhkePoint::from_hex_any_curve(
                "03c724d7e6a5443b39ac8acf11f40420adc4f99a02e7cc1b57703d9391f6d129cd"
            )
            .unwrap(),
            unblinded
        );
    }

    #[test]
    fn test_sign_message() {
        use super::*;
        let message = "test_message";
        let sec = BlindingFactor::from_hex(
            "0000000000000000000000000000000000000000000000000000000000000001",
        )
        .unwrap();

        let (blinded_message, _r) = blind_message(message.as_bytes(), Some(sec)).unwrap();
        // A
        let bob_sec = MintSecretKey::from_hex(
            "0000000000000000000000000000000000000000000000000000000000000001",
        )
        .unwrap();

        // C_
        let signed = sign_message(&bob_sec, &blinded_message).unwrap();

        assert_eq!(
            signed,
            DhkePoint::from_hex_any_curve(
                "025cc16fe33b953e2ace39653efb3e7a7049711ae1d8a2f7a9108753f1cdea742b"
            )
            .unwrap()
        );

        // A
        let bob_sec = MintSecretKey::from_hex(
            "7f7f7f7f7f7f7f7f7f7f7f7f7f7f7f7f7f7f7f7f7f7f7f7f7f7f7f7f7f7f7f7f",
        )
        .unwrap();

        // C_
        let signed = sign_message(&bob_sec, &blinded_message).unwrap();

        assert_eq!(
            signed,
            DhkePoint::from_hex_any_curve(
                "027726f0e5757b4202a27198369a3477a17bc275b7529da518fc7cb4a1d927cc0d"
            )
            .unwrap()
        );
    }

    #[test]
    fn test_full_bhke() {
        let message =
            hex::decode("d341ee4871f1f889041e63cf0d3823c713eea6aff01e80f1719f08f9e5be98f6")
                .unwrap();
        let alice_sec = BlindingFactor::from_hex(
            "99fce58439fc37412ab3468b73db0569322588f62fb3a49182d67e23d877824a",
        )
        .unwrap();

        let (b, r) = blind_message(&message, Some(alice_sec.clone())).unwrap();

        let bob_sec = MintSecretKey::from_hex(
            "0000000000000000000000000000000000000000000000000000000000000001",
        )
        .unwrap();

        // C_
        let signed = sign_message(&bob_sec, &b).unwrap();

        let unblinded = unblind_message(&signed, &r, &bob_sec.public_key()).unwrap();

        assert!(verify_message(&bob_sec, unblinded, &message).is_ok());
    }

    #[test]
    fn test_bls_full_dhke() {
        use std::collections::BTreeMap;

        use crate::nuts::nut02::{Id, KeySetVersion};
        use crate::Amount;

        let message = b"test message";
        let r = BlindingFactor::bls_from_reduced_bytes(&[42u8; 32]).unwrap();
        let mint_secret = MintSecretKey::bls_from_reduced_bytes(&[7u8; 32]).unwrap();

        let keyset_id =
            Id::from_bytes(&[vec![KeySetVersion::Version02.to_byte()], vec![1; 32]].concat())
                .expect("valid v3 id");
        let (blinded, returned_r) =
            blind_message_for_version(message, Some(r.clone()), KeySetVersion::Version02)
                .expect("blind");
        assert_eq!(returned_r, r);

        let signed = sign_message(&mint_secret, &blinded).expect("sign");
        let mut keys = BTreeMap::new();
        keys.insert(Amount::from(1), mint_secret.public_key());
        let keys = Keys::new(keys);
        let proof = construct_proofs(
            vec![BlindSignature {
                amount: Amount::from(1),
                keyset_id,
                c: signed,
                dleq: None,
            }],
            vec![r],
            vec![Secret::from_str("test message").expect("secret")],
            &keys,
        )
        .expect("proof")
        .pop()
        .expect("one proof");

        verify_bls_message(mint_secret.public_key(), proof.c, proof.secret.as_bytes())
            .expect("valid pairing");
        assert!(verify_bls_message(
            MintSecretKey::bls_from_reduced_bytes(&[8u8; 32])
                .unwrap()
                .public_key(),
            proof.c,
            proof.secret.as_bytes()
        )
        .is_err());
    }

    #[test]
    fn test_construct_proofs_rejects_v3_dleq() {
        use std::collections::BTreeMap;

        use crate::nuts::nut02::{Id, KeySetVersion};
        use crate::nuts::nut12::BlindSignatureDleq;
        use crate::Amount;

        let message = b"test message";
        let r = BlindingFactor::bls_from_reduced_bytes(&[42u8; 32]).unwrap();
        let mint_secret = MintSecretKey::bls_from_reduced_bytes(&[7u8; 32]).unwrap();
        let keyset_id =
            Id::from_bytes(&[vec![KeySetVersion::Version02.to_byte()], vec![1; 32]].concat())
                .expect("valid v3 id");
        let (blinded, _) =
            blind_message_for_version(message, Some(r.clone()), KeySetVersion::Version02)
                .expect("blind");
        let signed = sign_message(&mint_secret, &blinded).expect("sign");

        let mut keys = BTreeMap::new();
        keys.insert(Amount::from(1), mint_secret.public_key());
        let keys = Keys::new(keys);

        let result = construct_proofs(
            vec![BlindSignature {
                amount: Amount::from(1),
                keyset_id,
                c: signed,
                dleq: Some(BlindSignatureDleq {
                    e: SecretKey::generate(),
                    s: SecretKey::generate(),
                }),
            }],
            vec![r],
            vec![Secret::from_str("test message").expect("secret")],
            &keys,
        );

        assert!(matches!(result, Err(Error::TokenNotVerified)));
    }

    #[test]
    fn test_bls_hash_to_curve_nutshell_vectors() {
        let y = bls::BlsG1PublicKey::hash_to_curve(
            &hex::decode("0000000000000000000000000000000000000000000000000000000000000000")
                .expect("hex"),
        );
        assert_eq!(
            DhkePoint::from(y).to_hex(),
            "a0687086dadc17db3c73fc63d58d61569ca32752a9b92c4e543692bc6b87b293fdcb4e9c870ab6e6d08127deb9382fb9"
        );

        let y = bls::BlsG1PublicKey::hash_to_curve(
            &hex::decode("0000000000000000000000000000000000000000000000000000000000000001")
                .expect("hex"),
        );
        assert_eq!(
            DhkePoint::from(y).to_hex(),
            "8dbdd24f1bc6f485fda14721cb1f15ba72ba34c05f89b5ca38c2a222c07158f471011d50a371cdb365da6bc7ef4139f4"
        );
    }

    #[test]
    fn test_bls_dhke_nutshell_steps() {
        let secret_msg = b"test_message";
        let (blinded, r) =
            blind_message_for_version(secret_msg, None, KeySetVersion::Version02).expect("blind");

        let mint_secret = MintSecretKey::generate_bls().expect("OS randomness");
        let blinded_signature = sign_message(&mint_secret, &blinded).expect("sign");
        let unblinded_signature = blinded_signature
            .as_bls()
            .expect("bls g1")
            .mul(&r.as_bls().expect("bls scalar").invert().expect("inverse"))
            .into();

        verify_bls_message_keyed(&mint_secret, unblinded_signature, secret_msg)
            .expect("keyed verification");
        verify_bls_message(mint_secret.public_key(), unblinded_signature, secret_msg)
            .expect("pairing verification");
    }

    #[test]
    fn test_bls_blind_signature_pairing_verification() {
        let secret_msg = b"test_message";
        let (blinded, _) =
            blind_message_for_version(secret_msg, None, KeySetVersion::Version02).expect("blind");

        let mint_secret = MintSecretKey::generate_bls().expect("OS randomness");
        let blinded_signature = sign_message(&mint_secret, &blinded).expect("sign");

        verify_bls_blind_signature(mint_secret.public_key(), blinded_signature, blinded)
            .expect("valid blind signature pairing");

        let wrong_mint_pubkey = MintSecretKey::generate_bls()
            .expect("OS randomness")
            .public_key();
        assert!(verify_bls_blind_signature(wrong_mint_pubkey, blinded_signature, blinded).is_err());

        let (wrong_blinded, _) =
            blind_message_for_version(b"wrong_message", None, KeySetVersion::Version02)
                .expect("blind");
        assert!(verify_bls_blind_signature(
            mint_secret.public_key(),
            blinded_signature,
            wrong_blinded
        )
        .is_err());
    }

    #[test]
    fn test_bls_batch_pairing_verification_nutshell() {
        let secrets = [b"msg1".as_slice(), b"msg2".as_slice(), b"msg3".as_slice()];
        let mint_secret_1 = MintSecretKey::generate_bls().expect("OS randomness");
        let mint_secret_2 = MintSecretKey::generate_bls().expect("OS randomness");

        let mut mint_pubkeys = Vec::new();
        let mut signatures = Vec::new();

        for (secret, mint_secret) in [
            (secrets[0], &mint_secret_1),
            (secrets[1], &mint_secret_1),
            (secrets[2], &mint_secret_2),
        ] {
            let (blinded, r) =
                blind_message_for_version(secret, None, KeySetVersion::Version02).expect("blind");
            let blinded_signature = sign_message(mint_secret, &blinded).expect("sign");
            let signature = blinded_signature
                .as_bls()
                .expect("bls g1")
                .mul(&r.as_bls().expect("bls scalar").invert().expect("inverse"))
                .into();
            mint_pubkeys.push(mint_secret.public_key());
            signatures.push(signature);
        }

        batch_verify_bls_messages(&mint_pubkeys, &signatures, &secrets)
            .expect("valid batch pairing");

        signatures[0] = signatures[1];
        assert!(batch_verify_bls_messages(&mint_pubkeys, &signatures, &secrets).is_err());
    }

    #[test]
    fn test_deterministic_bls_steps_nutshell_vectors() {
        let secret_msg = b"test_message";
        assert_eq!(
            hash_to_curve_for_version(secret_msg, KeySetVersion::Version02)
                .unwrap()
                .to_hex(),
            "860d58e5aeda1376185436ed96412313424cc079e056d1dab595e6db4c2c9685fec7da052c8db68d88985b75a42388ad"
        );
        let r = BlindingFactor::bls_from_reduced_bytes(
            &hex::decode("0000000000000000000000000000000000000000000000000000000000000003")
                .expect("hex")
                .try_into()
                .expect("32 bytes"),
        )
        .unwrap();
        let mint_secret = MintSecretKey::bls_from_reduced_bytes(
            &hex::decode("0000000000000000000000000000000000000000000000000000000000000002")
                .expect("hex")
                .try_into()
                .expect("32 bytes"),
        )
        .unwrap();

        let (blinded, returned_r) =
            blind_message_for_version(secret_msg, Some(r.clone()), KeySetVersion::Version02)
                .expect("blind");
        assert_eq!(returned_r.to_secret_hex(), r.to_secret_hex());

        assert_eq!(
            mint_secret.public_key().to_hex(),
            "aa4edef9c1ed7f729f520e47730a124fd70662a904ba1074728114d1031e1572c6c886f6b57ec72a6178288c47c335771638533957d540a9d2370f17cc7ed5863bc0b995b8825e0ee1ea1e1e4d00dbae81f14b0bf3611b78c952aacab827a053"
        );
        let blinded_signature = sign_message(&mint_secret, &blinded).expect("sign");
        let unblinded_signature = blinded_signature
            .as_bls()
            .expect("bls g1")
            .mul(&r.as_bls().expect("bls scalar").invert().expect("inverse"))
            .into();

        verify_bls_message_keyed(&mint_secret, unblinded_signature, secret_msg)
            .expect("keyed verification");
        verify_bls_message(mint_secret.public_key(), unblinded_signature, secret_msg)
            .expect("pairing verification");

        assert_eq!(
            blinded.to_hex(),
            "8e88c5f6a93f653784a66b033a00e52128499e18b095c2a56f080d1c2a937ffc9ef4600804a48d087bbd1f662f6b068f"
        );
        assert_eq!(
            blinded_signature.to_hex(),
            "8d52d7a6cbe5e99858d5c15c092d11a0c387c78917471211082a6e5afc2a79680dfa188fafe5d4a51c5398ce160e7a16"
        );
        assert_eq!(
            unblinded_signature.to_hex(),
            "b7a4881059133fd91a8753600d9a5e524c65d6224f6fe2d5aef9e59f1507fdad90b3b4d48ee46da5c8dfaa0b88e28b69"
        );
    }

    /// Tests that `verify_message` correctly rejects verification when using an incorrect key.
    ///
    /// This test ensures that the verification process fails when attempting to verify
    /// a signature with a different key than the one used to create it. This is critical
    /// for security - if this check didn't exist, tokens could be forged by anyone.
    ///
    /// Mutant testing: Catches mutations that remove or weaken the key comparison logic
    /// in `verify_message`, such as always returning Ok or ignoring the key parameter.
    #[test]
    fn test_verify_message_wrong_key() {
        // Test that verify_message fails with wrong key
        let message = b"test message";
        let correct_key = MintSecretKey::from_hex(
            "0000000000000000000000000000000000000000000000000000000000000001",
        )
        .unwrap();
        let wrong_key = MintSecretKey::from_hex(
            "0000000000000000000000000000000000000000000000000000000000000002",
        )
        .unwrap();

        let (blinded, r) = blind_message(message, None).unwrap();
        let signed = sign_message(&correct_key, &blinded).unwrap();
        let unblinded = unblind_message(&signed, &r, &correct_key.public_key()).unwrap();

        // Should fail with wrong key
        assert!(verify_message(&wrong_key, unblinded, message).is_err());
    }

    /// Tests that `verify_message` correctly rejects verification when the message doesn't match.
    ///
    /// This test ensures that attempting to verify a signature against a different message
    /// than the one originally signed results in an error. This prevents message substitution
    /// attacks where an attacker might try to claim a signature for one message is valid
    /// for a different message.
    ///
    /// Mutant testing: Catches mutations that remove or weaken the message comparison logic,
    /// such as skipping the hash_to_curve step or ignoring the message parameter entirely.
    #[test]
    fn test_verify_message_wrong_message() {
        // Test that verify_message fails with wrong message
        let message = b"test message";
        let wrong_message = b"wrong message";
        let key = MintSecretKey::from_hex(
            "0000000000000000000000000000000000000000000000000000000000000001",
        )
        .unwrap();

        let (blinded, r) = blind_message(message, None).unwrap();
        let signed = sign_message(&key, &blinded).unwrap();
        let unblinded = unblind_message(&signed, &r, &key.public_key()).unwrap();

        // Should fail with wrong message
        assert!(verify_message(&key, unblinded, wrong_message).is_err());
    }

    /// Tests that `construct_proofs` returns an error when input vectors have mismatched lengths.
    ///
    /// This test verifies that the function properly validates that the `promises`, `rs`, and
    /// `secrets` vectors all have the same length before processing. This is essential for
    /// correctness - each proof requires exactly one promise, one blinding factor (r), and
    /// one secret. Mismatched lengths would indicate a programming error or corrupted data.
    ///
    /// Mutant testing: Catches mutations that remove or weaken the length validation check
    /// at the beginning of `construct_proofs`, such as changing `!=` to `==` or removing
    /// the validation entirely, which could lead to panics or incorrect proof construction.
    #[test]
    fn test_construct_proofs_length_mismatch() {
        use std::collections::BTreeMap;

        use crate::nuts::nut02::Id;
        use crate::Amount;

        // Test that construct_proofs fails when lengths don't match
        let mut keys_map = BTreeMap::new();
        keys_map.insert(Amount::from(1), MintSecretKey::generate().public_key());
        let keys = Keys::new(keys_map);

        // Mismatched promises and rs lengths
        let promise = BlindSignature {
            amount: Amount::from(1),
            c: SecretKey::generate().public_key().into(),
            keyset_id: Id::from_str("00deadbeef123456").unwrap(),
            dleq: None,
        };
        let promises = vec![promise];
        let rs = vec![BlindingFactor::generate(), BlindingFactor::generate()]; // Different length
        let secrets = vec![Secret::from_str("test").unwrap()];

        let result = construct_proofs(promises, rs, secrets, &keys);
        assert!(result.is_err());
    }

    /// Tests that `construct_proofs` returns the correct number of proof objects.
    ///
    /// This test verifies that when given N valid inputs (promises, blinding factors, secrets),
    /// the function returns exactly N proofs, not zero or any other count. This ensures that
    /// the loop in `construct_proofs` actually processes all inputs and accumulates results
    /// correctly.
    ///
    /// Mutant testing: Specifically designed to catch mutations that replace the function body
    /// with `Ok(Default::default())` or similar shortcuts that would return an empty vector
    /// instead of processing the inputs. This is a common mutation that could pass tests that
    /// only check for success without verifying the actual results.
    #[test]
    fn test_construct_proofs_returns_correct_count() {
        use std::collections::BTreeMap;

        use crate::nuts::nut02::Id;
        use crate::Amount;

        // Test that construct_proofs returns the correct number of proofs
        let secret_key = MintSecretKey::generate();
        let mut keys_map = BTreeMap::new();
        keys_map.insert(Amount::from(1), secret_key.public_key());
        let keys = Keys::new(keys_map);

        let secret = Secret::from_str("test").unwrap();
        let (blinded_message, r) = blind_message(secret.as_bytes(), None).unwrap();
        let signature = sign_message(&secret_key, &blinded_message).unwrap();

        let promise = BlindSignature {
            amount: Amount::from(1),
            c: signature,
            keyset_id: Id::from_str("00deadbeef123456").unwrap(),
            dleq: None,
        };

        let promises = vec![promise.clone(), promise.clone()];
        let rs = vec![r.clone(), r];
        let secrets = vec![secret.clone(), secret];

        let proofs = construct_proofs(promises, rs, secrets, &keys).unwrap();

        // Should return 2 proofs, not 0 (kills the Ok(Default::default()) mutant)
        assert_eq!(proofs.len(), 2);
    }

    /// Tests that hash_to_curve properly increments the counter and terminates.
    ///
    /// The hash_to_curve function uses a counter that increments in a loop at line 61.
    /// If the counter increment is mutated (e.g., to `counter *= 1`), the loop would
    /// never progress and would run until the timeout.
    ///
    /// This test uses a message that requires multiple iterations to find a valid point,
    /// ensuring the counter increment logic is working correctly.
    ///
    /// Mutant testing: Kills mutations that replace `counter += 1` with `counter *= 1`
    /// or other operations that don't advance the counter.
    #[test]
    fn test_hash_to_curve_counter_increments() {
        // This specific message is documented in test_hash_to_curve as taking
        // "a few iterations of the loop before finding a valid point"
        let secret = "0000000000000000000000000000000000000000000000000000000000000002";
        let sec_hex = hex::decode(secret).unwrap();

        let result = hash_to_curve(&sec_hex);
        assert!(result.is_ok(), "hash_to_curve should find a valid point");

        let y = result.unwrap();
        let expected_y = PublicKey::from_hex(
            "026cdbe15362df59cd1dd3c9c11de8aedac2106eca69236ecd9fbe117af897be4f",
        )
        .unwrap();
        assert_eq!(y, expected_y);
    }

    /// tests/00-tests.md transcript vectors: the mint's blind signatures
    /// under keyset 022546ab... (scalars 7, 13, 29, 71 for 1, 2, 4, 8)
    #[test]
    fn test_v3_transcript_vector_blind_signatures() {
        let scalar =
            |n: u8| MintSecretKey::bls_from_slice(&[[0u8; 31].as_slice(), &[n]].concat()).unwrap();
        let b = |hex: &str| DhkePoint::from_hex_for_version(hex, KeySetVersion::Version02).unwrap();
        let b1 = b("b42a0bcc39598db1dca617aeea6bc367f2566636826dc961a54faae15b3b8d10afc1cb0206e70ab3b0e12c2b9478cd55");
        let b2 = b("a14669f181e074cc466e2a165c47ab7db5b3939e42b349f1201874033943ef279f36476e627ab1c937e8ea6a76448fc2");
        // Partial mint: 4 sat on the first B_
        assert_eq!(
            sign_message(&scalar(29), &b1).unwrap().to_hex(),
            "98a471f36a34323f96a1b5783bd98829f9fc69687a9c4f6774c23d89b8f0f2c93e8510992c7942e1980746138c90ae26"
        );
        // Melt change: 1 and 2 sat on the blank outputs, in order
        assert_eq!(
            sign_message(&scalar(7), &b1).unwrap().to_hex(),
            "b8323ba04c279ce45cfd572020ceedd2925f5112769fdc97f8669f9b0e9080d78cc28c1846d59157f8fa6e329c90ad3d"
        );
        assert_eq!(
            sign_message(&scalar(13), &b2).unwrap().to_hex(),
            "a5845c6eff7c6eb2a1b17e7579ed880c1b335e137f1527c143b82c7e606f366bee79d7f9937799fb9941f1e9a060c752"
        );
    }
}
