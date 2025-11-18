//! NUT-12: Offline ecash signature validation
//!
//! <https://github.com/cashubtc/nuts/blob/main/12.md>

use core::ops::Deref;

use bitcoin::secp256k1::{self, Scalar};
use serde::{Deserialize, Serialize};
use thiserror::Error;

use super::nut00::{BlindSignature, Proof};
use super::nut01::{PublicKey, SecretKey};
use super::nut02::Id;
use crate::dhke::{hash_e, hash_to_curve};
use crate::{Amount, SECP256K1};

/// NUT12 Error
#[derive(Debug, Error)]
pub enum Error {
    /// Missing DLEQ Proof
    #[error("No DLEQ proof provided")]
    MissingDleqProof,
    /// Incomplete DLEQ Proof
    #[error("Incomplete DLEQ proof")]
    IncompleteDleqProof,
    /// Invalid DLEQ Proof
    #[error("Invalid DLEQ proof")]
    InvalidDleqProof,
    /// DHKE error
    #[error(transparent)]
    DHKE(#[from] crate::dhke::Error),
    /// NUT01 Error
    #[error(transparent)]
    NUT01(#[from] crate::nuts::nut01::Error),
    /// SECP256k1 Error
    #[error(transparent)]
    Secp256k1(#[from] secp256k1::Error),
}

/// Blinded Signature on Dleq
///
/// Defined in [NUT12](https://github.com/cashubtc/nuts/blob/main/12.md)
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "swagger", derive(utoipa::ToSchema))]
pub struct BlindSignatureDleq {
    /// e
    #[cfg_attr(feature = "swagger", schema(value_type = String))]
    pub e: SecretKey,
    /// s
    #[cfg_attr(feature = "swagger", schema(value_type = String))]
    pub s: SecretKey,
}

/// Proof Dleq
///
/// Defined in [NUT12](https://github.com/cashubtc/nuts/blob/main/12.md)
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "swagger", derive(utoipa::ToSchema))]
pub struct ProofDleq {
    /// e
    #[cfg_attr(feature = "swagger", schema(value_type = String))]
    pub e: SecretKey,
    /// s
    #[cfg_attr(feature = "swagger", schema(value_type = String))]
    pub s: SecretKey,
    /// Blinding factor
    #[cfg_attr(feature = "swagger", schema(value_type = String))]
    pub r: SecretKey,
}

impl ProofDleq {
    /// Create new [`ProofDleq`]
    pub fn new(e: SecretKey, s: SecretKey, r: SecretKey) -> Self {
        Self { e, s, r }
    }
}

/// Verify DLEQ
fn verify_dleq(
    blinded_message: PublicKey,   // B'
    blinded_signature: PublicKey, // C'
    e: &SecretKey,
    s: &SecretKey,
    mint_pubkey: PublicKey, // A
) -> Result<(), Error> {
    let e_bytes: [u8; 32] = e.to_secret_bytes();
    let e: Scalar = e.as_scalar();

    // a = e*A
    let a: PublicKey = mint_pubkey.mul_tweak(&SECP256K1, &e)?.into();

    // R1 = s*G - a
    let a: PublicKey = a.negate(&SECP256K1).into();
    let r1: PublicKey = s.public_key().combine(&a)?.into(); // s*G + (-a)

    // b = s*B'
    let s: Scalar = Scalar::from(s.deref().to_owned());
    let b: PublicKey = blinded_message.mul_tweak(&SECP256K1, &s)?.into();

    // c = e*C'
    let c: PublicKey = blinded_signature.mul_tweak(&SECP256K1, &e)?.into();

    // R2 = b - c
    let c: PublicKey = c.negate(&SECP256K1).into();
    let r2: PublicKey = b.combine(&c)?.into();

    // hash(R1,R2,A,C')
    let hash_e: [u8; 32] = hash_e([r1, r2, mint_pubkey, blinded_signature]);

    if e_bytes != hash_e {
        tracing::warn!("DLEQ on signature failed");
        tracing::debug!("e_bytes: {:?}, hash_e: {:?}", e_bytes, hash_e);
        return Err(Error::InvalidDleqProof);
    }

    Ok(())
}

