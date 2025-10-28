//! NUT-11: Pay to Public Key (P2PK)
//!
//! <https://github.com/cashubtc/nuts/blob/main/11.md>

use std::collections::{HashMap, HashSet};
use std::str::FromStr;
use std::{fmt, vec};

use bitcoin::hashes::sha256::Hash as Sha256Hash;
use bitcoin::hashes::Hash;
use bitcoin::secp256k1::schnorr::Signature;
use serde::de::{DeserializeOwned, Error as DeserializerError};
use serde::ser::SerializeSeq;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use thiserror::Error;

use super::nut00::Witness;
use super::nut01::PublicKey;
use super::nut05::MeltRequest;
use super::{Kind, Nut10Secret, Proof, Proofs, SecretKey};
use crate::nuts::nut00::BlindedMessage;
use crate::secret::Secret;
use crate::util::{hex, unix_time};
use crate::{ensure_cdk, SwapRequest};

pub mod serde_p2pk_witness;

/// Nut11 Error
#[derive(Debug, Error)]
pub enum Error {
    /// Incorrect secret kind
    #[error("Secret is not a p2pk secret")]
    IncorrectSecretKind,
    /// Incorrect secret kind
    #[error("Witness is not a p2pk witness")]
    IncorrectWitnessKind,
    /// P2PK locktime has already passed
    #[error("Locktime in past")]
    LocktimeInPast,
    /// Witness signature is not valid
    #[error("Invalid signature")]
    InvalidSignature,
    /// Unknown tag in P2PK secret
    #[error("Unknown tag P2PK secret")]
    UnknownTag,
    /// Unknown Sigflag
    #[error("Unknown sigflag")]
    UnknownSigFlag,
    /// P2PK Spend conditions not meet
    #[error("P2PK spend conditions are not met")]
    SpendConditionsNotMet,
    /// Pubkey must be in data field of P2PK
    #[error("P2PK required in secret data")]
    P2PKPubkeyRequired,
    /// Unknown Kind
    #[error("Kind not found")]
    KindNotFound,
    /// HTLC hash invalid
    #[error("Invalid hash")]
    InvalidHash,
    /// HTLC preimage too large
    #[error("Preimage exceeds maximum size of 32 bytes (64 hex characters)")]
    PreimageTooLarge,
    /// Witness Signatures not provided
    #[error("Witness signatures not provided")]
    SignaturesNotProvided,
    /// Duplicate signature from same pubkey
    #[error("Duplicate signature from the same pubkey detected")]
    DuplicateSignature,
    /// Parse Url Error
    #[error(transparent)]
    UrlParseError(#[from] url::ParseError),
    /// Parse int error
    #[error(transparent)]
    ParseInt(#[from] std::num::ParseIntError),
    /// From hex error
    #[error(transparent)]
    HexError(#[from] hex::Error),
    /// Serde Json error
    #[error(transparent)]
    SerdeJsonError(#[from] serde_json::Error),
    /// Secp256k1 error
    #[error(transparent)]
    Secp256k1(#[from] bitcoin::secp256k1::Error),
    /// NUT01 Error
    #[error(transparent)]
    NUT01(#[from] crate::nuts::nut01::Error),
    /// Secret error
    #[error(transparent)]
    Secret(#[from] crate::secret::Error),
}

/// P2Pk Witness
#[derive(Default, Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "swagger", derive(utoipa::ToSchema))]
pub struct P2PKWitness {
    /// Signatures
    pub signatures: Vec<String>,
}

impl P2PKWitness {
    #[inline]
    /// Check id Witness is empty
    pub fn is_empty(&self) -> bool {
        self.signatures.is_empty()
    }
}

impl Proof {
    /// Sign [Proof]
    pub fn sign_p2pk(&mut self, secret_key: SecretKey) -> Result<(), Error> {
        let msg: Vec<u8> = self.secret.to_bytes();
        let signature: Signature = secret_key.sign(&msg)?;

        let signatures = vec![signature.to_string()];

        match self.witness.as_mut() {
            Some(witness) => {
                witness.add_signatures(signatures);
            }
            None => {
                let mut p2pk_witness = Witness::P2PKWitness(P2PKWitness::default());
                p2pk_witness.add_signatures(signatures);
                self.witness = Some(p2pk_witness);
            }
        };

        Ok(())
    }

    /// Verify P2PK signature on [Proof]
    pub fn verify_p2pk(&self) -> Result<(), Error> {
        let secret: Nut10Secret = self.secret.clone().try_into()?;
        let spending_conditions: Conditions = secret
            .secret_data()
            .tags()
            .cloned()
            .unwrap_or_default()
            .try_into()?;
        let msg: &[u8] = self.secret.as_bytes();

        let mut verified_pubkeys = HashSet::new();

        let witness_signatures = match &self.witness {
            Some(witness) => witness.signatures(),
            None => None,
        };

        let witness_signatures = witness_signatures.ok_or(Error::SignaturesNotProvided)?;

        let mut pubkeys = spending_conditions.pubkeys.clone().unwrap_or_default();
        // NUT-11 enforcement per spec:
        // - If locktime has passed and refund keys are present, spend must be authorized by
        //   refund pubkeys (n_sigs_refund-of-refund). This supersedes normal pubkey enforcement
        //   after expiry.
        // - If locktime has passed and no refund keys are present, proof becomes spendable
        //   without further key checks (anyone-can-spend behavior).
        // - Otherwise (before locktime), enforce normal multisig on the set of authorized
        //   pubkeys: Secret.data plus optional `pubkeys` tag, requiring n_sigs unique signers.

        let now = unix_time();

        if let Some(locktime) = spending_conditions.locktime {
            if now >= locktime {
                if let Some(refund_keys) = spending_conditions.refund_keys.clone() {
                    let needed_refund_sigs =
                        spending_conditions.num_sigs_refund.unwrap_or(1) as usize;
                    let mut valid_pubkeys = HashSet::new();

                    // After locktime, require signatures from refund keys
                    for s in witness_signatures.iter() {
                        let sig = Signature::from_str(s).map_err(|_| Error::InvalidSignature)?;
                        for v in &refund_keys {
                            if v.verify(msg, &sig).is_ok() {
                                valid_pubkeys.insert(v);
                                if valid_pubkeys.len() >= needed_refund_sigs {
                                    return Ok(());
                                }
                            }
                        }
                    }

                    // If locktime and refund keys were specified they must sign after locktime
                    return Err(Error::SpendConditionsNotMet);
                } else {
                    // If only locktime is specified, consider it spendable after locktime
                    return Ok(());
                }
            }
        }

        if secret.kind().eq(&Kind::P2PK) {
            pubkeys.push(PublicKey::from_str(secret.secret_data().data())?);
        }

        for signature in witness_signatures.iter() {
            for v in &pubkeys {
                let sig = Signature::from_str(signature)?;

                if v.verify(msg, &sig).is_ok() {
                    // If the pubkey is already verified, return a duplicate signature error
                    if !verified_pubkeys.insert(*v) {
                        return Err(Error::DuplicateSignature);
                    }
                } else {
                    tracing::debug!(
                        "Could not verify signature: {sig} on message: {}",
                        self.secret.to_string()
                    )
                }
            }
        }

        let valid_sigs = verified_pubkeys.len() as u64;

        if valid_sigs >= spending_conditions.num_sigs.unwrap_or(1) {
            return Ok(());
        }

        Err(Error::SpendConditionsNotMet)
    }
}

/// Returns count of valid signatures (each public key is only counted once)
/// Returns error if the same pubkey has multiple valid signatures
pub fn valid_signatures(
    msg: &[u8],
    pubkeys: &[PublicKey],
    signatures: &[Signature],
) -> Result<u64, Error> {
    let mut verified_pubkeys = HashSet::new();

    for pubkey in pubkeys {
        for signature in signatures {
            if pubkey.verify(msg, signature).is_ok() {
                // If the pubkey is already verified, return a duplicate signature error
                if !verified_pubkeys.insert(*pubkey) {
                    return Err(Error::DuplicateSignature);
                }
            }
        }
    }

    Ok(verified_pubkeys.len() as u64)
}

impl BlindedMessage {
    /// Sign [BlindedMessage]
    pub fn sign_p2pk(&mut self, secret_key: SecretKey) -> Result<(), Error> {
        let msg: [u8; 33] = self.blinded_secret.to_bytes();
        let signature: Signature = secret_key.sign(&msg)?;

        let signatures = vec![signature.to_string()];

        match self.witness.as_mut() {
            Some(witness) => {
                witness.add_signatures(signatures);
            }
            None => {
                let mut p2pk_witness = Witness::P2PKWitness(P2PKWitness::default());
                p2pk_witness.add_signatures(signatures);
                self.witness = Some(p2pk_witness);
            }
        };

        Ok(())
    }

    /// Verify P2PK conditions on [BlindedMessage]
    pub fn verify_p2pk(&self, pubkeys: &Vec<PublicKey>, required_sigs: u64) -> Result<(), Error> {
        let mut verified_pubkeys = HashSet::new();
        if let Some(witness) = &self.witness {
            for signature in witness
                .signatures()
                .ok_or(Error::SignaturesNotProvided)?
                .iter()
            {
                for v in pubkeys {
                    let msg = &self.blinded_secret.to_bytes();
                    let sig = Signature::from_str(signature)?;

                    if v.verify(msg, &sig).is_ok() {
                        // If the pubkey is already verified, return a duplicate signature error
                        if !verified_pubkeys.insert(*v) {
                            return Err(Error::DuplicateSignature);
                        }
                    } else {
                        tracing::debug!(
                            "Could not verify signature: {sig} on message: {}",
                            self.blinded_secret
                        )
                    }
                }
            }
        }

        let valid_sigs = verified_pubkeys.len() as u64;

        if valid_sigs.ge(&required_sigs) {
            Ok(())
        } else {
            Err(Error::SpendConditionsNotMet)
        }
    }
}

/// Spending Conditions
///
/// Defined in [NUT10](https://github.com/cashubtc/nuts/blob/main/10.md)
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum SpendingConditions {
    /// NUT11 Spending conditions
    ///
    /// Defined in [NUT11](https://github.com/cashubtc/nuts/blob/main/11.md)
    P2PKConditions {
        /// The public key of the recipient of the locked ecash
        data: PublicKey,
        /// Additional Optional Spending [`Conditions`]
        conditions: Option<Conditions>,
    },
    /// NUT14 Spending conditions
    ///
    /// Dedined in [NUT14](https://github.com/cashubtc/nuts/blob/main/14.md)
    HTLCConditions {
        /// Hash Lock of ecash
        data: Sha256Hash,
        /// Additional Optional Spending [`Conditions`]
        conditions: Option<Conditions>,
    },
}

impl SpendingConditions {
    /// New HTLC [SpendingConditions]
    pub fn new_htlc(preimage: String, conditions: Option<Conditions>) -> Result<Self, Error> {
        const MAX_PREIMAGE_BYTES: usize = 32;

        let preimage_bytes = hex::decode(preimage)?;

        if preimage_bytes.len() != MAX_PREIMAGE_BYTES {
            return Err(Error::PreimageTooLarge);
        }

        let htlc = Sha256Hash::hash(&preimage_bytes);

        Ok(Self::HTLCConditions {
            data: htlc,
            conditions,
        })
    }

    /// New HTLC [SpendingConditions] from a hash directly instead of preimage
    pub fn new_htlc_hash(hash: &str, conditions: Option<Conditions>) -> Result<Self, Error> {
        let hash = Sha256Hash::from_str(hash).map_err(|_| Error::InvalidHash)?;

        Ok(Self::HTLCConditions {
            data: hash,
            conditions,
        })
    }

    /// New P2PK [SpendingConditions]
    pub fn new_p2pk(pubkey: PublicKey, conditions: Option<Conditions>) -> Self {
        Self::P2PKConditions {
            data: pubkey,
            conditions,
        }
    }

    /// Kind of [SpendingConditions]
    pub fn kind(&self) -> Kind {
        match self {
            Self::P2PKConditions { .. } => Kind::P2PK,
            Self::HTLCConditions { .. } => Kind::HTLC,
        }
    }

    /// Number if signatures required to unlock
    pub fn num_sigs(&self) -> Option<u64> {
        match self {
            Self::P2PKConditions { conditions, .. } => conditions.as_ref().and_then(|c| c.num_sigs),
            Self::HTLCConditions { conditions, .. } => conditions.as_ref().and_then(|c| c.num_sigs),
        }
    }

    /// Public keys of locked [`Proof`]
    pub fn pubkeys(&self) -> Option<Vec<PublicKey>> {
        match self {
            Self::P2PKConditions { data, conditions } => {
                let mut pubkeys = vec![*data];
                if let Some(conditions) = conditions {
                    pubkeys.extend(conditions.pubkeys.clone().unwrap_or_default());
                }
                // Remove duplicates
                let unique_pubkeys: HashSet<_> = pubkeys.into_iter().collect();
                Some(unique_pubkeys.into_iter().collect())
            }
            Self::HTLCConditions { conditions, .. } => conditions.clone().and_then(|c| c.pubkeys),
        }
    }

    /// Locktime of Spending Conditions
    pub fn locktime(&self) -> Option<u64> {
        match self {
            Self::P2PKConditions { conditions, .. } => conditions.as_ref().and_then(|c| c.locktime),
            Self::HTLCConditions { conditions, .. } => conditions.as_ref().and_then(|c| c.locktime),
        }
    }

    /// Refund keys
    pub fn refund_keys(&self) -> Option<Vec<PublicKey>> {
        match self {
            Self::P2PKConditions { conditions, .. } => {
                conditions.clone().and_then(|c| c.refund_keys)
            }
            Self::HTLCConditions { conditions, .. } => {
                conditions.clone().and_then(|c| c.refund_keys)
            }
        }
    }
}

impl TryFrom<&Secret> for SpendingConditions {
    type Error = Error;
    fn try_from(secret: &Secret) -> Result<SpendingConditions, Error> {
        let nut10_secret: Nut10Secret = secret.try_into()?;

        nut10_secret.try_into()
    }
}

impl TryFrom<Nut10Secret> for SpendingConditions {
    type Error = Error;
    fn try_from(secret: Nut10Secret) -> Result<SpendingConditions, Error> {
        match secret.kind() {
            Kind::P2PK => Ok(SpendingConditions::P2PKConditions {
                data: PublicKey::from_str(secret.secret_data().data())?,
                conditions: secret
                    .secret_data()
                    .tags()
                    .and_then(|t| t.clone().try_into().ok()),
            }),
            Kind::HTLC => Ok(Self::HTLCConditions {
                data: Sha256Hash::from_str(secret.secret_data().data())
                    .map_err(|_| Error::InvalidHash)?,
                conditions: secret
                    .secret_data()
                    .tags()
                    .and_then(|t| t.clone().try_into().ok()),
            }),
        }
    }
}

impl From<SpendingConditions> for super::nut10::Secret {
    fn from(conditions: SpendingConditions) -> super::nut10::Secret {
        match conditions {
            SpendingConditions::P2PKConditions { data, conditions } => {
                super::nut10::Secret::new(Kind::P2PK, data.to_hex(), conditions)
            }
            SpendingConditions::HTLCConditions { data, conditions } => {
                super::nut10::Secret::new(Kind::HTLC, data.to_string(), conditions)
            }
        }
    }
}

/// P2PK and HTLC spending conditions
#[derive(Debug, Clone, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
pub struct Conditions {
    /// Unix locktime after which refund keys can be used
    #[serde(skip_serializing_if = "Option::is_none")]
    pub locktime: Option<u64>,
    /// Additional Public keys
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pubkeys: Option<Vec<PublicKey>>,
    /// Refund keys
    #[serde(skip_serializing_if = "Option::is_none")]
    pub refund_keys: Option<Vec<PublicKey>>,
    /// Number of signatures required
    ///
    /// Default is 1
    #[serde(skip_serializing_if = "Option::is_none")]
    pub num_sigs: Option<u64>,
    /// Signature flag
    ///
    /// Default [`SigFlag::SigInputs`]
    pub sig_flag: SigFlag,
    /// Number of refund signatures required
    ///
    /// Default is 1
    #[serde(skip_serializing_if = "Option::is_none")]
    pub num_sigs_refund: Option<u64>,
}

impl Conditions {
    /// Create new Spending [`Conditions`]
    pub fn new(
        locktime: Option<u64>,
        pubkeys: Option<Vec<PublicKey>>,
        refund_keys: Option<Vec<PublicKey>>,
        num_sigs: Option<u64>,
        sig_flag: Option<SigFlag>,
        num_sigs_refund: Option<u64>,
    ) -> Result<Self, Error> {
        if let Some(locktime) = locktime {
            ensure_cdk!(locktime.ge(&unix_time()), Error::LocktimeInPast);
        }

        Ok(Self {
            locktime,
            pubkeys,
            refund_keys,
            num_sigs,
            sig_flag: sig_flag.unwrap_or_default(),
            num_sigs_refund,
        })
    }
}
impl From<Conditions> for Vec<Vec<String>> {
    fn from(conditions: Conditions) -> Vec<Vec<String>> {
        let Conditions {
            locktime,
            pubkeys,
            refund_keys,
            num_sigs,
            sig_flag,
            num_sigs_refund,
        } = conditions;

        let mut tags = Vec::new();

        if let Some(pubkeys) = pubkeys {
            tags.push(Tag::PubKeys(pubkeys.into_iter().collect()).as_vec());
        }

        if let Some(locktime) = locktime {
            tags.push(Tag::LockTime(locktime).as_vec());
        }

        if let Some(num_sigs) = num_sigs {
            tags.push(Tag::NSigs(num_sigs).as_vec());
        }

        if let Some(refund_keys) = refund_keys {
            tags.push(Tag::Refund(refund_keys).as_vec())
        }

        if let Some(num_sigs_refund) = num_sigs_refund {
            tags.push(Tag::NSigsRefund(num_sigs_refund).as_vec())
        }

        tags.push(Tag::SigFlag(sig_flag).as_vec());
        tags
    }
}

impl TryFrom<Vec<Vec<String>>> for Conditions {
    type Error = Error;
    fn try_from(tags: Vec<Vec<String>>) -> Result<Conditions, Self::Error> {
        let tags: HashMap<TagKind, Tag> = tags
            .into_iter()
            .map(|t| Tag::try_from(t).unwrap())
            .map(|t| (t.kind(), t))
            .collect();

        let pubkeys = match tags.get(&TagKind::Pubkeys) {
            Some(Tag::PubKeys(pubkeys)) => Some(pubkeys.clone()),
            _ => None,
        };

        let locktime = if let Some(tag) = tags.get(&TagKind::Locktime) {
            match tag {
                Tag::LockTime(locktime) => Some(*locktime),
                _ => None,
            }
        } else {
            None
        };

        let refund_keys = if let Some(tag) = tags.get(&TagKind::Refund) {
            match tag {
                Tag::Refund(keys) => Some(keys.clone()),
                _ => None,
            }
        } else {
            None
        };

        let sig_flag = if let Some(tag) = tags.get(&TagKind::SigFlag) {
            match tag {
                Tag::SigFlag(sigflag) => *sigflag,
                _ => SigFlag::SigInputs,
            }
        } else {
            SigFlag::SigInputs
        };

        let num_sigs = if let Some(tag) = tags.get(&TagKind::NSigs) {
            match tag {
                Tag::NSigs(num_sigs) => Some(*num_sigs),
                _ => None,
            }
        } else {
            None
        };

        let num_sigs_refund = if let Some(tag) = tags.get(&TagKind::NSigsRefund) {
            match tag {
                Tag::NSigsRefund(num_sigs) => Some(*num_sigs),
                _ => None,
            }
        } else {
            None
        };

        Ok(Conditions {
            locktime,
            pubkeys,
            refund_keys,
            num_sigs,
            sig_flag,
            num_sigs_refund,
        })
    }
}

/// P2PK and HTLC Spending condition tags
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, PartialOrd, Ord)]
#[serde(rename_all = "lowercase")]
pub enum TagKind {
    /// Signature flag
    SigFlag,
    /// Number signatures required
    #[serde(rename = "n_sigs")]
    NSigs,
    /// Locktime
    Locktime,
    /// Refund
    Refund,
    /// Pubkey
    Pubkeys,
    /// Number signatures required
    #[serde(rename = "n_sigs_refund")]
    NSigsRefund,
    /// Custom tag kind
    Custom(String),
}

impl fmt::Display for TagKind {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Self::SigFlag => write!(f, "sigflag"),
            Self::NSigs => write!(f, "n_sigs"),
            Self::Locktime => write!(f, "locktime"),
            Self::Refund => write!(f, "refund"),
            Self::Pubkeys => write!(f, "pubkeys"),
            Self::NSigsRefund => write!(f, "n_sigs_refund"),
            Self::Custom(c) => write!(f, "{c}"),
        }
    }
}

impl<S> From<S> for TagKind
where
    S: AsRef<str>,
{
    fn from(tag: S) -> Self {
        match tag.as_ref() {
            "sigflag" => Self::SigFlag,
            "n_sigs" => Self::NSigs,
            "locktime" => Self::Locktime,
            "refund" => Self::Refund,
            "pubkeys" => Self::Pubkeys,
            "n_sigs_refund" => Self::NSigsRefund,
            t => Self::Custom(t.to_owned()),
        }
    }
}

/// Signature flag
///
/// Defined in [NUT11](https://github.com/cashubtc/nuts/blob/main/11.md)
#[derive(
    Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, PartialOrd, Ord, Hash,
)]
pub enum SigFlag {
    #[default]
    /// Requires valid signatures on all inputs.
    /// It is the default signature flag and will be applied even if the
    /// `sigflag` tag is absent.
    SigInputs,
    /// Requires valid signatures on all inputs and on all outputs.
    SigAll,
}

impl fmt::Display for SigFlag {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Self::SigAll => write!(f, "SIG_ALL"),
            Self::SigInputs => write!(f, "SIG_INPUTS"),
        }
    }
}

