//! NUT-00: Notation and Models
//!
//! <https://github.com/cashubtc/nuts/blob/main/00.md>

use std::cmp::Ordering;
use std::collections::{HashMap, HashSet};
use std::fmt;
use std::hash::{Hash, Hasher};
use std::str::FromStr;
use std::string::FromUtf8Error;

use serde::{de, Deserialize, Deserializer, Serialize};
use thiserror::Error;

use super::nut02::ShortKeysetId;
#[cfg(feature = "wallet")]
use super::nut10;
#[cfg(feature = "wallet")]
use super::nut11::SpendingConditions;
#[cfg(feature = "wallet")]
use crate::amount::FeeAndAmounts;
#[cfg(feature = "wallet")]
use crate::amount::SplitTarget;
#[cfg(feature = "wallet")]
use crate::dhke::blind_message;
use crate::dhke::hash_to_curve;
use crate::nuts::nut01::PublicKey;
#[cfg(feature = "wallet")]
use crate::nuts::nut01::SecretKey;
use crate::nuts::nut11::{serde_p2pk_witness, P2PKWitness};
use crate::nuts::nut12::BlindSignatureDleq;
use crate::nuts::nut14::{serde_htlc_witness, HTLCWitness};
use crate::nuts::{Id, ProofDleq};
use crate::secret::Secret;
use crate::Amount;

pub mod token;
pub use token::{Token, TokenV3, TokenV4};

/// List of [Proof]
pub type Proofs = Vec<Proof>;

/// Utility methods for [Proofs]
pub trait ProofsMethods {
    /// Count proofs by keyset
    fn count_by_keyset(&self) -> HashMap<Id, u64>;

    /// Sum proofs by keyset
    fn sum_by_keyset(&self) -> HashMap<Id, Amount>;

    /// Try to sum up the amounts of all [Proof]s
    fn total_amount(&self) -> Result<Amount, Error>;

    /// Try to fetch the pubkeys of all [Proof]s
    fn ys(&self) -> Result<Vec<PublicKey>, Error>;

    /// Create a copy of proofs without dleqs
    fn without_dleqs(&self) -> Proofs;
}

impl ProofsMethods for Proofs {
    fn count_by_keyset(&self) -> HashMap<Id, u64> {
        count_by_keyset(self.iter())
    }

    fn sum_by_keyset(&self) -> HashMap<Id, Amount> {
        sum_by_keyset(self.iter())
    }

    fn total_amount(&self) -> Result<Amount, Error> {
        total_amount(self.iter())
    }

    fn ys(&self) -> Result<Vec<PublicKey>, Error> {
        ys(self.iter())
    }

    fn without_dleqs(&self) -> Proofs {
        self.iter()
            .map(|p| {
                let mut p = p.clone();
                p.dleq = None;
                p
            })
            .collect()
    }
}

impl ProofsMethods for HashSet<Proof> {
    fn count_by_keyset(&self) -> HashMap<Id, u64> {
        count_by_keyset(self.iter())
    }

    fn sum_by_keyset(&self) -> HashMap<Id, Amount> {
        sum_by_keyset(self.iter())
    }

    fn total_amount(&self) -> Result<Amount, Error> {
        total_amount(self.iter())
    }

    fn ys(&self) -> Result<Vec<PublicKey>, Error> {
        ys(self.iter())
    }

    fn without_dleqs(&self) -> Proofs {
        self.iter()
            .map(|p| {
                let mut p = p.clone();
                p.dleq = None;
                p
            })
            .collect()
    }
}

fn count_by_keyset<'a, I: Iterator<Item = &'a Proof>>(proofs: I) -> HashMap<Id, u64> {
    let mut counts = HashMap::new();
    for proof in proofs {
        *counts.entry(proof.keyset_id).or_insert(0) += 1;
    }
    counts
}

