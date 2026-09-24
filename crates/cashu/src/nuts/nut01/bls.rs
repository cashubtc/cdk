use core::fmt;
use std::collections::BTreeMap;
#[cfg(feature = "federation")]
use std::collections::BTreeSet;

use bitcoin::hashes::sha256::Hash as Sha256Hash;
use bitcoin::hashes::Hash;
use bitcoin::secp256k1::rand::rngs::OsRng;
use bitcoin::secp256k1::rand::RngCore;
use bls12_381::hash_to_curve::{ExpandMsgXmd, HashToCurve};
use bls12_381::{
    multi_miller_loop, pairing, G1Affine, G1Projective, G2Affine, G2Prepared, G2Projective, Gt,
    Scalar,
};
use group::Curve;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use sha2_09::Sha256;
use zeroize::Zeroize;

use super::Error;

const BLS_DST: &[u8] = b"CASHU_BLS12_381_G1_XMD:SHA-256_SSWU_RO_";
const BLS_BATCH_DST: &[u8] = b"Cashu_BLS_Batch_v1";
const BLS_BLIND_BATCH_DST: &[u8] = b"Cashu_BLS_Blind_Batch_v1";
#[cfg(feature = "federation")]
const BLS_THRESHOLD_SHARE_VERSION: u8 = 1;

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
    /// Generate a uniformly distributed non-zero BLS scalar.
    pub fn generate() -> Self {
        loop {
            let mut bytes = [0u8; 32];
            OsRng.fill_bytes(&mut bytes);
            if bytes != [0u8; 32] {
                if let Ok(secret_key) = Self::from_bytes(&bytes) {
                    bytes.zeroize();
                    return secret_key;
                }
            }
            bytes.zeroize();
        }
    }

    /// Derive a scalar by reducing 32-byte input modulo the BLS12-381 scalar field.
    pub fn from_reduced_bytes(bytes: &[u8; 32]) -> Self {
        let mut wide = [0u8; 64];
        for (dst, src) in wide.iter_mut().zip(bytes.iter().rev()) {
            *dst = *src;
        }
        let scalar = Scalar::from_bytes_wide(&wide);
        wide.zeroize();

        Self { scalar }
    }

    /// Parse a canonical scalar from 32 big-endian bytes.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, Error> {
        let mut secret_bytes: [u8; 32] =
            bytes.try_into().map_err(|_| Error::InvalidSecretKeySize {
                expected: 32,
                found: bytes.len(),
            })?;
        secret_bytes.reverse();
        let scalar = Option::<Scalar>::from(Scalar::from_bytes(&secret_bytes));
        secret_bytes.zeroize();
        let scalar = scalar.ok_or(Error::InvalidSecretKey)?;
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

impl Zeroize for BlsSecretKey {
    fn zeroize(&mut self) {
        self.scalar.zeroize();
    }
}

impl Drop for BlsSecretKey {
    fn drop(&mut self) {
        self.zeroize();
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

    /// Parse compressed G1 bytes from hex.
    pub fn from_hex<S>(hex: S) -> Result<Self, Error>
    where
        S: AsRef<str>,
    {
        let bytes = crate::util::hex::decode(hex.as_ref())?;
        Self::from_bytes(&bytes)
    }

    /// Return compressed G1 bytes as hex.
    #[must_use]
    pub fn to_hex(&self) -> String {
        crate::util::hex::encode(self.to_bytes())
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

impl Serialize for BlsG1PublicKey {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&self.to_hex())
    }
}

impl<'de> Deserialize<'de> for BlsG1PublicKey {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::from_hex(value).map_err(serde::de::Error::custom)
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

    /// Parse compressed G2 bytes from hex.
    pub fn from_hex<S>(hex: S) -> Result<Self, Error>
    where
        S: AsRef<str>,
    {
        let bytes = crate::util::hex::decode(hex.as_ref())?;
        Self::from_bytes(&bytes)
    }

    /// Return compressed G2 bytes as hex.
    #[must_use]
    pub fn to_hex(&self) -> String {
        crate::util::hex::encode(self.to_bytes())
    }

    /// Return the affine point.
    pub fn point(&self) -> G2Affine {
        self.point
    }
}

impl Serialize for BlsG2PublicKey {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&self.to_hex())
    }
}

impl<'de> Deserialize<'de> for BlsG2PublicKey {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::from_hex(value).map_err(serde::de::Error::custom)
    }
}

/// Non-zero signer ID used for threshold BLS interpolation.
#[cfg(feature = "federation")]
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct BlsSignerId(u16);