impl FromStr for SigFlag {
    type Err = Error;
    fn from_str(tag: &str) -> Result<Self, Self::Err> {
        match tag {
            "SIG_ALL" => Ok(Self::SigAll),
            "SIG_INPUTS" => Ok(Self::SigInputs),
            _ => Err(Error::UnknownSigFlag),
        }
    }
}

/// Get the signature flag that should be enforced for a set of proofs and the
/// public keys that signatures are valid for
pub fn enforce_sig_flag(proofs: Proofs) -> EnforceSigFlag {
    let mut sig_flag = SigFlag::SigInputs;
    let mut pubkeys = HashSet::new();
    let mut sigs_required = 1;
    for proof in proofs {
        if let Ok(secret) = Nut10Secret::try_from(proof.secret) {
            if secret.kind().eq(&Kind::P2PK) {
                if let Ok(verifying_key) = PublicKey::from_str(secret.secret_data().data()) {
                    pubkeys.insert(verifying_key);
                }
            }

            if let Some(tags) = secret.secret_data().tags() {
                if let Ok(conditions) = Conditions::try_from(tags.clone()) {
                    if conditions.sig_flag.eq(&SigFlag::SigAll) {
                        sig_flag = SigFlag::SigAll;
                    }

                    if let Some(sigs) = conditions.num_sigs {
                        if sigs > sigs_required {
                            sigs_required = sigs;
                        }
                    }

                    if let Some(pubs) = conditions.pubkeys {
                        pubkeys.extend(pubs);
                    }
                }
            }
        }
    }

    EnforceSigFlag {
        sig_flag,
        pubkeys,
        sigs_required,
    }
}