fn sum_by_keyset<'a, I: Iterator<Item = &'a Proof>>(proofs: I) -> HashMap<Id, Amount> {
    let mut sums = HashMap::new();
    for proof in proofs {
        *sums.entry(proof.keyset_id).or_insert(Amount::ZERO) += proof.amount;
    }
    sums
}

fn total_amount<'a, I: Iterator<Item = &'a Proof>>(proofs: I) -> Result<Amount, Error> {
    Amount::try_sum(proofs.map(|p| p.amount)).map_err(Into::into)
}

fn ys<'a, I: Iterator<Item = &'a Proof>>(proofs: I) -> Result<Vec<PublicKey>, Error> {
    proofs.map(|p| p.y()).collect::<Result<Vec<PublicKey>, _>>()
}

/// NUT00 Error
#[derive(Debug, Error)]
pub enum Error {
    /// Proofs required
    #[error("Proofs required in token")]
    ProofsRequired,
    /// Unsupported token
    #[error("Unsupported token")]
    UnsupportedToken,
    /// Unsupported token
    #[error("Unsupported unit")]
    UnsupportedUnit,
    /// Unsupported token
    #[error("Unsupported payment method")]
    UnsupportedPaymentMethod,
    /// Duplicate proofs in token
    #[error("Duplicate proofs in token")]
    DuplicateProofs,
    /// Serde Json error
    #[error(transparent)]
    SerdeJsonError(#[from] serde_json::Error),
    /// Utf8 parse error
    #[error(transparent)]
    Utf8ParseError(#[from] FromUtf8Error),
    /// Base64 error
    #[error(transparent)]
    Base64Error(#[from] bitcoin::base64::DecodeError),
    /// Ciborium deserialization error
    #[error(transparent)]
    CiboriumError(#[from] ciborium::de::Error<std::io::Error>),
    /// Ciborium serialization error
    #[error(transparent)]
    CiboriumSerError(#[from] ciborium::ser::Error<std::io::Error>),
    /// Amount Error
    #[error(transparent)]
    Amount(#[from] crate::amount::Error),
    /// Secret error
    #[error(transparent)]
    Secret(#[from] crate::secret::Error),
    /// DHKE error
    #[error(transparent)]
    DHKE(#[from] crate::dhke::Error),
    /// NUT10 error
    #[error(transparent)]
    NUT10(#[from] crate::nuts::nut10::Error),
    /// NUT11 error
    #[error(transparent)]
    NUT11(#[from] crate::nuts::nut11::Error),
    /// Short keyset id -> id error
    #[error(transparent)]
    NUT02(#[from] crate::nuts::nut02::Error),
}

/// Blinded Message (also called `output`)
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "swagger", derive(utoipa::ToSchema))]
pub struct BlindedMessage {
    /// Amount
    ///
    /// The value for the requested [BlindSignature]
    pub amount: Amount,
    /// Keyset ID
    ///
    /// ID from which we expect a signature.
    #[serde(rename = "id")]
    #[cfg_attr(feature = "swagger", schema(value_type = String))]
    pub keyset_id: Id,
    /// Blinded secret message (B_)
    ///
    /// The blinded secret message generated by the sender.
    #[serde(rename = "B_")]
    #[cfg_attr(feature = "swagger", schema(value_type = String))]
    pub blinded_secret: PublicKey,
    /// Witness
    ///
    /// <https://github.com/cashubtc/nuts/blob/main/11.md>
    #[serde(skip_serializing_if = "Option::is_none")]
    pub witness: Option<Witness>,
}

impl BlindedMessage {
    /// Compose new blinded message
    #[inline]
    pub fn new(amount: Amount, keyset_id: Id, blinded_secret: PublicKey) -> Self {
        Self {
            amount,
            keyset_id,
            blinded_secret,
            witness: None,
        }
    }

    /// Add witness
    #[inline]
    pub fn witness(&mut self, witness: Witness) {
        self.witness = Some(witness);
    }
}

/// Blind Signature (also called `promise`)
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "swagger", derive(utoipa::ToSchema))]
pub struct BlindSignature {
    /// Amount
    ///
    /// The value of the blinded token.
    pub amount: Amount,
    /// Keyset ID
    ///
    /// ID of the mint keys that signed the token.
    #[serde(rename = "id")]
    #[cfg_attr(feature = "swagger", schema(value_type = String))]
    pub keyset_id: Id,
    /// Blinded signature (C_)
    ///
    /// The blinded signature on the secret message `B_` of [BlindedMessage].
    #[serde(rename = "C_")]
    #[cfg_attr(feature = "swagger", schema(value_type = String))]
    pub c: PublicKey,
    /// DLEQ Proof
    ///
    /// <https://github.com/cashubtc/nuts/blob/main/12.md>
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dleq: Option<BlindSignatureDleq>,
}

impl Ord for BlindSignature {
    fn cmp(&self, other: &Self) -> Ordering {
        self.amount.cmp(&other.amount)
    }
}

impl PartialOrd for BlindSignature {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

/// Witness
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(untagged)]
#[cfg_attr(feature = "swagger", derive(utoipa::ToSchema))]
pub enum Witness {
    /// HTLC Witness
    #[serde(with = "serde_htlc_witness")]
    HTLCWitness(HTLCWitness),
    /// P2PK Witness
    #[serde(with = "serde_p2pk_witness")]
    P2PKWitness(P2PKWitness),
}

impl From<P2PKWitness> for Witness {
    fn from(witness: P2PKWitness) -> Self {
        Self::P2PKWitness(witness)
    }
}

impl From<HTLCWitness> for Witness {
    fn from(witness: HTLCWitness) -> Self {
        Self::HTLCWitness(witness)
    }
}

impl Witness {
    /// Add signatures to [`Witness`]
    pub fn add_signatures(&mut self, signatues: Vec<String>) {
        match self {
            Self::P2PKWitness(p2pk_witness) => p2pk_witness.signatures.extend(signatues),
            Self::HTLCWitness(htlc_witness) => {
                htlc_witness.signatures = htlc_witness.signatures.clone().map(|sigs| {
                    let mut sigs = sigs;
                    sigs.extend(signatues);
                    sigs
                });
            }
        }
    }

    /// Get signatures on [`Witness`]
    pub fn signatures(&self) -> Option<Vec<String>> {
        match self {
            Self::P2PKWitness(witness) => Some(witness.signatures.clone()),
            Self::HTLCWitness(witness) => witness.signatures.clone(),
        }
    }

    /// Get preimage from [`Witness`]
    pub fn preimage(&self) -> Option<String> {
        match self {
            Self::P2PKWitness(_witness) => None,
            Self::HTLCWitness(witness) => Some(witness.preimage.clone()),
        }
    }
}

/// Proofs
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "swagger", derive(utoipa::ToSchema))]
pub struct Proof {
    /// Amount
    pub amount: Amount,
    /// `Keyset id`
    #[serde(rename = "id")]
    #[cfg_attr(feature = "swagger", schema(value_type = String))]
    pub keyset_id: Id,
    /// Secret message
    #[cfg_attr(feature = "swagger", schema(value_type = String))]
    pub secret: Secret,
    /// Unblinded signature
    #[serde(rename = "C")]
    #[cfg_attr(feature = "swagger", schema(value_type = String))]
    pub c: PublicKey,
    /// Witness
    #[serde(skip_serializing_if = "Option::is_none")]
    pub witness: Option<Witness>,
    /// DLEQ Proof
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dleq: Option<ProofDleq>,
}

impl Proof {
    /// Create new [`Proof`]
    pub fn new(amount: Amount, keyset_id: Id, secret: Secret, c: PublicKey) -> Self {
        Proof {
            amount,
            keyset_id,
            secret,
            c,
            witness: None,
            dleq: None,
        }
    }

    /// Check if proof is in active keyset `Id`s
    pub fn is_active(&self, active_keyset_ids: &[Id]) -> bool {
        active_keyset_ids.contains(&self.keyset_id)
    }

    /// Get y from proof
    ///
    /// Where y is `hash_to_curve(secret)`
    pub fn y(&self) -> Result<PublicKey, Error> {
        Ok(hash_to_curve(self.secret.as_bytes())?)
    }
}

impl Hash for Proof {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.secret.hash(state);
    }
}

impl Ord for Proof {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.amount.cmp(&other.amount)
    }
}

impl PartialOrd for Proof {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

/// Proof V4
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProofV4 {
    /// Amount in satoshi
    #[serde(rename = "a")]
    pub amount: Amount,
    /// Secret message
    #[serde(rename = "s")]
    pub secret: Secret,
    /// Unblinded signature
    #[serde(
        serialize_with = "serialize_v4_pubkey",
        deserialize_with = "deserialize_v4_pubkey"
    )]
    pub c: PublicKey,
    /// Witness
    #[serde(default)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub witness: Option<Witness>,
    /// DLEQ Proof
    #[serde(rename = "d")]
    pub dleq: Option<ProofDleq>,
}

impl ProofV4 {
    /// [`ProofV4`] into [`Proof`]
    pub fn into_proof(&self, keyset_id: &Id) -> Proof {
        Proof {
            amount: self.amount,
            keyset_id: *keyset_id,
            secret: self.secret.clone(),
            c: self.c,
            witness: self.witness.clone(),
            dleq: self.dleq.clone(),
        }
    }
}

impl Hash for ProofV4 {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.secret.hash(state);
    }
}

impl From<Proof> for ProofV4 {
    fn from(proof: Proof) -> ProofV4 {
        let Proof {
            amount,
            keyset_id: _,
            secret,
            c,
            witness,
            dleq,
        } = proof;
        ProofV4 {
            amount,
            secret,
            c,
            witness,
            dleq,
        }
    }
}

impl From<ProofV3> for ProofV4 {
    fn from(proof: ProofV3) -> Self {
        Self {
            amount: proof.amount,
            secret: proof.secret,
            c: proof.c,
            witness: proof.witness,
            dleq: proof.dleq,
        }
    }
}

/// Proof v3 with short keyset id
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProofV3 {
    /// Amount
    pub amount: Amount,
    /// Short keyset id
    #[serde(rename = "id")]
    pub keyset_id: ShortKeysetId,
    /// Secret message
    pub secret: Secret,
    /// Unblinded signature
    #[serde(rename = "C")]
    pub c: PublicKey,
    /// Witness
    #[serde(skip_serializing_if = "Option::is_none")]
    pub witness: Option<Witness>,
    /// DLEQ Proof
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dleq: Option<ProofDleq>,
}

impl ProofV3 {
    /// [`ProofV3`] into [`Proof`]
    pub fn into_proof(&self, keyset_id: &Id) -> Proof {
        Proof {
            amount: self.amount,
            keyset_id: *keyset_id,
            secret: self.secret.clone(),
            c: self.c,
            witness: self.witness.clone(),
            dleq: self.dleq.clone(),
        }
    }
}

impl From<Proof> for ProofV3 {
    fn from(proof: Proof) -> ProofV3 {
        let Proof {
            amount,
            keyset_id,
            secret,
            c,
            witness,
            dleq,
        } = proof;
        ProofV3 {
            amount,
            secret,
            c,
            witness,
            dleq,
            keyset_id: keyset_id.into(),
        }
    }
}

impl Hash for ProofV3 {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.secret.hash(state);
    }
}

