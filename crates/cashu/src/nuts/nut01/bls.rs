use core::fmt;
use std::collections::BTreeMap;

use bitcoin::hashes::sha256::Hash as Sha256Hash;
use bitcoin::hashes::Hash;
use bls12_381::hash_to_curve::{ExpandMsgXmd, HashToCurve};
use bls12_381::{pairing, G1Affine, G1Projective, G2Affine, G2Projective, Gt, Scalar};
use group::Curve;
use sha2_09::Sha256;

use super::Error;

const BLS_DST: &[u8] = b"CASHU_BLS12_381_G1_XMD:SHA-256_SSWU_RO_";
const BLS_BATCH_DST: &[u8] = b"Cashu_BLS_Batch_v1";

/// BLS12-381 scalar/private key.
#[derive(Clone, PartialEq, Eq)]
pub struct BlsSecretKey {
    scalar: Scalar,
}

impl fmt::Debug for BlsSecretKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("BlsSecretKey(..)")
    }
}

impl BlsSecretKey {
    /// Derive a scalar by reducing 32-byte input modulo the BLS12-381 scalar field.
    pub fn from_reduced_bytes(bytes: &[u8; 32]) -> Self {
        let mut wide = [0u8; 64];
        for (dst, src) in wide.iter_mut().zip(bytes.iter().rev()) {
            *dst = *src;
        }
        Self {
            scalar: Scalar::from_bytes_wide(&wide),
        }
    }

    /// Parse a canonical scalar from 32 big-endian bytes.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, Error> {
        let mut bytes: [u8; 32] = bytes.try_into().map_err(|_| Error::InvalidSecretKeySize {
            expected: 32,
            found: bytes.len(),
        })?;
        bytes.reverse();
        let scalar =
            Option::<Scalar>::from(Scalar::from_bytes(&bytes)).ok_or(Error::InvalidSecretKey)?;
        Ok(Self { scalar })
    }

    /// Return canonical scalar bytes in big-endian order.
    pub fn to_bytes(&self) -> [u8; 32] {
        let mut bytes = self.scalar.to_bytes();
        bytes.reverse();
        bytes
    }

    /// Return the scalar.
    pub fn scalar(&self) -> Scalar {
        self.scalar
    }

    /// Return the multiplicative inverse.
    pub fn invert(&self) -> Result<Self, Error> {
        let scalar = Option::<Scalar>::from(self.scalar.invert()).ok_or(Error::InvalidSecretKey)?;
        Ok(Self { scalar })
    }

    /// Derive the mint public key in G2.
    pub fn public_key_g2(&self) -> BlsG2PublicKey {
        BlsG2PublicKey {
            point: (G2Projective::generator() * self.scalar).to_affine(),
        }
    }
}

impl Drop for BlsSecretKey {
    fn drop(&mut self) {
        self.scalar = Scalar::zero();
    }
}

/// BLS12-381 G1 public key, used for blinded messages and signatures.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BlsG1PublicKey {
    point: G1Affine,
}

impl BlsG1PublicKey {
    /// Hash arbitrary bytes to G1 with the Cashu BLS DST.
    pub fn hash_to_curve(message: &[u8]) -> Self {
        Self {
            point: <G1Projective as HashToCurve<ExpandMsgXmd<Sha256>>>::hash_to_curve(
                message, BLS_DST,
            )
            .to_affine(),
        }
    }

    /// Parse compressed G1 bytes.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, Error> {
        let bytes: [u8; 48] = bytes.try_into().map_err(|_| Error::InvalidPublicKeySize {
            expected: 48,
            found: bytes.len(),
        })?;
        let point = Option::<G1Affine>::from(G1Affine::from_compressed(&bytes))
            .ok_or(Error::InvalidPublicKey)?;
        if bool::from(point.is_identity()) {
            return Err(Error::InvalidPublicKey);
        }
        Ok(Self { point })
    }

    /// Return compressed G1 bytes.
    pub fn to_bytes(&self) -> [u8; 48] {
        self.point.to_compressed()
    }

    /// Multiply this G1 point by a scalar.
    pub fn mul(&self, scalar: &BlsSecretKey) -> Self {
        Self {
            point: (G1Projective::from(self.point) * scalar.scalar()).to_affine(),
        }
    }

    /// Return the affine point.
    pub fn point(&self) -> G1Affine {
        self.point
    }
}

/// BLS12-381 G2 public key, used for mint public keys.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BlsG2PublicKey {
    point: G2Affine,
}