/// Aggregate BLS public key produced from a threshold keyset.
#[cfg(feature = "federation")]
pub type BlsAggregatePublicKey = BlsG2PublicKey;

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

    /// Serialize this secret share as `version || signer_id || scalar`.
    ///
    /// The returned bytes contain secret key material and must only be written to
    /// private member configuration or secret storage.
    #[must_use]
    pub fn to_bytes(&self) -> [u8; 35] {
        let mut bytes = [0u8; 35];
        bytes[0] = BLS_THRESHOLD_SHARE_VERSION;
        bytes[1..3].copy_from_slice(&self.signer_id.to_bytes());
        let mut secret_bytes = self.secret_key.to_bytes();
        bytes[3..].copy_from_slice(&secret_bytes);
        secret_bytes.zeroize();
        bytes
    }

    /// Parse a secret share from `version || signer_id || scalar` bytes.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, Error> {
        validate_threshold_share_bytes(bytes, 35)?;

        let mut signer_id = [0u8; 2];
        signer_id.copy_from_slice(&bytes[1..3]);
        let signer_id = BlsSignerId::from_bytes(signer_id)?;
        let secret_key = BlsSecretKey::from_bytes(&bytes[3..])?;

        Ok(Self {
            signer_id,
            secret_key,
        })
    }
}

#[cfg(feature = "federation")]
impl Zeroize for BlsSecretKeyShare {
    fn zeroize(&mut self) {
        self.secret_key.zeroize();
    }
}

#[cfg(feature = "federation")]
impl Serialize for BlsSecretKeyShare {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut bytes = self.to_bytes();
        let mut encoded = crate::util::hex::encode(bytes);
        bytes.zeroize();
        let result = serializer.serialize_str(&encoded);
        encoded.zeroize();
        result
    }
}

#[cfg(feature = "federation")]
impl<'de> Deserialize<'de> for BlsSecretKeyShare {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let mut value = String::deserialize(deserializer)?;
        let bytes = crate::util::hex::decode(&value);
        value.zeroize();
        let mut bytes = bytes.map_err(serde::de::Error::custom)?;
        let secret_share = Self::from_bytes(&bytes).map_err(serde::de::Error::custom);
        bytes.zeroize();
        secret_share
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

    /// Serialize this public share as `version || signer_id || compressed_g2`.
    #[must_use]
    pub fn to_bytes(&self) -> [u8; 99] {
        let mut bytes = [0u8; 99];
        bytes[0] = BLS_THRESHOLD_SHARE_VERSION;
        bytes[1..3].copy_from_slice(&self.signer_id.to_bytes());
        bytes[3..].copy_from_slice(&self.public_key.to_bytes());
        bytes
    }

    /// Parse a public share from `version || signer_id || compressed_g2` bytes.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, Error> {
        validate_threshold_share_bytes(bytes, 99)?;

        let mut signer_id = [0u8; 2];
        signer_id.copy_from_slice(&bytes[1..3]);
        let signer_id = BlsSignerId::from_bytes(signer_id)?;
        let public_key = BlsG2PublicKey::from_bytes(&bytes[3..])?;

        Ok(Self {
            signer_id,
            public_key,
        })
    }
}

#[cfg(feature = "federation")]
impl Serialize for BlsPublicKeyShare {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&crate::util::hex::encode(self.to_bytes()))
    }
}

#[cfg(feature = "federation")]
impl<'de> Deserialize<'de> for BlsPublicKeyShare {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        let bytes = crate::util::hex::decode(&value).map_err(serde::de::Error::custom)?;
        Self::from_bytes(&bytes).map_err(serde::de::Error::custom)
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

    /// Serialize this blind signature share as `version || signer_id || compressed_g1`.
    #[must_use]
    pub fn to_bytes(&self) -> [u8; 51] {
        let mut bytes = [0u8; 51];
        bytes[0] = BLS_THRESHOLD_SHARE_VERSION;
        bytes[1..3].copy_from_slice(&self.signer_id.to_bytes());
        bytes[3..].copy_from_slice(&self.signature.to_bytes());
        bytes
    }

    /// Parse a blind signature share from `version || signer_id || compressed_g1` bytes.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, Error> {
        validate_threshold_share_bytes(bytes, 51)?;

        let mut signer_id = [0u8; 2];
        signer_id.copy_from_slice(&bytes[1..3]);
        let signer_id = BlsSignerId::from_bytes(signer_id)?;
        let signature = BlsG1PublicKey::from_bytes(&bytes[3..])?;

        Ok(Self {
            signer_id,
            signature,
        })
    }
}

#[cfg(feature = "federation")]
impl Serialize for BlsBlindSignatureShare {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&crate::util::hex::encode(self.to_bytes()))
    }
}

#[cfg(feature = "federation")]
impl<'de> Deserialize<'de> for BlsBlindSignatureShare {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        let bytes = crate::util::hex::decode(&value).map_err(serde::de::Error::custom)?;
        Self::from_bytes(&bytes).map_err(serde::de::Error::custom)
    }
}

