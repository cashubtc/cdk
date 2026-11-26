use core::fmt;
use std::collections::BTreeMap;
use std::sync::LazyLock;

use bitcoin::hashes::sha256::Hash as Sha256Hash;
use bitcoin::hashes::Hash;
use bitcoin::secp256k1::rand::rngs::OsRng;
use bitcoin::secp256k1::rand::RngCore;
use bls12_381::hash_to_curve::{ExpandMsgXmd, HashToCurve};
use bls12_381::{
    multi_miller_loop, G1Affine, G1Projective, G2Affine, G2Prepared, G2Projective, Gt, Scalar,
};
use group::Curve;
use sha2_09::Sha256;
use zeroize::Zeroize;

use super::Error;

const BLS_DST: &[u8] = b"CASHU_BLS12_381_G1_XMD:SHA-256_SSWU_RO_";
const BLS_BATCH_DST: &[u8] = b"Cashu_BLS_Batch_v1";

/// Decode the hex of a BLS12-381 point, which NUT-00 requires in lowercase.
pub(super) fn decode_point_hex(hex: &str) -> Result<Vec<u8>, Error> {
    if hex.bytes().any(|byte| byte.is_ascii_uppercase()) {
        return Err(Error::NonLowercaseHex);
    }
    Ok(crate::util::hex::decode(hex)?)
}

/// The G2 generator in Miller-loop form, prepared once and shared by every check.
static G2_GENERATOR_PREPARED: LazyLock<G2Prepared> =
    LazyLock::new(|| G2Prepared::from(G2Affine::generator()));

/// Non-zero BLS12-381 scalar/private key.
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
    /// Generate a random non-zero BLS12-381 scalar.
    pub fn generate() -> Result<Self, Error> {
        loop {
            let mut bytes = [0u8; 64];
            OsRng
                .try_fill_bytes(&mut bytes)
                .map_err(|_| Error::RandomnessUnavailable)?;
            let scalar = Scalar::from_bytes_wide(&bytes);
            if scalar != Scalar::zero() {
                return Ok(Self { scalar });
            }
        }
    }

    /// Derive a non-zero scalar by reducing 32-byte input modulo the BLS12-381 scalar field.
    /// Returns an error if the reduced scalar is zero.
    ///
    /// Test-only: reduction is biased; derived scalars use rejection sampling.
    #[cfg(test)]
    pub(crate) fn from_reduced_bytes(bytes: &[u8; 32]) -> Result<Self, Error> {
        let mut wide = [0u8; 64];
        for (dst, src) in wide.iter_mut().zip(bytes.iter().rev()) {
            *dst = *src;
        }
        Self::from_scalar(Scalar::from_bytes_wide(&wide))
    }

    /// Parse a canonical non-zero scalar from 32 big-endian bytes.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, Error> {
        let mut bytes: [u8; 32] = bytes.try_into().map_err(|_| Error::InvalidSecretKeySize {
            expected: 32,
            found: bytes.len(),
        })?;
        bytes.reverse();
        let scalar =
            Option::<Scalar>::from(Scalar::from_bytes(&bytes)).ok_or(Error::InvalidSecretKey)?;
        Self::from_scalar(scalar)
    }

    fn from_scalar(scalar: Scalar) -> Result<Self, Error> {
        if scalar == Scalar::zero() {
            return Err(Error::InvalidSecretKey);
        }
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
        self.scalar.zeroize();
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
    if bool::from(signature.point.is_identity()) || bool::from(mint_pubkey.point.is_identity()) {
        return false;
    }
    let y = BlsG1PublicKey::hash_to_curve(secret);
    pairing_equation_holds(&signature.point(), &y.point(), &mint_pubkey.point())
}

/// Verify `e(signature, G2) == e(blinded_message, mint_pubkey)`.
pub(crate) fn verify_blind_signature_pairing(
    signature: &BlsG1PublicKey,
    blinded_message: &BlsG1PublicKey,
    mint_pubkey: &BlsG2PublicKey,
) -> bool {
    if bool::from(signature.point.is_identity())
        || bool::from(blinded_message.point.is_identity())
        || bool::from(mint_pubkey.point.is_identity())
    {
        return false;
    }
    pairing_equation_holds(
        &signature.point(),
        &blinded_message.point(),
        &mint_pubkey.point(),
    )
}

