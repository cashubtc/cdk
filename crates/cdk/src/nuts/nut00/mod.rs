//! NUT-00: Notation and Models
//!
//! <https://github.com/cashubtc/nuts/blob/main/00.md>

use std::cmp::Ordering;
use std::fmt;
use std::hash::{Hash, Hasher};
use std::str::FromStr;
use std::string::FromUtf8Error;

use bitcoin::hashes::{sha256, Hash as _, HashEngine as _};
use serde::{Deserialize, Deserializer, Serialize};
use thiserror::Error;

use super::nut10;
use super::nut11::SpendingConditions;
use crate::amount::SplitTarget;
use crate::dhke::{blind_message, hash_to_curve};
use crate::nuts::nut01::{PublicKey, SecretKey};
use crate::nuts::nut11::{serde_p2pk_witness, P2PKWitness};
use crate::nuts::nut12::BlindSignatureDleq;
use crate::nuts::nut14::{serde_htlc_witness, HTLCWitness};
use crate::nuts::{Id, ProofDleq};
use crate::secret::Secret;
use crate::util::hex;
use crate::Amount;

pub mod token;
pub use token::{Token, TokenV3, TokenV4};

/// List of [Proof]
pub type Proofs = Vec<Proof>;

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
    /// Invalid Url
    #[error("Invalid URL")]
    InvalidUrl,
    /// Serde Json error
    #[error(transparent)]
    SerdeJsonError(#[from] serde_json::Error),
    /// Utf8 parse error
    #[error(transparent)]
    Utf8ParseError(#[from] FromUtf8Error),
    /// Base64 error
    #[error(transparent)]
    Base64Error(#[from] base64::DecodeError),
    /// Parse Url Error
    #[error(transparent)]
    UrlParseError(#[from] url::ParseError),
    /// Ciborium error
    #[error(transparent)]
    CiboriumError(#[from] ciborium::de::Error<std::io::Error>),
    /// CDK error
    #[error(transparent)]
    Cdk(#[from] crate::error::Error),
    /// NUT11 error
    #[error(transparent)]
    NUT11(#[from] crate::nuts::nut11::Error),
    /// Hex error
    #[error(transparent)]
    HexError(#[from] hex::Error),
}

/// Blinded Message (also called `output`)
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct BlindedMessage {
    /// Amount
    ///
    /// The value for the requested [BlindSignature]
    pub amount: Amount,
    /// Keyset ID
    ///
    /// ID from which we expect a signature.
    #[serde(rename = "id")]
    pub keyset_id: Id,
    /// Blinded secret message (B_)
    ///
    /// The blinded secret message generated by the sender.
    #[serde(rename = "B_")]
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
pub struct BlindSignature {
    /// Amount
    ///
    /// The value of the blinded token.
    pub amount: Amount,
    /// Keyset ID
    ///
    /// ID of the mint keys that signed the token.
    #[serde(rename = "id")]
    pub keyset_id: Id,
    /// Blinded signature (C_)
    ///
    /// The blinded signature on the secret message `B_` of [BlindedMessage].
    #[serde(rename = "C_")]
    pub c: PublicKey,
    /// DLEQ Proof
    ///
    /// <https://github.com/cashubtc/nuts/blob/main/12.md>
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dleq: Option<BlindSignatureDleq>,
}

/// Witness
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Witness {
    /// P2PK Witness
    #[serde(with = "serde_p2pk_witness")]
    P2PKWitness(P2PKWitness),
    /// HTLC Witness
    #[serde(with = "serde_htlc_witness")]
    HTLCWitness(HTLCWitness),
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
pub struct Proof {
    /// Amount
    pub amount: Amount,
    /// `Keyset id`
    #[serde(rename = "id")]
    pub keyset_id: Id,
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

/// Currency Unit
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Default)]
pub enum CurrencyUnit {
    /// Sat
    #[default]
    Sat,
    /// Msat
    Msat,
    /// Usd
    Usd,
    /// Euro
    Eur,
}

#[cfg(feature = "mint")]
impl CurrencyUnit {
    /// Derivation index mint will use for unit
    pub fn derivation_index(&self) -> u32 {
        match self {
            Self::Sat => 0,
            Self::Msat => 1,
            Self::Usd => 2,
            Self::Eur => 3,
        }
    }
}

impl FromStr for CurrencyUnit {
    type Err = Error;
    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "sat" => Ok(Self::Sat),
            "msat" => Ok(Self::Msat),
            "usd" => Ok(Self::Usd),
            "eur" => Ok(Self::Eur),
            _ => Err(Error::UnsupportedUnit),
        }
    }
}

impl fmt::Display for CurrencyUnit {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CurrencyUnit::Sat => write!(f, "sat"),
            CurrencyUnit::Msat => write!(f, "msat"),
            CurrencyUnit::Usd => write!(f, "usd"),
            CurrencyUnit::Eur => write!(f, "eur"),
        }
    }
}

impl Serialize for CurrencyUnit {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(&self.to_string())
    }
}

impl<'de> Deserialize<'de> for CurrencyUnit {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let currency: String = String::deserialize(deserializer)?;
        Self::from_str(&currency).map_err(|_| serde::de::Error::custom("Unsupported unit"))
    }
}

/// Payment Method
#[derive(Debug, Clone, Default, PartialEq, Eq, Hash)]
pub enum PaymentMethod {
    /// Bolt11 payment type
    #[default]
    Bolt11,
    /// Custom payment type:
    Custom(String),
}

impl<S> From<S> for PaymentMethod
where
    S: AsRef<str>,
{
    fn from(method: S) -> Self {
        match method.as_ref() {
            "bolt11" => Self::Bolt11,
            o => Self::Custom(o.to_string()),
        }
    }
}

impl fmt::Display for PaymentMethod {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PaymentMethod::Bolt11 => write!(f, "bolt11"),
            PaymentMethod::Custom(unit) => write!(f, "{}", unit),
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
        Ok(Self::from(payment_method))
    }
}

/// PreMint
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

impl Ord for PreMint {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.amount.cmp(&other.amount)
    }
}