fn calculate_dleq(
    blinded_signature: PublicKey, // C'
    blinded_message: &PublicKey,  // B'
    mint_secret_key: &SecretKey,  // a
) -> Result<BlindSignatureDleq, Error> {
    // Random nonce
    let r: SecretKey = SecretKey::generate();

    // R1 = r*G
    let r1 = r.public_key();

    // R2 = r*B'
    let r_scal: Scalar = r.as_scalar();
    let r2: PublicKey = blinded_message.mul_tweak(&SECP256K1, &r_scal)?.into();

    // e = hash(R1,R2,A,C')
    let e: [u8; 32] = hash_e([r1, r2, mint_secret_key.public_key(), blinded_signature]);
    let e_sk: SecretKey = SecretKey::from_slice(&e)?;

    // s1 = e*a
    let s1: SecretKey = e_sk.mul_tweak(&mint_secret_key.as_scalar())?.into();

    // s = r + s1
    let s: SecretKey = r.add_tweak(&s1.to_scalar())?.into();

    Ok(BlindSignatureDleq { e: e_sk, s })
}

impl Proof {
    /// Verify proof Dleq
    pub fn verify_dleq(&self, mint_pubkey: PublicKey) -> Result<(), Error> {
        match &self.dleq {
            Some(dleq) => {
                let y = hash_to_curve(self.secret.as_bytes())?;

                let r: Scalar = dleq.r.as_scalar();
                let bs1: PublicKey = mint_pubkey.mul_tweak(&SECP256K1, &r)?.into();

                let blinded_signature: PublicKey = self.c.combine(&bs1)?.into();
                let blinded_message: PublicKey = y.combine(&dleq.r.public_key())?.into();

                verify_dleq(
                    blinded_message,
                    blinded_signature,
                    &dleq.e,
                    &dleq.s,
                    mint_pubkey,
                )
            }
            None => Err(Error::MissingDleqProof),
        }
    }
}

impl BlindSignature {
    /// New DLEQ
    #[inline]
    pub fn new(
        amount: Amount,
        blinded_signature: PublicKey,
        keyset_id: Id,
        blinded_message: &PublicKey,
        mint_secretkey: SecretKey,
    ) -> Result<Self, Error> {
        Ok(Self {
            amount,
            keyset_id,
            c: blinded_signature,
            dleq: Some(calculate_dleq(
                blinded_signature,
                blinded_message,
                &mint_secretkey,
            )?),
        })
    }

    /// Verify dleq on proof
    #[inline]
    pub fn verify_dleq(
        &self,
        mint_pubkey: PublicKey,
        blinded_message: PublicKey,
    ) -> Result<(), Error> {
        match &self.dleq {
            Some(dleq) => verify_dleq(blinded_message, self.c, &dleq.e, &dleq.s, mint_pubkey),
            None => Err(Error::MissingDleqProof),
        }
    }

    /// Add Dleq to proof
    /*
    r = random nonce
    R1 = r*G
    R2 = r*B'
    e = hash(R1,R2,A,C')
    s = r + e*a
    */
    pub fn add_dleq_proof(
        &mut self,
        blinded_message: &PublicKey,
        mint_secretkey: &SecretKey,
    ) -> Result<(), Error> {
        let dleq: BlindSignatureDleq = calculate_dleq(self.c, blinded_message, mint_secretkey)?;
        self.dleq = Some(dleq);
        Ok(())
    }
}

#[cfg(test)]
mod tests {

    use std::str::FromStr;

    use super::*;

    #[test]
    fn test_blind_signature_dleq() {
        let blinded_sig = r#"{"amount":8,"id":"00882760bfa2eb41","C_":"02a9acc1e48c25eeeb9289b5031cc57da9fe72f3fe2861d264bdc074209b107ba2","dleq":{"e":"9818e061ee51d5c8edc3342369a554998ff7b4381c8652d724cdf46429be73d9","s":"9818e061ee51d5c8edc3342369a554998ff7b4381c8652d724cdf46429be73da"}}"#;

        let blinded: BlindSignature = serde_json::from_str(blinded_sig).unwrap();

        let secret_key =
            SecretKey::from_hex("0000000000000000000000000000000000000000000000000000000000000001")
                .unwrap();

        let mint_key = secret_key.public_key();

        let blinded_secret = PublicKey::from_str(
            "02a9acc1e48c25eeeb9289b5031cc57da9fe72f3fe2861d264bdc074209b107ba2",
        )
        .unwrap();

        blinded.verify_dleq(mint_key, blinded_secret).unwrap()
    }

