//! NUT-00: Notation and Models
//!
//! <https://github.com/cashubtc/nuts/blob/main/00.md>

use std::cmp::Ordering;
use std::collections::{HashMap, HashSet};
use std::fmt;
use std::hash::{Hash, Hasher};
use std::str::FromStr;
use std::string::FromUtf8Error;

use serde::{Deserialize, Deserializer, Serialize};
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

    /// Create a copy of proofs without P2BK nonce
    fn without_p2pk_e(&self) -> Proofs;
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

    fn without_p2pk_e(&self) -> Proofs {
        self.iter()
            .map(|p| {
                let mut p = p.clone();
                p.p2pk_e = None;
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

    fn without_p2pk_e(&self) -> Proofs {
        self.iter()
            .map(|p| {
                let mut p = p.clone();
                p.p2pk_e = None;
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
    proofs.map(Proof::y).collect::<Result<Vec<PublicKey>, _>>()
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
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
    pub fn add_signatures(&mut self, signatures: Vec<String>) {
        match self {
            Self::P2PKWitness(p2pk_witness) => p2pk_witness.signatures.extend(signatures),
            Self::HTLCWitness(htlc_witness) => match &mut htlc_witness.signatures {
                Some(sigs) => sigs.extend(signatures),
                None => htlc_witness.signatures = Some(signatures),
            },
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub witness: Option<Witness>,
    /// DLEQ Proof
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dleq: Option<ProofDleq>,
    /// P2BK Ephemeral Public Key (NUT-28)
    /// Used for Pay-to-Blinded-Key privacy feature
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub p2pk_e: Option<PublicKey>,
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
            p2pk_e: None,
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub witness: Option<Witness>,
    /// DLEQ Proof
    #[serde(rename = "d")]
    pub dleq: Option<ProofDleq>,
    /// P2BK Ephemeral Public Key (NUT-28)
    #[serde(rename = "pe", default, skip_serializing_if = "Option::is_none")]
    pub p2pk_e: Option<PublicKey>,
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
            p2pk_e: self.p2pk_e.clone(),
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
            secret,
            c,
            witness,
            dleq,
            p2pk_e,
            keyset_id: _,
        } = proof;
        ProofV4 {
            amount,
            secret,
            c,
            witness,
            dleq,
            p2pk_e,
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
            p2pk_e: None,
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub witness: Option<Witness>,
    /// DLEQ Proof
    #[serde(default, skip_serializing_if = "Option::is_none")]
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
            p2pk_e: None,
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
            p2pk_e: _,
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

/// Currency Unit
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Default)]
#[cfg_attr(feature = "swagger", derive(utoipa::ToSchema))]
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
    /// Auth
    Auth,
    /// Custom currency unit
    Custom(String),
}

#[cfg(feature = "mint")]
impl CurrencyUnit {
    /// Derivation index mint will use for unit
    pub fn derivation_index(&self) -> Option<u32> {
        match self {
            Self::Sat => Some(0),
            Self::Msat => Some(1),
            Self::Usd => Some(2),
            Self::Eur => Some(3),
            Self::Auth => Some(4),
            _ => None,
        }
    }
}

impl FromStr for CurrencyUnit {
    type Err = Error;
    fn from_str(value: &str) -> Result<Self, Self::Err> {
        let upper_value = value.to_uppercase();
        match upper_value.as_str() {
            "SAT" => Ok(Self::Sat),
            "MSAT" => Ok(Self::Msat),
            "USD" => Ok(Self::Usd),
            "EUR" => Ok(Self::Eur),
            "AUTH" => Ok(Self::Auth),
            _ => Ok(Self::Custom(value.to_string())),
        }
    }
}

impl fmt::Display for CurrencyUnit {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            CurrencyUnit::Sat => "SAT",
            CurrencyUnit::Msat => "MSAT",
            CurrencyUnit::Usd => "USD",
            CurrencyUnit::Eur => "EUR",
            CurrencyUnit::Auth => "AUTH",
            CurrencyUnit::Custom(unit) => unit,
        };
        if let Some(width) = f.width() {
            write!(f, "{:width$}", s.to_lowercase(), width = width)
        } else {
            write!(f, "{}", s.to_lowercase())
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

/// Known payment methods
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "swagger", derive(utoipa::ToSchema))]
pub enum KnownMethod {
    /// Lightning BOLT11
    Bolt11,
    /// Lightning BOLT12
    Bolt12,
}

impl KnownMethod {
    /// Get the method name as a string
    pub fn as_str(&self) -> &str {
        match self {
            Self::Bolt11 => "bolt11",
            Self::Bolt12 => "bolt12",
        }
    }
}

impl fmt::Display for KnownMethod {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

impl FromStr for KnownMethod {
    type Err = Error;
    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value.to_lowercase().as_str() {
            "bolt11" => Ok(Self::Bolt11),
            "bolt12" => Ok(Self::Bolt12),
            _ => Err(Error::UnsupportedPaymentMethod),
        }
    }
}

/// Payment Method
///
/// Represents either a known payment method (bolt11, bolt12) or a custom payment method.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "swagger", derive(utoipa::ToSchema))]
pub enum PaymentMethod {
    /// Known payment method (bolt11, bolt12)
    Known(KnownMethod),
    /// Custom payment method (e.g., "paypal", "stripe")
    Custom(String),
}

impl PaymentMethod {
    /// BOLT11 payment method
    pub const BOLT11: Self = Self::Known(KnownMethod::Bolt11);
    /// BOLT12 payment method
    pub const BOLT12: Self = Self::Known(KnownMethod::Bolt12);

    /// Create a new PaymentMethod from a string
    pub fn new(method: String) -> Self {
        Self::from_str(&method).unwrap_or_else(|_| Self::Custom(method.to_lowercase()))
    }

    /// Get the method name as a string
    pub fn as_str(&self) -> &str {
        match self {
            Self::Known(known) => known.as_str(),
            Self::Custom(custom) => custom.as_str(),
        }
    }

    /// Check if this is a known method
    pub fn is_known(&self) -> bool {
        matches!(self, Self::Known(_))
    }

    /// Check if this is a custom method
    pub fn is_custom(&self) -> bool {
        matches!(self, Self::Custom(_))
    }

    /// Check if this is bolt11
    pub fn is_bolt11(&self) -> bool {
        matches!(self, Self::Known(KnownMethod::Bolt11))
    }

    /// Check if this is bolt12
    pub fn is_bolt12(&self) -> bool {
        matches!(self, Self::Known(KnownMethod::Bolt12))
    }
}

impl FromStr for PaymentMethod {
    type Err = Error;
    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match KnownMethod::from_str(value) {
            Ok(known) => Ok(Self::Known(known)),
            Err(_) => Ok(Self::Custom(value.to_lowercase())),
        }
    }
}

impl fmt::Display for PaymentMethod {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

impl From<String> for PaymentMethod {
    fn from(s: String) -> Self {
        Self::from_str(&s).unwrap_or_else(|_| Self::Custom(s.to_lowercase()))
    }
}

impl From<&str> for PaymentMethod {
    fn from(s: &str) -> Self {
        Self::from_str(s).unwrap_or_else(|_| Self::Custom(s.to_lowercase()))
    }
}

impl From<KnownMethod> for PaymentMethod {
    fn from(known: KnownMethod) -> Self {
        Self::Known(known)
    }
}

// Implement PartialEq with &str for ergonomic comparisons
impl PartialEq<&str> for PaymentMethod {
    fn eq(&self, other: &&str) -> bool {
        self.as_str() == *other
    }
}

impl PartialEq<str> for PaymentMethod {
    fn eq(&self, other: &str) -> bool {
        self.as_str() == other
    }
}

impl PartialEq<PaymentMethod> for &str {
    fn eq(&self, other: &PaymentMethod) -> bool {
        *self == other.as_str()
    }
}

impl PartialEq<KnownMethod> for PaymentMethod {
    fn eq(&self, other: &KnownMethod) -> bool {
        matches!(self, Self::Known(k) if k == other)
    }
}

impl PartialEq<PaymentMethod> for KnownMethod {
    fn eq(&self, other: &PaymentMethod) -> bool {
        matches!(other, PaymentMethod::Known(k) if k == self)
    }
}

impl Serialize for PaymentMethod {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for PaymentMethod {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let payment_method: String = String::deserialize(deserializer)?;
        Ok(Self::from_str(&payment_method).unwrap_or(Self::Custom(payment_method)))
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
        if self.secrets.is_empty() {
            return None;
        }
        Some(self.secrets.remove(0))
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
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
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
    fn custom_unit_ser_der() {
        let unit = CurrencyUnit::Custom(String::from("test"));
        let serialized = serde_json::to_string(&unit).unwrap();
        let deserialized: CurrencyUnit = serde_json::from_str(&serialized).unwrap();
        assert_eq!(unit, deserialized)
    }

    #[test]
    fn test_currency_unit_parsing() {
        assert_eq!(CurrencyUnit::from_str("sat").unwrap(), CurrencyUnit::Sat);
        assert_eq!(CurrencyUnit::from_str("SAT").unwrap(), CurrencyUnit::Sat);
        assert_eq!(CurrencyUnit::from_str("msat").unwrap(), CurrencyUnit::Msat);
        assert_eq!(CurrencyUnit::from_str("MSAT").unwrap(), CurrencyUnit::Msat);
        assert_eq!(CurrencyUnit::from_str("usd").unwrap(), CurrencyUnit::Usd);
        assert_eq!(CurrencyUnit::from_str("USD").unwrap(), CurrencyUnit::Usd);
        assert_eq!(CurrencyUnit::from_str("eur").unwrap(), CurrencyUnit::Eur);
        assert_eq!(CurrencyUnit::from_str("EUR").unwrap(), CurrencyUnit::Eur);
        assert_eq!(CurrencyUnit::from_str("auth").unwrap(), CurrencyUnit::Auth);
        assert_eq!(CurrencyUnit::from_str("AUTH").unwrap(), CurrencyUnit::Auth);

        // Custom
        assert_eq!(
            CurrencyUnit::from_str("custom").unwrap(),
            CurrencyUnit::Custom("custom".to_string())
        );
    }

    #[test]
    fn test_payment_method_parsing() {
        // Test known methods (case insensitive)
        assert_eq!(
            PaymentMethod::from_str("bolt11").unwrap(),
            PaymentMethod::BOLT11
        );
        assert_eq!(
            PaymentMethod::from_str("BOLT11").unwrap(),
            PaymentMethod::BOLT11
        );
        assert_eq!(
            PaymentMethod::from_str("Bolt11").unwrap(),
            PaymentMethod::Known(KnownMethod::Bolt11)
        );

        assert_eq!(
            PaymentMethod::from_str("bolt12").unwrap(),
            PaymentMethod::BOLT12
        );
        assert_eq!(
            PaymentMethod::from_str("BOLT12").unwrap(),
            PaymentMethod::Known(KnownMethod::Bolt12)
        );

        // Test custom methods
        assert_eq!(
            PaymentMethod::from_str("custom").unwrap(),
            PaymentMethod::Custom("custom".to_string())
        );
        assert_eq!(
            PaymentMethod::from_str("PAYPAL").unwrap(),
            PaymentMethod::Custom("paypal".to_string())
        );

        // Test string conversion
        assert_eq!(PaymentMethod::BOLT11.as_str(), "bolt11");
        assert_eq!(PaymentMethod::BOLT12.as_str(), "bolt12");
        assert_eq!(PaymentMethod::from("paypal").as_str(), "paypal");

        // Test ergonomic comparisons with strings
        assert!(PaymentMethod::BOLT11 == "bolt11");
        assert!(PaymentMethod::BOLT12 == "bolt12");
        assert!(PaymentMethod::Custom("paypal".to_string()) == "paypal");

        // Test comparison with KnownMethod
        assert!(PaymentMethod::BOLT11 == KnownMethod::Bolt11);
        assert!(PaymentMethod::BOLT12 == KnownMethod::Bolt12);

        // Test serialization/deserialization consistency
        let methods = vec![
            PaymentMethod::BOLT11,
            PaymentMethod::BOLT12,
            PaymentMethod::Custom("test".to_string()),
        ];

        for method in methods {
            let serialized = serde_json::to_string(&method).unwrap();
            let deserialized: PaymentMethod = serde_json::from_str(&serialized).unwrap();
            assert_eq!(method, deserialized);
        }
    }

    /// Tests that is_bolt12 correctly identifies BOLT12 payment methods.
    ///
    /// This is critical for code that needs to distinguish between BOLT11 and BOLT12.
    /// If is_bolt12 always returns true or false, the wrong payment flow may be used.
    ///
    /// Mutant testing: Kills mutations that:
    /// - Replace is_bolt12 with true
    /// - Replace is_bolt12 with false
    #[test]
    fn test_is_bolt12_with_bolt12() {
        // BOLT12 should return true
        let method = PaymentMethod::BOLT12;
        assert!(method.is_bolt12());

        // Known BOLT12 should also return true
        let method = PaymentMethod::Known(KnownMethod::Bolt12);
        assert!(method.is_bolt12());
    }

    #[test]
    fn test_is_bolt12_with_non_bolt12() {
        // BOLT11 should return false
        let method = PaymentMethod::BOLT11;
        assert!(!method.is_bolt12());

        // Known BOLT11 should return false
        let method = PaymentMethod::Known(KnownMethod::Bolt11);
        assert!(!method.is_bolt12());

        // Custom methods should return false
        let method = PaymentMethod::Custom("paypal".to_string());
        assert!(!method.is_bolt12());

        let method = PaymentMethod::Custom("bolt12".to_string());
        assert!(!method.is_bolt12()); // String match is not the same as actual BOLT12
    }

    /// Tests that is_bolt12 correctly distinguishes between all payment method variants.
    #[test]
    fn test_is_bolt12_comprehensive() {
        // Test all variants
        assert!(PaymentMethod::BOLT12.is_bolt12());
        assert!(PaymentMethod::Known(KnownMethod::Bolt12).is_bolt12());

        assert!(!PaymentMethod::BOLT11.is_bolt12());
        assert!(!PaymentMethod::Known(KnownMethod::Bolt11).is_bolt12());
        assert!(!PaymentMethod::Custom("anything".to_string()).is_bolt12());
        assert!(!PaymentMethod::Custom("bolt12".to_string()).is_bolt12());
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

    #[test]
    fn test_proofs_methods_count_by_keyset() {
        let proofs: Proofs = serde_json::from_str(
            r#"[
                {"id":"009a1f293253e41e","amount":2,"secret":"secret1","C":"02bc9097997d81afb2cc7346b5e4345a9346bd2a506eb7958598a72f0cf85163ea"},
                {"id":"009a1f293253e41e","amount":8,"secret":"secret2","C":"029e8e5050b890a7d6c0968db16bc1d5d5fa040ea1de284f6ec69d61299f671059"},
                {"id":"00ad268c4d1f5826","amount":4,"secret":"secret3","C":"02bc9097997d81afb2cc7346b5e4345a9346bd2a506eb7958598a72f0cf85163ea"}
            ]"#,
        ).unwrap();

        let counts = proofs.count_by_keyset();
        assert_eq!(counts.len(), 2);
        assert_eq!(counts[&Id::from_str("009a1f293253e41e").unwrap()], 2);
        assert_eq!(counts[&Id::from_str("00ad268c4d1f5826").unwrap()], 1);
    }

    #[test]
    fn test_proofs_methods_sum_by_keyset() {
        let proofs: Proofs = serde_json::from_str(
            r#"[
                {"id":"009a1f293253e41e","amount":2,"secret":"secret1","C":"02bc9097997d81afb2cc7346b5e4345a9346bd2a506eb7958598a72f0cf85163ea"},
                {"id":"009a1f293253e41e","amount":8,"secret":"secret2","C":"029e8e5050b890a7d6c0968db16bc1d5d5fa040ea1de284f6ec69d61299f671059"},
                {"id":"00ad268c4d1f5826","amount":4,"secret":"secret3","C":"02bc9097997d81afb2cc7346b5e4345a9346bd2a506eb7958598a72f0cf85163ea"}
            ]"#,
        ).unwrap();

        let sums = proofs.sum_by_keyset();
        assert_eq!(sums.len(), 2);
        assert_eq!(
            sums[&Id::from_str("009a1f293253e41e").unwrap()],
            Amount::from(10)
        );
        assert_eq!(
            sums[&Id::from_str("00ad268c4d1f5826").unwrap()],
            Amount::from(4)
        );
    }

    #[test]
    fn test_proofs_methods_total_amount() {
        let proofs: Proofs = serde_json::from_str(
            r#"[
                {"id":"009a1f293253e41e","amount":2,"secret":"secret1","C":"02bc9097997d81afb2cc7346b5e4345a9346bd2a506eb7958598a72f0cf85163ea"},
                {"id":"009a1f293253e41e","amount":8,"secret":"secret2","C":"029e8e5050b890a7d6c0968db16bc1d5d5fa040ea1de284f6ec69d61299f671059"},
                {"id":"00ad268c4d1f5826","amount":4,"secret":"secret3","C":"02bc9097997d81afb2cc7346b5e4345a9346bd2a506eb7958598a72f0cf85163ea"}
            ]"#,
        ).unwrap();

        let total = proofs.total_amount().unwrap();
        assert_eq!(total, Amount::from(14));
    }

    #[test]
    fn test_proofs_methods_ys() {
        let proofs: Proofs = serde_json::from_str(
            r#"[
                {"id":"009a1f293253e41e","amount":2,"secret":"secret1","C":"02bc9097997d81afb2cc7346b5e4345a9346bd2a506eb7958598a72f0cf85163ea"},
                {"id":"009a1f293253e41e","amount":8,"secret":"secret2","C":"029e8e5050b890a7d6c0968db16bc1d5d5fa040ea1de284f6ec69d61299f671059"}
            ]"#,
        ).unwrap();

        let ys = proofs.ys().unwrap();
        assert_eq!(ys.len(), 2);
        // Each Y is hash_to_curve of the secret, verify they're different
        assert_ne!(ys[0], ys[1]);
    }

    #[test]
    fn test_proofs_methods_hashset() {
        let proofs: Proofs = serde_json::from_str(
            r#"[
                {"id":"009a1f293253e41e","amount":2,"secret":"secret1","C":"02bc9097997d81afb2cc7346b5e4345a9346bd2a506eb7958598a72f0cf85163ea"},
                {"id":"009a1f293253e41e","amount":8,"secret":"secret2","C":"029e8e5050b890a7d6c0968db16bc1d5d5fa040ea1de284f6ec69d61299f671059"},
                {"id":"00ad268c4d1f5826","amount":4,"secret":"secret3","C":"02bc9097997d81afb2cc7346b5e4345a9346bd2a506eb7958598a72f0cf85163ea"}
            ]"#,
        ).unwrap();

        let proof_set: HashSet<Proof> = proofs.into_iter().collect();

        // Test HashSet implementation of ProofsMethods
        let counts = proof_set.count_by_keyset();
        assert_eq!(counts.len(), 2);

        let sums = proof_set.sum_by_keyset();
        assert_eq!(sums.len(), 2);
        // Total should be 14 (2 + 8 + 4)
        let total: u64 = sums.values().map(|a| u64::from(*a)).sum();
        assert_eq!(total, 14);
    }

    #[test]
    fn test_hashset_total_amount() {
        let proofs: Proofs = serde_json::from_str(
            r#"[
                {"id":"009a1f293253e41e","amount":2,"secret":"secret1","C":"02bc9097997d81afb2cc7346b5e4345a9346bd2a506eb7958598a72f0cf85163ea"},
                {"id":"009a1f293253e41e","amount":8,"secret":"secret2","C":"029e8e5050b890a7d6c0968db16bc1d5d5fa040ea1de284f6ec69d61299f671059"},
                {"id":"00ad268c4d1f5826","amount":4,"secret":"secret3","C":"02bc9097997d81afb2cc7346b5e4345a9346bd2a506eb7958598a72f0cf85163ea"}
            ]"#,
        )
        .unwrap();

        let proof_set: HashSet<Proof> = proofs.into_iter().collect();

        // Test total_amount directly on HashSet
        let total = proof_set.total_amount().unwrap();
        assert_eq!(total, Amount::from(14));
    }

    #[test]
    fn test_hashset_ys() {
        let proofs: Proofs = serde_json::from_str(
            r#"[
                {"id":"009a1f293253e41e","amount":2,"secret":"secret1","C":"02bc9097997d81afb2cc7346b5e4345a9346bd2a506eb7958598a72f0cf85163ea"},
                {"id":"009a1f293253e41e","amount":8,"secret":"secret2","C":"029e8e5050b890a7d6c0968db16bc1d5d5fa040ea1de284f6ec69d61299f671059"}
            ]"#,
        )
        .unwrap();

        let proof_set: HashSet<Proof> = proofs.into_iter().collect();

        // Test ys() directly on HashSet - should return 2 public keys
        let ys = proof_set.ys().unwrap();
        assert_eq!(ys.len(), 2);
        // Each Y is hash_to_curve of the secret, verify they're different
        assert_ne!(ys[0], ys[1]);
    }

    #[test]
    fn test_hashset_without_dleqs() {
        let proofs: Proofs = serde_json::from_str(
            r#"[
                {"id":"009a1f293253e41e","amount":2,"secret":"secret1","C":"02bc9097997d81afb2cc7346b5e4345a9346bd2a506eb7958598a72f0cf85163ea"},
                {"id":"009a1f293253e41e","amount":8,"secret":"secret2","C":"029e8e5050b890a7d6c0968db16bc1d5d5fa040ea1de284f6ec69d61299f671059"}
            ]"#,
        )
        .unwrap();

        let proof_set: HashSet<Proof> = proofs.into_iter().collect();

        // Test without_dleqs() directly on HashSet
        let proofs_without_dleqs = proof_set.without_dleqs();
        assert_eq!(proofs_without_dleqs.len(), 2);
        // Verify all dleqs are None
        for proof in &proofs_without_dleqs {
            assert!(proof.dleq.is_none());
        }
    }

    #[test]
    fn test_blind_signature_partial_cmp() {
        let sig1 = BlindSignature {
            amount: Amount::from(10),
            keyset_id: Id::from_str("009a1f293253e41e").unwrap(),
            c: PublicKey::from_str(
                "02bc9097997d81afb2cc7346b5e4345a9346bd2a506eb7958598a72f0cf85163ea",
            )
            .unwrap(),
            dleq: None,
        };
        let sig2 = BlindSignature {
            amount: Amount::from(20),
            keyset_id: Id::from_str("009a1f293253e41e").unwrap(),
            c: PublicKey::from_str(
                "02bc9097997d81afb2cc7346b5e4345a9346bd2a506eb7958598a72f0cf85163ea",
            )
            .unwrap(),
            dleq: None,
        };
        let sig3 = BlindSignature {
            amount: Amount::from(10),
            keyset_id: Id::from_str("009a1f293253e41e").unwrap(),
            c: PublicKey::from_str(
                "02bc9097997d81afb2cc7346b5e4345a9346bd2a506eb7958598a72f0cf85163ea",
            )
            .unwrap(),
            dleq: None,
        };

        // Test partial_cmp
        assert_eq!(sig1.partial_cmp(&sig2), Some(Ordering::Less));
        assert_eq!(sig2.partial_cmp(&sig1), Some(Ordering::Greater));
        assert_eq!(sig1.partial_cmp(&sig3), Some(Ordering::Equal));

        // Verify sorting works
        let mut sigs = vec![sig2.clone(), sig1.clone(), sig3.clone()];
        sigs.sort();
        assert_eq!(sigs[0].amount, Amount::from(10));
        assert_eq!(sigs[2].amount, Amount::from(20));
    }

    #[test]
    fn test_witness_preimage() {
        // Test HTLCWitness returns Some(preimage)
        let htlc_witness = HTLCWitness {
            preimage: "test_preimage".to_string(),
            signatures: Some(vec!["sig1".to_string()]),
        };
        let witness = Witness::HTLCWitness(htlc_witness);
        assert_eq!(witness.preimage(), Some("test_preimage".to_string()));

        // Test P2PKWitness returns None
        let p2pk_witness = P2PKWitness {
            signatures: vec!["sig1".to_string()],
        };
        let witness = Witness::P2PKWitness(p2pk_witness);
        assert_eq!(witness.preimage(), None);
    }

    #[test]
    fn test_proof_is_active() {
        let proof: Proof = serde_json::from_str(
            r#"{"id":"009a1f293253e41e","amount":2,"secret":"secret1","C":"02bc9097997d81afb2cc7346b5e4345a9346bd2a506eb7958598a72f0cf85163ea"}"#,
        ).unwrap();

        let active_keyset_id = Id::from_str("009a1f293253e41e").unwrap();
        let inactive_keyset_id = Id::from_str("00ad268c4d1f5826").unwrap();

        // Test is_active returns true when keyset is in active list
        assert!(proof.is_active(&[active_keyset_id]));

        // Test is_active returns false when keyset is not in active list
        assert!(!proof.is_active(&[inactive_keyset_id]));

        // Test with empty list
        assert!(!proof.is_active(&[]));

        // Test with multiple active keysets
        assert!(proof.is_active(&[inactive_keyset_id, active_keyset_id]));
    }

    /// Helper function to compute hash of a value
    fn compute_hash<T: Hash>(value: &T) -> u64 {
        let mut hasher = DefaultHasher::new();
        value.hash(&mut hasher);
        hasher.finish()
    }

    #[test]
    fn test_proof_hash_uses_secret() {
        // Two proofs with same secret should hash to the same value
        let proof1: Proof = serde_json::from_str(
            r#"{"id":"009a1f293253e41e","amount":2,"secret":"same_secret","C":"02bc9097997d81afb2cc7346b5e4345a9346bd2a506eb7958598a72f0cf85163ea"}"#,
        ).unwrap();

        let proof2: Proof = serde_json::from_str(
            r#"{"id":"00ad268c4d1f5826","amount":8,"secret":"same_secret","C":"029e8e5050b890a7d6c0968db16bc1d5d5fa040ea1de284f6ec69d61299f671059"}"#,
        ).unwrap();

        // Same secret = same hash (even with different keyset_id and amount)
        assert_eq!(compute_hash(&proof1), compute_hash(&proof2));

        // Different secret = different hash
        let proof3: Proof = serde_json::from_str(
            r#"{"id":"009a1f293253e41e","amount":2,"secret":"different_secret","C":"02bc9097997d81afb2cc7346b5e4345a9346bd2a506eb7958598a72f0cf85163ea"}"#,
        ).unwrap();

        assert_ne!(compute_hash(&proof1), compute_hash(&proof3));
    }

    #[test]
    fn test_proof_v4_hash_uses_secret() {
        let proof1: Proof = serde_json::from_str(
            r#"{"id":"009a1f293253e41e","amount":2,"secret":"same_secret","C":"02bc9097997d81afb2cc7346b5e4345a9346bd2a506eb7958598a72f0cf85163ea"}"#,
        ).unwrap();

        let proof2: Proof = serde_json::from_str(
            r#"{"id":"00ad268c4d1f5826","amount":8,"secret":"same_secret","C":"029e8e5050b890a7d6c0968db16bc1d5d5fa040ea1de284f6ec69d61299f671059"}"#,
        ).unwrap();

        let proof_v4_1: ProofV4 = proof1.into();
        let proof_v4_2: ProofV4 = proof2.into();

        // Same secret = same hash
        assert_eq!(compute_hash(&proof_v4_1), compute_hash(&proof_v4_2));

        // Different secret = different hash
        let proof3: Proof = serde_json::from_str(
            r#"{"id":"009a1f293253e41e","amount":2,"secret":"different_secret","C":"02bc9097997d81afb2cc7346b5e4345a9346bd2a506eb7958598a72f0cf85163ea"}"#,
        ).unwrap();
        let proof_v4_3: ProofV4 = proof3.into();

        assert_ne!(compute_hash(&proof_v4_1), compute_hash(&proof_v4_3));
    }

    #[test]
    fn test_proof_v3_hash_uses_secret() {
        let proof1: Proof = serde_json::from_str(
            r#"{"id":"009a1f293253e41e","amount":2,"secret":"same_secret","C":"02bc9097997d81afb2cc7346b5e4345a9346bd2a506eb7958598a72f0cf85163ea"}"#,
        ).unwrap();

        let proof2: Proof = serde_json::from_str(
            r#"{"id":"00ad268c4d1f5826","amount":8,"secret":"same_secret","C":"029e8e5050b890a7d6c0968db16bc1d5d5fa040ea1de284f6ec69d61299f671059"}"#,
        ).unwrap();

        let proof_v3_1: ProofV3 = proof1.into();
        let proof_v3_2: ProofV3 = proof2.into();

        // Same secret = same hash
        assert_eq!(compute_hash(&proof_v3_1), compute_hash(&proof_v3_2));

        // Different secret = different hash
        let proof3: Proof = serde_json::from_str(
            r#"{"id":"009a1f293253e41e","amount":2,"secret":"different_secret","C":"02bc9097997d81afb2cc7346b5e4345a9346bd2a506eb7958598a72f0cf85163ea"}"#,
        ).unwrap();
        let proof_v3_3: ProofV3 = proof3.into();

        assert_ne!(compute_hash(&proof_v3_1), compute_hash(&proof_v3_3));
    }

    #[test]
    #[cfg(feature = "mint")]
    fn test_currency_unit_derivation_index() {
        // Each currency unit should have a specific derivation index
        // These values are important for key derivation compatibility
        assert_eq!(CurrencyUnit::Sat.derivation_index(), Some(0));
        assert_eq!(CurrencyUnit::Msat.derivation_index(), Some(1));
        assert_eq!(CurrencyUnit::Usd.derivation_index(), Some(2));
        assert_eq!(CurrencyUnit::Eur.derivation_index(), Some(3));
        assert_eq!(CurrencyUnit::Auth.derivation_index(), Some(4));

        // Custom units should return None
        assert_eq!(
            CurrencyUnit::Custom("btc".to_string()).derivation_index(),
            None
        );
    }
}