fn serialize_v4_pubkey<S>(key: &PublicKey, serializer: S) -> Result<S::Ok, S::Error>
where
    S: serde::Serializer,
{
    serializer.serialize_bytes(&key.to_bytes())
}

fn deserialize_v4_pubkey<'de, D>(deserializer: D) -> Result<PublicKey, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let bytes = Vec::<u8>::deserialize(deserializer)?;
    PublicKey::from_slice(&bytes).map_err(serde::de::Error::custom)
}

/// Payment Method
#[derive(Debug, Clone, Default, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "swagger", derive(utoipa::ToSchema))]
pub enum PaymentMethod {
    /// Bolt11 payment type
    #[default]
    Bolt11,
    /// Bolt12
    Bolt12,
    /// Custom
    Custom(String),
}

impl FromStr for PaymentMethod {
    type Err = Error;
    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value.to_lowercase().as_str() {
            "bolt11" => Ok(Self::Bolt11),
            "bolt12" => Ok(Self::Bolt12),
            c => Ok(Self::Custom(c.to_string())),
        }
    }
}

impl fmt::Display for PaymentMethod {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PaymentMethod::Bolt11 => write!(f, "bolt11"),
            PaymentMethod::Bolt12 => write!(f, "bolt12"),
            PaymentMethod::Custom(p) => write!(f, "{p}"),
        }
    }
}

