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
use super::nut10::SpendingConditionVerification;
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
    /// Preimage not supported in P2PK
    #[error("P2PK does not support preimage requirements")]
    PreimageNotSupportedInP2PK,
    /// SIG_ALL not supported in this context
    #[error("SIG_ALL proofs must be verified using a different method")]
    SigAllNotSupportedHere,
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
    ///
    /// Per NUT-11, there are two spending pathways after locktime:
    /// 1. Primary path (data + pubkeys): ALWAYS available
    /// 2. Refund path (refund keys): available AFTER locktime
    ///
    /// The verification tries both paths - if either succeeds, the proof is valid.
    pub fn verify_p2pk(&self) -> Result<(), Error> {
        let secret: Nut10Secret = self.secret.clone().try_into()?;
        let spending_conditions: Conditions = secret
            .secret_data()
            .tags()
            .cloned()
            .unwrap_or_default()
            .try_into()?;

        if spending_conditions.sig_flag == SigFlag::SigAll {
            return Err(Error::SigAllNotSupportedHere);
        }

        if secret.kind() != Kind::P2PK {
            return Err(Error::IncorrectSecretKind);
        }

        // Get spending requirements (includes both primary and refund paths)
        let now = unix_time();
        let requirements = super::nut10::get_pubkeys_and_required_sigs(&secret, now)?;

        if requirements.preimage_needed {
            return Err(Error::PreimageNotSupportedInP2PK);
        }

        // Extract witness signatures
        let witness_signatures = match &self.witness {
            Some(witness) => witness.signatures(),
            None => None,
        };

        let msg: &[u8] = self.secret.as_bytes();

        // Try primary path first (data + pubkeys)
        // Per NUT-11: "Locktime Multisig conditions continue to apply"
        {
            let primary_valid = witness_signatures
                .as_ref()
                .and_then(|sigs| {
                    sigs.iter()
                        .map(|s| Signature::from_str(s))
                        .collect::<Result<Vec<_>, _>>()
                        .ok()
                })
                .and_then(|sigs| valid_signatures(msg, &requirements.pubkeys, &sigs).ok())
                .is_some_and(|count| count >= requirements.required_sigs);

            if primary_valid {
                return Ok(());
            }
        }

        // Primary path failed or no signatures - try refund path if available
        {
            if let Some(refund_path) = &requirements.refund_path {
                // Anyone can spend (locktime passed, no refund keys)
                if refund_path.required_sigs == 0 {
                    return Ok(());
                }

                // Need signatures for refund path
                let refund_valid = witness_signatures
                    .as_ref()
                    .and_then(|sigs| {
                        sigs.iter()
                            .map(|s| Signature::from_str(s))
                            .collect::<Result<Vec<_>, _>>()
                            .ok()
                    })
                    .and_then(|sigs| valid_signatures(msg, &refund_path.pubkeys, &sigs).ok())
                    .is_some_and(|count| count >= refund_path.required_sigs);

                if refund_valid {
                    return Ok(());
                }
            }
        }

        // Neither path succeeded
        if witness_signatures.is_none() {
            Err(Error::SignaturesNotProvided)
        } else {
            Err(Error::SpendConditionsNotMet)
        }
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
            .map(|t| Tag::try_from(t).map(|tag| (tag.kind(), tag)))
            .collect::<Result<_, _>>()?;

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
    /// Sign swap request with SIG_ALL
    pub fn sign_sig_all(&mut self, secret_key: SecretKey) -> Result<(), Error> {
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
}

impl<Q> MeltRequest<Q>
where
    Q: std::fmt::Display + Serialize + DeserializeOwned,
{
    /// Sign melt request with SIG_ALL
    pub fn sign_sig_all(&mut self, secret_key: SecretKey) -> Result<(), Error> {
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

        // Per NUT-11: primary path (pubkeys) is ALWAYS available, even after locktime
        // Signing with a key from pubkeys should succeed
        assert!(proof.verify_p2pk().is_ok());

        proof.witness = None;

        // Sign with secret_key (pubkey = v_key, which is the data key and part of primary path)
        // Per NUT-11: primary path is always available, and data key is part of primary path
        proof.sign_p2pk(secret_key).unwrap();
        assert!(
            proof.verify_p2pk().is_ok(),
            "Data key signature should satisfy primary path"
        );

        // Adding more signatures still works (but wasn't needed for primary path)
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
            melt.verify_spending_conditions().is_err(),
            "Unsigned melt request should fail verification"
        );

        // Sign the request
        assert!(
            melt.sign_sig_all(secret_key).is_ok(),
            "Signing should succeed"
        );

        // After signing, should pass verification
        assert!(
            melt.verify_spending_conditions().is_ok(),
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

        // Sign with unauthorized key
        let unauthorized_key =
            SecretKey::from_str("0000000000000000000000000000000000000000000000000000000000000001")
                .unwrap();
        melt.sign_sig_all(unauthorized_key).unwrap();

        // Verification should fail (unauthorized signature)
        assert!(
            melt.verify_spending_conditions().is_err(),
            "Verification should fail with unauthorized key signature"
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

        // Signing
        melt.sign_sig_all(secret_key).unwrap();

        // Verification should fail (wrong flag - expected SIG_ALL)
        assert!(
            melt.verify_spending_conditions().is_err(),
            "Verification should fail with SIG_INPUTS flag when expecting SIG_ALL"
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
            melt.verify_spending_conditions().is_ok(),
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

        // Signing should succeed (no validation during signing)
        melt.sign_sig_all(secret_key).unwrap();

        // Verification should fail (catches mismatched inputs)
        assert!(
            melt.verify_spending_conditions().is_err(),
            "Verification should fail with mismatched input secrets"
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
            pubkeys: Some(vec![pubkey2]),
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
            melt.verify_spending_conditions().is_err(),
            "Single signature should not verify when two required"
        );

        // Second signature
        assert!(
            melt.sign_sig_all(secret_key2).is_ok(),
            "Second signature should succeed"
        );

        assert!(
            melt.verify_spending_conditions().is_ok(),
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

    // "SIG_ALL Test Vectors", starting with swaps : https://github.com/cashubtc/nuts/blob/5b050c7960607cca0481c28517cab5cc091b5d2e/tests/11-test.md#sig_all-test-vectors
    #[test]
    fn test_sig_all_swap_single_sig() {
        // Valid SwapRequest with SIG_ALL signature
        let valid_swap = r#"{
          "inputs": [
            {
              "amount": 2,
              "id": "00bfa73302d12ffd",
              "secret": "[\"P2PK\",{\"nonce\":\"c7f280eb55c1e8564e03db06973e94bc9b666d9e1ca42ad278408fe625950303\",\"data\":\"030d8acedfe072c9fa449a1efe0817157403fbec460d8e79f957966056e5dd76c1\",\"tags\":[[\"sigflag\",\"SIG_ALL\"]]}]",
              "C": "02c97ee3d1db41cf0a3ddb601724be8711a032950811bf326f8219c50c4808d3cd",
              "witness": "{\"signatures\":[\"ce017ca25b1b97df2f72e4b49f69ac26a240ce14b3690a8fe619d41ccc42d3c1282e073f85acd36dc50011638906f35b56615f24e4d03e8effe8257f6a808538\"]}"
            }
          ],
          "outputs": [
            {
              "amount": 2,
              "id": "00bfa73302d12ffd",
              "B_": "038ec853d65ae1b79b5cdbc2774150b2cb288d6d26e12958a16fb33c32d9a86c39"
            }
          ]
}"#;

        let valid_swap: SwapRequest = serde_json::from_str(valid_swap).unwrap();

        // Verify the message format
        let msg_to_sign = valid_swap.sig_all_msg_to_sign();
        assert_eq!(
            msg_to_sign,
            "[\"P2PK\",{\"nonce\":\"c7f280eb55c1e8564e03db06973e94bc9b666d9e1ca42ad278408fe625950303\",\"data\":\"030d8acedfe072c9fa449a1efe0817157403fbec460d8e79f957966056e5dd76c1\",\"tags\":[[\"sigflag\",\"SIG_ALL\"]]}]02c97ee3d1db41cf0a3ddb601724be8711a032950811bf326f8219c50c4808d3cd2038ec853d65ae1b79b5cdbc2774150b2cb288d6d26e12958a16fb33c32d9a86c39"
        );

        // Verify the SHA256 hash of the message
        use bitcoin::hashes::{sha256, Hash};
        let msg_hash = sha256::Hash::hash(msg_to_sign.as_bytes());
        assert_eq!(
            msg_hash.to_string(),
            "de7f9e3ca0fcc5ed3258fcf83dbf1be7fa78a5ed6da7bf2aa60d61e9dc6eb09a"
        );

        assert!(
            valid_swap.verify_spending_conditions().is_ok(),
            "Valid SIG_ALL swap request should verify"
        );
    }

    #[test]
    fn test_sig_all_swap_single_sig_2() {
        // The following is a SwapRequest with a valid sig_all signature.
        let valid_swap = r#"{
              "inputs": [
                {
                  "amount": 2,
                  "id": "00bfa73302d12ffd",
                  "secret": "[\"P2PK\",{\"nonce\":\"c7f280eb55c1e8564e03db06973e94bc9b666d9e1ca42ad278408fe625950303\",\"data\":\"030d8acedfe072c9fa449a1efe0817157403fbec460d8e79f957966056e5dd76c1\",\"tags\":[[\"sigflag\",\"SIG_ALL\"]]}]",
                  "C": "02c97ee3d1db41cf0a3ddb601724be8711a032950811bf326f8219c50c4808d3cd",
                  "witness": "{\"signatures\":[\"ce017ca25b1b97df2f72e4b49f69ac26a240ce14b3690a8fe619d41ccc42d3c1282e073f85acd36dc50011638906f35b56615f24e4d03e8effe8257f6a808538\"]}"
                }
              ],
              "outputs": [
                {
                  "amount": 2,
                  "id": "00bfa73302d12ffd",
                  "B_": "038ec853d65ae1b79b5cdbc2774150b2cb288d6d26e12958a16fb33c32d9a86c39"
                }
              ]
}"#;

        let valid_swap: SwapRequest = serde_json::from_str(valid_swap).unwrap();

        assert!(
            valid_swap.verify_spending_conditions().is_ok(),
            "Valid SIG_ALL swap request should verify"
        );
    }

    #[test]
    fn test_sig_all_multiple_secrets() {
        // The following is a SwapRequest that is invalid as there are multiple secrets.
        let invalid_swap = r#"{
                  "inputs": [
                {
                  "amount": 1,
                  "id": "00bfa73302d12ffd",
                  "secret": "[\"P2PK\",{\"nonce\":\"fa6dd3fac9086c153878dec90b9e37163d38ff2ecf8b37db6470e9d185abbbae\",\"data\":\"033b42b04e659fed13b669f8b16cdaffc3ee5738608810cf97a7631d09bd01399d\",\"tags\":[[\"sigflag\",\"SIG_ALL\"]]}]",
                  "C": "024d232312bab25af2e73f41d56864d378edca9109ae8f76e1030e02e585847786",
                  "witness": "{\"signatures\":[\"27b4d260a1186e3b62a26c0d14ffeab3b9f7c3889e78707b8fd3836b473a00601afbd53a2288ad20a624a8bbe3344453215ea075fc0ce479dd8666fd3d9162cc\"]}"
                },
                {
                  "amount": 2,
                  "id": "00bfa73302d12ffd",
                  "secret": "[\"P2PK\",{\"nonce\":\"4007b21fc5f5b1d4920bc0a08b158d98fd0fb2b0b0262b57ff53c6c5d6c2ae8c\",\"data\":\"033b42b04e659fed13b669f8b16cdaffc3ee5738608810cf97a7631d09bd01399d\",\"tags\":[[\"locktime\",\"122222222222222\"],[\"sigflag\",\"SIG_ALL\"]]}]",
                  "C": "02417400f2af09772219c831501afcbab4efb3b2e75175635d5474069608deb641"
                }
              ],
              "outputs": [
                {
                  "amount": 1,
                  "id": "00bfa73302d12ffd",
                  "B_": "038ec853d65ae1b79b5cdbc2774150b2cb288d6d26e12958a16fb33c32d9a86c39"
                },
                {
                  "amount": 1,
                  "id": "00bfa73302d12ffd",
                  "B_": "03afe7c87e32d436f0957f1d70a2bca025822a84a8623e3a33aed0a167016e0ca5"
                },
                {
                  "amount": 1,
                  "id": "00bfa73302d12ffd",
                  "B_": "02c0d4fce02a7a0f09e3f1bca952db910b17e81a7ebcbce62cd8dcfb127d21e37b"
                }
              ]
}"#;

        let invalid_swap: SwapRequest = serde_json::from_str(invalid_swap).unwrap();

        assert!(
            invalid_swap.verify_spending_conditions().is_err(),
            "Invalid swap with multiple secrets shouldn't be accepted"
        );
    }

    #[test]
    fn test_sig_all_multiple_signatures_provided() {
        // The following is a SwapRequest multiple valid signatures are provided and required.
        let valid_swap = r#"{
              "inputs": [
                {
                  "amount": 2,
                  "id": "00bfa73302d12ffd",
                  "secret": "[\"P2PK\",{\"nonce\":\"04bfd885fc982d553711092d037fdceb7320fd8f96b0d4fd6d31a65b83b94272\",\"data\":\"0275e78025b558dbe6cb8fdd032a2e7613ca14fda5c1f4c4e3427f5077a7bd90e4\",\"tags\":[[\"pubkeys\",\"035163650bbd5ed4be7693f40f340346ba548b941074e9138b67ef6c42755f3449\",\"02817d22a8edc44c4141e192995a7976647c335092199f9e076a170c7336e2f5cc\"],[\"n_sigs\",\"2\"],[\"sigflag\",\"SIG_ALL\"]]}]",
                  "C": "03866a09946562482c576ca989d06371e412b221890804c7da8887d321380755be",
                  "witness": "{\"signatures\":[\"be1d72c5ca16a93c5a34f25ec63ce632ddc3176787dac363321af3fd0f55d1927e07451bc451ffe5c682d76688ea9925d7977dffbb15bd79763b527f474734b0\",\"669d6d10d7ed35395009f222f6c7bdc28a378a1ebb72ee43117be5754648501da3bedf2fd6ff0c7849ac92683538c60af0af504102e40f2d8daca8e08b1ca16b\"]}"
                }
              ],
              "outputs": [
                {
                  "amount": 2,
                  "id": "00bfa73302d12ffd",
                  "B_": "038ec853d65ae1b79b5cdbc2774150b2cb288d6d26e12958a16fb33c32d9a86c39"
                }
              ]
}"#;

        let valid_swap: SwapRequest = serde_json::from_str(valid_swap).unwrap();

        assert!(
            valid_swap.verify_spending_conditions().is_ok(),
            "Valid swap with multiple signatures should be accepted"
        );
    }

    #[test]
    fn test_sig_all_mixed_pubkeys_and_refund() {
        // The following is an invalid SwapRequest with pubkeys and refund mixed.
        let invalid_swap = r#"{
                      "inputs": [
                {
                  "amount": 2,
                  "id": "00bfa73302d12ffd",
                  "secret": "[\"P2PK\",{\"nonce\":\"3e9253419a11f0a541dd6baeddecf8356fc864b5d061f12f05632bc3aee6b5c4\",\"data\":\"0343cca0e48ce9e3fdcddba4637ff8cdbf6f5ed9cfdf1873e63827e760f0ed4db5\",\"tags\":[[\"pubkeys\",\"0235e0a719f8b046cee90f55a59b1cdd6ca75ce23e49cbcd82c9e5b7310e21ebcd\",\"020443f98b356e021bae82bdfc05ff433cab21e27fca9ab7b0995aedb2e7aabc43\"],[\"locktime\",\"100\"],[\"n_sigs\",\"2\"],[\"refund\",\"026b432e62b041bf9cdae534203739c73fa506c9a2d6aa58a52bc601a1dec421e1\",\"02e3494a2e07e7f6e7d4567e0da7a563592bff1e121df2383667f15b83e9168a9e\"],[\"n_sigs_refund\",\"2\"],[\"sigflag\",\"SIG_ALL\"]]}]",
                  "C": "026c12ee3bffa5c617debcf823bf1af6a9b47145b699f2737bba3394f0893eb869",
                  "witness": "{\"signatures\":[\"bfe884145ce6512331324321c3946dfd812428a53656b108b59d26559a186ba2ab45e5be9ce94e2dff0d09078e25ccb82d06a8b3a63cd3dc67065b8f77292776\",\"236e5cc9c30f85a893a29a4302e41e6f2015caef4229f28fa65e2f5c9d55515cc9a1852093a81a5095055d85fd55bf4da124e55354b56e0a39e83b58b0afc197\"]}"
                }
              ],
              "outputs": [
                {
                  "amount": 1,
                  "id": "00bfa73302d12ffd",
                  "B_": "038ec853d65ae1b79b5cdbc2774150b2cb288d6d26e12958a16fb33c32d9a86c39"
                },
                {
                  "amount": 1,
                  "id": "00bfa73302d12ffd",
                  "B_": "03afe7c87e32d436f0957f1d70a2bca025822a84a8623e3a33aed0a167016e0ca5"
                }
              ]
}"#;

        let invalid_swap: SwapRequest = serde_json::from_str(invalid_swap).unwrap();

        assert!(
            invalid_swap.verify_spending_conditions().is_err(),
            "Invalid swap with mixed refunds and pubkeys shouldn't be accepted"
        );
    }

    #[test]
    fn test_sig_all_locktime_passed_with_valid_refund_key_sigs() {
        // The following is a SwapRequest with locktime passed and refund keys signatures are valid
        let valid_swap = r#"{
                      "inputs": [
                {
                  "amount": 2,
                  "id": "00bfa73302d12ffd",
                  "secret": "[\"P2PK\",{\"nonce\":\"9ea35553beb18d553d0a53120d0175a0991ca6109370338406eed007b26eacd1\",\"data\":\"02af21e09300af92e7b48c48afdb12e22933738cfb9bba67b27c00c679aae3ec25\",\"tags\":[[\"locktime\",\"1\"],[\"refund\",\"02637c19143c58b2c58bd378400a7b82bdc91d6dedaeb803b28640ef7d28a887ac\",\"0345c7fdf7ec7c8e746cca264bf27509eb4edb9ac421f8fbfab1dec64945a4d797\"],[\"n_sigs_refund\",\"2\"],[\"sigflag\",\"SIG_ALL\"]]}]",
                  "C": "03dd83536fbbcbb74ccb3c87147df26753fd499cc2c095f74367fff0fb459c312e",
                  "witness": "{\"signatures\":[\"23b58ef28cd22f3dff421121240ddd621deee83a3bc229fd67019c2e338d91e2c61577e081e1375dbab369307bba265e887857110ca3b4bd949211a0a298805f\",\"7e75948ef1513564fdcecfcbd389deac67c730f7004f8631ba90c0844d3e8c0cf470b656306877df5141f65fd3b7e85445a8452c3323ab273e6d0d44843817ed\"]}"
                }
              ],
              "outputs": [
                {
                  "amount": 2,
                  "id": "00bfa73302d12ffd",
                  "B_": "038ec853d65ae1b79b5cdbc2774150b2cb288d6d26e12958a16fb33c32d9a86c39"
                }
              ]
}"#;

        let valid_swap: SwapRequest = serde_json::from_str(valid_swap).unwrap();

        assert!(
            valid_swap.verify_spending_conditions().is_ok(),
            "Valid post-locktime swap with refund keys should be accepted"
        );
    }

    #[test]
    fn test_sig_all_htlc_and_pubkey() {
        // The following is a valid `SwapRequest` with an HTLC also locked to a public key
        let valid_swap = r#"{
                      "inputs": [
                {
                  "amount": 2,
                  "id": "00bfa73302d12ffd",
                  "secret": "[\"HTLC\",{\"nonce\":\"d730dd70cd7ec6e687829857de8e70aab2b970712f4dbe288343eca20e63c28c\",\"data\":\"ec4916dd28fc4c10d78e287ca5d9cc51ee1ae73cbfde08c6b37324cbfaac8bc5\",\"tags\":[[\"pubkeys\",\"0350cda8a1d5257dbd6ba8401a9a27384b9ab699e636e986101172167799469b14\"],[\"sigflag\",\"SIG_ALL\"]]}]",
                  "C": "03ff6567e2e6c31db5cb7189dab2b5121930086791c93899e4eff3dda61cb57273",
                  "witness": "{\"preimage\":\"0000000000000000000000000000000000000000000000000000000000000001\",\"signatures\":[\"a4c00a9ad07f9936e404494fda99a9b935c82d7c053173b304b8663124c81d4b00f64a225f5acf41043ca52b06382722bd04ded0fbeb0fcc404eed3b24778b88\"]}"
                }
              ],
              "outputs": [
                {
                  "amount": 2,
                  "id": "00bfa73302d12ffd",
                  "B_": "038ec853d65ae1b79b5cdbc2774150b2cb288d6d26e12958a16fb33c32d9a86c39"
                }
              ]
}"#;

        let valid_swap: SwapRequest = serde_json::from_str(valid_swap).unwrap();

        assert!(
            valid_swap.verify_spending_conditions().is_ok(),
            "Valid swap with htlc and pubkey should be accepted"
        );
    }

    #[test]
    fn test_sig_all_enforce_locktime_with_only_refund_signed() {
        // The following is an invalid SwapRequest with an HTLC also locked to a public key, locktime and refund key. locktime is not expired but proof is signed with refund key.
        let invalid_swap = r#"{
              "inputs": [
                {
                  "amount": 2,
                  "id": "00bfa73302d12ffd",
                  "secret": "[\"HTLC\",{\"nonce\":\"512c4045f12fdfd6f55059669c189e040c37c1ce2f8be104ed6aec296acce4e9\",\"data\":\"ec4916dd28fc4c10d78e287ca5d9cc51ee1ae73cbfde08c6b37324cbfaac8bc5\",\"tags\":[[\"pubkeys\",\"03ba83defd31c63f8841d188f0d41b5bb3af1bb3c08d0ba46f8f1d26a4d45e8cad\"],[\"locktime\",\"4854185133\"],[\"refund\",\"032f1008a79c722e93a1b4b853f85f38283f9ef74ee4c5c91293eb1cc3c5e46e34\"],[\"sigflag\",\"SIG_ALL\"]]}]",
                  "C": "02207abeff828146f1fc3909c74613d5605bd057f16791994b3c91f045b39a6939",
                  "witness": "{\"preimage\":\"0000000000000000000000000000000000000000000000000000000000000001\",\"signatures\":[\"7816d57871bde5be2e4281065dbe5b15f641d8f1ed9437a3ae556464d6f9b8a0a2e6660337a915f2c26dce1453a416daf682b8fb593b67a0750fce071e0759b9\"]}"
                }
              ],
              "outputs": [
                {
                  "amount": 1,
                  "id": "00bfa73302d12ffd",
                  "B_": "038ec853d65ae1b79b5cdbc2774150b2cb288d6d26e12958a16fb33c32d9a86c39"
                },
                {
                  "amount": 1,
                  "id": "00bfa73302d12ffd",
                  "B_": "03afe7c87e32d436f0957f1d70a2bca025822a84a8623e3a33aed0a167016e0ca5"
                }
              ]
}"#;

        let invalid_swap: SwapRequest = serde_json::from_str(invalid_swap).unwrap();

        assert!(
            invalid_swap.verify_spending_conditions().is_err(),
            "Invalid swap with pre-locktime conditions not met shouldn't be accepted"
        );
    }

    #[test]
    fn test_sig_all_htlc_post_locktime() {
        // The following is a valid SwapRequest with a multisig HTLC using the refund path.
        // Per NUT-14: After locktime, the refund path is available when preimage is invalid/missing.
        // The preimage is intentionally invalid (all zeros) to test the refund path.
        let valid_swap = r#"{
              "inputs": [
                {
                  "amount": 2,
                  "id": "00bfa73302d12ffd",
                  "secret": "[\"HTLC\",{\"nonce\":\"c9b0fabb8007c0db4bef64d5d128cdcf3c79e8bb780c3294adf4c88e96c32647\",\"data\":\"ec4916dd28fc4c10d78e287ca5d9cc51ee1ae73cbfde08c6b37324cbfaac8bc5\",\"tags\":[[\"pubkeys\",\"039e6ec7e922abb4162235b3a42965eb11510b07b7461f6b1a17478b1c9c64d100\"],[\"locktime\",\"1\"],[\"refund\",\"02ce1bbd2c9a4be8029c9a6435ad601c45677f5cde81f8a7f0ed535e0039d0eb6c\",\"03c43c00ff57f63cfa9e732f0520c342123e21331d0121139f1b636921eeec095f\"],[\"n_sigs_refund\",\"2\"],[\"sigflag\",\"SIG_ALL\"]]}]",
                  "C": "0344b6f1471cf18a8cbae0e624018c816be5e3a9b04dcb7689f64173c1ae90a3a5",
                  "witness": "{\"preimage\":\"0000000000000000000000000000000000000000000000000000000000000000\",\"signatures\":[\"98e21672d409cc782c720f203d8284f0af0c8713f18167499f9f101b7050c3e657fb0e57478ebd8bd561c31aa6c30f4cd20ec38c73f5755b7b4ddee693bca5a5\",\"693f40129dbf905ed9c8008081c694f72a36de354f9f4fa7a61b389cf781f62a0ae0586612fb2eb504faaf897fefb6742309186117f4743bcebcb8e350e975e2\"]}"
                }
              ],
              "outputs": [
                {
                  "amount": 2,
                  "id": "00bfa73302d12ffd",
                  "B_": "038ec853d65ae1b79b5cdbc2774150b2cb288d6d26e12958a16fb33c32d9a86c39"
                }
              ]
}"#;

        let valid_swap: SwapRequest = serde_json::from_str(valid_swap).unwrap();

        assert!(
            valid_swap.verify_spending_conditions().is_ok(),
            "Valid post-locktime swap with htlc should be accepted"
        );
    }

    #[test]
    fn test_sig_all_swap_mismatched_inputs() {
        // Invalid SwapRequest - mismatched inputs with SIG_ALL
        let invalid_swap = r#"{
            "inputs": [
        {
          "amount": 1,
          "id": "00bfa73302d12ffd",
          "secret": "[\"P2PK\",{\"nonce\":\"e2a221fe361f19d95c5c3312ccff3ffa075b4fe37beec99de85a6ee70568385b\",\"data\":\"03dad7f9c588f4cbb55c2e1b7b802fa2bbc63a614d9e9ecdf56a8e7ee8ca65be86\",\"tags\":[[\"pubkeys\",\"025f2af63fd65ca97c3bde4070549683e72769d28def2f1cd3d63576cd9c2ffa6c\"],[\"n_sigs\",\"2\"],[\"sigflag\",\"SIG_ALL\"]]}]",
          "C": "02a79c09b0605f4e7a21976b511cc7be01cdaeac54b29645258c84f2e74bff13f6",
          "witness": "{\"signatures\":[\"b42c7af7e98ca4e3bba8b73702120970286196340b340c21299676dbc7b10cafaa7baeb243affc01afce3218616cf8b3f6b4baaf4414fedb31b0c6653912f769\",\"17781910e2d806cae464f8a692929ee31124c0cd7eaf1e0d94292c6cbc122da09076b649080b8de9201f87d83b99fe04e33d701817eb287d1cdd9c4d0410e625\"]}"
        },
        {
          "amount": 2,
          "id": "00bfa73302d12ffd",
          "secret": "[\"P2PK\",{\"nonce\":\"973c78b5e84c0986209dc14ba57682baf38fa4c1ea60c4c5f6834779a1a13e6d\",\"data\":\"02685df03c777837bc7155bd2d0d8e98eede7e956a4cd8a9edac84532584e68e0f\",\"tags\":[[\"pubkeys\",\"025f2af63fd65ca97c3bde4070549683e72769d28def2f1cd3d63576cd9c2ffa6c\"],[\"n_sigs\",\"2\"],[\"sigflag\",\"SIG_ALL\"]]}]",
          "C": "02be48c564cf6a7b4d09fbaf3a78a153a79f687ac4623e48ce1788effc3fb1e024"
        }
      ],
      "outputs": [
        {
          "amount": 1,
          "id": "00bfa73302d12ffd",
          "B_": "038ec853d65ae1b79b5cdbc2774150b2cb288d6d26e12958a16fb33c32d9a86c39"
        },
        {
          "amount": 1,
          "id": "00bfa73302d12ffd",
          "B_": "03afe7c87e32d436f0957f1d70a2bca025822a84a8623e3a33aed0a167016e0ca5"
        },
        {
          "amount": 1,
          "id": "00bfa73302d12ffd",
          "B_": "02c0d4fce02a7a0f09e3f1bca952db910b17e81a7ebcbce62cd8dcfb127d21e37b"
        }
      ]
    }"#;

        let invalid_swap: SwapRequest = serde_json::from_str(invalid_swap).unwrap();
        assert!(
            invalid_swap.verify_spending_conditions().is_err(),
            "Invalid SIG_ALL swap request should fail verification"
        );
    }

    #[test]
    fn test_sig_all_mixed_pubkeys_and_refund_pubkeys() {
        // SwapRequest with mixed up signatures from pubkey and refund_pubkeys
        let invalidsig_all_swap = r#"{
  "inputs": [
    {
      "amount": 2,
      "id": "00bfa73302d12ffd",
      "secret": "[\"P2PK\",{\"nonce\":\"cc93775c74df53d7c97eb37f72018d166a45ce4f4c65f11c4014b19acd02bd2f\",\"data\":\"02f515ab63e973e0dadfc284bf2ef330b01aa99c3ff775d88272f9c17afa25568c\",\"tags\":[[\"pubkeys\",\"026925e5bb547a3ec6b2d9b8934e23b882f54f89b2a9f45300bf81fd1b311d9c97\"],[\"n_sigs\",\"2\"],[\"refund\",\"03c8cd46b7e6592c41df38bc54dce2555586e7adbb15cc80a02d1a05829677286d\"],[\"n_sigs_refund\",\"1\"],[\"sigflag\",\"SIG_ALL\"]]}]",
      "C": "03f6d40d0ab11f4082ee7e977534a6fcd151394d647cde4ab122157e6d755410fd",
      "witness": "{\"signatures\":[\"a9f61c2b7161a50839bf7f3e2e1cb9bd7bdacd2ce62c0d458a5969db44646dad409a282241b412e8b191cc7432bcfebf16ad72339a9fb966ca71c8bd971662cc\",\"aa778ec15fe9408e1989c712c823e833f33d45780b9a25555ea76004b05d495e99fd326914484f92e7e91f919ee575e79add26e9d4bbe4349d7333d7e0021af7\"]}"
    }
  ],
  "outputs": [
    {
      "amount": 1,
      "id": "00bfa73302d12ffd",
      "B_": "038ec853d65ae1b79b5cdbc2774150b2cb288d6d26e12958a16fb33c32d9a86c39"
    },
    {
      "amount": 1,
      "id": "00bfa73302d12ffd",
      "B_": "03afe7c87e32d436f0957f1d70a2bca025822a84a8623e3a33aed0a167016e0ca5"
    }
  ]
}"#;

        let invalid_swap: SwapRequest = serde_json::from_str(invalidsig_all_swap).unwrap();
        assert!(
            invalid_swap.verify_spending_conditions().is_err(),
            "Invalid SIG_ALL swap request should fail verification"
        );
    }

    #[test]
    fn test_sig_all_htlc_unexpired_timelock_refund_signature() {
        // SwapRequest signed with refund_pubkey without expiration
        let invalidsig_all_swap = r#"{
  "inputs": [
    {
      "amount": 2,
      "id": "00bfa73302d12ffd",
      "secret": "[\"HTLC\",{\"nonce\":\"b6f0c59ea4084369d4196e1318477121c2451d59ae767060e083cb6846e6bbe0\",\"data\":\"ec4916dd28fc4c10d78e287ca5d9cc51ee1ae73cbfde08c6b37324cbfaac8bc5\",\"tags\":[[\"pubkeys\",\"0329fdfde4becf9ff871129653ff6464bb2c922fbcba442e6166a8b5849599604f\"],[\"locktime\",\"4854185133\"],[\"refund\",\"035fcf4a5393e4bdef0567aa0b8a9555edba36e5fcb283f3bbce52d86a687817d3\"],[\"sigflag\",\"SIG_ALL\"]]}]",
      "C": "024fbbee3f3cc306a48841ba327435b64de20b8b172b98296a3e573c673d52562b",
      "witness": "{\"preimage\":\"0000000000000000000000000000000000000000000000000000000000000001\",\"signatures\":[\"7526819070a291f731e77acfbe9da71ddc0f748fd2a3e6c2510bc83c61daaa656df345afa3832fe7cb94352c8835a4794ad499760729c0be29417387d1fc3cd1\"]}"
    }
  ],
  "outputs": [
    {
      "amount": 2,
      "id": "00bfa73302d12ffd",
      "B_": "038ec853d65ae1b79b5cdbc2774150b2cb288d6d26e12958a16fb33c32d9a86c39"
    }
  ]
}"#;

        let invalid_swap: SwapRequest = serde_json::from_str(invalidsig_all_swap).unwrap();
        assert!(
            invalid_swap.verify_spending_conditions().is_err(),
            "Invalid SIG_ALL swap request should fail verification"
        );
    }

    // Now the Melt examples at the end of 11-test.md
    #[test]
    fn test_sig_all_melt() {
        // Valid MeltRequest with SIG_ALL signature
        // Example MeltRequest:
        let valid_melt = r#"{
              "quote": "cF8911fzT88aEi1d-6boZZkq5lYxbUSVs-HbJxK0",
              "inputs": [
                {
                  "amount": 2,
                  "id": "00bfa73302d12ffd",
                  "secret": "[\"P2PK\",{\"nonce\":\"bbf9edf441d17097e39f5095a3313ba24d3055ab8a32f758ff41c10d45c4f3de\",\"data\":\"029116d32e7da635c8feeb9f1f4559eb3d9b42d400f9d22a64834d89cde0eb6835\",\"tags\":[[\"sigflag\",\"SIG_ALL\"]]}]",
                  "C": "02a9d461ff36448469dccf828fa143833ae71c689886ac51b62c8d61ddaa10028b",
                  "witness": "{\"signatures\":[\"478224fbe715e34f78cb33451db6fcf8ab948afb8bd04ff1a952c92e562ac0f7c1cb5e61809410635be0aa94d0448f7f7959bd5762cc3802b0a00ff58b2da747\"]}"
                }
              ],
              "outputs": [
                {
                  "amount": 0,
                  "id": "00bfa73302d12ffd",
                  "B_": "038ec853d65ae1b79b5cdbc2774150b2cb288d6d26e12958a16fb33c32d9a86c39"
                }
              ]
}"#;

        let valid_melt: MeltRequest<String> = serde_json::from_str(valid_melt).unwrap();

        // Verify the message format
        let msg_to_sign = valid_melt.sig_all_msg_to_sign();
        assert_eq!(
            msg_to_sign,
            r#"["P2PK",{"nonce":"bbf9edf441d17097e39f5095a3313ba24d3055ab8a32f758ff41c10d45c4f3de","data":"029116d32e7da635c8feeb9f1f4559eb3d9b42d400f9d22a64834d89cde0eb6835","tags":[["sigflag","SIG_ALL"]]}]02a9d461ff36448469dccf828fa143833ae71c689886ac51b62c8d61ddaa10028b0038ec853d65ae1b79b5cdbc2774150b2cb288d6d26e12958a16fb33c32d9a86c39cF8911fzT88aEi1d-6boZZkq5lYxbUSVs-HbJxK0"#
        );

        // Verify the SHA256 hash of the message
        use bitcoin::hashes::{sha256, Hash};
        let msg_hash = sha256::Hash::hash(msg_to_sign.as_bytes());
        assert_eq!(
            msg_hash.to_string(),
            "9efa1067cc7dc870f4074f695115829c3cd817a6866c3b84e9814adf3c3cf262"
        );

        assert!(
            valid_melt.verify_spending_conditions().is_ok(),
            "Valid SIG_ALL melt request should verify"
        );
    }

    #[test]
    fn test_sig_all_valid_melt() {
        // The following is a valid SIG_ALL MeltRequest.
        let valid_melt = r#"{
              "quote": "cF8911fzT88aEi1d-6boZZkq5lYxbUSVs-HbJxK0",
              "inputs": [
                {
                  "amount": 2,
                  "id": "00bfa73302d12ffd",
                  "secret": "[\"P2PK\",{\"nonce\":\"bbf9edf441d17097e39f5095a3313ba24d3055ab8a32f758ff41c10d45c4f3de\",\"data\":\"029116d32e7da635c8feeb9f1f4559eb3d9b42d400f9d22a64834d89cde0eb6835\",\"tags\":[[\"sigflag\",\"SIG_ALL\"]]}]",
                  "C": "02a9d461ff36448469dccf828fa143833ae71c689886ac51b62c8d61ddaa10028b",
                  "witness": "{\"signatures\":[\"478224fbe715e34f78cb33451db6fcf8ab948afb8bd04ff1a952c92e562ac0f7c1cb5e61809410635be0aa94d0448f7f7959bd5762cc3802b0a00ff58b2da747\"]}"
                }
              ],
              "outputs": [
                {
                  "amount": 0,
                  "id": "00bfa73302d12ffd",
                  "B_": "038ec853d65ae1b79b5cdbc2774150b2cb288d6d26e12958a16fb33c32d9a86c39"
                }
              ]
}"#;

        let valid_melt: MeltRequest<String> = serde_json::from_str(valid_melt).unwrap();

        assert!(
            valid_melt.verify_spending_conditions().is_ok(),
            "Valid SIG_ALL melt request should verify"
        );
    }

    #[test]
    fn test_sig_all_valid_multisig_melt() {
        // The following is a valid multi-sig SIG_ALL MeltRequest.
        let valid_melt = r#"{
              "quote": "Db3qEMVwFN2tf_1JxbZp29aL5cVXpSMIwpYfyOVF",
              "inputs": [
                {
                  "amount": 2,
                  "id": "00bfa73302d12ffd",
                  "secret": "[\"P2PK\",{\"nonce\":\"68d7822538740e4f9c9ebf5183ef6c4501c7a9bca4e509ce2e41e1d62e7b8a99\",\"data\":\"0394e841bd59aeadce16380df6174cb29c9fea83b0b65b226575e6d73cc5a1bd59\",\"tags\":[[\"pubkeys\",\"033d892d7ad2a7d53708b7a5a2af101cbcef69522bd368eacf55fcb4f1b0494058\"],[\"n_sigs\",\"2\"],[\"sigflag\",\"SIG_ALL\"]]}]",
                  "C": "03a70c42ec9d7192422c7f7a3ad017deda309fb4a2453fcf9357795ea706cc87a9",
                  "witness": "{\"signatures\":[\"ed739970d003f703da2f101a51767b63858f4894468cc334be04aa3befab1617a81e3eef093441afb499974152d279e59d9582a31dc68adbc17ffc22a2516086\",\"f9efe1c70eb61e7ad8bd615c50ff850410a4135ea73ba5fd8e12a734743ad045e575e9e76ea5c52c8e7908d3ad5c0eaae93337e5c11109e52848dc328d6757a2\"]}"
                }
              ],
              "outputs": [
                {
                  "amount": 0,
                  "id": "00bfa73302d12ffd",
                  "B_": "038ec853d65ae1b79b5cdbc2774150b2cb288d6d26e12958a16fb33c32d9a86c39"
                }
              ]
}"#;

        let valid_melt: MeltRequest<String> = serde_json::from_str(valid_melt).unwrap();

        assert!(
            valid_melt.verify_spending_conditions().is_ok(),
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
            invalid_melt.verify_spending_conditions().is_err(),
            "Invalid SIG_ALL melt request should fail verification"
        );
    }

    #[test]
    #[ignore]
    fn test_sig_all_melt_msg_to_sign() {
        let multisig_melt = r#"{
  "quote": "uHwJ-f6HFAC-lU2dMw0KOu6gd5S571FXQQHioYMD",
  "inputs": [
    {
      "amount": 4,
      "id": "00bfa73302d12ffd",
      "secret": "[\"P2PK\",{\"nonce\":\"f5c26c928fb4433131780105eac330338bb9c0af2b2fd29fad9e4f18c4a96d84\",\"data\":\"03c4840e19277822bfeecf104dcd3f38d95b33249983ac6fed755869f23484fb2a\",\"tags\":[[\"pubkeys\",\"0256dcc53d9330e0bc6e9b3d47c26287695aba9fe55cafdde6f46ef56e09582bfb\"],[\"n_sigs\",\"1\"],[\"sigflag\",\"SIG_ALL\"]]}]",
      "C": "02174667f98114abeb741f4964bdc88a3b86efde0afa38f791094c1e07e5df3beb",
      "witness": "{\"signatures\":[\"abeeceba92bc7d1c514844ddb354d1e88a9776dfb55d3cdc5c289240386e401c3d983b68371ce5530e86c8fc4ff90195982a262f83fa8a5335b43e75af5f5fc7\"]}"
    }
  ],
  "outputs": [
    {
      "amount": 0,
      "id": "00bfa73302d12ffd",
      "B_": "038ec853d65ae1b79b5cdbc2774150b2cb288d6d26e12958a16fb33c32d9a86c39"
    }
  ]
}"#;

        let multisig_melt: MeltRequest<String> = serde_json::from_str(multisig_melt).unwrap();

        assert!(
            multisig_melt.verify_spending_conditions().is_ok(),
            "melt request with SIG_ALL should succeed"
        );

        let msg_to_sign = multisig_melt.sig_all_msg_to_sign();

        assert_eq!(
            msg_to_sign,
            r#"["P2PK",{"nonce":"f5c26c928fb4433131780105eac330338bb9c0af2b2fd29fad9e4f18c4a96d84","data":"03c4840e19277822bfeecf104dcd3f38d95b33249983ac6fed755869f23484fb2a","tags":[["pubkeys","0256dcc53d9330e0bc6e9b3d47c26287695aba9fe55cafdde6f46ef56e09582bfb"],["n_sigs","1"],["sigflag","SIG_ALL"]]}]02174667f98114abeb741f4964bdc88a3b86efde0afa38f791094c1e07e5df3beb000bfa73302d12ffd038ec853d65ae1b79b5cdbc2774150b2cb288d6d26e12958a16fb33c32d9a86c39uHwJ-f6HFAC-lU2dMw0KOu6gd5S571FXQQHioYMD"#
        );
    }

    #[test]
    #[ignore]
    fn test_sig_all_melt_multi_sig() {
        // MeltRequest with multi-sig SIG_ALL requiring 2 signatures
        let multisig_melt = r#"{
  "quote": "wYHbJm5S1GTL28tDHoUAwcvb-31vu5kfDhnLxV9D",
  "inputs": [
    {
      "amount": 4,
      "id": "00bfa73302d12ffd",
      "secret": "[\"P2PK\",{\"nonce\":\"1705e988054354b703bc9103472cc5646ec76ed557517410186fa827c19c444d\",\"data\":\"024c8b5ec0e560f1fc77d7872ab75dd10a00af73a8ba715b81093b800849cb21fb\",\"tags\":[[\"pubkeys\",\"028d32bc906b3724724244812c450f688c548020f5d5a8c1d6cd1075650933d1a3\"],[\"n_sigs\",\"2\"],[\"sigflag\",\"SIG_ALL\"]]}]",
      "C": "02f2a0ff12c4dd95f2476662f1df49e5126f09a5ea1f3ce13b985db57661953072",
      "witness": "{\"signatures\":[\"a98a2616716d7813394a54ddc82234e5c47f0ddbddb98ccd1cad25236758fa235c8ae64d9fccd15efbe0ad5eba52a3df8433e9f1c05bc50defcb9161a5bd4bc4\",\"dd418cbbb23276dab8d72632ee77de730b932a3c6e8e15bc8802cef13db0b346915fe6e04e7fae03c3b5af026e25f71a24dc05b28135f0a9b69bc6c7289b6b8d\"]}"
    }
  ],
  "outputs": [
    {
      "amount": 0,
      "id": "00bfa73302d12ffd",
      "B_": "038ec853d65ae1b79b5cdbc2774150b2cb288d6d26e12958a16fb33c32d9a86c39"
    }
  ]
}"#;

        let multisig_melt: MeltRequest<String> = serde_json::from_str(multisig_melt).unwrap();
        assert!(
            multisig_melt.verify_spending_conditions().is_ok(),
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
            insufficient_sigs_melt.verify_spending_conditions().is_err(),
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
            swap.verify_spending_conditions().is_err(),
            "Unsigned swap should fail verification"
        );

        assert!(
            swap.sign_sig_all(secret_key).is_ok(),
            "Signing should succeed"
        );

        println!("{}", serde_json::to_string(&swap).unwrap());

        assert!(
            swap.verify_spending_conditions().is_ok(),
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

        // Signing should succeed (no validation)
        swap.sign_sig_all(unauthorized_key).unwrap();

        // Verification should fail (unauthorized signature)
        assert!(
            swap.verify_spending_conditions().is_err(),
            "Verification should fail with unauthorized key signature"
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

        // Signing should succeed (no validation)
        swap.sign_sig_all(secret_key).unwrap();

        // Verification should fail (mismatched secrets)
        assert!(
            swap.verify_spending_conditions().is_err(),
            "Verification should fail with mismatched secrets"
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

        // Signing should succeed (no validation)
        swap.sign_sig_all(secret_key).unwrap();

        // Verification should fail (wrong flag - has SIG_INPUTS but sign_sig_all expects SIG_ALL)
        assert!(
            swap.verify_spending_conditions().is_err(),
            "Verification should fail with SIG_INPUTS flag when sign_sig_all was used"
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
            swap.verify_spending_conditions().is_err(),
            "Single signature should not verify when two required"
        );

        // Sign with second key
        assert!(
            swap.sign_sig_all(secret_key2).is_ok(),
            "Second signature should succeed"
        );

        assert!(
            swap.verify_spending_conditions().is_ok(),
            "Both signatures should verify"
        );
    }
}