impl PartialOrd for PreMint {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

/// Premint Secrets
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PreMintSecrets {
    /// Secrets
    pub secrets: Vec<PreMint>,
    /// Keyset Id
    pub keyset_id: Id,
}

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
    ) -> Result<Self, Error> {
        let amount_split = amount.split_targeted(amount_split_target)?;

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
    ) -> Result<Self, Error> {
        let amount_split = amount.split_targeted(amount_split_target)?;

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
impl Iterator for PreMintSecrets {
    type Item = PreMint;

    fn next(&mut self) -> Option<Self::Item> {
        // Use the iterator of the vector
        self.secrets.pop()
    }
}

impl Ord for PreMintSecrets {
    fn cmp(&self, other: &Self) -> Ordering {
        self.secrets.cmp(&other.secrets)
    }
}

impl PartialOrd for PreMintSecrets {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

/// Transaction ID
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct TransactionId([u8; 32]);

impl TransactionId {
    /// Create new [`TransactionId`]
    pub fn new(proofs: Proofs) -> Result<Self, Error> {
        let mut ys = proofs
            .iter()
            .map(|proof| proof.y())
            .collect::<Result<Vec<PublicKey>, Error>>()?;
        ys.sort();
        let mut hasher = sha256::Hash::engine();
        for y in ys {
            hasher.input(&y.to_bytes());
        }
        let hash = sha256::Hash::from_engine(hasher);
        Ok(Self(hash.to_byte_array()))
    }

    /// Get inner value
    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }
}

impl fmt::Display for TransactionId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", hex::encode(&self.0))
    }
}

impl FromStr for TransactionId {
    type Err = Error;
    fn from_str(value: &str) -> Result<Self, Self::Err> {
        let bytes = hex::decode(value)?;
        let mut array = [0u8; 32];
        array.copy_from_slice(&bytes);
        Ok(Self(array))
    }
}

#[cfg(test)]
mod tests {
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
    fn test_transaction_id() {
        let proof = "[{\"id\":\"009a1f293253e41e\",\"amount\":2,\"secret\":\"407915bc212be61a77e3e6d2aeb4c727980bda51cd06a6afc29e2861768a7837\",\"C\":\"02bc9097997d81afb2cc7346b5e4345a9346bd2a506eb7958598a72f0cf85163ea\"},{\"id\":\"009a1f293253e41e\",\"amount\":8,\"secret\":\"fe15109314e61d7756b0f8ee0f23a624acaa3f4e042f61433c728c7057b931be\",\"C\":\"029e8e5050b890a7d6c0968db16bc1d5d5fa040ea1de284f6ec69d61299f671059\"}]";
        let mut proofs: Proofs = serde_json::from_str(proof).unwrap();

        let transaction_id = TransactionId::new(proofs.clone()).unwrap();
        assert_eq!(
            transaction_id.to_string(),
            "dac0748828d855ac4bc0e0a008cbc4b02e7d4238af06d730461cc559a5ae24b1"
        );

        proofs.reverse();
        let rev_transaction_id = TransactionId::new(proofs).unwrap();
        assert_eq!(transaction_id, rev_transaction_id);
    }

    #[test]
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
}
