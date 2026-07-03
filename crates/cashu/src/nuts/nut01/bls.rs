use core::fmt;
use std::collections::BTreeMap;
#[cfg(feature = "federation")]
use std::collections::BTreeSet;

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

/// Non-zero signer ID used for threshold BLS interpolation.
#[cfg(feature = "federation")]
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct BlsSignerId(u16);

#[cfg(feature = "federation")]
impl BlsSignerId {
    /// Create a non-zero signer ID.
    pub fn new(id: u16) -> Result<Self, Error> {
        if id == 0 {
            return Err(Error::InvalidThresholdSignerId(id));
        }

        Ok(Self(id))
    }

    /// Return the signer ID as an integer.
    #[must_use]
    pub fn get(self) -> u16 {
        self.0
    }

    /// Return big-endian signer ID bytes.
    #[must_use]
    pub fn to_bytes(self) -> [u8; 2] {
        self.0.to_be_bytes()
    }

    /// Parse a signer ID from big-endian bytes.
    pub fn from_bytes(bytes: [u8; 2]) -> Result<Self, Error> {
        Self::new(u16::from_be_bytes(bytes))
    }

    fn scalar(self) -> Scalar {
        Scalar::from(u64::from(self.0))
    }
}

/// Secret threshold BLS key share for one federation member.
#[cfg(feature = "federation")]
#[derive(Clone, PartialEq, Eq)]
pub struct BlsSecretKeyShare {
    signer_id: BlsSignerId,
    secret_key: BlsSecretKey,
}

#[cfg(feature = "federation")]
impl fmt::Debug for BlsSecretKeyShare {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("BlsSecretKeyShare")
            .field("signer_id", &self.signer_id)
            .finish_non_exhaustive()
    }
}

#[cfg(feature = "federation")]
impl BlsSecretKeyShare {
    /// Create a secret key share.
    pub fn new(signer_id: BlsSignerId, secret_key: BlsSecretKey) -> Self {
        Self {
            signer_id,
            secret_key,
        }
    }

    /// Return the signer ID.
    #[must_use]
    pub fn signer_id(&self) -> BlsSignerId {
        self.signer_id
    }

    /// Derive this member's public key share.
    #[must_use]
    pub fn public_key_share(&self) -> BlsPublicKeyShare {
        BlsPublicKeyShare {
            signer_id: self.signer_id,
            public_key: self.secret_key.public_key_g2(),
        }
    }

    /// Blind-sign a G1 blinded message with this secret share.
    #[must_use]
    pub fn blind_sign_share(&self, blinded_message: &BlsG1PublicKey) -> BlsBlindSignatureShare {
        BlsBlindSignatureShare {
            signer_id: self.signer_id,
            signature: blinded_message.mul(&self.secret_key),
        }
    }

    /// Return canonical scalar bytes for local secret-share storage.
    #[must_use]
    pub fn to_secret_bytes(&self) -> [u8; 32] {
        self.secret_key.to_bytes()
    }

    /// Serialize this secret share as `signer_id || scalar`.
    ///
    /// The returned bytes contain secret key material and must only be written to
    /// private member configuration or secret storage.
    #[must_use]
    pub fn to_bytes(&self) -> [u8; 34] {
        let mut bytes = [0u8; 34];
        bytes[..2].copy_from_slice(&self.signer_id.to_bytes());
        bytes[2..].copy_from_slice(&self.secret_key.to_bytes());
        bytes
    }

    /// Parse a secret share from `signer_id || scalar` bytes.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, Error> {
        if bytes.len() != 34 {
            return Err(Error::InvalidThresholdShareSize {
                expected: 34,
                found: bytes.len(),
            });
        }

        let mut signer_id = [0u8; 2];
        signer_id.copy_from_slice(&bytes[..2]);
        let signer_id = BlsSignerId::from_bytes(signer_id)?;
        let secret_key = BlsSecretKey::from_bytes(&bytes[2..])?;

        Ok(Self {
            signer_id,
            secret_key,
        })
    }
}

/// Public threshold BLS key share for one federation member.
#[cfg(feature = "federation")]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BlsPublicKeyShare {
    signer_id: BlsSignerId,
    public_key: BlsG2PublicKey,
}

#[cfg(feature = "federation")]
impl BlsPublicKeyShare {
    /// Create a public key share.
    pub fn new(signer_id: BlsSignerId, public_key: BlsG2PublicKey) -> Self {
        Self {
            signer_id,
            public_key,
        }
    }

