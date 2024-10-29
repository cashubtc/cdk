//! NUT-11: Pay to Public Key (P2PK)
//!
//! <https://github.com/cashubtc/nuts/blob/main/11.md>

use std::collections::HashMap;
#[cfg(feature = "mint")]
use std::collections::HashSet;
use std::str::FromStr;
use std::{fmt, vec};

use bitcoin::hashes::sha256::Hash as Sha256Hash;
use bitcoin::hashes::Hash;
use bitcoin::secp256k1::schnorr::Signature;
use serde::de::Error as DeserializerError;
use serde::ser::SerializeSeq;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use thiserror::Error;

use super::nut00::Witness;
use super::nut01::PublicKey;
use super::nutdlc::DLCRoot;
#[cfg(feature = "mint")]
use super::Proofs;
use super::{Kind, Nut10Secret, Proof, SecretKey};
use crate::nuts::nut00::BlindedMessage;
use crate::secret::Secret;
use crate::util::{hex, unix_time};

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
    /// Witness Signatures not provided
    #[error("Witness signatures not provided")]
    SignaturesNotProvided,
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
        let spending_conditions: Conditions =
            secret.secret_data.tags.unwrap_or_default().try_into()?;
        let msg: &[u8] = self.secret.as_bytes();

        let mut valid_sigs = 0;

        let witness_signatures = match &self.witness {
            Some(witness) => witness.signatures(),
            None => None,
        };

        let witness_signatures = witness_signatures.ok_or(Error::SignaturesNotProvided)?;

        let mut pubkeys = spending_conditions.pubkeys.clone().unwrap_or_default();

        if secret.kind.eq(&Kind::P2PK) {
            pubkeys.push(PublicKey::from_str(&secret.secret_data.data)?);
        }

        for signature in witness_signatures.iter() {
            for v in &pubkeys {
                let sig = Signature::from_str(signature)?;

                if v.verify(msg, &sig).is_ok() {
                    valid_sigs += 1;
                } else {
                    tracing::debug!(
                        "Could not verify signature: {sig} on message: {}",
                        self.secret.to_string()
                    )
                }
            }
        }

        if valid_sigs >= spending_conditions.num_sigs.unwrap_or(1) {
            return Ok(());
        }

        if let (Some(locktime), Some(refund_keys)) = (
            spending_conditions.locktime,
            spending_conditions.refund_keys,
        ) {
            // If lock time has passed check if refund witness signature is valid
            if locktime.lt(&unix_time()) {
                for s in witness_signatures.iter() {
                    for v in &refund_keys {
                        let sig = Signature::from_str(s).map_err(|_| Error::InvalidSignature)?;

                        // As long as there is one valid refund signature it can be spent
                        if v.verify(msg, &sig).is_ok() {
                            return Ok(());
                        }
                    }
                }
            }
        }

        Err(Error::SpendConditionsNotMet)
    }
}

/// Returns count of valid signatures
pub fn valid_signatures(msg: &[u8], pubkeys: &[PublicKey], signatures: &[Signature]) -> u64 {
    let mut count = 0;

    for pubkey in pubkeys {
        for signature in signatures {
            if pubkey.verify(msg, signature).is_ok() {
                count += 1;
            }
        }
    }

    count
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
        let mut valid_sigs = 0;
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
                        valid_sigs += 1;
                    } else {
                        tracing::debug!(
                            "Could not verify signature: {sig} on message: {}",
                            self.blinded_secret
                        )
                    }
                }
            }
        }

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

    /// NUT-DLC Spending conditions
    DLCConditions {
        /// dlc root
        data: String,
        /// Additional Optional Spending [`Conditions`]
        conditions: Option<Conditions>,
    },

    /// NUT-SCT Spending conditions
    SCTConditions {
        /// Merkle root hash of spending conditions
        data: String,
    },
}