impl Serialize for PaymentMethod {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(&self.to_string())
    }
}

impl<'de> Deserialize<'de> for PaymentMethod {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let payment_method: String = String::deserialize(deserializer)?;
        Self::from_str(&payment_method).map_err(|_| de::Error::custom("Unsupported payment method"))
    }
}

/// PreMint
#[cfg(feature = "wallet")]
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PreMint {
    /// Blinded message
    pub blinded_message: BlindedMessage,
    /// Secret
    pub secret: Secret,
    /// R
    pub r: SecretKey,
    /// Amount
    pub amount: Amount,
}

#[cfg(feature = "wallet")]
impl Ord for PreMint {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.amount.cmp(&other.amount)
    }
}

#[cfg(feature = "wallet")]
impl PartialOrd for PreMint {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

/// Premint Secrets
#[cfg(feature = "wallet")]
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PreMintSecrets {
    /// Secrets
    pub secrets: Vec<PreMint>,
    /// Keyset Id
    pub keyset_id: Id,
}

#[cfg(feature = "wallet")]
impl PreMintSecrets {
    /// Create new [`PreMintSecrets`]
    pub fn new(keyset_id: Id) -> Self {
        Self {
            secrets: Vec::new(),
            keyset_id,
        }
    }

    /// Outputs for speceifed amount with random secret
    pub fn random(
        keyset_id: Id,
        amount: Amount,
        amount_split_target: &SplitTarget,
        fee_and_amounts: &FeeAndAmounts,
    ) -> Result<Self, Error> {
        let amount_split = amount.split_targeted(amount_split_target, fee_and_amounts)?;

        let mut output = Vec::with_capacity(amount_split.len());

        for amount in amount_split {
            let secret = Secret::generate();
            let (blinded, r) = blind_message(&secret.to_bytes(), None)?;

            let blinded_message = BlindedMessage::new(amount, keyset_id, blinded);

            output.push(PreMint {
                secret,
                blinded_message,
                r,
                amount,
            });
        }

        Ok(PreMintSecrets {
            secrets: output,
            keyset_id,
        })
    }

