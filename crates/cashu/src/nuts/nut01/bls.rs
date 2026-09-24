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

/// Verify `e(signature, G2) == e(blinded_message, mint_pubkey)`.
pub(crate) fn verify_blind_signature_pairing(
    signature: &BlsG1PublicKey,
    blinded_message: &BlsG1PublicKey,
    mint_pubkey: &BlsG2PublicKey,
) -> bool {
    pairing(&signature.point(), &G2Affine::generator())
        == pairing(&blinded_message.point(), &mint_pubkey.point())
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
            // Per NUT-00, derive each weight by rejection sampling in `Fr*`:
            //   h = SHA256(challenge || u32_BE(i) || u32_BE(ctr))
            //   x = OS2IP(h); reject if x == 0 or x >= BLS_FR_ORDER.
            // `BlsSecretKey::from_bytes` interprets `h` big-endian and only succeeds
            // when x < BLS_FR_ORDER (canonical), so it rejects the out-of-range case;
            // we additionally reject the zero scalar. Plain modular reduction would
            // bias the distribution and diverge from the spec's deterministic weights.
            let mut counter = 0u32;
            loop {
                let mut weight_material = Vec::with_capacity(40);
                weight_material.extend_from_slice(&challenge);
                weight_material.extend_from_slice(&(i as u32).to_be_bytes());
                weight_material.extend_from_slice(&counter.to_be_bytes());
                let weight = Sha256Hash::hash(&weight_material).to_byte_array();
                if let Ok(scalar) = BlsSecretKey::from_bytes(&weight) {
                    if scalar.scalar() != Scalar::zero() {
                        return scalar;
                    }
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

    fn hex_to_g1(hex: &str) -> BlsG1PublicKey {
        BlsG1PublicKey::from_bytes(&crate::util::hex::decode(hex).expect("hex")).expect("g1")
    }

    fn hex_to_g2(hex: &str) -> BlsG2PublicKey {
        BlsG2PublicKey::from_bytes(&crate::util::hex::decode(hex).expect("hex")).expect("g2")
    }

    /// NUT-00 batch verification test vector: two proofs under the same mint key
    /// `K = 2·G2`. Exercises both rejection-sampling code paths: `weight_1` is
    /// accepted at `ctr = 4` and `weight_2` at `ctr = 0`.
    #[test]
    fn test_batch_weight_derivation_nut00_vector() {
        let k = hex_to_g2(
            "aa4edef9c1ed7f729f520e47730a124fd70662a904ba1074728114d1031e1572c6c886f6b57ec72a6178288c47c335771638533957d540a9d2370f17cc7ed5863bc0b995b8825e0ee1ea1e1e4d00dbae81f14b0bf3611b78c952aacab827a053",
        );
        let c1 = hex_to_g1(
            "acebf797506a7031cef3189904715cb22792528f1ea0e6ab25341401d245539438ed97122f00e38ee6185cc20b09ba11",
        );
        let c2 = hex_to_g1(
            "9776497ad47a00f8a56233fb88f939b0572cf174a4c6d2446c0b1060434e305fae6845fd1f68b70376ba53ffe67f0414",
        );
        let mint_pubkeys = [k, k];
        let signatures = [c1, c2];
        let messages: [&[u8]; 2] = [b"batch_proof_1", b"batch_proof_2"];

        let weights = derive_batch_weights(&mint_pubkeys, &signatures, &messages);

        assert_eq!(
            crate::util::hex::encode(weights[0].to_bytes()),
            "0e7ff8be2ccb756d4ef390991bdd77eb65e8db624a2729fa1657c3cf8d7d4b55"
        );
        assert_eq!(
            crate::util::hex::encode(weights[1].to_bytes()),
            "6d026a181a6215b233e73b121d01908a1a1eb6911955bea5130bbf2f2966554d"
        );
        assert!(batch_verify_pairing(&mint_pubkeys, &signatures, &messages));
    }

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