/// Check `e(signature, G2) == e(message, mint_pubkey)`.
///
/// Moves the signature to the other side by negating it, so both terms share
/// one Miller loop and one final exponentiation instead of two full pairings:
/// `e(-signature, G2) * e(message, mint_pubkey) == 1`.
fn pairing_equation_holds(
    signature: &G1Affine,
    message: &G1Affine,
    mint_pubkey: &G2Affine,
) -> bool {
    let neg_signature = -signature;
    let mint_pubkey = G2Prepared::from(*mint_pubkey);
    multi_miller_loop(&[
        (&neg_signature, &G2_GENERATOR_PREPARED),
        (message, &mint_pubkey),
    ])
    .final_exponentiation()
        == Gt::identity()
}

fn derive_batch_challenge(
    mint_pubkeys: &[BlsG2PublicKey],
    signatures: &[BlsG1PublicKey],
    messages: &[&[u8]],
) -> [u8; 32] {
    let mut transcript = Vec::new();
    transcript.extend_from_slice(BLS_BATCH_DST);
    for ((mint_pubkey, signature), message) in mint_pubkeys.iter().zip(signatures).zip(messages) {
        transcript.extend_from_slice(&signature.to_bytes());
        transcript.extend_from_slice(&mint_pubkey.to_bytes());
        transcript.extend_from_slice(&(message.len() as u32).to_be_bytes());
        transcript.extend_from_slice(message);
    }

    Sha256Hash::hash(&transcript).to_byte_array()
}