    /// Outputs from pre defined secrets
    pub fn from_secrets(
        keyset_id: Id,
        amounts: Vec<Amount>,
        secrets: Vec<Secret>,
    ) -> Result<Self, Error> {
        let mut output = Vec::with_capacity(secrets.len());

        for (secret, amount) in secrets.into_iter().zip(amounts) {
            let (blinded, r) = blind_message(&secret.to_bytes(), None)?;

            let blinded_message = BlindedMessage::new(amount, keyset_id, blinded);

            output.push(PreMint {
                secret,
                blinded_message,
                r,
                amount,
            });
        }

        Ok(PreMintSecrets {
            secrets: output,
            keyset_id,
        })
    }

    /// Blank Outputs used for NUT-08 change
    pub fn blank(keyset_id: Id, fee_reserve: Amount) -> Result<Self, Error> {
        let count = ((u64::from(fee_reserve) as f64).log2().ceil() as u64).max(1);

        let mut output = Vec::with_capacity(count as usize);

        for _i in 0..count {
            let secret = Secret::generate();
            let (blinded, r) = blind_message(&secret.to_bytes(), None)?;

            let blinded_message = BlindedMessage::new(Amount::ZERO, keyset_id, blinded);

            output.push(PreMint {
                secret,
                blinded_message,
                r,
                amount: Amount::ZERO,
            })
        }

        Ok(PreMintSecrets {
            secrets: output,
            keyset_id,
        })
    }