/// Enforce Sigflag info
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EnforceSigFlag {
    /// Sigflag required for proofs
    pub sig_flag: SigFlag,
    /// Pubkeys that can sign for proofs
    pub pubkeys: HashSet<PublicKey>,
    /// Number of sigs required for proofs
    pub sigs_required: u64,
}

/// Tag
#[derive(Debug, Clone, Hash, PartialEq, Eq)]
pub enum Tag {
    /// Sigflag [`Tag`]
    SigFlag(SigFlag),
    /// Number of Sigs [`Tag`]
    NSigs(u64),
    /// Locktime [`Tag`]
    LockTime(u64),
    /// Refund [`Tag`]
    Refund(Vec<PublicKey>),
    /// Pubkeys [`Tag`]
    PubKeys(Vec<PublicKey>),
    /// Number of Sigs refund [`Tag`]
    NSigsRefund(u64),
    /// Custom tag
    Custom(String, Vec<String>),
}

impl Tag {
    /// Get [`Tag`] Kind
    pub fn kind(&self) -> TagKind {
        match self {
            Self::SigFlag(_) => TagKind::SigFlag,
            Self::NSigs(_) => TagKind::NSigs,
            Self::LockTime(_) => TagKind::Locktime,
            Self::Refund(_) => TagKind::Refund,
            Self::PubKeys(_) => TagKind::Pubkeys,
            Self::NSigsRefund(_) => TagKind::NSigsRefund,
            Self::Custom(tag, _) => TagKind::Custom(tag.to_string()),
        }
    }

    /// Get [`Tag`] as string vector
    pub fn as_vec(&self) -> Vec<String> {
        self.clone().into()
    }
}

impl<S> TryFrom<Vec<S>> for Tag
where
    S: AsRef<str>,
{
    type Error = Error;

    fn try_from(tag: Vec<S>) -> Result<Self, Self::Error> {
        let tag_kind = tag.first().map(TagKind::from).ok_or(Error::KindNotFound)?;

        match tag_kind {
            TagKind::SigFlag => Ok(Tag::SigFlag(SigFlag::from_str(tag[1].as_ref())?)),
            TagKind::NSigs => Ok(Tag::NSigs(tag[1].as_ref().parse()?)),
            TagKind::Locktime => Ok(Tag::LockTime(tag[1].as_ref().parse()?)),
            TagKind::Refund => {
                let pubkeys = tag
                    .iter()
                    .skip(1)
                    .map(|p| PublicKey::from_str(p.as_ref()))
                    .collect::<Result<Vec<PublicKey>, _>>()?;

                Ok(Self::Refund(pubkeys))
            }
            TagKind::Pubkeys => {
                let pubkeys = tag
                    .iter()
                    .skip(1)
                    .map(|p| PublicKey::from_str(p.as_ref()))
                    .collect::<Result<Vec<PublicKey>, _>>()?;

                Ok(Self::PubKeys(pubkeys))
            }
            TagKind::NSigsRefund => Ok(Tag::NSigsRefund(tag[1].as_ref().parse()?)),
            TagKind::Custom(name) => {
                let tags = tag
                    .iter()
                    .skip(1)
                    .map(|p| p.as_ref().to_string())
                    .collect::<Vec<String>>();

                Ok(Self::Custom(name, tags))
            }
        }
    }
}

impl From<Tag> for Vec<String> {
    fn from(data: Tag) -> Self {
        match data {
            Tag::SigFlag(sigflag) => vec![TagKind::SigFlag.to_string(), sigflag.to_string()],
            Tag::NSigs(num_sig) => vec![TagKind::NSigs.to_string(), num_sig.to_string()],
            Tag::LockTime(locktime) => vec![TagKind::Locktime.to_string(), locktime.to_string()],
            Tag::PubKeys(pubkeys) => {
                let mut tag = vec![TagKind::Pubkeys.to_string()];
                for pubkey in pubkeys.into_iter() {
                    tag.push(pubkey.to_string())
                }
                tag
            }
            Tag::Refund(pubkeys) => {
                let mut tag = vec![TagKind::Refund.to_string()];

                for pubkey in pubkeys {
                    tag.push(pubkey.to_string())
                }
                tag
            }
            Tag::NSigsRefund(num_sigs) => {
                vec![TagKind::NSigsRefund.to_string(), num_sigs.to_string()]
            }
            Tag::Custom(name, c) => {
                let mut tag = vec![name];

                for t in c {
                    tag.push(t);
                }

                tag
            }
        }
    }
}

impl SwapRequest {
    /// Generate the message to sign for SIG_ALL validation
    /// Concatenates all input secrets and output blinded messages in order
    fn sig_all_msg_to_sign(&self) -> String {
        let mut msg_to_sign = String::new();

        // Add all input secrets in order
        for proof in self.inputs() {
            let secret = proof.secret.to_string();
            msg_to_sign.push_str(&secret);
        }

        // Add all output blinded messages in order
        for output in self.outputs() {
            let message = output.blinded_secret.to_string();
            msg_to_sign.push_str(&message);
        }

        msg_to_sign
    }

    /// Get required signature count from first input's spending conditions
    fn get_sig_all_required_sigs(&self) -> Result<(u64, SpendingConditions), Error> {
        let first_input = self.inputs().first().ok_or(Error::SpendConditionsNotMet)?;
        let first_conditions: SpendingConditions =
            SpendingConditions::try_from(&first_input.secret)?;

        let required_sigs = match first_conditions.clone() {
            SpendingConditions::P2PKConditions { conditions, .. } => {
                let conditions = conditions.ok_or(Error::IncorrectSecretKind)?;

                if SigFlag::SigAll != conditions.sig_flag {
                    return Err(Error::IncorrectSecretKind);
                }

                conditions.num_sigs.unwrap_or(1)
            }
            _ => return Err(Error::IncorrectSecretKind),
        };

        Ok((required_sigs, first_conditions))
    }

    /// Verify all inputs have matching secrets and tags
    fn verify_matching_conditions(&self) -> Result<(), Error> {
        let first_input = self.inputs().first().ok_or(Error::SpendConditionsNotMet)?;
        let first_nut10: Nut10Secret = (&first_input.secret).try_into()?;

        for proof in self.inputs().iter().skip(1) {
            let current_secret: Nut10Secret = proof.secret.clone().try_into()?;

            // Check data matches
            if current_secret.secret_data().data() != first_nut10.secret_data().data() {
                return Err(Error::SpendConditionsNotMet);
            }

            // Check tags match
            if current_secret.secret_data().tags() != first_nut10.secret_data().tags() {
                return Err(Error::SpendConditionsNotMet);
            }
        }
        Ok(())
    }

    /// Get validated signatures from first input's witness
    fn get_valid_witness_signatures(&self) -> Result<Vec<Signature>, Error> {
        let first_input = self.inputs().first().ok_or(Error::SpendConditionsNotMet)?;
        let first_witness = first_input
            .witness
            .as_ref()
            .ok_or(Error::SignaturesNotProvided)?;

        let witness_sigs = first_witness
            .signatures()
            .ok_or(Error::SignaturesNotProvided)?;

        // Convert witness strings to signatures
        witness_sigs
            .iter()
            .map(|s| Signature::from_str(s))
            .collect::<Result<Vec<Signature>, _>>()
            .map_err(Error::from)
    }

    /// Check if swap request can be signed with the given secret key
    fn can_sign_sig_all(
        &self,
        secret_key: &SecretKey,
    ) -> Result<(SpendingConditions, PublicKey), Error> {
        // Get the first input since all must match for SIG_ALL
        let first_input = self.inputs().first().ok_or(Error::SpendConditionsNotMet)?;
        let first_conditions: SpendingConditions =
            SpendingConditions::try_from(&first_input.secret)?;

        // Verify this is a P2PK condition with SIG_ALL
        match first_conditions.clone() {
            SpendingConditions::P2PKConditions { conditions, .. } => {
                let conditions = conditions.ok_or(Error::IncorrectSecretKind)?;
                if conditions.sig_flag != SigFlag::SigAll {
                    return Err(Error::IncorrectSecretKind);
                }
                conditions
            }
            _ => return Err(Error::IncorrectSecretKind),
        };

        // Get authorized keys and verify secret_key matches one
        let pubkey = secret_key.public_key();
        let authorized_keys = first_conditions
            .pubkeys()
            .ok_or(Error::P2PKPubkeyRequired)?;

        if !authorized_keys.contains(&pubkey) {
            return Err(Error::SpendConditionsNotMet);
        }

        Ok((first_conditions, pubkey))
    }