fn derive_batch_weights(
    mint_pubkeys: &[BlsG2PublicKey],
    signatures: &[BlsG1PublicKey],
    messages: &[&[u8]],
) -> Option<Vec<BlsSecretKey>> {
    let challenge = derive_batch_challenge(mint_pubkeys, signatures, messages);
    (0..mint_pubkeys.len())
        .map(|i| {
            // Per NUT-00, derive each weight by rejection sampling in `Fr*`:
            //   h = SHA256(challenge || u32_BE(i) || u32_BE(ctr))
            //   x = OS2IP(h); reject if x == 0 or x >= BLS_FR_ORDER.
            // `BlsSecretKey::from_bytes` interprets `h` big-endian and only succeeds
            // when 0 < x < BLS_FR_ORDER. Plain modular reduction would
            // bias the distribution and diverge from the spec's deterministic weights.
            let mut counter = 0u32;
            loop {
                let mut weight_material = Vec::with_capacity(40);
                weight_material.extend_from_slice(&challenge);
                weight_material.extend_from_slice(&(i as u32).to_be_bytes());
                weight_material.extend_from_slice(&counter.to_be_bytes());
                let weight = Sha256Hash::hash(&weight_material).to_byte_array();
                if let Ok(scalar) = BlsSecretKey::from_bytes(&weight) {
                    return Some(scalar);
                }
                counter = counter.checked_add(1)?;
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
    if signatures
        .iter()
        .any(|signature| bool::from(signature.point.is_identity()))
        || mint_pubkeys
            .iter()
            .any(|key| bool::from(key.point.is_identity()))
    {
        return false;
    }

    let Some(weights) = derive_batch_weights(mint_pubkeys, signatures, messages) else {
        return false;
    };
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

    // Move the signature pairing to the other side by negating its G1 point:
    // e(-sum(w_i * C_i), G2) * product_k e(sum(w_i * Y_i), K_k) == 1.
    // Evaluate all terms together with just one final exponentiation.
    let mut terms = Vec::with_capacity(weighted_messages.len() + 1);
    terms.push((
        (-weighted_signatures).to_affine(),
        G2_GENERATOR_PREPARED.clone(),
    ));
    terms.extend(
        weighted_messages
            .into_values()
            .map(|(key, message)| (message.to_affine(), G2Prepared::from(key.point()))),
    );
    let term_refs: Vec<_> = terms.iter().map(|(message, key)| (message, key)).collect();

    multi_miller_loop(&term_refs).final_exponentiation() == Gt::identity()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_bls_secret_key_rejects_zero() {
        for hex in [
            "0000000000000000000000000000000000000000000000000000000000000000",
            "73eda753299d7d483339d80809a1d80553bda402fffe5bfeffffffff00000001",
            "e7db4ea6533afa906673b0101343b00aa77b4805fffcb7fdfffffffe00000002",
        ] {
            let bytes: [u8; 32] = crate::util::hex::decode(hex).unwrap().try_into().unwrap();
            assert!(matches!(
                BlsSecretKey::from_bytes(&bytes),
                Err(Error::InvalidSecretKey)
            ));
            assert!(matches!(
                BlsSecretKey::from_reduced_bytes(&bytes),
                Err(Error::InvalidSecretKey)
            ));
            assert!(crate::nuts::BlindingFactor::bls_from_reduced_bytes(&bytes).is_err());
        }
        let order_plus_one: [u8; 32] = crate::util::hex::decode(
            "73eda753299d7d483339d80809a1d80553bda402fffe5bfeffffffff00000002",
        )
        .unwrap()
        .try_into()
        .unwrap();
        let mut one = [0; 32];
        one[31] = 1;
        assert_eq!(
            BlsSecretKey::from_reduced_bytes(&order_plus_one).unwrap(),
            BlsSecretKey::from_bytes(&one).unwrap(),
        );
    }

    #[test]
    fn test_bls_verification_rejects_identity_operands() {
        use crate::dhke::{
            batch_verify_bls_messages, verify_bls_blind_signature, verify_bls_message,
        };

        let identity_g1 = BlsG1PublicKey {
            point: G1Affine::identity(),
        };
        let identity_g2 = BlsG2PublicKey {
            point: G2Affine::identity(),
        };
        let key = BlsSecretKey::from_bytes(&[1; 32]).unwrap();
        let pubkey = key.public_key_g2();
        let message = b"verification operands";
        let y = BlsG1PublicKey::hash_to_curve(message);
        let signature = y.mul(&key);

        verify_bls_message(pubkey.into(), signature.into(), message).unwrap();
        verify_bls_blind_signature(pubkey.into(), signature.into(), y.into()).unwrap();
        for (candidate_key, candidate_signature) in [
            (identity_g2, identity_g1),
            (pubkey, identity_g1),
            (identity_g2, signature),
        ] {
            assert!(
                verify_bls_message(candidate_key.into(), candidate_signature.into(), message)
                    .is_err()
            );
            assert!(batch_verify_bls_messages(
                &[pubkey.into(), candidate_key.into()],
                &[signature.into(), candidate_signature.into()],
                &[message, message],
            )
            .is_err());
        }
        for (key, signature, blinded) in [
            (identity_g2, identity_g1, y),
            (pubkey, identity_g1, identity_g1),
            (identity_g2, identity_g1, identity_g1),
        ] {
            assert!(
                verify_bls_blind_signature(key.into(), signature.into(), blinded.into()).is_err()
            );
        }
    }

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

        assert_eq!(
            crate::util::hex::encode(derive_batch_challenge(
                &mint_pubkeys,
                &signatures,
                &messages
            )),
            "539b5df396e82adab0760459590d38122d2552bc74f6bd860e915ff3b95e550a"
        );
        let weights = derive_batch_weights(&mint_pubkeys, &signatures, &messages)
            .expect("batch weights should be derivable");

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

    /// NUT-00 batch verification over two v3 point secrets. Each secret
    /// enters the challenge as its 33 decoded bytes, taken here through the
    /// same `hash_input` the proof verifier uses. `weight_1` accepts at
    /// `ctr = 0` and `weight_2` at `ctr = 2`.
    #[test]
    fn test_batch_weight_derivation_nut00_v3_secret_vector() {
        use std::str::FromStr;

        use crate::nuts::nut02::KeySetVersion;
        use crate::secret::Secret;

        let k = hex_to_g2(
            "aa4edef9c1ed7f729f520e47730a124fd70662a904ba1074728114d1031e1572c6c886f6b57ec72a6178288c47c335771638533957d540a9d2370f17cc7ed5863bc0b995b8825e0ee1ea1e1e4d00dbae81f14b0bf3611b78c952aacab827a053",
        );
        let c1 = hex_to_g1(
            "a34dbfad742c0f97cd4b6ca0249b3353a8a0eb4cbdb4d8cf83fe8b22d39b1a6205623ca819b4985238968ad0328caeba",
        );
        let c2 = hex_to_g1(
            "a2bf34cf329d2cffca9442224279c261c6d47f1236d4b6aafd4ddcade73c25f719a55d747b0935c9fc182343067514b7",
        );
        let secrets = [
            "0240c186f3e19f6f8cd00db81c5266d5c74ddca48ff848efd0e3e125b2846f30d9",
            "03a3e12cc077e5605f36441046f50c114fcc883b079a34028bed66732e3a419e51",
        ]
        .map(|hex| Secret::from_str(hex).unwrap());
        let inputs = secrets
            .iter()
            .map(|secret| secret.hash_input(KeySetVersion::Version02).unwrap())
            .collect::<Vec<_>>();
        assert!(inputs.iter().all(|input| input.len() == 33));

        let mint_pubkeys = [k, k];
        let signatures = [c1, c2];
        let messages: [&[u8]; 2] = [&inputs[0], &inputs[1]];

        assert_eq!(
            crate::util::hex::encode(derive_batch_challenge(
                &mint_pubkeys,
                &signatures,
                &messages
            )),
            "c8eac68470172516ac2290ccf3b345fca136e2e5d74adaf16b05d6389e3867c9"
        );
        let weights = derive_batch_weights(&mint_pubkeys, &signatures, &messages)
            .expect("batch weights should be derivable");
        assert_eq!(
            crate::util::hex::encode(weights[0].to_bytes()),
            "5b3c830c6690f9494e1b8a3b9e4a75aa21af4d55eb158444aa5e0a4118187851"
        );
        assert_eq!(
            crate::util::hex::encode(weights[1].to_bytes()),
            "26cf70a127f195374a3d0b0086e809592c2624cee02312bdf095a2fed775fa13"
        );
        assert!(batch_verify_pairing(&mint_pubkeys, &signatures, &messages));

        // Taking each secret as its 66 hex characters signs different
        // messages, so the batch no longer verifies.
        let hex_messages: [&[u8]; 2] = [secrets[0].as_bytes(), secrets[1].as_bytes()];
        assert!(!batch_verify_pairing(
            &mint_pubkeys,
            &signatures,
            &hex_messages
        ));
    }

    #[test]
    fn test_batch_pairing_single_and_mixed_keys() {
        let first_key = BlsSecretKey {
            scalar: Scalar::from(2u64),
        };
        let second_key = BlsSecretKey {
            scalar: Scalar::from(3u64),
        };
        let keys = [&first_key, &first_key, &second_key];
        let messages: [&[u8]; 3] = [b"first", b"second", b"third"];
        let pubkeys = keys.map(BlsSecretKey::public_key_g2);
        let signatures: Vec<_> = messages
            .iter()
            .zip(keys)
            .map(|(message, key)| BlsG1PublicKey::hash_to_curve(message).mul(key))
            .collect();

        for count in 1..=messages.len() {
            assert!(batch_verify_pairing(
                &pubkeys[..count],
                &signatures[..count],
                &messages[..count]
            ));
        }

        for index in 0..messages.len() {
            let mut wrong_messages = messages;
            wrong_messages[index] = b"different";
            assert!(!batch_verify_pairing(
                &pubkeys,
                &signatures,
                &wrong_messages
            ));

            let mut wrong_signatures = signatures.clone();
            wrong_signatures[index] = signatures[(index + 1) % signatures.len()];
            assert!(!batch_verify_pairing(
                &pubkeys,
                &wrong_signatures,
                &messages
            ));

            let mut wrong_pubkeys = pubkeys;
            wrong_pubkeys[index] = match index {
                2 => first_key.public_key_g2(),
                _ => second_key.public_key_g2(),
            };
            assert!(!batch_verify_pairing(
                &wrong_pubkeys,
                &signatures,
                &messages
            ));
        }
    }

    #[test]
    fn test_batch_pairing_empty_and_mismatched_lengths() {
        assert!(batch_verify_pairing(&[], &[], &[]));

        let key = BlsSecretKey {
            scalar: Scalar::from(2u64),
        };
        let message: &[u8] = b"message";
        let signature = BlsG1PublicKey::hash_to_curve(message).mul(&key);
        let pubkey = key.public_key_g2();
        assert!(!batch_verify_pairing(&[pubkey], &[], &[]));
        assert!(!batch_verify_pairing(&[], &[signature], &[]));
        assert!(!batch_verify_pairing(&[], &[], &[message]));
        assert!(!batch_verify_pairing(&[pubkey], &[signature], &[]));
        assert!(!batch_verify_pairing(&[pubkey], &[], &[message]));
        assert!(!batch_verify_pairing(&[], &[signature], &[message]));
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

    /// The BLS12-381 base field modulus `p`, big-endian.
    const FP_MODULUS: [u8; 48] = [
        0x1a, 0x01, 0x11, 0xea, 0x39, 0x7f, 0xe6, 0x9a, 0x4b, 0x1b, 0xa7, 0xb6, 0x43, 0x4b, 0xac,
        0xd7, 0x64, 0x77, 0x4b, 0x84, 0xf3, 0x85, 0x12, 0xbf, 0x67, 0x30, 0xd2, 0xa0, 0xf6, 0xb0,
        0xf6, 0x24, 0x1e, 0xab, 0xff, 0xfe, 0xb1, 0x53, 0xff, 0xff, 0xb9, 0xfe, 0xff, 0xff, 0xff,
        0xff, 0xaa, 0xab,
    ];

    /// Add `p` to a big-endian field element, returning the carry-out.
    fn add_modulus(element: &mut [u8]) -> bool {
        let mut carry = 0u16;
        for (byte, modulus) in element.iter_mut().rev().zip(FP_MODULUS.iter().rev()) {
            let sum = u16::from(*byte) + u16::from(*modulus) + carry;
            *byte = (sum & 0xff) as u8;
            carry = sum >> 8;
        }
        carry != 0
    }

    /// Point validation (NUT-00): every invalid G1 encoding is refused.
    #[test]
    fn test_reject_invalid_g1_encodings() {
        let generator = G1Affine::generator().to_compressed();
        assert!(BlsG1PublicKey::from_bytes(&generator).is_ok());

        // Compression flag cleared.
        let mut uncompressed_flag = generator;
        uncompressed_flag[0] &= 0x7f;
        assert!(BlsG1PublicKey::from_bytes(&uncompressed_flag).is_err());

        // Infinity flag with a non-zero x.
        let mut infinity_with_x = [0u8; 48];
        infinity_with_x[0] = 0xc0;
        infinity_with_x[47] = 1;
        assert!(BlsG1PublicKey::from_bytes(&infinity_with_x).is_err());

        // Identity with the sort flag set.
        let mut sorted_identity = [0u8; 48];
        sorted_identity[0] = 0xe0;
        assert!(BlsG1PublicKey::from_bytes(&sorted_identity).is_err());

        // Uncompressed width.
        assert!(BlsG1PublicKey::from_bytes(&G1Affine::generator().to_uncompressed()).is_err());

        // Non-canonical x: a subgroup point's x + p, which still fits beside
        // the flag bits. Search multiples of g1 for an x small enough.
        let canonical = (1u64..)
            .map(|k| (G1Projective::generator() * Scalar::from(k)).to_affine())
            .map(|point| point.to_compressed())
            .find(|bytes| bytes[0] & 0x1f < 0x05)
            .expect("a multiple of g1 with a small x");
        assert!(BlsG1PublicKey::from_bytes(&canonical).is_ok());
        let flags = canonical[0] & 0xe0;
        let mut non_canonical = canonical;
        non_canonical[0] &= 0x1f;
        assert!(!add_modulus(&mut non_canonical));
        assert_eq!(non_canonical[0] & 0xe0, 0, "x + p must fit in 381 bits");
        non_canonical[0] |= flags;
        assert!(BlsG1PublicKey::from_bytes(&non_canonical).is_err());

        // On the curve but outside the prime-order subgroup.
        let off_subgroup = (1u8..=u8::MAX)
            .map(|x| {
                let mut bytes = [0u8; 48];
                bytes[0] = 0x80;
                bytes[47] = x;
                bytes
            })
            .find(|bytes| {
                bool::from(G1Affine::from_compressed_unchecked(bytes).is_some())
                    && bool::from(G1Affine::from_compressed(bytes).is_none())
            })
            .expect("a small x on the curve outside the subgroup");
        assert!(BlsG1PublicKey::from_bytes(&off_subgroup).is_err());
    }

    /// Point validation (NUT-00): every invalid G2 encoding is refused.
    #[test]
    fn test_reject_invalid_g2_encodings() {
        let generator = G2Affine::generator().to_compressed();
        assert!(BlsG2PublicKey::from_bytes(&generator).is_ok());

        // Compression flag cleared.
        let mut uncompressed_flag = generator;
        uncompressed_flag[0] &= 0x7f;
        assert!(BlsG2PublicKey::from_bytes(&uncompressed_flag).is_err());

        // Infinity flag with a non-zero x.
        let mut infinity_with_x = [0u8; 96];
        infinity_with_x[0] = 0xc0;
        infinity_with_x[95] = 1;
        assert!(BlsG2PublicKey::from_bytes(&infinity_with_x).is_err());

        // Identity with the sort flag set.
        let mut sorted_identity = [0u8; 96];
        sorted_identity[0] = 0xe0;
        assert!(BlsG2PublicKey::from_bytes(&sorted_identity).is_err());

        // Uncompressed width.
        assert!(BlsG2PublicKey::from_bytes(&G2Affine::generator().to_uncompressed()).is_err());

        // Non-canonical x: g2 with p added to x's c0 half, which has no flags.
        let mut non_canonical = generator;
        assert!(!add_modulus(&mut non_canonical[48..]));
        assert!(BlsG2PublicKey::from_bytes(&non_canonical).is_err());

        // On the curve but outside the prime-order subgroup.
        let off_subgroup = (1u8..=u8::MAX)
            .map(|x| {
                let mut bytes = [0u8; 96];
                bytes[0] = 0x80;
                bytes[95] = x;
                bytes
            })
            .find(|bytes| {
                bool::from(G2Affine::from_compressed_unchecked(bytes).is_some())
                    && bool::from(G2Affine::from_compressed(bytes).is_none())
            })
            .expect("a small x on the curve outside the subgroup");
        assert!(BlsG2PublicKey::from_bytes(&off_subgroup).is_err());
    }

    /// NUT-00: hex for v3 points is lowercase; legacy points keep accepting either case.
    #[test]
    fn test_v3_point_hex_must_be_lowercase() {
        use crate::nuts::nut02::KeySetVersion;
        use crate::nuts::{DhkePoint, MintPublicKey};

        let g1_hex = crate::util::hex::encode(BlsG1PublicKey::hash_to_curve(b"case").to_bytes());
        assert!(DhkePoint::from_hex_for_version(&g1_hex, KeySetVersion::Version02).is_ok());
        assert!(matches!(
            DhkePoint::from_hex_for_version(g1_hex.to_uppercase(), KeySetVersion::Version02),
            Err(Error::NonLowercaseHex)
        ));

        let g2_hex = crate::util::hex::encode(
            BlsSecretKey::from_bytes(&[1; 32])
                .unwrap()
                .public_key_g2()
                .to_bytes(),
        );
        assert!(MintPublicKey::from_hex(&g2_hex).is_ok());
        assert!(matches!(
            MintPublicKey::from_hex(g2_hex.to_uppercase()),
            Err(Error::NonLowercaseHex)
        ));

        let secp_hex = crate::nuts::SecretKey::generate().public_key().to_hex();
        for hex in [secp_hex.clone(), secp_hex.to_uppercase()] {
            assert!(DhkePoint::from_hex_for_version(&hex, KeySetVersion::Version00).is_ok());
            assert!(MintPublicKey::from_hex(&hex).is_ok());
        }
    }
}