    /// Outputs with specific spending conditions
    pub fn with_conditions(
        keyset_id: Id,
        amount: Amount,
        amount_split_target: &SplitTarget,
        conditions: &SpendingConditions,
        fee_and_amounts: &FeeAndAmounts,
    ) -> Result<Self, Error> {
        let amount_split = amount.split_targeted(amount_split_target, fee_and_amounts)?;

        let mut output = Vec::with_capacity(amount_split.len());

        for amount in amount_split {
            let secret: nut10::Secret = conditions.clone().into();

            let secret: Secret = secret.try_into()?;
            let (blinded, r) = blind_message(&secret.to_bytes(), None)?;

            let blinded_message = BlindedMessage::new(amount, keyset_id, blinded);

            output.push(PreMint {
                secret,
                blinded_message,
                r,
                amount,
            });
        }

        Ok(PreMintSecrets {
            secrets: output,
            keyset_id,
        })
    }

    /// Iterate over secrets
    #[inline]
    pub fn iter(&self) -> impl Iterator<Item = &PreMint> {
        self.secrets.iter()
    }

    /// Length of secrets
    #[inline]
    pub fn len(&self) -> usize {
        self.secrets.len()
    }

    /// If secrets is empty
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.secrets.is_empty()
    }

    /// Totoal amount of secrets
    pub fn total_amount(&self) -> Result<Amount, Error> {
        Ok(Amount::try_sum(
            self.secrets.iter().map(|PreMint { amount, .. }| *amount),
        )?)
    }

    /// [`BlindedMessage`]s from [`PreMintSecrets`]
    #[inline]
    pub fn blinded_messages(&self) -> Vec<BlindedMessage> {
        self.iter().map(|pm| pm.blinded_message.clone()).collect()
    }

    /// [`Secret`]s from [`PreMintSecrets`]
    #[inline]
    pub fn secrets(&self) -> Vec<Secret> {
        self.iter().map(|pm| pm.secret.clone()).collect()
    }

    /// Blinding factor from [`PreMintSecrets`]
    #[inline]
    pub fn rs(&self) -> Vec<SecretKey> {
        self.iter().map(|pm| pm.r.clone()).collect()
    }

    /// Amounts from [`PreMintSecrets`]
    #[inline]
    pub fn amounts(&self) -> Vec<Amount> {
        self.iter().map(|pm| pm.amount).collect()
    }

    /// Combine [`PreMintSecrets`]
    #[inline]
    pub fn combine(&mut self, mut other: Self) {
        self.secrets.append(&mut other.secrets)
    }

    /// Sort [`PreMintSecrets`] by [`Amount`]
    #[inline]
    pub fn sort_secrets(&mut self) {
        self.secrets.sort();
    }
}

// Implement Iterator for PreMintSecrets
#[cfg(feature = "wallet")]
impl Iterator for PreMintSecrets {
    type Item = PreMint;

    fn next(&mut self) -> Option<Self::Item> {
        // Use the iterator of the vector
        self.secrets.pop()
    }
}

#[cfg(feature = "wallet")]
impl Ord for PreMintSecrets {
    fn cmp(&self, other: &Self) -> Ordering {
        self.secrets.cmp(&other.secrets)
    }
}

#[cfg(feature = "wallet")]
impl PartialOrd for PreMintSecrets {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

#[cfg(test)]
mod tests {
    use std::str::FromStr;

    use super::*;