    /// Sign swap request with SIG_ALL if conditions are met
    pub fn sign_sig_all(&mut self, secret_key: SecretKey) -> Result<(), Error> {
        // Verify we can sign and get conditions
        let (_first_conditions, _) = self.can_sign_sig_all(&secret_key)?;

        // Verify all inputs have matching conditions
        self.verify_matching_conditions()?;

        // Get message to sign
        let msg = self.sig_all_msg_to_sign();
        let signature = secret_key.sign(msg.as_bytes())?;

        // Add signature to first input witness
        let first_input = self
            .inputs_mut()
            .first_mut()
            .ok_or(Error::IncorrectSecretKind)?;

        match first_input.witness.as_mut() {
            Some(witness) => {
                witness.add_signatures(vec![signature.to_string()]);
            }
            None => {
                let mut p2pk_witness = Witness::P2PKWitness(P2PKWitness::default());
                p2pk_witness.add_signatures(vec![signature.to_string()]);
                first_input.witness = Some(p2pk_witness);
            }
        };

        Ok(())
    }

    /// Validate SIG_ALL conditions and signatures for the swap request
    pub fn verify_sig_all(&self) -> Result<(), Error> {
        // Get required signatures and conditions from first input
        let (required_sigs, first_conditions) = self.get_sig_all_required_sigs()?;

        // Verify all inputs have matching secrets
        self.verify_matching_conditions()?;

        // Get and validate witness signatures
        let signatures = self.get_valid_witness_signatures()?;

        // Get signing pubkeys
        let verifying_pubkeys = first_conditions
            .pubkeys()
            .ok_or(Error::P2PKPubkeyRequired)?;

        // Get aggregated message and validate signatures
        let msg = self.sig_all_msg_to_sign();
        let valid_sigs = valid_signatures(msg.as_bytes(), &verifying_pubkeys, &signatures)?;

        if valid_sigs >= required_sigs {
            Ok(())
        } else {
            Err(Error::SpendConditionsNotMet)
        }
    }
}

impl<Q: std::fmt::Display + Serialize + DeserializeOwned> MeltRequest<Q> {
    /// Generate the message to sign for SIG_ALL validation
    /// Concatenates all input secrets, blank outputs, and quote ID in order
    fn sig_all_msg_to_sign(&self) -> String {
        let mut msg_to_sign = String::new();

        // Add all input secrets in order
        for proof in self.inputs() {
            let secret = proof.secret.to_string();
            msg_to_sign.push_str(&secret);
        }

        // Add all blank outputs in order if they exist
        if let Some(outputs) = self.outputs() {
            for output in outputs {
                msg_to_sign.push_str(&output.blinded_secret.to_hex());
            }
        }

        // Add quote ID
        msg_to_sign.push_str(&self.quote().to_string());

        msg_to_sign
    }

    /// Get required signature count from first input's spending conditions
    fn get_sig_all_required_sigs(&self) -> Result<(u64, SpendingConditions), Error> {
        let first_input = self.inputs().first().ok_or(Error::SpendConditionsNotMet)?;
        let first_conditions: SpendingConditions =
            SpendingConditions::try_from(&first_input.secret)?;

        let required_sigs = match first_conditions.clone() {
            SpendingConditions::P2PKConditions { conditions, .. } => {
                let conditions = conditions.ok_or(Error::IncorrectSecretKind)?;

                if SigFlag::SigAll != conditions.sig_flag {
                    return Err(Error::IncorrectSecretKind);
                }

                conditions.num_sigs.unwrap_or(1)
            }
            _ => return Err(Error::IncorrectSecretKind),
        };

        Ok((required_sigs, first_conditions))
    }

    /// Verify all inputs have matching secrets and tags
    fn verify_matching_conditions(&self) -> Result<(), Error> {
        let first_input = self.inputs().first().ok_or(Error::SpendConditionsNotMet)?;
        let first_nut10: Nut10Secret = (&first_input.secret).try_into()?;

        for proof in self.inputs().iter().skip(1) {
            let current_secret: Nut10Secret = proof.secret.clone().try_into()?;

            // Check data matches
            if current_secret.secret_data().data() != first_nut10.secret_data().data() {
                return Err(Error::SpendConditionsNotMet);
            }

            // Check tags match
            if current_secret.secret_data().tags() != first_nut10.secret_data().tags() {
                return Err(Error::SpendConditionsNotMet);
            }
        }
        Ok(())
    }

    /// Get validated signatures from first input's witness
    fn get_valid_witness_signatures(&self) -> Result<Vec<Signature>, Error> {
        let first_input = self.inputs().first().ok_or(Error::SpendConditionsNotMet)?;
        let first_witness = first_input
            .witness
            .as_ref()
            .ok_or(Error::SignaturesNotProvided)?;

        let witness_sigs = first_witness
            .signatures()
            .ok_or(Error::SignaturesNotProvided)?;

        // Convert witness strings to signatures
        witness_sigs
            .iter()
            .map(|s| Signature::from_str(s))
            .collect::<Result<Vec<Signature>, _>>()
            .map_err(Error::from)
    }

    /// Check if melt request can be signed with the given secret key
    fn can_sign_sig_all(
        &self,
        secret_key: &SecretKey,
    ) -> Result<(SpendingConditions, PublicKey), Error> {
        // Get the first input since all must match for SIG_ALL
        let first_input = self.inputs().first().ok_or(Error::SpendConditionsNotMet)?;
        let first_conditions: SpendingConditions =
            SpendingConditions::try_from(&first_input.secret)?;

        // Verify this is a P2PK condition with SIG_ALL
        match first_conditions.clone() {
            SpendingConditions::P2PKConditions { conditions, .. } => {
                let conditions = conditions.ok_or(Error::IncorrectSecretKind)?;
                if conditions.sig_flag != SigFlag::SigAll {
                    return Err(Error::IncorrectSecretKind);
                }
                conditions
            }
            _ => return Err(Error::IncorrectSecretKind),
        };

        // Get authorized keys and verify secret_key matches one
        let pubkey = secret_key.public_key();
        let authorized_keys = first_conditions
            .pubkeys()
            .ok_or(Error::P2PKPubkeyRequired)?;

        if !authorized_keys.contains(&pubkey) {
            return Err(Error::SpendConditionsNotMet);
        }

        Ok((first_conditions, pubkey))
    }

    /// Sign melt request with SIG_ALL if conditions are met
    pub fn sign_sig_all(&mut self, secret_key: SecretKey) -> Result<(), Error> {
        // Verify we can sign and get conditions
        let (_first_conditions, _) = self.can_sign_sig_all(&secret_key)?;

        // Verify all inputs have matching conditions
        self.verify_matching_conditions()?;

        // Get message to sign
        let msg = self.sig_all_msg_to_sign();
        let signature = secret_key.sign(msg.as_bytes())?;

        // Add signature to first input witness
        let first_input = self
            .inputs_mut()
            .first_mut()
            .ok_or(Error::SpendConditionsNotMet)?;

        match first_input.witness.as_mut() {
            Some(witness) => {
                witness.add_signatures(vec![signature.to_string()]);
            }
            None => {
                let mut p2pk_witness = Witness::P2PKWitness(P2PKWitness::default());
                p2pk_witness.add_signatures(vec![signature.to_string()]);
                first_input.witness = Some(p2pk_witness);
            }
        };

        Ok(())
    }

    /// Validate SIG_ALL conditions and signatures for the melt request
    pub fn verify_sig_all(&self) -> Result<(), Error> {
        // Get required signatures and conditions from first input
        let (required_sigs, first_conditions) = self.get_sig_all_required_sigs()?;

        // Verify all inputs have matching secrets
        self.verify_matching_conditions()?;

        // Get and validate witness signatures
        let signatures = self.get_valid_witness_signatures()?;

        // Get signing pubkeys
        let verifying_pubkeys = first_conditions
            .pubkeys()
            .ok_or(Error::P2PKPubkeyRequired)?;

        // Get aggregated message and validate signatures
        let msg = self.sig_all_msg_to_sign();
        let valid_sigs = valid_signatures(msg.as_bytes(), &verifying_pubkeys, &signatures)?;

        if valid_sigs >= required_sigs {
            Ok(())
        } else {
            Err(Error::SpendConditionsNotMet)
        }
    }
}

impl Serialize for Tag {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let data: Vec<String> = self.as_vec();
        let mut seq = serializer.serialize_seq(Some(data.len()))?;
        for element in data.into_iter() {
            seq.serialize_element(&element)?;
        }
        seq.end()
    }
}

impl<'de> Deserialize<'de> for Tag {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        type Data = Vec<String>;
        let vec: Vec<String> = Data::deserialize(deserializer)?;
        Self::try_from(vec).map_err(DeserializerError::custom)
    }
}

#[cfg(feature = "mint")]
#[cfg(test)]
mod tests {
    use std::str::FromStr;

    use uuid::Uuid;

    use super::*;
    use crate::nuts::Id;
    use crate::quote_id::QuoteId;
    use crate::secret::Secret;
    use crate::{Amount, BlindedMessage};

    #[test]
    fn test_secret_ser() {
        let data = PublicKey::from_str(
            "033281c37677ea273eb7183b783067f5244933ef78d8c3f15b1a77cb246099c26e",
        )
        .unwrap();

        let conditions = Conditions {
            locktime: Some(99999),
            pubkeys: Some(vec![
                PublicKey::from_str(
                    "02698c4e2b5f9534cd0687d87513c759790cf829aa5739184a3e3735471fbda904",
                )
                .unwrap(),
                PublicKey::from_str(
                    "023192200a0cfd3867e48eb63b03ff599c7e46c8f4e41146b2d281173ca6c50c54",
                )
                .unwrap(),
            ]),
            refund_keys: Some(vec![PublicKey::from_str(
                "033281c37677ea273eb7183b783067f5244933ef78d8c3f15b1a77cb246099c26e",
            )
            .unwrap()]),
            num_sigs: Some(2),
            sig_flag: SigFlag::SigAll,
            num_sigs_refund: None,
        };

        let secret: Nut10Secret = Nut10Secret::new(Kind::P2PK, data.to_string(), Some(conditions));

        let secret_str = serde_json::to_string(&secret).unwrap();

        let secret_der: Nut10Secret = serde_json::from_str(&secret_str).unwrap();

        assert_eq!(secret_der, secret);
    }

    #[test]
    fn sign_proof() {
        let secret_key =
            SecretKey::from_str("99590802251e78ee1051648439eedb003dc539093a48a44e7b8f2642c909ea37")
                .unwrap();

        let signing_key_two =
            SecretKey::from_str("0000000000000000000000000000000000000000000000000000000000000001")
                .unwrap();

        let signing_key_three =
            SecretKey::from_str("7f7f7f7f7f7f7f7f7f7f7f7f7f7f7f7f7f7f7f7f7f7f7f7f7f7f7f7f7f7f7f7f")
                .unwrap();
        let v_key: PublicKey = secret_key.public_key();
        let v_key_two: PublicKey = signing_key_two.public_key();
        let v_key_three: PublicKey = signing_key_three.public_key();

        let conditions = Conditions {
            locktime: Some(21000000000),
            pubkeys: Some(vec![v_key_two, v_key_three]),
            refund_keys: Some(vec![v_key]),
            num_sigs: Some(2),
            sig_flag: SigFlag::SigInputs,
            num_sigs_refund: None,
        };

        let secret: Secret = Nut10Secret::new(Kind::P2PK, v_key.to_string(), Some(conditions))
            .try_into()
            .unwrap();

        let mut proof = Proof {
            keyset_id: Id::from_str("009a1f293253e41e").unwrap(),
            amount: Amount::ZERO,
            secret,
            c: PublicKey::from_str(
                "02698c4e2b5f9534cd0687d87513c759790cf829aa5739184a3e3735471fbda904",
            )
            .unwrap(),
            witness: Some(Witness::P2PKWitness(P2PKWitness { signatures: vec![] })),
            dleq: None,
        };

        proof.sign_p2pk(secret_key).unwrap();
        proof.sign_p2pk(signing_key_two).unwrap();

        assert!(proof.verify_p2pk().is_ok());
    }