    /// Return the signer ID.
    #[must_use]
    pub fn signer_id(&self) -> BlsSignerId {
        self.signer_id
    }

    /// Return the public key share.
    #[must_use]
    pub fn public_key(&self) -> BlsG2PublicKey {
        self.public_key
    }

    /// Serialize this public share as `signer_id || compressed_g2`.
    #[must_use]
    pub fn to_bytes(&self) -> [u8; 98] {
        let mut bytes = [0u8; 98];
        bytes[..2].copy_from_slice(&self.signer_id.to_bytes());
        bytes[2..].copy_from_slice(&self.public_key.to_bytes());
        bytes
    }

    /// Parse a public share from `signer_id || compressed_g2` bytes.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, Error> {
        if bytes.len() != 98 {
            return Err(Error::InvalidThresholdShareSize {
                expected: 98,
                found: bytes.len(),
            });
        }

        let mut signer_id = [0u8; 2];
        signer_id.copy_from_slice(&bytes[..2]);
        let signer_id = BlsSignerId::from_bytes(signer_id)?;
        let public_key = BlsG2PublicKey::from_bytes(&bytes[2..])?;

        Ok(Self {
            signer_id,
            public_key,
        })
    }
}

/// Blind signature share produced by one threshold BLS signer.
#[cfg(feature = "federation")]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BlsBlindSignatureShare {
    signer_id: BlsSignerId,
    signature: BlsG1PublicKey,
}

#[cfg(feature = "federation")]
impl BlsBlindSignatureShare {
    /// Create a blind signature share.
    pub fn new(signer_id: BlsSignerId, signature: BlsG1PublicKey) -> Self {
        Self {
            signer_id,
            signature,
        }
    }

    /// Return the signer ID.
    #[must_use]
    pub fn signer_id(&self) -> BlsSignerId {
        self.signer_id
    }

    /// Return the blind signature share point.
    #[must_use]
    pub fn signature(&self) -> BlsG1PublicKey {
        self.signature
    }

    /// Serialize this blind signature share as `signer_id || compressed_g1`.
    #[must_use]
    pub fn to_bytes(&self) -> [u8; 50] {
        let mut bytes = [0u8; 50];
        bytes[..2].copy_from_slice(&self.signer_id.to_bytes());
        bytes[2..].copy_from_slice(&self.signature.to_bytes());
        bytes
    }

    /// Parse a blind signature share from `signer_id || compressed_g1` bytes.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, Error> {
        if bytes.len() != 50 {
            return Err(Error::InvalidThresholdShareSize {
                expected: 50,
                found: bytes.len(),
            });
        }

        let mut signer_id = [0u8; 2];
        signer_id.copy_from_slice(&bytes[..2]);
        let signer_id = BlsSignerId::from_bytes(signer_id)?;
        let signature = BlsG1PublicKey::from_bytes(&bytes[2..])?;

        Ok(Self {
            signer_id,
            signature,
        })
    }
}

/// Trusted-dealer threshold BLS keyset.
#[cfg(feature = "federation")]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlsThresholdKeySet {
    threshold: usize,
    aggregate_public_key: BlsG2PublicKey,
    secret_shares: Vec<BlsSecretKeyShare>,
    public_shares: Vec<BlsPublicKeyShare>,
}

#[cfg(feature = "federation")]
impl BlsThresholdKeySet {
    /// Return the signature threshold.
    #[must_use]
    pub fn threshold(&self) -> usize {
        self.threshold
    }

    /// Return the aggregate public key.
    #[must_use]
    pub fn aggregate_public_key(&self) -> BlsG2PublicKey {
        self.aggregate_public_key
    }

    /// Return all secret key shares.
    #[must_use]
    pub fn secret_shares(&self) -> &[BlsSecretKeyShare] {
        &self.secret_shares
    }

    /// Return all public key shares.
    #[must_use]
    pub fn public_shares(&self) -> &[BlsPublicKeyShare] {
        &self.public_shares
    }
}