impl BlsG2PublicKey {
    /// Parse compressed G2 bytes.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, Error> {
        let bytes: [u8; 96] = bytes.try_into().map_err(|_| Error::InvalidPublicKeySize {
            expected: 96,
            found: bytes.len(),
        })?;
        let point = Option::<G2Affine>::from(G2Affine::from_compressed(&bytes))
            .ok_or(Error::InvalidPublicKey)?;
        if bool::from(point.is_identity()) {
            return Err(Error::InvalidPublicKey);
        }
        Ok(Self { point })
    }

    /// Return compressed G2 bytes.
    pub fn to_bytes(&self) -> [u8; 96] {
        self.point.to_compressed()
    }

    /// Return the affine point.
    pub fn point(&self) -> G2Affine {
        self.point
    }
}

/// Verify `e(signature, G2) == e(hash_to_curve(secret), mint_pubkey)`.
pub(crate) fn verify_pairing(
    signature: &BlsG1PublicKey,
    secret: &[u8],
    mint_pubkey: &BlsG2PublicKey,
) -> bool {
    let y = BlsG1PublicKey::hash_to_curve(secret);
    pairing(&signature.point(), &G2Affine::generator()) == pairing(&y.point(), &mint_pubkey.point())
}

fn derive_batch_weights(
    mint_pubkeys: &[BlsG2PublicKey],
    signatures: &[BlsG1PublicKey],
    messages: &[&[u8]],
) -> Vec<BlsSecretKey> {
    let mut transcript = Vec::new();
    transcript.extend_from_slice(BLS_BATCH_DST);
    for ((mint_pubkey, signature), message) in mint_pubkeys.iter().zip(signatures).zip(messages) {
        transcript.extend_from_slice(&signature.to_bytes());
        transcript.extend_from_slice(&mint_pubkey.to_bytes());
        transcript.extend_from_slice(&(message.len() as u32).to_be_bytes());
        transcript.extend_from_slice(message);
    }

    let challenge = Sha256Hash::hash(&transcript).to_byte_array();
    (0..mint_pubkeys.len())
        .map(|i| {
            let mut counter = 0u8;
            loop {
                let mut weight_material = Vec::with_capacity(37);
                weight_material.extend_from_slice(&challenge);
                weight_material.extend_from_slice(&(i as u32).to_be_bytes());
                weight_material.push(counter);
                let weight = Sha256Hash::hash(&weight_material).to_byte_array();
                let scalar = BlsSecretKey::from_reduced_bytes(&weight);
                if scalar.scalar() != Scalar::zero() {
                    return scalar;
                }
                counter = counter
                    .checked_add(1)
                    .expect("BLS batch weight derivation failed");
            }
        })
        .collect()
}

/// Batch verify BLS proofs using deterministic transcript-derived weights.
pub(crate) fn batch_verify_pairing(
    mint_pubkeys: &[BlsG2PublicKey],
    signatures: &[BlsG1PublicKey],
    messages: &[&[u8]],
) -> bool {
    if mint_pubkeys.len() != signatures.len() || signatures.len() != messages.len() {
        return false;
    }
    if signatures.is_empty() {
        return true;
    }

    let weights = derive_batch_weights(mint_pubkeys, signatures, messages);
    let mut weighted_signatures = G1Projective::identity();
    let mut weighted_messages = BTreeMap::<[u8; 96], (BlsG2PublicKey, G1Projective)>::new();

    for (((mint_pubkey, signature), message), weight) in mint_pubkeys
        .iter()
        .zip(signatures)
        .zip(messages)
        .zip(&weights)
    {
        weighted_signatures += G1Projective::from(signature.point()) * weight.scalar();
        let weighted_message =
            G1Projective::from(BlsG1PublicKey::hash_to_curve(message).point()) * weight.scalar();
        weighted_messages
            .entry(mint_pubkey.to_bytes())
            .and_modify(|(_, sum)| *sum += weighted_message)
            .or_insert((*mint_pubkey, weighted_message));
    }

    let left = pairing(&weighted_signatures.to_affine(), &G2Affine::generator());
    let right = weighted_messages.into_values().fold(
        Gt::identity(),
        |acc, (mint_pubkey, weighted_message)| {
            &acc + &pairing(&weighted_message.to_affine(), &mint_pubkey.point())
        },
    );

    left == right
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_reject_g1_identity_point() {
        let identity = G1Projective::identity().to_affine().to_compressed();
        assert!(BlsG1PublicKey::from_bytes(&identity).is_err());
    }

    #[test]
    fn test_reject_g2_identity_point() {
        let identity = G2Projective::identity().to_affine().to_compressed();
        assert!(BlsG2PublicKey::from_bytes(&identity).is_err());
    }
}