    #[test]
    fn test_proof_serialize() {
        let proof = "[{\"id\":\"009a1f293253e41e\",\"amount\":2,\"secret\":\"407915bc212be61a77e3e6d2aeb4c727980bda51cd06a6afc29e2861768a7837\",\"C\":\"02bc9097997d81afb2cc7346b5e4345a9346bd2a506eb7958598a72f0cf85163ea\"},{\"id\":\"009a1f293253e41e\",\"amount\":8,\"secret\":\"fe15109314e61d7756b0f8ee0f23a624acaa3f4e042f61433c728c7057b931be\",\"C\":\"029e8e5050b890a7d6c0968db16bc1d5d5fa040ea1de284f6ec69d61299f671059\"}]";
        let proof: Proofs = serde_json::from_str(proof).unwrap();

        assert_eq!(
            proof[0].clone().keyset_id,
            Id::from_str("009a1f293253e41e").unwrap()
        );

        assert_eq!(proof.len(), 2);
    }

    #[test]
    #[cfg(feature = "wallet")]
    fn test_blank_blinded_messages() {
        let b = PreMintSecrets::blank(
            Id::from_str("009a1f293253e41e").unwrap(),
            Amount::from(1000),
        )
        .unwrap();
        assert_eq!(b.len(), 10);

        let b = PreMintSecrets::blank(Id::from_str("009a1f293253e41e").unwrap(), Amount::from(1))
            .unwrap();
        assert_eq!(b.len(), 1);
    }

    #[test]
    fn test_payment_method_parsing() {
        // Test standard variants
        assert_eq!(
            PaymentMethod::from_str("bolt11").unwrap(),
            PaymentMethod::Bolt11
        );
        assert_eq!(
            PaymentMethod::from_str("BOLT11").unwrap(),
            PaymentMethod::Bolt11
        );
        assert_eq!(
            PaymentMethod::from_str("Bolt11").unwrap(),
            PaymentMethod::Bolt11
        );

        assert_eq!(
            PaymentMethod::from_str("bolt12").unwrap(),
            PaymentMethod::Bolt12
        );
        assert_eq!(
            PaymentMethod::from_str("BOLT12").unwrap(),
            PaymentMethod::Bolt12
        );
        assert_eq!(
            PaymentMethod::from_str("Bolt12").unwrap(),
            PaymentMethod::Bolt12
        );

        // Test custom variants
        assert_eq!(
            PaymentMethod::from_str("custom").unwrap(),
            PaymentMethod::Custom("custom".to_string())
        );
        assert_eq!(
            PaymentMethod::from_str("CUSTOM").unwrap(),
            PaymentMethod::Custom("custom".to_string())
        );

        // Test serialization/deserialization consistency
        let methods = vec![
            PaymentMethod::Bolt11,
            PaymentMethod::Bolt12,
            PaymentMethod::Custom("test".to_string()),
        ];

        for method in methods {
            let serialized = serde_json::to_string(&method).unwrap();
            let deserialized: PaymentMethod = serde_json::from_str(&serialized).unwrap();
            assert_eq!(method, deserialized);
        }
    }

    #[test]
    fn test_witness_serialization() {
        let htlc_witness = HTLCWitness {
            preimage: "preimage".to_string(),
            signatures: Some(vec!["sig1".to_string()]),
        };
        let witness = Witness::HTLCWitness(htlc_witness);

        let serialized = serde_json::to_string(&witness).unwrap();
        let deserialized: Witness = serde_json::from_str(&serialized).unwrap();

        assert!(matches!(deserialized, Witness::HTLCWitness(_)));

        let p2pk_witness = P2PKWitness {
            signatures: vec!["sig1".to_string(), "sig2".to_string()],
        };
        let witness = Witness::P2PKWitness(p2pk_witness);

        let serialized = serde_json::to_string(&witness).unwrap();
        let deserialized: Witness = serde_json::from_str(&serialized).unwrap();

        assert!(matches!(deserialized, Witness::P2PKWitness(_)));
    }
}