/// Trusted-dealer threshold BLS keyset.
#[cfg(feature = "federation")]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlsThresholdKeySet {
    threshold: usize,
    aggregate_public_key: BlsAggregatePublicKey,
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
    pub fn aggregate_public_key(&self) -> BlsAggregatePublicKey {
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

/// Evaluate secret polynomial coefficients at one signer ID.
///
/// This helper is used by mocked DKG setup and by tests for future real DKG
/// share-delivery code. The returned share is for one participant polynomial;
/// final member shares are created by summing one valid contribution from each
/// participant with [`aggregate_secret_key_shares`].
#[cfg(feature = "federation")]
pub fn evaluate_secret_key_coefficients(
    signer_id: BlsSignerId,
    coefficients: &[BlsSecretKey],
) -> Result<BlsSecretKeyShare, Error> {
    if coefficients.is_empty() {
        return Err(Error::InsufficientThresholdShares {
            threshold: 1,
            found: 0,
        });
    }

    let secret_key = evaluate_polynomial(coefficients, signer_id.scalar());
    reject_zero_secret_key(&secret_key)?;
    Ok(BlsSecretKeyShare::new(signer_id, secret_key))
}

/// Aggregate secret key shares for the same signer ID.
///
/// In dealer-free DKG, each participant contributes a private scalar share to a
/// recipient. The recipient sums those same-signer contributions to obtain the
/// final threshold secret share matching the aggregate public commitments.
#[cfg(feature = "federation")]
pub fn aggregate_secret_key_shares(
    signer_id: BlsSignerId,
    shares: &[BlsSecretKeyShare],
) -> Result<BlsSecretKeyShare, Error> {
    if shares.is_empty() {
        return Err(Error::InsufficientThresholdShares {
            threshold: 1,
            found: 0,
        });
    }

    let mut aggregate = Scalar::zero();
    for share in shares {
        if share.signer_id() != signer_id {
            return Err(Error::ThresholdSignerMismatch {
                expected: signer_id.get(),
                found: share.signer_id().get(),
            });
        }
        aggregate += share.secret_key.scalar();
    }

    let secret_key = BlsSecretKey { scalar: aggregate };
    reject_zero_secret_key(&secret_key)?;
    Ok(BlsSecretKeyShare::new(signer_id, secret_key))
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

/// Batch verify blind signature shares against their signer public shares.
///
/// The batch uses domain-separated transcript-derived non-zero weights. A
/// caller that needs to identify an invalid signer can fall back to
/// [`verify_blind_signature_share`] after this function returns `false`.
#[cfg(feature = "federation")]
pub fn batch_verify_blind_signature_shares(
    shares: &[BlsBlindSignatureShare],
    blinded_messages: &[BlsG1PublicKey],
    public_shares: &[BlsPublicKeyShare],
) -> Result<bool, Error> {
    if shares.len() != blinded_messages.len() || shares.len() != public_shares.len() {
        return Ok(false);
    }

    for (share, public_share) in shares.iter().zip(public_shares) {
        if share.signer_id != public_share.signer_id {
            return Err(Error::ThresholdSignerMismatch {
                expected: public_share.signer_id.get(),
                found: share.signer_id.get(),
            });
        }
    }

    let signatures = shares
        .iter()
        .map(BlsBlindSignatureShare::signature)
        .collect::<Vec<_>>();
    let mint_pubkeys = public_shares
        .iter()
        .map(BlsPublicKeyShare::public_key)
        .collect::<Vec<_>>();

    Ok(batch_verify_blind_signature_pairing(
        &mint_pubkeys,
        &signatures,
        blinded_messages,
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

/// Sum public G2 commitments into one aggregate public key.
///
/// This is used by dealer-free DKG code after every participant reveals public
/// polynomial commitments. It intentionally accepts plain public keys rather
/// than threshold shares because each participant contributes one public point
/// to the final aggregate key.
#[cfg(feature = "federation")]
pub fn aggregate_public_key_commitments(
    commitments: &[BlsAggregatePublicKey],
) -> Result<BlsAggregatePublicKey, Error> {
    let mut aggregate = G2Projective::identity();
    for commitment in commitments {
        aggregate += G2Projective::from(commitment.point());
    }

    g2_from_projective(aggregate)
}

/// Evaluate public polynomial commitments at one signer ID.
///
/// If a participant reveals G2 commitments `[C_0, C_1, ...]`, this returns the
/// public key share corresponding to `C_0 + x*C_1 + x^2*C_2 + ...`, where `x`
/// is the non-zero signer ID. The secret scalar is never needed for this
/// verification step.
#[cfg(feature = "federation")]
pub fn evaluate_public_key_commitments(
    signer_id: BlsSignerId,
    commitments: &[BlsAggregatePublicKey],
) -> Result<BlsPublicKeyShare, Error> {
    // Horner evaluation needs one fewer group multiplication and no scalar powers.
    let public_share = commitments
        .iter()
        .rev()
        .map(|commitment| G2Projective::from(commitment.point()))
        .reduce(|value, coefficient| multiply_by_public_signer_id(value, signer_id) + coefficient)
        .unwrap_or_else(G2Projective::identity);

    Ok(BlsPublicKeyShare::new(
        signer_id,
        g2_from_projective(public_share)?,
    ))
}

// Variable-time multiplication is safe here because both the point and signer ID
// are public. Keep the typed ID boundary: secret scalars must use the group library.
#[cfg(feature = "federation")]
fn multiply_by_public_signer_id(point: G2Projective, signer_id: BlsSignerId) -> G2Projective {
    let id = signer_id.get();
    // BlsSignerId excludes zero, so the leading set bit initializes the result.
    let mut result = point;
    for bit in (0..u16::BITS - id.leading_zeros() - 1).rev() {
        result = result.double();
        if (id >> bit) & 1 != 0 {
            result += point;
        }
    }
    result
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
fn validate_threshold_share_bytes(bytes: &[u8], expected_len: usize) -> Result<(), Error> {
    if bytes.len() != expected_len {
        return Err(Error::InvalidThresholdShareSize {
            expected: expected_len,
            found: bytes.len(),
        });
    }

    if bytes[0] != BLS_THRESHOLD_SHARE_VERSION {
        return Err(Error::InvalidThresholdShareVersion {
            expected: BLS_THRESHOLD_SHARE_VERSION,
            found: bytes[0],
        });
    }

    Ok(())
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
fn reject_zero_secret_key(secret_key: &BlsSecretKey) -> Result<(), Error> {
    if bool::from(secret_key.public_key_g2().point().is_identity()) {
        return Err(Error::InvalidSecretKey);
    }

    Ok(())
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

    derive_nonzero_batch_weights(&transcript, mint_pubkeys.len())
}

fn derive_blind_batch_weights(
    mint_pubkeys: &[BlsG2PublicKey],
    signatures: &[BlsG1PublicKey],
    blinded_messages: &[BlsG1PublicKey],
) -> Vec<BlsSecretKey> {
    let mut transcript = Vec::new();
    transcript.extend_from_slice(BLS_BLIND_BATCH_DST);
    for ((mint_pubkey, signature), blinded_message) in
        mint_pubkeys.iter().zip(signatures).zip(blinded_messages)
    {
        transcript.extend_from_slice(&signature.to_bytes());
        transcript.extend_from_slice(&mint_pubkey.to_bytes());
        transcript.extend_from_slice(&blinded_message.to_bytes());
    }

    derive_nonzero_batch_weights(&transcript, mint_pubkeys.len())
}

fn derive_nonzero_batch_weights(transcript: &[u8], count: usize) -> Vec<BlsSecretKey> {
    let challenge = Sha256Hash::hash(transcript).to_byte_array();
    (0..count)
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

    // Move the signature pairing to the other side by negating its G1 point:
    // e(-sum(w_i * C_i), G2) * product_k e(sum(w_i * Y_i), K_k) == 1.
    // Evaluate all terms together with just one final exponentiation.
    let mut terms = Vec::with_capacity(weighted_messages.len() + 1);
    terms.push((
        (-weighted_signatures).to_affine(),
        G2Prepared::from(G2Affine::generator()),
    ));
    terms.extend(
        weighted_messages
            .into_values()
            .map(|(key, message)| (message.to_affine(), G2Prepared::from(key.point()))),
    );
    let term_refs: Vec<_> = terms.iter().map(|(message, key)| (message, key)).collect();

    multi_miller_loop(&term_refs).final_exponentiation() == Gt::identity()
}

/// Batch verify `e(signature, G2) == e(blinded_message, mint_pubkey)`.
pub(crate) fn batch_verify_blind_signature_pairing(
    mint_pubkeys: &[BlsG2PublicKey],
    signatures: &[BlsG1PublicKey],
    blinded_messages: &[BlsG1PublicKey],
) -> bool {
    if mint_pubkeys.len() != signatures.len() || signatures.len() != blinded_messages.len() {
        return false;
    }
    if signatures.is_empty() {
        return true;
    }

    let weights = derive_blind_batch_weights(mint_pubkeys, signatures, blinded_messages);
    let mut weighted_signatures = G1Projective::identity();
    let mut weighted_messages = BTreeMap::<[u8; 96], (BlsG2PublicKey, G1Projective)>::new();

    for (((mint_pubkey, signature), blinded_message), weight) in mint_pubkeys
        .iter()
        .zip(signatures)
        .zip(blinded_messages)
        .zip(&weights)
    {
        weighted_signatures += G1Projective::from(signature.point()) * weight.scalar();
        let weighted_message = G1Projective::from(blinded_message.point()) * weight.scalar();
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
        G2Prepared::from(G2Affine::generator()),
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

    #[cfg(feature = "federation")]
    #[derive(Debug)]
    struct DeterministicTestRng {
        state: u64,
    }

    #[cfg(feature = "federation")]
    impl DeterministicTestRng {
        fn new(seed: u64) -> Self {
            Self { state: seed }
        }

        fn next_u64(&mut self) -> u64 {
            let mut value = self.state;
            value ^= value << 13;
            value ^= value >> 7;
            value ^= value << 17;
            self.state = value;
            value
        }

        fn next_usize(&mut self, upper_bound: usize) -> usize {
            (self.next_u64() as usize) % upper_bound
        }

        fn fill_bytes(&mut self, bytes: &mut [u8]) {
            for chunk in bytes.chunks_mut(8) {
                let random = self.next_u64().to_be_bytes();
                chunk.copy_from_slice(&random[..chunk.len()]);
            }
        }

        fn bls_secret_key(&mut self) -> BlsSecretKey {
            let mut bytes = [0u8; 32];
            self.fill_bytes(&mut bytes);
            bytes[31] |= 1;
            BlsSecretKey::from_reduced_bytes(&bytes)
        }

        fn shuffled_indices(&mut self, len: usize) -> Vec<usize> {
            let mut indices = (0..len).collect::<Vec<_>>();
            for index in (1..indices.len()).rev() {
                let swap_index = self.next_usize(index + 1);
                indices.swap(index, swap_index);
            }
            indices
        }
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
    fn test_public_polynomial_commitments_match_trusted_dealer_shares() {
        let signer_ids = signer_ids(&[1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 20, u16::MAX]);
        for threshold in [1, 3, 8, 14] {
            let coefficients = (1..=threshold).map(bls_key).collect::<Vec<_>>();
            let keyset = trusted_dealer_keygen(&coefficients, &signer_ids).expect("keyset");
            let commitments = coefficients
                .iter()
                .map(BlsSecretKey::public_key_g2)
                .collect::<Vec<_>>();

            assert_eq!(
                aggregate_public_key_commitments(&[commitments[0]]).expect("aggregate commitment"),
                keyset.aggregate_public_key()
            );
            for (signer_id, trusted_share) in signer_ids.iter().zip(keyset.public_shares()) {
                let derived = evaluate_public_key_commitments(*signer_id, &commitments)
                    .expect("derived public share");
                assert_eq!(derived, *trusted_share);
                let mut power = Scalar::one();
                let mut reference = G2Projective::identity();
                for commitment in &commitments {
                    reference += G2Projective::from(commitment.point()) * power;
                    power *= signer_id.scalar();
                }
                assert_eq!(
                    derived,
                    BlsPublicKeyShare::new(
                        *signer_id,
                        g2_from_projective(reference).expect("power-sum reference"),
                    )
                );
            }
        }
    }

    #[cfg(feature = "federation")]
    #[test]
    fn test_public_signer_multiplication_matches_scalar_multiplication() {
        let mut ids = vec![1, u16::MAX];
        for bit in 1..u16::BITS {
            let power = 1_u16 << bit;
            ids.extend([power - 1, power, power + 1]);
        }
        for point in [
            G2Projective::identity(),
            G2Projective::generator(),
            -G2Projective::from(bls_key(7).public_key_g2().point()),
        ] {
            for id in &ids {
                let signer = BlsSignerId::new(*id).expect("nonzero boundary ID");
                assert_eq!(
                    multiply_by_public_signer_id(point, signer),
                    point * signer.scalar(),
                    "public ID {id}"
                );
            }
        }
    }

    #[cfg(feature = "federation")]
    #[test]
    fn test_public_polynomial_rejects_only_final_identity() {
        let signer = BlsSignerId::new(1).expect("signer");
        let positive = bls_key(7).public_key_g2();
        let negative = BlsG2PublicKey {
            point: -positive.point(),
        };
        assert!(matches!(
            evaluate_public_key_commitments(signer, &[]),
            Err(Error::InvalidPublicKey)
        ));
        assert!(matches!(
            evaluate_public_key_commitments(signer, &[positive, negative]),
            Err(Error::InvalidPublicKey)
        ));
        // A zero intermediate sum is valid if the final polynomial is nonzero.
        let share = evaluate_public_key_commitments(signer, &[positive, negative, positive])
            .expect("nonzero final polynomial");
        assert_eq!(share, BlsPublicKeyShare::new(signer, positive));
    }

    #[cfg(feature = "federation")]
    #[test]
    fn test_secret_polynomial_contributions_aggregate_to_public_commitments() {
        let first_coefficients = vec![bls_key(3), bls_key(5)];
        let second_coefficients = vec![bls_key(7), bls_key(11)];
        let first_commitments = first_coefficients
            .iter()
            .map(BlsSecretKey::public_key_g2)
            .collect::<Vec<_>>();
        let second_commitments = second_coefficients
            .iter()
            .map(BlsSecretKey::public_key_g2)
            .collect::<Vec<_>>();

        for signer_id in signer_ids(&[1, 2, 3]) {
            let first_share = evaluate_secret_key_coefficients(signer_id, &first_coefficients)
                .expect("first secret contribution");
            let second_share = evaluate_secret_key_coefficients(signer_id, &second_coefficients)
                .expect("second secret contribution");
            let aggregate_secret =
                aggregate_secret_key_shares(signer_id, &[first_share, second_share])
                    .expect("aggregate secret share");

            let first_public = evaluate_public_key_commitments(signer_id, &first_commitments)
                .expect("first public contribution");
            let second_public = evaluate_public_key_commitments(signer_id, &second_commitments)
                .expect("second public contribution");
            let aggregate_public = aggregate_public_key_commitments(&[
                first_public.public_key(),
                second_public.public_key(),
            ])
            .expect("aggregate public share");

            assert_eq!(
                aggregate_secret.public_key_share(),
                BlsPublicKeyShare::new(signer_id, aggregate_public)
            );
        }
    }

    #[cfg(feature = "federation")]
    #[test]
    fn test_secret_share_aggregation_rejects_missing_or_wrong_signer() {
        let signer_one = BlsSignerId::new(1).expect("signer one");
        let signer_two = BlsSignerId::new(2).expect("signer two");
        let share_one =
            evaluate_secret_key_coefficients(signer_one, &[bls_key(3)]).expect("share one");
        let share_two =
            evaluate_secret_key_coefficients(signer_two, &[bls_key(5)]).expect("share two");

        assert!(matches!(
            evaluate_secret_key_coefficients(signer_one, &[]),
            Err(Error::InsufficientThresholdShares {
                threshold: 1,
                found: 0
            })
        ));
        assert!(matches!(
            aggregate_secret_key_shares(signer_one, &[]),
            Err(Error::InsufficientThresholdShares {
                threshold: 1,
                found: 0
            })
        ));
        assert!(matches!(
            aggregate_secret_key_shares(signer_one, &[share_one, share_two]),
            Err(Error::ThresholdSignerMismatch {
                expected: 1,
                found: 2
            })
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
    fn test_batch_verify_blind_signature_shares_matches_scalar_verification() {
        let coefficients = vec![bls_key(3), bls_key(5), bls_key(9)];
        let keyset =
            trusted_dealer_keygen(&coefficients, &signer_ids(&[1, 2, 3, 4, 5])).expect("keyset");
        let blinded_messages = (0..14)
            .map(|index| BlsG1PublicKey::hash_to_curve(format!("batch output {index}").as_bytes()))
            .collect::<Vec<_>>();
        let shares = blinded_messages
            .iter()
            .map(|message| keyset.secret_shares()[0].blind_sign_share(message))
            .collect::<Vec<_>>();
        let public_shares = vec![keyset.public_shares()[0]; blinded_messages.len()];

        assert!(
            batch_verify_blind_signature_shares(&shares, &blinded_messages, &public_shares)
                .expect("batch verification")
        );
        for ((share, message), public_share) in
            shares.iter().zip(&blinded_messages).zip(&public_shares)
        {
            assert!(verify_blind_signature_share(share, message, public_share)
                .expect("scalar verification"));
        }
    }

    #[cfg(feature = "federation")]
    #[test]
    fn test_batch_verify_blind_signature_shares_rejects_corruption_and_bad_shape() {
        let coefficients = vec![bls_key(11), bls_key(13), bls_key(17)];
        let keyset =
            trusted_dealer_keygen(&coefficients, &signer_ids(&[1, 2, 3, 4, 5])).expect("keyset");
        let blinded_messages = [
            BlsG1PublicKey::hash_to_curve(b"batch output one"),
            BlsG1PublicKey::hash_to_curve(b"batch output two"),
            BlsG1PublicKey::hash_to_curve(b"batch output three"),
        ];
        let mut shares = blinded_messages
            .iter()
            .map(|message| keyset.secret_shares()[0].blind_sign_share(message))
            .collect::<Vec<_>>();
        let public_shares = vec![keyset.public_shares()[0]; blinded_messages.len()];

        shares[1] = keyset.secret_shares()[0].blind_sign_share(&blinded_messages[0]);
        assert!(
            !batch_verify_blind_signature_shares(&shares, &blinded_messages, &public_shares)
                .expect("batch corruption verdict")
        );
        assert!(!batch_verify_blind_signature_shares(
            &shares[..2],
            &blinded_messages,
            &public_shares
        )
        .expect("batch length verdict"));

        let mismatched_public_shares = vec![keyset.public_shares()[1]; blinded_messages.len()];
        assert!(matches!(
            batch_verify_blind_signature_shares(
                &shares,
                &blinded_messages,
                &mismatched_public_shares
            ),
            Err(Error::ThresholdSignerMismatch {
                expected: 2,
                found: 1
            })
        ));
    }

    #[cfg(feature = "federation")]
    #[test]
    fn test_threshold_bls_four_of_seven_aggregates_valid_quorums() {
        let coefficients = vec![bls_key(17), bls_key(19), bls_key(23), bls_key(29)];
        let keyset = trusted_dealer_keygen(&coefficients, &signer_ids(&[1, 2, 3, 4, 5, 6, 7]))
            .expect("keyset");
        let blinded_message = BlsG1PublicKey::hash_to_curve(b"threshold four of seven");

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

        let subset_a = [shares[0], shares[2], shares[4], shares[6]];
        let subset_b = [shares[1], shares[3], shares[5], shares[6]];
        let aggregate_a =
            aggregate_blind_signature_shares(keyset.threshold(), &subset_a).expect("signature");
        let aggregate_b =
            aggregate_blind_signature_shares(keyset.threshold(), &subset_b).expect("signature");
        let direct_signature = blinded_message.mul(&coefficients[0]);

        assert_eq!(keyset.threshold(), 4);
        assert_eq!(aggregate_a, direct_signature);
        assert_eq!(aggregate_b, direct_signature);
        assert_eq!(aggregate_a, aggregate_b);
        assert!(verify_blind_signature_pairing(
            &aggregate_a,
            &blinded_message,
            &keyset.aggregate_public_key()
        ));

        let public_subset = [
            keyset.public_shares()[0],
            keyset.public_shares()[2],
            keyset.public_shares()[4],
            keyset.public_shares()[6],
        ];
        assert_eq!(
            aggregate_public_key_shares(keyset.threshold(), &public_subset)
                .expect("aggregate public key"),
            keyset.aggregate_public_key()
        );
    }

    #[cfg(feature = "federation")]
    #[test]
    fn test_threshold_bls_subsets_and_order_permutations_match() {
        let cases = [
            (1usize, vec![bls_key(2)], vec![1u16, 2, 3]),
            (2usize, vec![bls_key(3), bls_key(5)], vec![1u16, 2, 3, 4]),
            (
                3usize,
                vec![bls_key(7), bls_key(11), bls_key(13)],
                vec![1u16, 2, 3, 4, 5],
            ),
            (
                4usize,
                vec![bls_key(17), bls_key(19), bls_key(23), bls_key(29)],
                vec![1u16, 2, 3, 4, 5, 6],
            ),
        ];

        for (threshold, coefficients, ids) in cases {
            let keyset = trusted_dealer_keygen(&coefficients, &signer_ids(&ids)).expect("keyset");
            assert_eq!(keyset.threshold(), threshold);

            let blinded_message =
                BlsG1PublicKey::hash_to_curve(format!("subset threshold {threshold}").as_bytes());
            let shares = keyset
                .secret_shares()
                .iter()
                .map(|share| share.blind_sign_share(&blinded_message))
                .collect::<Vec<_>>();
            let direct_signature = blinded_message.mul(&coefficients[0]);

            for start in 0..=(shares.len() - threshold) {
                let subset = shares[start..start + threshold].to_vec();
                let mut reversed_subset = subset.clone();
                reversed_subset.reverse();

                let aggregate =
                    aggregate_blind_signature_shares(threshold, &subset).expect("signature");
                let reversed_aggregate =
                    aggregate_blind_signature_shares(threshold, &reversed_subset)
                        .expect("signature");

                assert_eq!(aggregate, direct_signature);
                assert_eq!(reversed_aggregate, direct_signature);
                assert_eq!(aggregate, reversed_aggregate);
            }

            let public_subset = keyset.public_shares()[..threshold].to_vec();
            let mut reversed_public_subset = public_subset.clone();
            reversed_public_subset.reverse();
            assert_eq!(
                aggregate_public_key_shares(threshold, &public_subset)
                    .expect("aggregate public key"),
                keyset.aggregate_public_key()
            );
            assert_eq!(
                aggregate_public_key_shares(threshold, &reversed_public_subset)
                    .expect("aggregate public key"),
                keyset.aggregate_public_key()
            );
        }
    }

    #[cfg(feature = "federation")]
    #[test]
    fn test_threshold_bls_seeded_random_messages_thresholds_and_subsets() {
        let mut rng = DeterministicTestRng::new(0x0c_a5_1f_ed_b1_51_70_01);

        for case_index in 0..16u8 {
            let member_count = 2 + rng.next_usize(7);
            let threshold = 1 + rng.next_usize(member_count);
            let coefficients = (0..threshold)
                .map(|_| rng.bls_secret_key())
                .collect::<Vec<_>>();
            let signer_id_values = (1..=member_count as u16).collect::<Vec<_>>();
            let keyset = trusted_dealer_keygen(&coefficients, &signer_ids(&signer_id_values))
                .expect("threshold keyset");

            let mut message = [0u8; 40];
            rng.fill_bytes(&mut message);
            message[0] = case_index;
            let blinded_message = BlsG1PublicKey::hash_to_curve(&message);
            let direct_signature = blinded_message.mul(&coefficients[0]);
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

            for _ in 0..3 {
                let subset_indices = rng.shuffled_indices(member_count);
                let share_subset = subset_indices
                    .iter()
                    .take(threshold)
                    .map(|index| shares[*index])
                    .collect::<Vec<_>>();
                let public_subset = subset_indices
                    .iter()
                    .take(threshold)
                    .map(|index| keyset.public_shares()[*index])
                    .collect::<Vec<_>>();

                let aggregate = aggregate_blind_signature_shares(threshold, &share_subset)
                    .expect("aggregate signature");
                let aggregate_public_key = aggregate_public_key_shares(threshold, &public_subset)
                    .expect("aggregate public key");

                let mut reversed_share_subset = share_subset.clone();
                reversed_share_subset.reverse();
                let reversed_aggregate =
                    aggregate_blind_signature_shares(threshold, &reversed_share_subset)
                        .expect("reversed aggregate signature");

                assert_eq!(aggregate, direct_signature);
                assert_eq!(reversed_aggregate, direct_signature);
                assert_eq!(aggregate_public_key, keyset.aggregate_public_key());
                assert!(verify_blind_signature_pairing(
                    &aggregate,
                    &blinded_message,
                    &keyset.aggregate_public_key()
                ));
            }
        }
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
        let g1_json = serde_json::to_string(&blinded_message).expect("g1 json");
        let g2_json =
            serde_json::to_string(&keyset.aggregate_public_key()).expect("aggregate key json");
        let secret_share_json = serde_json::to_string(secret_share).expect("secret share json");
        let public_share_json = serde_json::to_string(&public_share).expect("public share json");
        let signature_share_json =
            serde_json::to_string(&signature_share).expect("signature share json");
        assert_eq!(
            serde_json::from_str::<BlsG1PublicKey>(&g1_json).expect("g1 from json"),
            blinded_message
        );
        assert_eq!(
            serde_json::from_str::<BlsG2PublicKey>(&g2_json).expect("g2 from json"),
            keyset.aggregate_public_key()
        );
        assert_eq!(
            serde_json::from_str::<BlsSecretKeyShare>(&secret_share_json)
                .expect("secret share from json"),
            *secret_share
        );
        assert_eq!(
            serde_json::from_str::<BlsPublicKeyShare>(&public_share_json)
                .expect("public share from json"),
            public_share
        );
        assert_eq!(
            serde_json::from_str::<BlsBlindSignatureShare>(&signature_share_json)
                .expect("signature share from json"),
            signature_share
        );
        assert!(matches!(
            BlsBlindSignatureShare::from_bytes(&signature_share.to_bytes()[..50]),
            Err(Error::InvalidThresholdShareSize {
                expected: 51,
                found: 50
            })
        ));
        let mut bad_version = signature_share.to_bytes();
        bad_version[0] = 2;
        assert!(matches!(
            BlsBlindSignatureShare::from_bytes(&bad_version),
            Err(Error::InvalidThresholdShareVersion {
                expected: 1,
                found: 2
            })
        ));
    }

    #[cfg(feature = "federation")]
    #[test]
    fn test_threshold_bls_secret_share_zeroize_clears_secret_material() {
        let coefficients = vec![bls_key(31), bls_key(37), bls_key(41)];
        let keyset =
            trusted_dealer_keygen(&coefficients, &signer_ids(&[1, 2, 3, 4, 5])).expect("keyset");
        let mut secret_share = keyset.secret_shares()[0].clone();
        let signer_id = secret_share.signer_id();

        assert_ne!(secret_share.to_secret_bytes(), [0u8; 32]);

        secret_share.zeroize();

        assert_eq!(secret_share.signer_id(), signer_id);
        assert_eq!(secret_share.to_secret_bytes(), [0u8; 32]);
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