    #[test]
    fn test_proof_dleq() {
        let proof = r#"{"amount": 1,"id": "00882760bfa2eb41","secret": "daf4dd00a2b68a0858a80450f52c8a7d2ccf87d375e43e216e0c571f089f63e9","C": "024369d2d22a80ecf78f3937da9d5f30c1b9f74f0c32684d583cca0fa6a61cdcfc","dleq": {"e": "b31e58ac6527f34975ffab13e70a48b6d2b0d35abc4b03f0151f09ee1a9763d4","s": "8fbae004c59e754d71df67e392b6ae4e29293113ddc2ec86592a0431d16306d8","r": "a6d13fcd7a18442e6076f5e1e7c887ad5de40a019824bdfa9fe740d302e8d861"}}"#;

        let proof: Proof = serde_json::from_str(proof).unwrap();

        // A
        let a: PublicKey = PublicKey::from_str(
            "0279be667ef9dcbbac55a06295ce870b07029bfcdb2dce28d959f2815b16f81798",
        )
        .unwrap();

        assert!(proof.verify_dleq(a).is_ok());
    }

    /// Tests that verify_dleq correctly rejects verification with a wrong mint key.
    ///
    /// This test is critical for security - if the verification function doesn't properly
    /// check the mint key, an attacker could forge proofs using any key.
    ///
    /// Mutant testing: Catches mutations that replace verify_dleq with Ok(()) or remove
    /// the verification logic.
    #[test]
    fn test_proof_dleq_wrong_mint_key() {
        let proof = r#"{"amount": 1,"id": "00882760bfa2eb41","secret": "daf4dd00a2b68a0858a80450f52c8a7d2ccf87d375e43e216e0c571f089f63e9","C": "024369d2d22a80ecf78f3937da9d5f30c1b9f74f0c32684d583cca0fa6a61cdcfc","dleq": {"e": "b31e58ac6527f34975ffab13e70a48b6d2b0d35abc4b03f0151f09ee1a9763d4","s": "8fbae004c59e754d71df67e392b6ae4e29293113ddc2ec86592a0431d16306d8","r": "a6d13fcd7a18442e6076f5e1e7c887ad5de40a019824bdfa9fe740d302e8d861"}}"#;

        let proof: Proof = serde_json::from_str(proof).unwrap();

        // Wrong mint key - different from the one used to create the proof
        let wrong_key: PublicKey = PublicKey::from_str(
            "02c6047f9441ed7d6d3045406e95c07cd85c778e4b8cef3ca7abac09b95c709ee5",
        )
        .unwrap();

        // Verification should fail with wrong key
        assert!(proof.verify_dleq(wrong_key).is_err());
    }

