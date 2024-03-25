//! NUT-12: Offline ecash signature validation
//! https://github.com/cashubtc/nuts/blob/main/12.md
use std::ops::Mul;

use k256::Scalar;
use log::{debug, warn};
use serde::{Deserialize, Serialize};
use thiserror::Error;

use super::{BlindedSignature, Id, Proof, PublicKey, SecretKey};
use crate::dhke::{hash_e, hash_to_curve};
use crate::Amount;

#[derive(Debug, Error)]
pub enum Error {
    #[error("No Dleq Proof provided")]
    MissingDleqProof,
    #[error("Incomplete DLEQ Proof")]
    IncompleteDleqProof,
    #[error("Invalid Dleq Prood")]
    InvalidDleqProof,
    #[error("`{0}`")]
    EllipticCurve(#[from] k256::elliptic_curve::Error),
    #[error("`{0}`")]
    Cashu(#[from] crate::error::Error),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DleqProof {
    pub e: SecretKey,
    pub s: SecretKey,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub r: Option<SecretKey>,
}

fn verify_dleq(
    blinded_message: k256::PublicKey,
    blinded_signature: k256::PublicKey,
    e: k256::SecretKey,
    s: k256::SecretKey,
    mint_pubkey: k256::PublicKey,
) -> Result<(), Error> {
    let r1 = s.public_key().to_projective()
        - mint_pubkey
            .as_affine()
            .mul(Scalar::from(e.as_scalar_primitive()));

    let r2 = blinded_message
        .as_affine()
        .mul(Scalar::from(s.as_scalar_primitive()))
        - blinded_signature
            .as_affine()
            .mul(Scalar::from(e.as_scalar_primitive()));

    let e_bytes = e.to_bytes().to_vec();

    let hash_e = hash_e(vec![
        k256::PublicKey::try_from(r1)?,
        k256::PublicKey::try_from(r2)?,
        mint_pubkey,
        blinded_signature,
    ]);

    if e_bytes.ne(&hash_e) {
        warn!("DLEQ on signature failed");
        debug!("e_bytes: {:?}, Hash e: {:?}", e_bytes, hash_e);
        return Err(Error::InvalidDleqProof);
    }

    Ok(())
}

fn calculate_dleq(
    blinded_signature: k256::PublicKey,
    blinded_message: &k256::PublicKey,
    mint_secretkey: &k256::SecretKey,
) -> Result<DleqProof, Error> {
    // Random nonce
    let r: k256::SecretKey = SecretKey::random().into();

    let r1 = r.public_key();

    let r2: k256::PublicKey = blinded_message
        .as_affine()
        .mul(Scalar::from(r.as_scalar_primitive()))
        .try_into()?;

    let e = hash_e(vec![r1, r2, mint_secretkey.public_key(), blinded_signature]);

    let e_sk = k256::SecretKey::from_slice(&e)?;

    let s = Scalar::from(r.as_scalar_primitive())
        + Scalar::from(e_sk.as_scalar_primitive())
            * Scalar::from(mint_secretkey.as_scalar_primitive());

    let s: k256::SecretKey = k256::SecretKey::new(s.into());

    Ok(DleqProof {
        e: e_sk.into(),
        s: s.into(),
        r: None,
    })
}

impl Proof {
    pub fn verify_dleq(&self, mint_pubkey: &PublicKey) -> Result<(), Error> {
        let (e, s, blinding_factor): (k256::SecretKey, k256::SecretKey, k256::SecretKey) =
            if let Some(dleq) = self.dleq.clone() {
                if let Some(r) = dleq.r {
                    (dleq.e.into(), dleq.s.into(), r.into())
                } else {
                    return Err(Error::IncompleteDleqProof);
                }
            } else {
                return Err(Error::MissingDleqProof);
            };

        let c: k256::PublicKey = (&self.c).into();
        let mint_pubkey: k256::PublicKey = mint_pubkey.into();

        let y = hash_to_curve(self.secret.0.as_bytes())?;
        let blinded_signature = c.to_projective()
            + mint_pubkey
                .as_affine()
                .mul(Scalar::from(blinding_factor.as_scalar_primitive()));
        let blinded_message = y.to_projective() + blinding_factor.public_key().to_projective();

        let blinded_signature = k256::PublicKey::try_from(blinded_signature)?;
        let blinded_message = k256::PublicKey::try_from(blinded_message)?;

        verify_dleq(blinded_message, blinded_signature, e, s, mint_pubkey)
    }
}

impl BlindedSignature {
    pub fn new_dleq(
        amount: Amount,
        blinded_signature: PublicKey,
        keyset_id: Id,
        blinded_message: &PublicKey,
        mint_secretkey: SecretKey,
    ) -> Result<Self, Error> {
        let blinded_message: k256::PublicKey = blinded_message.into();
        let mint_secretkey: k256::SecretKey = mint_secretkey.into();

        let dleq = calculate_dleq(
            blinded_signature.clone().into(),
            &blinded_message,
            &mint_secretkey,
        )?;

        Ok(BlindedSignature {
            amount,
            keyset_id,
            c: blinded_signature,
            dleq: Some(dleq),
        })
    }

    pub fn verify_dleq(
        &self,
        mint_pubkey: &PublicKey,
        blinded_message: &PublicKey,
    ) -> Result<(), Error> {
        let (e, s): (k256::SecretKey, k256::SecretKey) = if let Some(dleq) = &self.dleq {
            (dleq.e.clone().into(), dleq.s.clone().into())
        } else {
            return Err(Error::MissingDleqProof);
        };

        let mint_pubkey: k256::PublicKey = mint_pubkey.into();
        let blinded_message: k256::PublicKey = blinded_message.into();

        let c: k256::PublicKey = (&self.c).into();
        verify_dleq(blinded_message, c, e, s, mint_pubkey)
    }

    /*
    r = random nonce
    R1 = r*G
    R2 = r*B'
    e = hash(R1,R2,A,C')
    s = r + e*a
    */
    #[cfg(feature = "mint")]
    pub fn add_dleq_proof(
        &mut self,
        blinded_message: &PublicKey,
        mint_secretkey: &SecretKey,
    ) -> Result<(), Error> {
        let blinded_message: k256::PublicKey = blinded_message.into();
        let mint_secretkey: k256::SecretKey = mint_secretkey.clone().into();

        let dleq = calculate_dleq(self.c.clone().into(), &blinded_message, &mint_secretkey)?;
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

        let blinded: BlindedSignature = serde_json::from_str(blinded_sig).unwrap();

        let secret_key =
            SecretKey::from_hex("0000000000000000000000000000000000000000000000000000000000000001")
                .unwrap();

        let mint_key = secret_key.public_key();

        let blinded_secret = PublicKey::from_str(
            "02a9acc1e48c25eeeb9289b5031cc57da9fe72f3fe2861d264bdc074209b107ba2",
        )
        .unwrap();

        blinded.verify_dleq(&mint_key, &blinded_secret).unwrap()
    }
}