impl SpendingConditions {
    /// New HTLC [SpendingConditions]
    pub fn new_htlc(preimage: String, conditions: Option<Conditions>) -> Result<Self, Error> {
        let htlc = Sha256Hash::hash(&hex::decode(preimage)?);

        Ok(Self::HTLCConditions {
            data: htlc,
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

    /// New DLC [SpendingConditions]
    pub fn new_dlc(dlc_root: &DLCRoot, conditions: Option<Conditions>) -> Self {
        Self::DLCConditions {
            data: dlc_root.to_string(),
            conditions,
        }
    }

    /// New SCT [SpendingConditions]
    pub fn new_sct(merkle_root: [u8; 32]) -> Self {
        Self::SCTConditions {
            data: hex::encode(merkle_root),
        }
    }

    /// New SCT [SpendingConditions] for a DLC
    pub fn new_dlc_sct(secrets: Vec<Secret>, secret_to_prove: usize) -> (Self, Vec<String>) {
        let leaf_hashes = crate::nuts::nutsct::sct_leaf_hashes(secrets.clone());
        let proof = crate::nuts::nutsct::merkle_prove(leaf_hashes.clone(), secret_to_prove);
        let root = crate::nuts::nutsct::merkle_root(&leaf_hashes);

        let expected_proof = vec![Sha256Hash::hash(&secrets[1].to_bytes()).to_byte_array()];
        assert_eq!(proof, expected_proof);

        let proof = proof
            .iter()
            .map(|h| hex::encode(h))
            .collect::<Vec<String>>();

        let valid =
            crate::nuts::nutsct::merkle_verify(&root, &leaf_hashes[secret_to_prove], &proof);
        assert!(valid);

        let sct_conditions = SpendingConditions::new_sct(root);

        (sct_conditions, proof)
    }

    /// Kind of [SpendingConditions]
    pub fn kind(&self) -> Kind {
        match self {
            Self::P2PKConditions { .. } => Kind::P2PK,
            Self::HTLCConditions { .. } => Kind::HTLC,
            Self::DLCConditions { .. } => Kind::DLC,
            Self::SCTConditions { .. } => Kind::SCT,
        }
    }

    /// Number if signatures required to unlock
    pub fn num_sigs(&self) -> Option<u64> {
        match self {
            Self::P2PKConditions { conditions, .. } => conditions.as_ref().and_then(|c| c.num_sigs),
            Self::HTLCConditions { conditions, .. } => conditions.as_ref().and_then(|c| c.num_sigs),
            Self::DLCConditions { .. } => todo!(),
            Self::SCTConditions { .. } => todo!(),
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

                Some(pubkeys)
            }
            Self::HTLCConditions { conditions, .. } => conditions.clone().and_then(|c| c.pubkeys),
            Self::DLCConditions { .. } => todo!(),
            Self::SCTConditions { .. } => todo!(),
        }
    }

    /// Locktime of Spending Conditions
    pub fn locktime(&self) -> Option<u64> {
        match self {
            Self::P2PKConditions { conditions, .. } => conditions.as_ref().and_then(|c| c.locktime),
            Self::HTLCConditions { conditions, .. } => conditions.as_ref().and_then(|c| c.locktime),
            Self::DLCConditions { .. } => todo!(),
            Self::SCTConditions { .. } => todo!(),
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

            Self::DLCConditions { .. } => todo!(),
            Self::SCTConditions { .. } => todo!(),
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
        match secret.kind {
            Kind::P2PK => Ok(SpendingConditions::P2PKConditions {
                data: PublicKey::from_str(&secret.secret_data.data)?,
                conditions: secret.secret_data.tags.and_then(|t| t.try_into().ok()),
            }),
            Kind::HTLC => Ok(Self::HTLCConditions {
                data: Sha256Hash::from_str(&secret.secret_data.data)
                    .map_err(|_| Error::InvalidHash)?,
                conditions: secret.secret_data.tags.and_then(|t| t.try_into().ok()),
            }),
            Kind::DLC => Ok(Self::DLCConditions {
                data: DLCRoot::from_str(&secret.secret_data.data)?.to_string(),
                conditions: secret.secret_data.tags.and_then(|t| t.try_into().ok()),
            }),
            Kind::SCT => Ok(Self::SCTConditions {
                data: secret.secret_data.data,
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
            SpendingConditions::DLCConditions { data, conditions } => {
                super::nut10::Secret::new(Kind::DLC, data.to_string(), conditions)
            }
            SpendingConditions::SCTConditions { data } => {
                super::nut10::Secret::new(Kind::SCT, data.to_string(), None::<Conditions>)
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
    /// Numbedr of signatures required
    ///
    /// Default is 1
    #[serde(skip_serializing_if = "Option::is_none")]
    pub num_sigs: Option<u64>,
    /// Signature flag
    ///
    /// Default [`SigFlag::SigInputs`]
    pub sig_flag: SigFlag,
    /// DLC funding threshold
    #[serde(skip_serializing_if = "Option::is_none")]
    pub threshold: Option<u64>,
}

impl Conditions {
    /// Create new Spending [`Conditions`]
    pub fn new(
        locktime: Option<u64>,
        pubkeys: Option<Vec<PublicKey>>,
        refund_keys: Option<Vec<PublicKey>>,
        num_sigs: Option<u64>,
        sig_flag: Option<SigFlag>,
        threshold: Option<u64>,
    ) -> Result<Self, Error> {
        if let Some(locktime) = locktime {
            if locktime.lt(&unix_time()) {
                return Err(Error::LocktimeInPast);
            }
        }

        Ok(Self {
            locktime,
            pubkeys,
            refund_keys,
            num_sigs,
            sig_flag: sig_flag.unwrap_or_default(),
            threshold,
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
            threshold,
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
        tags.push(Tag::SigFlag(sig_flag).as_vec());
        if let Some(threshold) = threshold {
            tags.push(Tag::DLCThreshold(threshold).as_vec());
        }

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

        let threshold = if let Some(tag) = tags.get(&TagKind::DLCThreshold) {
            match tag {
                Tag::DLCThreshold(threshold) => Some(*threshold),
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
            threshold,
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
    /// DLC funding threshold
    #[serde(rename = "threshold")]
    DLCThreshold,
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
            Self::DLCThreshold => write!(f, "threshold"),
            Self::Custom(kind) => write!(f, "{}", kind),
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
            "threshold" => Self::DLCThreshold,
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

#[cfg(feature = "mint")]
/// Get the signature flag that should be enforced for a set of proofs and the
/// public keys that signatures are valid for
pub(crate) fn enforce_sig_flag(proofs: Proofs) -> EnforceSigFlag {
    let mut sig_flag = SigFlag::SigInputs;
    let mut pubkeys = HashSet::new();
    let mut sigs_required = 1;
    for proof in proofs {
        if let Ok(secret) = Nut10Secret::try_from(proof.secret) {
            if secret.kind.eq(&Kind::P2PK) {
                if let Ok(verifying_key) = PublicKey::from_str(&secret.secret_data.data) {
                    pubkeys.insert(verifying_key);
                }
            }

            if let Some(tags) = secret.secret_data.tags {
                if let Ok(conditions) = Conditions::try_from(tags) {
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

#[cfg(feature = "mint")]
/// Enforce Sigflag info
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct EnforceSigFlag {
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
    /// DLC funding threshold [`Tag`]
    DLCThreshold(u64),
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
            Self::DLCThreshold(_) => TagKind::DLCThreshold,
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
        let tag_kind: TagKind = match tag.first() {
            Some(kind) => TagKind::from(kind),
            None => return Err(Error::KindNotFound),
        };

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
            TagKind::DLCThreshold => Ok(Tag::DLCThreshold(tag[1].as_ref().parse()?)),
            _ => Err(Error::UnknownTag),
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
            Tag::DLCThreshold(threshold) => {
                let mut tag = vec![TagKind::DLCThreshold.to_string()];
                tag.push(threshold.to_string());
                tag
            }
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

#[cfg(test)]
mod tests {
    use std::str::FromStr;

    use super::*;
    use crate::nuts::Id;
    use crate::secret::Secret;
    use crate::Amount;

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
            threshold: None,
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
            threshold: None,
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
}