/// Create threshold BLS shares from trusted-dealer polynomial coefficients.
///
/// `coefficients[0]` is the aggregate secret. The number of coefficients is
/// the threshold, and the polynomial is evaluated at each non-zero signer ID.
#[cfg(feature = "federation")]
pub fn trusted_dealer_keygen(
    coefficients: &[BlsSecretKey],
    signer_ids: &[BlsSignerId],
) -> Result<BlsThresholdKeySet, Error> {
    validate_threshold_inputs(coefficients.len(), signer_ids)?;
    let aggregate_public_key = coefficients[0].public_key_g2();

    let mut secret_shares = Vec::with_capacity(signer_ids.len());
    let mut public_shares = Vec::with_capacity(signer_ids.len());

    for signer_id in signer_ids {
        let secret_key = evaluate_polynomial(coefficients, signer_id.scalar());
        let secret_share = BlsSecretKeyShare::new(*signer_id, secret_key);
        public_shares.push(secret_share.public_key_share());
        secret_shares.push(secret_share);
    }

    Ok(BlsThresholdKeySet {
        threshold: coefficients.len(),
        aggregate_public_key,
        secret_shares,
        public_shares,
    })
}

/// Verify a blind signature share against the signer's public key share.
#[cfg(feature = "federation")]
pub fn verify_blind_signature_share(
    share: &BlsBlindSignatureShare,
    blinded_message: &BlsG1PublicKey,
    public_share: &BlsPublicKeyShare,
) -> Result<bool, Error> {
    if share.signer_id != public_share.signer_id {
        return Err(Error::ThresholdSignerMismatch {
            expected: public_share.signer_id.get(),
            found: share.signer_id.get(),
        });
    }

    Ok(verify_blind_signature_pairing(
        &share.signature,
        blinded_message,
        &public_share.public_key,
    ))
}

/// Aggregate threshold blind signature shares into one BLS blind signature.
#[cfg(feature = "federation")]
pub fn aggregate_blind_signature_shares(
    threshold: usize,
    shares: &[BlsBlindSignatureShare],
) -> Result<BlsG1PublicKey, Error> {
    validate_threshold_shares(
        threshold,
        shares.iter().map(BlsBlindSignatureShare::signer_id),
    )?;

    let mut aggregate = G1Projective::identity();
    for share in shares {
        let coefficient = lagrange_coefficient_at_zero(
            share.signer_id,
            shares.iter().map(BlsBlindSignatureShare::signer_id),
        )?;
        aggregate += G1Projective::from(share.signature.point()) * coefficient;
    }

    g1_from_projective(aggregate)
}

/// Aggregate threshold public key shares into the aggregate public key.
#[cfg(feature = "federation")]
pub fn aggregate_public_key_shares(
    threshold: usize,
    shares: &[BlsPublicKeyShare],
) -> Result<BlsG2PublicKey, Error> {
    validate_threshold_shares(threshold, shares.iter().map(BlsPublicKeyShare::signer_id))?;

    let mut aggregate = G2Projective::identity();
    for share in shares {
        let coefficient = lagrange_coefficient_at_zero(
            share.signer_id,
            shares.iter().map(BlsPublicKeyShare::signer_id),
        )?;
        aggregate += G2Projective::from(share.public_key.point()) * coefficient;
    }

    g2_from_projective(aggregate)
}

#[cfg(feature = "federation")]
fn validate_threshold_inputs(threshold: usize, signer_ids: &[BlsSignerId]) -> Result<(), Error> {
    if threshold == 0 || threshold > signer_ids.len() {
        return Err(Error::InvalidThreshold {
            threshold,
            participants: signer_ids.len(),
        });
    }

    reject_duplicate_signer_ids(signer_ids.iter().copied())
}

#[cfg(feature = "federation")]
fn validate_threshold_shares<I>(threshold: usize, signer_ids: I) -> Result<(), Error>
where
    I: IntoIterator<Item = BlsSignerId>,
{
    let signer_ids = signer_ids.into_iter().collect::<Vec<_>>();
    if signer_ids.len() < threshold {
        return Err(Error::InsufficientThresholdShares {
            threshold,
            found: signer_ids.len(),
        });
    }

    validate_threshold_inputs(threshold, &signer_ids)
}

#[cfg(feature = "federation")]
fn reject_duplicate_signer_ids<I>(signer_ids: I) -> Result<(), Error>
where
    I: IntoIterator<Item = BlsSignerId>,
{
    let mut seen = BTreeSet::new();
    for signer_id in signer_ids {
        if !seen.insert(signer_id) {
            return Err(Error::DuplicateThresholdSignerId {
                id: signer_id.get(),
            });
        }
    }

    Ok(())
}

#[cfg(feature = "federation")]
fn evaluate_polynomial(coefficients: &[BlsSecretKey], x: Scalar) -> BlsSecretKey {
    let mut result = Scalar::zero();
    for coefficient in coefficients.iter().rev() {
        result *= x;
        result += coefficient.scalar();
    }

    BlsSecretKey { scalar: result }
}