    /// Tests that verify_dleq correctly rejects proofs with missing DLEQ data.
    ///
    /// This test ensures that proofs without DLEQ data are rejected when DLEQ
    /// verification is required.
    ///
    /// Mutant testing: Catches mutations that replace verify_dleq with Ok(()) or
    /// remove the None check.
    #[test]
    fn test_proof_dleq_missing() {
        let proof = r#"{"amount": 1,"id": "00882760bfa2eb41","secret": "daf4dd00a2b68a0858a80450f52c8a7d2ccf87d375e43e216e0c571f089f63e9","C": "024369d2d22a80ecf78f3937da9d5f30c1b9f74f0c32684d583cca0fa6a61cdcfc"}"#;

        let proof: Proof = serde_json::from_str(proof).unwrap();

        let a: PublicKey = PublicKey::from_str(
            "0279be667ef9dcbbac55a06295ce870b07029bfcdb2dce28d959f2815b16f81798",
        )
        .unwrap();

        // Verification should fail when DLEQ is missing
        let result = proof.verify_dleq(a);
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), Error::MissingDleqProof));
    }

    /// Tests that BlindSignature::verify_dleq correctly rejects verification with wrong mint key.
    ///
    /// This test ensures that blind signature DLEQ verification properly validates the mint key.
    ///
    /// Mutant testing: Catches mutations that replace BlindSignature::verify_dleq with Ok(())
    /// or remove the verification logic.
    #[test]
    fn test_blind_signature_dleq_wrong_key() {
        let blinded_sig = r#"{"amount":8,"id":"00882760bfa2eb41","C_":"02a9acc1e48c25eeeb9289b5031cc57da9fe72f3fe2861d264bdc074209b107ba2","dleq":{"e":"9818e061ee51d5c8edc3342369a554998ff7b4381c8652d724cdf46429be73d9","s":"9818e061ee51d5c8edc3342369a554998ff7b4381c8652d724cdf46429be73da"}}"#;

        let blinded: BlindSignature = serde_json::from_str(blinded_sig).unwrap();

        // Wrong secret key - different from the one used to create the signature
        let wrong_key =
            SecretKey::from_hex("0000000000000000000000000000000000000000000000000000000000000002")
                .unwrap();

        let blinded_secret = PublicKey::from_str(
            "02a9acc1e48c25eeeb9289b5031cc57da9fe72f3fe2861d264bdc074209b107ba2",
        )
        .unwrap();

        // Verification should fail with wrong key
        assert!(blinded
            .verify_dleq(wrong_key.public_key(), blinded_secret)
            .is_err());
    }

    /// Tests that BlindSignature::verify_dleq correctly rejects verification with tampered DLEQ data.
    ///
    /// This test ensures that tampering with the 'e' or 's' values in the DLEQ proof
    /// causes verification to fail.
    ///
    /// Mutant testing: Catches mutations that replace verify_dleq with Ok(()) or
    /// weaken the cryptographic checks.
    #[test]
    fn test_blind_signature_dleq_tampered() {
        // Tampered DLEQ data - 'e' and 's' values have been modified to wrong (but valid) values
        let tampered_sig = r#"{"amount":8,"id":"00882760bfa2eb41","C_":"02a9acc1e48c25eeeb9289b5031cc57da9fe72f3fe2861d264bdc074209b107ba2","dleq":{"e":"0000000000000000000000000000000000000000000000000000000000000001","s":"0000000000000000000000000000000000000000000000000000000000000002"}}"#;

        let blinded: BlindSignature = serde_json::from_str(tampered_sig).unwrap();

        let secret_key =
            SecretKey::from_hex("0000000000000000000000000000000000000000000000000000000000000001")
                .unwrap();

        let blinded_secret = PublicKey::from_str(
            "02a9acc1e48c25eeeb9289b5031cc57da9fe72f3fe2861d264bdc074209b107ba2",
        )
        .unwrap();

        // Verification should fail with tampered data
        assert!(blinded
            .verify_dleq(secret_key.public_key(), blinded_secret)
            .is_err());
    }

    /// Tests that BlindSignature::add_dleq_proof properly generates DLEQ data.
    ///
    /// This test ensures that add_dleq_proof actually adds the DLEQ proof and doesn't
    /// just return Ok(()) without doing anything.
    ///
    /// Mutant testing: Catches mutations that replace add_dleq_proof with Ok(())
    /// without actually adding the proof.
    #[test]
    fn test_add_dleq_proof() {
        use crate::nuts::nut02::Id;

        let secret_key =
            SecretKey::from_hex("0000000000000000000000000000000000000000000000000000000000000001")
                .unwrap();

        let blinded_message = PublicKey::from_str(
            "02a9acc1e48c25eeeb9289b5031cc57da9fe72f3fe2861d264bdc074209b107ba2",
        )
        .unwrap();

        let blinded_signature = PublicKey::from_str(
            "02a9acc1e48c25eeeb9289b5031cc57da9fe72f3fe2861d264bdc074209b107ba2",
        )
        .unwrap();

        let mut blind_sig = BlindSignature {
            amount: Amount::from(1),
            keyset_id: Id::from_str("00882760bfa2eb41").unwrap(),
            c: blinded_signature,
            dleq: None,
        };

        // Initially, DLEQ should be None
        assert!(blind_sig.dleq.is_none());

        // Add DLEQ proof
        blind_sig
            .add_dleq_proof(&blinded_message, &secret_key)
            .unwrap();

        // After adding, DLEQ should be Some
        assert!(blind_sig.dleq.is_some());

        // Verify the added DLEQ is valid
        assert!(blind_sig
            .verify_dleq(secret_key.public_key(), blinded_message)
            .is_ok());
    }
}