    #[test]
    fn test_verify() {
        // Proof with a valid signature
        let json: &str = r#"{
            "amount":1,
            "secret":"[\"P2PK\",{\"nonce\":\"859d4935c4907062a6297cf4e663e2835d90d97ecdd510745d32f6816323a41f\",\"data\":\"0249098aa8b9d2fbec49ff8598feb17b592b986e62319a4fa488a3dc36387157a7\",\"tags\":[[\"sigflag\",\"SIG_INPUTS\"]]}]",
            "C":"02698c4e2b5f9534cd0687d87513c759790cf829aa5739184a3e3735471fbda904",
            "id":"009a1f293253e41e",
            "witness":"{\"signatures\":[\"60f3c9b766770b46caac1d27e1ae6b77c8866ebaeba0b9489fe6a15a837eaa6fcd6eaa825499c72ac342983983fd3ba3a8a41f56677cc99ffd73da68b59e1383\"]}"
        }"#;
        let valid_proof: Proof = serde_json::from_str(json).unwrap();

        valid_proof.verify_p2pk().unwrap();
        assert!(valid_proof.verify_p2pk().is_ok());

        // Proof with a signature that is in a different secret
        let invalid_proof = r#"{"amount":1,"secret":"[\"P2PK\",{\"nonce\":\"859d4935c4907062a6297cf4e663e2835d90d97ecdd510745d32f6816323a41f\",\"data\":\"0249098aa8b9d2fbec49ff8598feb17b592b986e62319a4fa488a3dc36387157a7\",\"tags\":[[\"sigflag\",\"SIG_INPUTS\"]]}]","C":"02698c4e2b5f9534cd0687d87513c759790cf829aa5739184a3e3735471fbda904","id":"009a1f293253e41e","witness":"{\"signatures\":[\"3426df9730d365a9d18d79bed2f3e78e9172d7107c55306ac5ddd1b2d065893366cfa24ff3c874ebf1fc22360ba5888ddf6ff5dbcb9e5f2f5a1368f7afc64f15\"]}"}"#;

        let invalid_proof: Proof = serde_json::from_str(invalid_proof).unwrap();