#[cfg(feature = "federation")]
fn lagrange_coefficient_at_zero<I>(signer_id: BlsSignerId, signer_ids: I) -> Result<Scalar, Error>
where
    I: IntoIterator<Item = BlsSignerId>,
{
    let x_i = signer_id.scalar();
    let mut numerator = Scalar::one();
    let mut denominator = Scalar::one();

    for other_id in signer_ids {
        if other_id == signer_id {
            continue;
        }

        let x_j = other_id.scalar();
        numerator *= -x_j;
        denominator *= x_i - x_j;
    }

    let denominator_inverse =
        Option::<Scalar>::from(denominator.invert()).ok_or(Error::InvalidSecretKey)?;

    Ok(numerator * denominator_inverse)
}

#[cfg(feature = "federation")]
fn g1_from_projective(point: G1Projective) -> Result<BlsG1PublicKey, Error> {
    let point = point.to_affine();
    if bool::from(point.is_identity()) {
        return Err(Error::InvalidPublicKey);
    }

    Ok(BlsG1PublicKey { point })
}

#[cfg(feature = "federation")]
fn g2_from_projective(point: G2Projective) -> Result<BlsG2PublicKey, Error> {
    let point = point.to_affine();
    if bool::from(point.is_identity()) {
        return Err(Error::InvalidPublicKey);
    }

    Ok(BlsG2PublicKey { point })
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

    #[cfg(feature = "federation")]
    fn signer_ids(ids: &[u16]) -> Vec<BlsSignerId> {
        ids.iter()
            .map(|id| BlsSignerId::new(*id).expect("non-zero signer id"))
            .collect()
    }

    #[cfg(feature = "federation")]
    fn bls_key(seed: u8) -> BlsSecretKey {
        BlsSecretKey::from_reduced_bytes(&[seed; 32])
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

    #[cfg(feature = "federation")]
    #[test]
    fn test_threshold_bls_one_of_one_matches_standalone_signature() {
        let coefficients = vec![bls_key(7)];
        let keyset = trusted_dealer_keygen(&coefficients, &signer_ids(&[1])).expect("keyset");
        let blinded_message = BlsG1PublicKey::hash_to_curve(b"threshold one of one");

        let share = keyset.secret_shares()[0].blind_sign_share(&blinded_message);
        assert!(
            verify_blind_signature_share(&share, &blinded_message, &keyset.public_shares()[0])
                .expect("share verification result")
        );

        let aggregate_signature =
            aggregate_blind_signature_shares(keyset.threshold(), &[share]).expect("signature");
        let direct_signature = blinded_message.mul(&coefficients[0]);

        assert_eq!(aggregate_signature, direct_signature);
        assert_eq!(
            keyset.aggregate_public_key(),
            coefficients[0].public_key_g2()
        );
        assert_eq!(
            aggregate_public_key_shares(keyset.threshold(), keyset.public_shares())
                .expect("aggregate public key"),
            keyset.aggregate_public_key()
        );
        assert!(verify_blind_signature_pairing(
            &aggregate_signature,
            &blinded_message,
            &keyset.aggregate_public_key()
        ));
    }

    #[cfg(feature = "federation")]
    #[test]
    fn test_threshold_bls_three_of_five_aggregates_any_valid_subset() {
        let coefficients = vec![bls_key(3), bls_key(5), bls_key(9)];
        let keyset =
            trusted_dealer_keygen(&coefficients, &signer_ids(&[1, 2, 3, 4, 5])).expect("keyset");
        let blinded_message = BlsG1PublicKey::hash_to_curve(b"threshold three of five");

        let shares = keyset
            .secret_shares()
            .iter()
            .map(|share| share.blind_sign_share(&blinded_message))
            .collect::<Vec<_>>();

        for (share, public_share) in shares.iter().zip(keyset.public_shares()) {
            assert!(
                verify_blind_signature_share(share, &blinded_message, public_share)
                    .expect("share verification result")
            );
        }

        let subset_a = [shares[0], shares[2], shares[4]];
        let subset_b = [shares[1], shares[2], shares[3]];
        let aggregate_a =
            aggregate_blind_signature_shares(keyset.threshold(), &subset_a).expect("signature");
        let aggregate_b =
            aggregate_blind_signature_shares(keyset.threshold(), &subset_b).expect("signature");
        let direct_signature = blinded_message.mul(&coefficients[0]);

        assert_eq!(aggregate_a, direct_signature);
        assert_eq!(aggregate_b, direct_signature);
        assert!(verify_blind_signature_pairing(
            &aggregate_a,
            &blinded_message,
            &keyset.aggregate_public_key()
        ));

        let public_subset = [
            keyset.public_shares()[0],
            keyset.public_shares()[2],
            keyset.public_shares()[4],
        ];
        assert_eq!(
            aggregate_public_key_shares(keyset.threshold(), &public_subset)
                .expect("aggregate public key"),
            keyset.aggregate_public_key()
        );
    }

    #[cfg(feature = "federation")]
    #[test]
    fn test_threshold_bls_rejects_bad_share_sets() {
        let coefficients = vec![bls_key(11), bls_key(13), bls_key(17)];
        let keyset =
            trusted_dealer_keygen(&coefficients, &signer_ids(&[1, 2, 3, 4, 5])).expect("keyset");
        let blinded_message = BlsG1PublicKey::hash_to_curve(b"valid message");
        let wrong_message = BlsG1PublicKey::hash_to_curve(b"wrong message");
        let share = keyset.secret_shares()[0].blind_sign_share(&blinded_message);
        let wrong_share = keyset.secret_shares()[0].blind_sign_share(&wrong_message);

        assert!(matches!(
            aggregate_blind_signature_shares(keyset.threshold(), &[share]),
            Err(Error::InsufficientThresholdShares {
                threshold: 3,
                found: 1
            })
        ));
        assert!(matches!(
            aggregate_blind_signature_shares(keyset.threshold(), &[share, share, share]),
            Err(Error::DuplicateThresholdSignerId { id: 1 })
        ));
        assert!(!verify_blind_signature_share(
            &wrong_share,
            &blinded_message,
            &keyset.public_shares()[0]
        )
        .expect("share verification result"));
        assert!(matches!(
            verify_blind_signature_share(&share, &blinded_message, &keyset.public_shares()[1]),
            Err(Error::ThresholdSignerMismatch {
                expected: 2,
                found: 1
            })
        ));
    }

    #[cfg(feature = "federation")]
    #[test]
    fn test_threshold_bls_share_serialization_round_trips() {
        let coefficients = vec![bls_key(19), bls_key(23), bls_key(29)];
        let keyset =
            trusted_dealer_keygen(&coefficients, &signer_ids(&[1, 2, 3, 4, 5])).expect("keyset");
        let blinded_message = BlsG1PublicKey::hash_to_curve(b"share serialization");
        let secret_share = &keyset.secret_shares()[0];
        let public_share = keyset.public_shares()[0];
        let signature_share = secret_share.blind_sign_share(&blinded_message);

        assert_eq!(
            BlsSecretKeyShare::from_bytes(&secret_share.to_bytes()).expect("secret share"),
            *secret_share
        );
        assert_eq!(
            BlsPublicKeyShare::from_bytes(&public_share.to_bytes()).expect("public share"),
            public_share
        );
        assert_eq!(
            BlsBlindSignatureShare::from_bytes(&signature_share.to_bytes()).expect("signature"),
            signature_share
        );
        assert!(matches!(
            BlsBlindSignatureShare::from_bytes(&signature_share.to_bytes()[..49]),
            Err(Error::InvalidThresholdShareSize {
                expected: 50,
                found: 49
            })
        ));
    }

    #[cfg(feature = "federation")]
    #[test]
    fn test_threshold_bls_aggregate_signature_unblinds_and_verifies() {
        let coefficients = vec![bls_key(31), bls_key(37), bls_key(41)];
        let keyset =
            trusted_dealer_keygen(&coefficients, &signer_ids(&[1, 2, 3, 4, 5])).expect("keyset");
        let secret = b"threshold aggregate proof";
        let blinding_factor = bls_key(43);
        let blinded_message = BlsG1PublicKey::hash_to_curve(secret).mul(&blinding_factor);
        let shares = keyset
            .secret_shares()
            .iter()
            .take(3)
            .map(|share| share.blind_sign_share(&blinded_message))
            .collect::<Vec<_>>();

        let blinded_signature =
            aggregate_blind_signature_shares(keyset.threshold(), &shares).expect("signature");
        let unblinded_signature = blinded_signature.mul(
            &blinding_factor
                .invert()
                .expect("non-zero blinding factor inverse"),
        );

        assert!(verify_pairing(
            &unblinded_signature,
            secret,
            &keyset.aggregate_public_key()
        ));
    }
}