        assert!(invalid_proof.verify_p2pk().is_err());
    }

    #[test]
    fn verify_multi_sig() {
        // Proof with 2 valid signatures to satifiy the condition
        let valid_proof = r#"{"amount":0,"secret":"[\"P2PK\",{\"nonce\":\"0ed3fcb22c649dd7bbbdcca36e0c52d4f0187dd3b6a19efcc2bfbebb5f85b2a1\",\"data\":\"0249098aa8b9d2fbec49ff8598feb17b592b986e62319a4fa488a3dc36387157a7\",\"tags\":[[\"pubkeys\",\"0279be667ef9dcbbac55a06295ce870b07029bfcdb2dce28d959f2815b16f81798\",\"02142715675faf8da1ecc4d51e0b9e539fa0d52fdd96ed60dbe99adb15d6b05ad9\"],[\"n_sigs\",\"2\"],[\"sigflag\",\"SIG_INPUTS\"]]}]","C":"02698c4e2b5f9534cd0687d87513c759790cf829aa5739184a3e3735471fbda904","id":"009a1f293253e41e","witness":"{\"signatures\":[\"83564aca48c668f50d022a426ce0ed19d3a9bdcffeeaee0dc1e7ea7e98e9eff1840fcc821724f623468c94f72a8b0a7280fa9ef5a54a1b130ef3055217f467b3\",\"9a72ca2d4d5075be5b511ee48dbc5e45f259bcf4a4e8bf18587f433098a9cd61ff9737dc6e8022de57c76560214c4568377792d4c2c6432886cc7050487a1f22\"]}"}"#;

        let valid_proof: Proof = serde_json::from_str(valid_proof).unwrap();

        assert!(valid_proof.verify_p2pk().is_ok());

        // Proof with only one of the required signatures
        let invalid_proof = r#"{"amount":0,"secret":"[\"P2PK\",{\"nonce\":\"0ed3fcb22c649dd7bbbdcca36e0c52d4f0187dd3b6a19efcc2bfbebb5f85b2a1\",\"data\":\"0249098aa8b9d2fbec49ff8598feb17b592b986e62319a4fa488a3dc36387157a7\",\"tags\":[[\"pubkeys\",\"0279be667ef9dcbbac55a06295ce870b07029bfcdb2dce28d959f2815b16f81798\",\"02142715675faf8da1ecc4d51e0b9e539fa0d52fdd96ed60dbe99adb15d6b05ad9\"],[\"n_sigs\",\"2\"],[\"sigflag\",\"SIG_INPUTS\"]]}]","C":"02698c4e2b5f9534cd0687d87513c759790cf829aa5739184a3e3735471fbda904","id":"009a1f293253e41e","witness":"{\"signatures\":[\"83564aca48c668f50d022a426ce0ed19d3a9bdcffeeaee0dc1e7ea7e98e9eff1840fcc821724f623468c94f72a8b0a7280fa9ef5a54a1b130ef3055217f467b3\"]}"}"#;

        let invalid_proof: Proof = serde_json::from_str(invalid_proof).unwrap();

        // Verification should fail without the requires signatures
        assert!(invalid_proof.verify_p2pk().is_err());
    }

    #[test]
    fn verify_refund() {
        let valid_proof = r#"{"amount":1,"id":"009a1f293253e41e","secret":"[\"P2PK\",{\"nonce\":\"902685f492ef3bb2ca35a47ddbba484a3365d143b9776d453947dcbf1ddf9689\",\"data\":\"026f6a2b1d709dbca78124a9f30a742985f7eddd894e72f637f7085bf69b997b9a\",\"tags\":[[\"pubkeys\",\"0279be667ef9dcbbac55a06295ce870b07029bfcdb2dce28d959f2815b16f81798\",\"03142715675faf8da1ecc4d51e0b9e539fa0d52fdd96ed60dbe99adb15d6b05ad9\"],[\"locktime\",\"21\"],[\"n_sigs\",\"2\"],[\"refund\",\"026f6a2b1d709dbca78124a9f30a742985f7eddd894e72f637f7085bf69b997b9a\"],[\"sigflag\",\"SIG_INPUTS\"]]}]","C":"02698c4e2b5f9534cd0687d87513c759790cf829aa5739184a3e3735471fbda904","witness":"{\"signatures\":[\"710507b4bc202355c91ea3c147c0d0189c75e179d995e566336afd759cb342bcad9a593345f559d9b9e108ac2c9b5bd9f0b4b6a295028a98606a0a2e95eb54f7\"]}"}"#;

        let valid_proof: Proof = serde_json::from_str(valid_proof).unwrap();
        assert!(valid_proof.verify_p2pk().is_ok());

        let invalid_proof = r#"{"amount":1,"id":"009a1f293253e41e","secret":"[\"P2PK\",{\"nonce\":\"64c46e5d30df27286166814b71b5d69801704f23a7ad626b05688fbdb48dcc98\",\"data\":\"026f6a2b1d709dbca78124a9f30a742985f7eddd894e72f637f7085bf69b997b9a\",\"tags\":[[\"pubkeys\",\"0279be667ef9dcbbac55a06295ce870b07029bfcdb2dce28d959f2815b16f81798\",\"03142715675faf8da1ecc4d51e0b9e539fa0d52fdd96ed60dbe99adb15d6b05ad9\"],[\"locktime\",\"21\"],[\"n_sigs\",\"2\"],[\"refund\",\"0279be667ef9dcbbac55a06295ce870b07029bfcdb2dce28d959f2815b16f81798\"],[\"sigflag\",\"SIG_INPUTS\"]]}]","C":"02698c4e2b5f9534cd0687d87513c759790cf829aa5739184a3e3735471fbda904","witness":"{\"signatures\":[\"f661d3dc046d636d47cb3d06586da42c498f0300373d1c2a4f417a44252cdf3809bce207c8888f934dba0d2b1671f1b8622d526840f2d5883e571b462630c1ff\"]}"}"#;

        let invalid_proof: Proof = serde_json::from_str(invalid_proof).unwrap();

        assert!(invalid_proof.verify_p2pk().is_err());
    }

    #[test]
    fn sig_with_non_refund_keys_after_locktime() {
        let secret_key =
            SecretKey::from_str("99590802251e78ee1051648439eedb003dc539093a48a44e7b8f2642c909ea37")
                .unwrap();

        let signing_key_two =
            SecretKey::from_str("0000000000000000000000000000000000000000000000000000000000000001")
                .unwrap();

        let signing_key_three =
            SecretKey::from_str("7f7f7f7f7f7f7f7f7f7f7f7f7f7f7f7f7f7f7f7f7f7f7f7f7f7f7f7f7f7f7f7f")
                .unwrap();
        let v_key: PublicKey = secret_key.public_key();
        let v_key_two: PublicKey = signing_key_two.public_key();
        let v_key_three: PublicKey = signing_key_three.public_key();

        let conditions = Conditions {
            locktime: Some(21),
            pubkeys: Some(vec![v_key_three]),
            refund_keys: Some(vec![v_key, v_key_two]),
            num_sigs: None,
            sig_flag: SigFlag::SigInputs,
            num_sigs_refund: Some(2),
        };

        let secret: Secret = Nut10Secret::new(Kind::P2PK, v_key.to_string(), Some(conditions))
            .try_into()
            .unwrap();

        let mut proof = Proof {
            keyset_id: Id::from_str("009a1f293253e41e").unwrap(),
            amount: Amount::ZERO,
            secret,
            c: PublicKey::from_str(
                "02698c4e2b5f9534cd0687d87513c759790cf829aa5739184a3e3735471fbda904",
            )
            .unwrap(),
            witness: Some(Witness::P2PKWitness(P2PKWitness { signatures: vec![] })),
            dleq: None,
        };

        proof.sign_p2pk(signing_key_three.clone()).unwrap();

        assert!(proof.verify_p2pk().is_err());

        proof.witness = None;

        proof.sign_p2pk(secret_key).unwrap();
        assert!(proof.verify_p2pk().is_err());
        proof.sign_p2pk(signing_key_two).unwrap();

        assert!(proof.verify_p2pk().is_ok());
    }

    // Helper functions for melt request tests
    fn create_test_proof(secret: Secret, pubkey: PublicKey, id: &str) -> Proof {
        Proof {
            keyset_id: Id::from_str(id).unwrap(),
            amount: Amount::ZERO,
            secret,
            c: pubkey,
            witness: None,
            dleq: None,
        }
    }

    fn create_test_secret(pubkey: PublicKey, conditions: Conditions) -> Secret {
        Nut10Secret::new(Kind::P2PK, pubkey.to_string(), Some(conditions))
            .try_into()
            .unwrap()
    }

    fn create_test_blinded_msg(pubkey: PublicKey) -> BlindedMessage {
        BlindedMessage {
            amount: Amount::ZERO,
            blinded_secret: pubkey,
            keyset_id: Id::from_str("009a1f293253e41e").unwrap(),
            witness: None,
        }
    }

    #[test]
    fn test_melt_sig_all_basic_signing() {
        let secret_key =
            SecretKey::from_str("99590802251e78ee1051648439eedb003dc539093a48a44e7b8f2642c909ea37")
                .unwrap();
        let pubkey = secret_key.public_key();

        // Create conditions with SIG_ALL
        let conditions = Conditions {
            sig_flag: SigFlag::SigAll,
            pubkeys: Some(vec![pubkey]),
            ..Default::default()
        };

        let secret = create_test_secret(pubkey, conditions);
        let proof = create_test_proof(secret, pubkey, "009a1f293253e41e");
        let blinded_msg = create_test_blinded_msg(pubkey);

        // Create melt request
        let mut melt = MeltRequest::new(
            QuoteId::UUID(Uuid::new_v4()),
            vec![proof],
            Some(vec![blinded_msg]),
        );

        // Before signing, should fail verification
        assert!(
            melt.verify_sig_all().is_err(),
            "Unsigned melt request should fail verification"
        );

        // Sign the request
        assert!(
            melt.sign_sig_all(secret_key).is_ok(),
            "Signing should succeed"
        );

        // After signing, should pass verification
        assert!(
            melt.verify_sig_all().is_ok(),
            "Signed melt request should pass verification"
        );
    }

    #[test]
    fn test_melt_sig_all_unauthorized_key() {
        let secret_key =
            SecretKey::from_str("99590802251e78ee1051648439eedb003dc539093a48a44e7b8f2642c909ea37")
                .unwrap();
        let pubkey = secret_key.public_key();

        // Create conditions with explicit authorized pubkey
        let conditions = Conditions {
            sig_flag: SigFlag::SigAll,
            pubkeys: Some(vec![pubkey]),
            ..Default::default()
        };

        let secret = create_test_secret(pubkey, conditions);
        let proof = create_test_proof(secret, pubkey, "009a1f293253e41e");

        let mut melt = MeltRequest::new(Uuid::new_v4(), vec![proof], None);

        // Try to sign with unauthorized key
        let unauthorized_key =
            SecretKey::from_str("0000000000000000000000000000000000000000000000000000000000000001")
                .unwrap();

        assert!(
            melt.sign_sig_all(unauthorized_key).is_err(),
            "Signing with unauthorized key should fail"
        );
    }

    #[test]
    fn test_melt_sig_all_wrong_flag() {
        let secret_key =
            SecretKey::from_str("99590802251e78ee1051648439eedb003dc539093a48a44e7b8f2642c909ea37")
                .unwrap();
        let pubkey = secret_key.public_key();

        // Create conditions with SIG_INPUTS instead of SIG_ALL
        let conditions = Conditions {
            sig_flag: SigFlag::SigInputs,
            pubkeys: Some(vec![pubkey]),
            ..Default::default()
        };

        let secret = create_test_secret(pubkey, conditions);
        let proof = create_test_proof(secret, pubkey, "009a1f293253e41e");

        let mut melt = MeltRequest::new(Uuid::new_v4(), vec![proof], None);

        assert!(
            melt.sign_sig_all(secret_key).is_err(),
            "Signing with SIG_INPUTS flag should fail"
        );
    }

    #[test]
    fn test_melt_sig_all_multiple_inputs() {
        let secret_key =
            SecretKey::from_str("99590802251e78ee1051648439eedb003dc539093a48a44e7b8f2642c909ea37")
                .unwrap();
        let pubkey = secret_key.public_key();

        // Create conditions
        let conditions = Conditions {
            sig_flag: SigFlag::SigAll,
            pubkeys: Some(vec![pubkey]),
            ..Default::default()
        };

        let secret = create_test_secret(pubkey, conditions);

        // Create two proofs with same secret
        let proof1 = create_test_proof(secret.clone(), pubkey, "009a1f293253e41e");
        let proof2 = create_test_proof(secret, pubkey, "009a1f293253e41f");

        let mut melt = MeltRequest::new(Uuid::new_v4(), vec![proof1, proof2], None);

        // Signing should work with multiple matching inputs
        assert!(
            melt.sign_sig_all(secret_key).is_ok(),
            "Signing with multiple matching inputs should succeed"
        );
        assert!(
            melt.verify_sig_all().is_ok(),
            "Verification should succeed with multiple matching inputs"
        );
    }

    #[test]
    fn test_melt_sig_all_mismatched_inputs() {
        let secret_key =
            SecretKey::from_str("99590802251e78ee1051648439eedb003dc539093a48a44e7b8f2642c909ea37")
                .unwrap();
        let pubkey = secret_key.public_key();

        // Create first secret and proof
        let conditions1 = Conditions {
            sig_flag: SigFlag::SigAll,
            ..Default::default()
        };
        let secret1 = create_test_secret(pubkey, conditions1.clone());
        let proof1 = create_test_proof(secret1, pubkey, "009a1f293253e41e");

        // Create second secret with different data
        let conditions2 = conditions1.clone();
        let secret2 = Nut10Secret::new(
            Kind::P2PK,
            "02698c4e2b5f9534cd0687d87513c759790cf829aa5739184a3e3735471fbda904",
            Some(conditions2),
        )
        .try_into()
        .unwrap();
        let proof2 = create_test_proof(secret2, pubkey, "009a1f293253e41f");

        let mut melt = MeltRequest::new(Uuid::new_v4(), vec![proof1, proof2], None);

        assert!(
            melt.sign_sig_all(secret_key).is_err(),
            "Signing with mismatched input secrets should fail"
        );
    }

    #[test]
    fn test_melt_sig_all_multiple_signatures() {
        let secret_key1 =
            SecretKey::from_str("99590802251e78ee1051648439eedb003dc539093a48a44e7b8f2642c909ea37")
                .unwrap();
        let pubkey1 = secret_key1.public_key();

        let secret_key2 =
            SecretKey::from_str("7f7f7f7f7f7f7f7f7f7f7f7f7f7f7f7f7f7f7f7f7f7f7f7f7f7f7f7f7f7f7f7f")
                .unwrap();
        let pubkey2 = secret_key2.public_key();

        // Create conditions requiring 2 signatures
        let conditions = Conditions {
            sig_flag: SigFlag::SigAll,
            num_sigs: Some(2),
            pubkeys: Some(vec![pubkey1, pubkey2]),
            ..Default::default()
        };

        let secret = create_test_secret(pubkey1, conditions);
        let proof = create_test_proof(secret, pubkey1, "009a1f293253e41e");

        let mut melt = MeltRequest::new(
            Uuid::new_v4(),
            vec![proof],
            Some(vec![create_test_blinded_msg(
                SecretKey::generate().public_key(),
            )]),
        );

        // First signature
        assert!(
            melt.sign_sig_all(secret_key1).is_ok(),
            "First signature should succeed"
        );
        assert!(
            melt.verify_sig_all().is_err(),
            "Single signature should not verify when two required"
        );

        // Second signature
        assert!(
            melt.sign_sig_all(secret_key2).is_ok(),
            "Second signature should succeed"
        );

        assert!(
            melt.verify_sig_all().is_ok(),
            "Both signatures should verify successfully"
        );
    }

    #[test]
    fn test_melt_sig_all_message_components() {
        let secret_key =
            SecretKey::from_str("99590802251e78ee1051648439eedb003dc539093a48a44e7b8f2642c909ea37")
                .unwrap();
        let pubkey = secret_key.public_key();

        let conditions = Conditions {
            sig_flag: SigFlag::SigAll,
            pubkeys: Some(vec![pubkey]),
            ..Default::default()
        };

        let secret = create_test_secret(pubkey, conditions);
        let proof = create_test_proof(secret.clone(), pubkey, "009a1f293253e41e");
        let blinded_msg = create_test_blinded_msg(pubkey);
        let quote_id = Uuid::new_v4();

        let melt = MeltRequest::new(quote_id, vec![proof], Some(vec![blinded_msg.clone()]));

        // Get message to sign
        let msg = melt.sig_all_msg_to_sign();

        // Verify all components are present in the message
        assert!(
            msg.contains(&secret.to_string()),
            "Message should contain secret"
        );
        assert!(
            msg.contains(&blinded_msg.blinded_secret.to_hex()),
            "Message should contain blinded message in hex format"
        );
        assert!(
            msg.contains(&quote_id.to_string()),
            "Message should contain quote ID"
        );
    }

    #[test]
    fn test_sig_all_swap_single_sig() {
        // Valid SwapRequest with SIG_ALL signature
        let valid_swap = r#"{"inputs":[{"amount":0,"id":"009a1f293253e41e","secret":"[\"P2PK\",{\"nonce\":\"fc14ca312b7442d05231239d0e3cdcb6b2335250defcb8bec7d2efe9e26c90a6\",\"data\":\"026f6a2b1d709dbca78124a9f30a742985f7eddd894e72f637f7085bf69b997b9a\",\"tags\":[[\"sigflag\",\"SIG_ALL\"]]}]","C":"026f6a2b1d709dbca78124a9f30a742985f7eddd894e72f637f7085bf69b997b9a","witness":"{\"signatures\":[\"aa6f3b3f112ec3e834aded446ea67a90cdb26b43e08cfed259e0bbd953395c4af11117c58ec0ec3de404f31076692426cde40d2c1602d9dd067a872cb11ac3c0\"]}"},{"amount":0,"id":"009a1f293253e41f","secret":"[\"P2PK\",{\"nonce\":\"fc14ca312b7442d05231239d0e3cdcb6b2335250defcb8bec7d2efe9e26c90a6\",\"data\":\"026f6a2b1d709dbca78124a9f30a742985f7eddd894e72f637f7085bf69b997b9a\",\"tags\":[[\"sigflag\",\"SIG_ALL\"]]}]","C":"026f6a2b1d709dbca78124a9f30a742985f7eddd894e72f637f7085bf69b997b9a"}],"outputs":[{"amount":0,"id":"009a1f293253e41e","B_":"026f6a2b1d709dbca78124a9f30a742985f7eddd894e72f637f7085bf69b997b9a"}]}"#;

        let valid_swap: SwapRequest = serde_json::from_str(valid_swap).unwrap();
        assert!(
            valid_swap.verify_sig_all().is_ok(),
            "Valid SIG_ALL swap request should verify"
        );
    }

    #[test]
    fn test_sig_all_swap_mismatched_inputs() {
        // Invalid SwapRequest - mismatched inputs with SIG_ALL
        let invalid_swap = r#"{
            "inputs": [{
                "amount": 1,
                "secret": "[\"P2PK\",{\"nonce\":\"859d4935c4907062a6297cf4e663e2835d90d97ecdd510745d32f6816323a41f\",\"data\":\"0249098aa8b9d2fbec49ff8598feb17b592b986e62319a4fa488a3dc36387157a7\",\"tags\":[[\"sigflag\",\"SIG_ALL\"]]}]",
                "C": "02698c4e2b5f9534cd0687d87513c759790cf829aa5739184a3e3735471fbda904",
                "id": "009a1f293253e41e",
                "witness": "{\"signatures\":[\"60f3c9b766770b46caac1d27e1ae6b77c8866ebaeba0b9489fe6a15a837eaa6fcd6eaa825499c72ac342983983fd3ba3a8a41f56677cc99ffd73da68b59e1383\"]}"
            }, {
                "amount": 1,
                "secret": "[\"P2PK\",{\"nonce\":\"859d4935c4907062a6297cf4e663e2835d90d97ecdd510745d32f6816323a41f\",\"data\":\"02a60c27104cf6023581e790970fc33994a320abe36e7ceed16771b0f8d76f0666\",\"tags\":[[\"sigflag\",\"SIG_ALL\"]]}]",
                "C": "02698c4e2b5f9534cd0687d87513c759790cf829aa5739184a3e3735471fbda904",
                "id": "009a1f293253e41f",
                "witness": "{\"signatures\":[\"60f3c9b766770b46caac1d27e1ae6b77c8866ebaeba0b9489fe6a15a837eaa6fcd6eaa825499c72ac342983983fd3ba3a8a41f56677cc99ffd73da68b59e1383\"]}"
            }],
            "outputs": [{
                "amount": 2,
                "B_": "02698c4e2b5f9534cd0687d87513c759790cf829aa5739184a3e3735471fbda904",
                "id": "009a1f293253e41e"
            }]
        }"#;

        let invalid_swap: SwapRequest = serde_json::from_str(invalid_swap).unwrap();
        assert!(
            invalid_swap.verify_sig_all().is_err(),
            "Invalid SIG_ALL swap request should fail verification"
        );
    }

    #[test]
    fn test_sig_all_swap_multi_sig() {
        // SwapRequest with multi-sig SIG_ALL requiring 2 signatures
        let multisig_swap = r#"{"inputs":[{"amount":0,"id":"009a1f293253e41e","secret":"[\"P2PK\",{\"nonce\":\"c537ea76c1ac9cfa44d15dac91a63315903a3b4afa8e4e20f868f87f65ff2d16\",\"data\":\"026f6a2b1d709dbca78124a9f30a742985f7eddd894e72f637f7085bf69b997b9a\",\"tags\":[[\"pubkeys\",\"03142715675faf8da1ecc4d51e0b9e539fa0d52fdd96ed60dbe99adb15d6b05ad9\"],[\"n_sigs\",\"2\"],[\"sigflag\",\"SIG_ALL\"]]}]","C":"026f6a2b1d709dbca78124a9f30a742985f7eddd894e72f637f7085bf69b997b9a","witness":"{\"signatures\":[\"c38cf7943f59206dc22734d39c17e342674a4025e6d3b424eb79d445a57257d57b45dd94fcd1b8dd8013e9240a4133bdef6523f64cd7288d890f3bbb8e3c6453\",\"f766dbb80e5c27de9a4770486e11e1bac0b1c4f782bf807a5189ea9c3e294559a3de4e217d3dfceafd4d9e8dcbfe4e9a188052d6dab9df07df7844224292de36\"]}"}],"outputs":[{"amount":0,"id":"009a1f293253e41e","B_":"026f6a2b1d709dbca78124a9f30a742985f7eddd894e72f637f7085bf69b997b9a"}]}"#;

        let multisig_swap: SwapRequest = serde_json::from_str(multisig_swap).unwrap();
        assert!(
            multisig_swap.verify_sig_all().is_ok(),
            "Multi-sig SIG_ALL swap request should verify with both signatures"
        );
    }

    #[test]
    fn test_sig_all_swap_msg_to_sign() {
        // SwapRequest with multi-sig SIG_ALL requiring 2 signatures
        let multisig_swap = r#"{"inputs":[{"amount":0,"id":"009a1f293253e41e","secret":"[\"P2PK\",{\"nonce\":\"c537ea76c1ac9cfa44d15dac91a63315903a3b4afa8e4e20f868f87f65ff2d16\",\"data\":\"026f6a2b1d709dbca78124a9f30a742985f7eddd894e72f637f7085bf69b997b9a\",\"tags\":[[\"pubkeys\",\"03142715675faf8da1ecc4d51e0b9e539fa0d52fdd96ed60dbe99adb15d6b05ad9\"],[\"n_sigs\",\"2\"],[\"sigflag\",\"SIG_ALL\"]]}]","C":"026f6a2b1d709dbca78124a9f30a742985f7eddd894e72f637f7085bf69b997b9a","witness":"{\"signatures\":[\"c38cf7943f59206dc22734d39c17e342674a4025e6d3b424eb79d445a57257d57b45dd94fcd1b8dd8013e9240a4133bdef6523f64cd7288d890f3bbb8e3c6453\",\"f766dbb80e5c27de9a4770486e11e1bac0b1c4f782bf807a5189ea9c3e294559a3de4e217d3dfceafd4d9e8dcbfe4e9a188052d6dab9df07df7844224292de36\"]}"}],"outputs":[{"amount":0,"id":"009a1f293253e41e","B_":"026f6a2b1d709dbca78124a9f30a742985f7eddd894e72f637f7085bf69b997b9a"}]}"#;

        let multisig_swap: SwapRequest = serde_json::from_str(multisig_swap).unwrap();

        let msg_to_sign = multisig_swap.sig_all_msg_to_sign();

        println!("{}", msg_to_sign);

        assert_eq!(
            msg_to_sign,
            r#"["P2PK",{"nonce":"c537ea76c1ac9cfa44d15dac91a63315903a3b4afa8e4e20f868f87f65ff2d16","data":"026f6a2b1d709dbca78124a9f30a742985f7eddd894e72f637f7085bf69b997b9a","tags":[["pubkeys","03142715675faf8da1ecc4d51e0b9e539fa0d52fdd96ed60dbe99adb15d6b05ad9"],["n_sigs","2"],["sigflag","SIG_ALL"]]}]026f6a2b1d709dbca78124a9f30a742985f7eddd894e72f637f7085bf69b997b9a"#
        )
    }

    #[test]
    fn test_sig_all_melt() {
        // MeltRequest with valid SIG_ALL signature
        let valid_melt = r#"{"quote":"0f983814-de91-46b8-8875-1b358a35298a","inputs":[{"amount":0,"id":"009a1f293253e41e","secret":"[\"P2PK\",{\"nonce\":\"600050bd36cccdc71dec82e97679fa3e7712c22ea33cf4fe69d4d78223757e57\",\"data\":\"026f6a2b1d709dbca78124a9f30a742985f7eddd894e72f637f7085bf69b997b9a\",\"tags\":[[\"pubkeys\",\"026f6a2b1d709dbca78124a9f30a742985f7eddd894e72f637f7085bf69b997b9a\"],[\"sigflag\",\"SIG_ALL\"]]}]","C":"026f6a2b1d709dbca78124a9f30a742985f7eddd894e72f637f7085bf69b997b9a","witness":"{\"signatures\":[\"b66c342654ccc95a62100f8f4a76afe1aea612c9c63383be3c7feb5110bb8eabe7ccaa9f117abd524be8c9a2e331e7d70248aeae337b9ce405625b3c49fc627d\"]}"}],"outputs":[{"amount":0,"id":"009a1f293253e41e","B_":"026f6a2b1d709dbca78124a9f30a742985f7eddd894e72f637f7085bf69b997b9a"}]}"#;

        let valid_melt: MeltRequest<String> = serde_json::from_str(valid_melt).unwrap();
        assert!(
            valid_melt.verify_sig_all().is_ok(),
            "Valid SIG_ALL melt request should verify"
        );
    }

    #[test]
    fn test_sig_all_melt_wrong_sig() {
        // Invalid MeltRequest - wrong signature for SIG_ALL
        let invalid_melt = r#"{
            "inputs": [{
                "amount": 1,
                "secret": "[\"P2PK\",{\"nonce\":\"859d4935c4907062a6297cf4e663e2835d90d97ecdd510745d32f6816323a41f\",\"data\":\"0249098aa8b9d2fbec49ff8598feb17b592b986e62319a4fa488a3dc36387157a7\",\"tags\":[[\"sigflag\",\"SIG_ALL\"]]}]",
                "C": "02698c4e2b5f9534cd0687d87513c759790cf829aa5739184a3e3735471fbda904",
                "id": "009a1f293253e41e",
                "witness": "{\"signatures\":[\"3426df9730d365a9d18d79bed2f3e78e9172d7107c55306ac5ddd1b2d065893366cfa24ff3c874ebf1fc22360ba5888ddf6ff5dbcb9e5f2f5a1368f7afc64f15\"]}"
            }],
            "quote": "test_quote_123",
            "outputs": null
        }"#;

        let invalid_melt: MeltRequest<String> = serde_json::from_str(invalid_melt).unwrap();
        assert!(
            invalid_melt.verify_sig_all().is_err(),
            "Invalid SIG_ALL melt request should fail verification"
        );
    }

    #[test]
    fn test_sig_all_melt_msg_to_sign() {
        let multisig_melt = r#"{"quote":"2fc40ad3-2f6a-4a7e-91fb-b8c2b5dc2bf7","inputs":[{"amount":0,"id":"009a1f293253e41e","secret":"[\"P2PK\",{\"nonce\":\"1d0db9cbd2aa7370a3d6e0e3ce5714758ed7a085e2f8da9814924100e1fc622e\",\"data\":\"026f6a2b1d709dbca78124a9f30a742985f7eddd894e72f637f7085bf69b997b9a\",\"tags\":[[\"pubkeys\",\"026f6a2b1d709dbca78124a9f30a742985f7eddd894e72f637f7085bf69b997b9a\",\"03142715675faf8da1ecc4d51e0b9e539fa0d52fdd96ed60dbe99adb15d6b05ad9\"],[\"n_sigs\",\"2\"],[\"sigflag\",\"SIG_ALL\"]]}]","C":"026f6a2b1d709dbca78124a9f30a742985f7eddd894e72f637f7085bf69b997b9a","witness":"{\"signatures\":[\"b2077717cfe43086582679ce3fbe1802f9b8652f93828c2e1a75b9e553c0ab66cd14b9c5f6c45a098375fe6583e106c7ccdb1421636daf893576e15815f3483f\",\"179f687c2236c3d0767f3b2af88478cad312e7f76183fb5781754494709334c578c7232dc57017d06b9130a406f8e3ece18245064cda4ef66808ed3ff68c933e\"]}"}],"outputs":[{"amount":0,"id":"009a1f293253e41e","B_":"028b708cfd03b38bdc0a561008119594106f0c563061ae3fbfc8981b5595fd4e2b"}]}"#;

        let multisig_melt: MeltRequest<String> = serde_json::from_str(multisig_melt).unwrap();

        let msg_to_sign = multisig_melt.sig_all_msg_to_sign();

        assert_eq!(
            msg_to_sign,
            r#"["P2PK",{"nonce":"1d0db9cbd2aa7370a3d6e0e3ce5714758ed7a085e2f8da9814924100e1fc622e","data":"026f6a2b1d709dbca78124a9f30a742985f7eddd894e72f637f7085bf69b997b9a","tags":[["pubkeys","026f6a2b1d709dbca78124a9f30a742985f7eddd894e72f637f7085bf69b997b9a","03142715675faf8da1ecc4d51e0b9e539fa0d52fdd96ed60dbe99adb15d6b05ad9"],["n_sigs","2"],["sigflag","SIG_ALL"]]}]028b708cfd03b38bdc0a561008119594106f0c563061ae3fbfc8981b5595fd4e2b2fc40ad3-2f6a-4a7e-91fb-b8c2b5dc2bf7"#
        );
    }

    #[test]
    fn test_sig_all_melt_multi_sig() {
        // MeltRequest with multi-sig SIG_ALL requiring 2 signatures
        let multisig_melt = r#"{"quote":"2fc40ad3-2f6a-4a7e-91fb-b8c2b5dc2bf7","inputs":[{"amount":0,"id":"009a1f293253e41e","secret":"[\"P2PK\",{\"nonce\":\"1d0db9cbd2aa7370a3d6e0e3ce5714758ed7a085e2f8da9814924100e1fc622e\",\"data\":\"026f6a2b1d709dbca78124a9f30a742985f7eddd894e72f637f7085bf69b997b9a\",\"tags\":[[\"pubkeys\",\"026f6a2b1d709dbca78124a9f30a742985f7eddd894e72f637f7085bf69b997b9a\",\"03142715675faf8da1ecc4d51e0b9e539fa0d52fdd96ed60dbe99adb15d6b05ad9\"],[\"n_sigs\",\"2\"],[\"sigflag\",\"SIG_ALL\"]]}]","C":"026f6a2b1d709dbca78124a9f30a742985f7eddd894e72f637f7085bf69b997b9a","witness":"{\"signatures\":[\"b2077717cfe43086582679ce3fbe1802f9b8652f93828c2e1a75b9e553c0ab66cd14b9c5f6c45a098375fe6583e106c7ccdb1421636daf893576e15815f3483f\",\"179f687c2236c3d0767f3b2af88478cad312e7f76183fb5781754494709334c578c7232dc57017d06b9130a406f8e3ece18245064cda4ef66808ed3ff68c933e\"]}"}],"outputs":[{"amount":0,"id":"009a1f293253e41e","B_":"028b708cfd03b38bdc0a561008119594106f0c563061ae3fbfc8981b5595fd4e2b"}]}"#;

        let multisig_melt: MeltRequest<String> = serde_json::from_str(multisig_melt).unwrap();
        assert!(
            multisig_melt.verify_sig_all().is_ok(),
            "Multi-sig SIG_ALL melt request should verify with both signatures"
        );

        // MeltRequest with insufficient signatures for multi-sig SIG_ALL
        let insufficient_sigs_melt = r#"{
            "inputs": [{
                "amount": 1,
                "secret": "[\"P2PK\",{\"nonce\":\"859d4935c4907062a6297cf4e663e2835d90d97ecdd510745d32f6816323a41f\",\"data\":\"0249098aa8b9d2fbec49ff8598feb17b592b986e62319a4fa488a3dc36387157a7\",\"tags\":[[\"sigflag\",\"SIG_ALL\"],[\"pubkeys\",\"0279be667ef9dcbbac55a06295ce870b07029bfcdb2dce28d959f2815b16f81798\",\"02142715675faf8da1ecc4d51e0b9e539fa0d52fdd96ed60dbe99adb15d6b05ad9\"],[\"n_sigs\",\"2\"]]}]",
                "C": "02698c4e2b5f9534cd0687d87513c759790cf829aa5739184a3e3735471fbda904",
                "id": "009a1f293253e41e",
                "witness": "{\"signatures\":[\"83564aca48c668f50d022a426ce0ed19d3a9bdcffeeaee0dc1e7ea7e98e9eff1840fcc821724f623468c94f72a8b0a7280fa9ef5a54a1b130ef3055217f467b3\"]}"
            }],
            "quote": "test_quote_123",
            "outputs": null
        }"#;

        let insufficient_sigs_melt: MeltRequest<String> =
            serde_json::from_str(insufficient_sigs_melt).unwrap();
        assert!(
            insufficient_sigs_melt.verify_sig_all().is_err(),
            "Multi-sig SIG_ALL melt request should fail with insufficient signatures"
        );
    }

    // Helper functions for tests
    fn create_test_keys() -> (SecretKey, PublicKey) {
        let secret_key =
            SecretKey::from_str("99590802251e78ee1051648439eedb003dc539093a48a44e7b8f2642c909ea37")
                .unwrap();
        let pubkey = secret_key.public_key();
        (secret_key, pubkey)
    }

    #[test]
    fn test_sig_all_basic_signing_verification() {
        let (secret_key, pubkey) = create_test_keys();

        // Create basic SIG_ALL conditions
        let conditions = Conditions {
            sig_flag: SigFlag::SigAll,
            ..Default::default()
        };

        let secret = create_test_secret(pubkey, conditions);
        let proof1 = create_test_proof(secret.clone(), pubkey, "009a1f293253e41e");
        let proof2 = create_test_proof(secret, pubkey, "009a1f293253e41f");
        let blinded_msg = create_test_blinded_msg(pubkey);

        // Test basic signing flow
        let mut swap = SwapRequest::new(vec![proof1, proof2], vec![blinded_msg]);
        assert!(
            swap.verify_sig_all().is_err(),
            "Unsigned swap should fail verification"
        );

        assert!(
            swap.sign_sig_all(secret_key).is_ok(),
            "Signing should succeed"
        );

        println!("{}", serde_json::to_string(&swap).unwrap());

        assert!(
            swap.verify_sig_all().is_ok(),
            "Signed swap should pass verification"
        );
    }

    #[test]
    fn test_sig_all_unauthorized_key() {
        let (_secret_key, pubkey) = create_test_keys();

        let conditions = Conditions {
            sig_flag: SigFlag::SigAll,
            ..Default::default()
        };

        let secret = create_test_secret(pubkey, conditions);
        let proof = create_test_proof(secret, pubkey, "009a1f293253e41e");
        let blinded_msg = create_test_blinded_msg(pubkey);

        // Create unauthorized key
        let unauthorized_key =
            SecretKey::from_str("0000000000000000000000000000000000000000000000000000000000000001")
                .unwrap();

        let mut swap = SwapRequest::new(vec![proof], vec![blinded_msg]);
        assert!(
            swap.sign_sig_all(unauthorized_key).is_err(),
            "Signing with unauthorized key should fail"
        );
    }

    #[test]
    fn test_sig_all_mismatched_secrets() {
        let (secret_key, pubkey) = create_test_keys();

        let conditions = Conditions {
            sig_flag: SigFlag::SigAll,
            ..Default::default()
        };

        // Create first proof with original secret
        let secret1 = create_test_secret(pubkey, conditions.clone());

        // Create second proof with different secret data
        let different_secret = Nut10Secret::new(
            Kind::P2PK,
            "02698c4e2b5f9534cd0687d87513c759790cf829aa5739184a3e3735471fbda904",
            Some(conditions),
        )
        .try_into()
        .unwrap();

        let proof1 = create_test_proof(secret1, pubkey, "009a1f293253e41e");
        let proof2 = create_test_proof(different_secret, pubkey, "009a1f293253e41f");
        let blinded_msg = create_test_blinded_msg(pubkey);

        let mut swap = SwapRequest::new(vec![proof1, proof2], vec![blinded_msg]);
        assert!(
            swap.sign_sig_all(secret_key).is_err(),
            "Signing with mismatched secrets should fail"
        );
    }

    #[test]
    fn test_sig_all_wrong_flag() {
        let (secret_key, pubkey) = create_test_keys();

        // Create conditions with SIG_INPUTS instead of SIG_ALL
        let sig_inputs_conditions = Conditions {
            sig_flag: SigFlag::SigInputs,
            ..Default::default()
        };

        let secret = create_test_secret(pubkey, sig_inputs_conditions);
        let proof = create_test_proof(secret, pubkey, "009a1f293253e41e");
        let blinded_msg = create_test_blinded_msg(pubkey);

        let mut swap = SwapRequest::new(vec![proof], vec![blinded_msg]);
        assert!(
            swap.sign_sig_all(secret_key).is_err(),
            "Signing with SIG_INPUTS flag should fail"
        );
    }

    #[test]
    fn test_sig_all_multiple_signatures() {
        let (secret_key1, pubkey1) = create_test_keys();
        let secret_key2 =
            SecretKey::from_str("7f7f7f7f7f7f7f7f7f7f7f7f7f7f7f7f7f7f7f7f7f7f7f7f7f7f7f7f7f7f7f7f")
                .unwrap();
        let pubkey2 = secret_key2.public_key();

        // Create conditions requiring 2 signatures
        let conditions = Conditions {
            num_sigs: Some(2),
            sig_flag: SigFlag::SigAll,
            pubkeys: Some(vec![pubkey2]),
            ..Default::default()
        };

        let secret = create_test_secret(pubkey1, conditions);
        let proof = create_test_proof(secret, pubkey1, "009a1f293253e41e");
        let blinded_msg = create_test_blinded_msg(pubkey1);

        let mut swap = SwapRequest::new(vec![proof], vec![blinded_msg]);

        // Sign with first key
        assert!(
            swap.sign_sig_all(secret_key1).is_ok(),
            "First signature should succeed"
        );
        assert!(
            swap.verify_sig_all().is_err(),
            "Single signature should not verify when two required"
        );

        // Sign with second key
        assert!(
            swap.sign_sig_all(secret_key2).is_ok(),
            "Second signature should succeed"
        );

        assert!(
            swap.verify_sig_all().is_ok(),
            "Both signatures should verify"
        );
    }
}
