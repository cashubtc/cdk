//! NUT-11: Pay to Public Key (P2PK)
//!
//! <https://github.com/cashubtc/nuts/blob/main/11.md>

use std::collections::HashMap;
use std::fmt;
use std::ops::Deref;
use std::str::FromStr;

use bitcoin::hashes::sha256::Hash as Sha256Hash;
use bitcoin::hashes::Hash;
use bitcoin::secp256k1::schnorr::Signature;
use bitcoin::secp256k1::{
    KeyPair, Message, Parity, PublicKey as NormalizedPublicKey, XOnlyPublicKey,
};
use serde::de::Error as DeserializerError;
use serde::ser::SerializeSeq;
use serde::{de, ser, Deserialize, Deserializer, Serialize, Serializer};
use thiserror::Error;

use super::nut01::PublicKey;
use super::nut10::{Secret, SecretData};
use super::{Proof, SecretKey};
use crate::nuts::nut00::BlindedMessage;
use crate::util::{hex, unix_time};
use crate::SECP256K1;

#[derive(Debug, Error)]
pub enum Error {
    /// Incorrect secret kind
    #[error("Secret is not a p2pk secret")]
    IncorrectSecretKind,
    /// P2PK locktime has already passed
    #[error("Locktime in past")]
    LocktimeInPast,
    /// Witness signature is not valid
    #[error("Invalid signature")]
    InvalidSignature,
    /// Unknown tag in P2PK secret
    #[error("Unknown Tag P2PK secret")]
    UnknownTag,
    /// P2PK Spend conditions not meet
    #[error("P2PK Spend conditions are not met")]
    SpendConditionsNotMet,
    /// Pubkey must be in data field of P2PK
    #[error("P2PK Required in secret data")]
    P2PKPubkeyRequired,
    #[error("Kind not found")]
    KindNotFound,
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

#[derive(Default, Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Signatures {
    #[serde(default)]
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub signatures: Vec<String>,
}

impl Signatures {
    pub fn is_empty(&self) -> bool {
        self.signatures.is_empty()
    }
}

pub fn witness_serialize<S>(x: &Option<Signatures>, s: S) -> Result<S::Ok, S::Error>
where
    S: Serializer,
{
    s.serialize_str(&serde_json::to_string(&x).map_err(ser::Error::custom)?)
}

pub fn witness_deserialize<'de, D>(deserializer: D) -> Result<Option<Signatures>, D::Error>
where
    D: de::Deserializer<'de>,
{
    let s: String = String::deserialize(deserializer)?;
    serde_json::from_str(&s).map_err(de::Error::custom)
}

impl Proof {
    pub fn verify_p2pk(&self) -> Result<(), Error> {
        if !self.secret.is_p2pk() {
            return Err(Error::IncorrectSecretKind);
        }

        let secret: Secret = self.secret.clone().try_into()?;
        let spending_conditions: P2PKConditions = secret.clone().try_into()?;
        let msg: &[u8] = self.secret.as_bytes();

        let mut valid_sigs = 0;

        if let Some(witness) = &self.witness {
            for signature in witness.signatures.iter() {
                let mut pubkeys = spending_conditions.pubkeys.clone();

                pubkeys.push(VerifyingKey::from_str(&secret.secret_data.data)?);

                for v in &spending_conditions.pubkeys {
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
                if let Some(signatures) = &self.witness {
                    for s in &signatures.signatures {
                        for v in &refund_keys {
                            let sig =
                                Signature::from_str(s).map_err(|_| Error::InvalidSignature)?;

                            // As long as there is one valid refund signature it can be spent
                            if v.verify(msg, &sig).is_ok() {
                                return Ok(());
                            }
                        }
                    }
                }
            }
        }

        Err(Error::SpendConditionsNotMet)
    }

    pub fn sign_p2pk(&mut self, secret_key: SigningKey) -> Result<(), Error> {
        let msg: Vec<u8> = self.secret.to_bytes();
        let signature: Signature = secret_key.sign(&msg)?;

        self.witness
            .as_mut()
            .unwrap_or(&mut Signatures::default())
            .signatures
            .push(signature.to_string());

        Ok(())
    }
}

impl BlindedMessage {
    pub fn sign_p2pk(&mut self, secret_key: SigningKey) -> Result<(), Error> {
        let msg: [u8; 33] = self.b.to_bytes();
        let signature: Signature = secret_key.sign(&msg)?;

        self.witness
            .as_mut()
            .unwrap_or(&mut Signatures::default())
            .signatures
            .push(signature.to_string());

        Ok(())
    }

    pub fn verify_p2pk(
        &self,
        pubkeys: &Vec<VerifyingKey>,
        required_sigs: u64,
    ) -> Result<(), Error> {
        let mut valid_sigs = 0;
        if let Some(witness) = &self.witness {
            for signature in &witness.signatures {
                for v in pubkeys {
                    let msg = &self.b.to_bytes();
                    let sig = Signature::from_str(signature)?;

                    if v.verify(msg, &sig).is_ok() {
                        valid_sigs += 1;
                    } else {
                        tracing::debug!("Could not verify signature: {sig} on message: {}", self.b)
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct P2PKConditions {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub locktime: Option<u64>,
    pub pubkeys: Vec<VerifyingKey>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub refund_keys: Option<Vec<VerifyingKey>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub num_sigs: Option<u64>,
    pub sig_flag: SigFlag,
}

impl P2PKConditions {
    pub fn new(
        locktime: Option<u64>,
        pubkeys: Vec<VerifyingKey>,
        refund_keys: Option<Vec<VerifyingKey>>,
        num_sigs: Option<u64>,
        sig_flag: Option<SigFlag>,
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
        })
    }
}

impl TryFrom<P2PKConditions> for Secret {
    type Error = Error;
    fn try_from(conditions: P2PKConditions) -> Result<Secret, Self::Error> {
        let P2PKConditions {
            locktime,
            pubkeys,
            refund_keys,
            num_sigs,
            sig_flag,
        } = conditions;

        let data = match pubkeys.first() {
            Some(data) => data.to_string(),
            None => return Err(Error::P2PKPubkeyRequired),
        };

        let data = data.to_string();

        let mut tags = Vec::new();

        if pubkeys.len().gt(&1) {
            tags.push(Tag::PubKeys(pubkeys.into_iter().skip(1).collect()).as_vec());
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

        Ok(Secret {
            kind: super::nut10::Kind::P2PK,
            secret_data: SecretData {
                nonce: crate::secret::Secret::default().to_string(),
                data,
                tags,
            },
        })
    }
}

impl TryFrom<P2PKConditions> for crate::secret::Secret {
    type Error = Error;
    fn try_from(conditions: P2PKConditions) -> Result<crate::secret::Secret, Self::Error> {
        let secret: Secret = conditions.try_into()?;

        secret.try_into().map_err(|_| Error::IncorrectSecretKind)
    }
}

impl TryFrom<Secret> for P2PKConditions {
    type Error = Error;
    fn try_from(secret: Secret) -> Result<P2PKConditions, Self::Error> {
        let tags: HashMap<TagKind, Tag> = secret
            .clone()
            .secret_data
            .tags
            .into_iter()
            .map(|t| Tag::try_from(t).unwrap())
            .map(|t| (t.kind(), t))
            .collect();

        let mut pubkeys: Vec<VerifyingKey> = vec![];

        if let Some(Tag::PubKeys(keys)) = tags.get(&TagKind::Pubkeys) {
            let mut keys = keys.clone();
            pubkeys.append(&mut keys);
        }

        let data_pubkey = VerifyingKey::from_str(&secret.secret_data.data)?;
        pubkeys.push(data_pubkey);

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
                Tag::SigFlag(sigflag) => sigflag.clone(),
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

        Ok(P2PKConditions {
            locktime,
            pubkeys,
            refund_keys,
            num_sigs,
            sig_flag,
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord, Hash)]
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
            t => Self::Custom(t.to_owned()),
        }
    }
}

#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize, PartialOrd, Ord, Hash)]
pub enum SigFlag {
    #[default]
    SigInputs,
    SigAll,
    Custom(String),
}

impl fmt::Display for SigFlag {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Self::SigAll => write!(f, "SIG_ALL"),
            Self::SigInputs => write!(f, "SIG_INPUTS"),
            Self::Custom(flag) => write!(f, "{}", flag),
        }
    }
}

impl<S> From<S> for SigFlag
where
    S: AsRef<str>,
{
    fn from(tag: S) -> Self {
        match tag.as_ref() {
            "SIG_ALL" => Self::SigAll,
            "SIG_INPUTS" => Self::SigInputs,
            tag => Self::Custom(tag.to_string()),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Tag {
    SigFlag(SigFlag),
    NSigs(u64),
    LockTime(u64),
    Refund(Vec<VerifyingKey>),
    PubKeys(Vec<VerifyingKey>),
}

impl Tag {
    pub fn kind(&self) -> TagKind {
        match self {
            Self::SigFlag(_) => TagKind::SigFlag,
            Self::NSigs(_) => TagKind::NSigs,
            Self::LockTime(_) => TagKind::Locktime,
            Self::Refund(_) => TagKind::Refund,
            Self::PubKeys(_) => TagKind::Pubkeys,
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
            TagKind::SigFlag => Ok(Tag::SigFlag(SigFlag::from(tag[1].as_ref()))),
            TagKind::NSigs => Ok(Tag::NSigs(tag[1].as_ref().parse()?)),
            TagKind::Locktime => Ok(Tag::LockTime(tag[1].as_ref().parse()?)),
            TagKind::Refund => {
                let pubkeys = tag
                    .iter()
                    .skip(1)
                    .flat_map(|p| VerifyingKey::from_str(p.as_ref()))
                    .collect();

                Ok(Self::Refund(pubkeys))
            }
            TagKind::Pubkeys => {
                let pubkeys = tag
                    .iter()
                    .skip(1)
                    .flat_map(|p| VerifyingKey::from_str(p.as_ref()))
                    .collect();

                Ok(Self::PubKeys(pubkeys))
            }
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
                    let pubkey: PublicKey = pubkey.to_normalized_public_key();
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct VerifyingKey(XOnlyPublicKey);

impl VerifyingKey {
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, Error> {
        Ok(Self(XOnlyPublicKey::from_slice(bytes)?))
    }

    pub fn from_public_key(public_key: PublicKey) -> Self {
        let (pk, ..) = public_key.x_only_public_key();
        Self(pk)
    }

    pub fn to_normalized_public_key(self) -> PublicKey {
        NormalizedPublicKey::from_x_only_public_key(self.0, Parity::Even).into()
    }

    pub fn verify(&self, msg: &[u8], sig: &Signature) -> Result<(), Error> {
        let hash: Sha256Hash = Sha256Hash::hash(msg);
        let msg = Message::from_slice(hash.as_ref())?;
        SECP256K1.verify_schnorr(sig, &msg, &self.0)?;
        Ok(())
    }
}

impl FromStr for VerifyingKey {
    type Err = Error;

    fn from_str(hex: &str) -> Result<Self, Self::Err> {
        let bytes: Vec<u8> = hex::decode(hex)?;

        // Check len
        let bytes: &[u8] = if bytes.len() == 33 {
            &bytes[1..]
        } else {
            &bytes
        };

        Ok(Self(XOnlyPublicKey::from_slice(bytes)?))
    }
}

impl fmt::Display for VerifyingKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl TryFrom<PublicKey> for VerifyingKey {
    type Error = Error;
    fn try_from(value: PublicKey) -> Result<VerifyingKey, Self::Error> {
        (&value).try_into()
    }
}

impl TryFrom<&PublicKey> for VerifyingKey {
    type Error = Error;
    fn try_from(value: &PublicKey) -> Result<VerifyingKey, Self::Error> {
        let bytes = value.to_bytes();

        let bytes = if bytes.len().eq(&33) {
            bytes.iter().skip(1).cloned().collect()
        } else {
            bytes.to_vec()
        };

        VerifyingKey::from_bytes(&bytes)
    }
}

#[derive(Clone, Serialize, Deserialize)]
pub struct SigningKey {
    secret_key: SecretKey,
    key_pair: KeyPair,
}

impl Deref for SigningKey {
    type Target = SecretKey;

    fn deref(&self) -> &Self::Target {
        &self.secret_key
    }
}

impl SigningKey {
    #[inline]
    pub fn new(secret_key: SecretKey) -> Self {
        Self {
            key_pair: KeyPair::from_secret_key(&SECP256K1, &secret_key),
            secret_key,
        }
    }
    pub fn sign(&self, msg: &[u8]) -> Result<Signature, Error> {
        let hash: Sha256Hash = Sha256Hash::hash(msg);
        let msg = Message::from_slice(hash.as_ref())?;
        Ok(SECP256K1.sign_schnorr(&msg, &self.key_pair))
    }

    #[inline]
    pub fn verifying_key(&self) -> VerifyingKey {
        let public_key: PublicKey = self.public_key();
        VerifyingKey::from_public_key(public_key)
    }
}

impl FromStr for SigningKey {
    type Err = Error;

    fn from_str(hex: &str) -> Result<Self, Self::Err> {
        let secret_key = SecretKey::from_hex(hex)?;
        Ok(Self::new(secret_key))
    }
}

impl fmt::Display for SigningKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.secret_key)
    }
}

#[cfg(test)]
mod tests {
    use std::str::FromStr;

    use super::*;
    use crate::nuts::Id;
    use crate::Amount;

    #[test]
    fn test_secret_ser() {
        let conditions = P2PKConditions {
            locktime: Some(99999),
            pubkeys: vec![
                VerifyingKey::from_str(
                    "033281c37677ea273eb7183b783067f5244933ef78d8c3f15b1a77cb246099c26e",
                )
                .unwrap(),
                VerifyingKey::from_str(
                    "02698c4e2b5f9534cd0687d87513c759790cf829aa5739184a3e3735471fbda904",
                )
                .unwrap(),
                VerifyingKey::from_str(
                    "023192200a0cfd3867e48eb63b03ff599c7e46c8f4e41146b2d281173ca6c50c54",
                )
                .unwrap(),
            ],
            refund_keys: Some(vec![VerifyingKey::from_str(
                "033281c37677ea273eb7183b783067f5244933ef78d8c3f15b1a77cb246099c26e",
            )
            .unwrap()]),
            num_sigs: Some(2),
            sig_flag: SigFlag::SigAll,
        };

        let secret: Secret = conditions.try_into().unwrap();

        let secret_str = serde_json::to_string(&secret).unwrap();

        let secret_der: Secret = serde_json::from_str(&secret_str).unwrap();

        assert_eq!(secret_der, secret);
    }

    #[test]
    fn sign_proof() {
        let secret_key = SigningKey::from_str(
            "04918dfc36c93e7db6cc0d60f37e1522f1c36b64d3f4b424c532d7c595febbc5",
        )
        .unwrap();

        let signing_key_two = SigningKey::from_str(
            "0000000000000000000000000000000000000000000000000000000000000001",
        )
        .unwrap();

        let signing_key_three = SigningKey::from_str(
            "7f7f7f7f7f7f7f7f7f7f7f7f7f7f7f7f7f7f7f7f7f7f7f7f7f7f7f7f7f7f7f7f",
        )
        .unwrap();
        let v_key: VerifyingKey = secret_key.verifying_key();
        let v_key_two: VerifyingKey = signing_key_two.verifying_key();
        let v_key_three: VerifyingKey = signing_key_three.verifying_key();

        let conditions = P2PKConditions {
            locktime: Some(21),
            pubkeys: vec![v_key.clone(), v_key_two, v_key_three],
            refund_keys: Some(vec![v_key]),
            num_sigs: Some(2),
            sig_flag: SigFlag::SigInputs,
        };

        let secret: super::Secret = conditions.try_into().unwrap();

        let mut proof = Proof {
            keyset_id: Id::from_str("009a1f293253e41e").unwrap(),
            amount: Amount::ZERO,
            secret: secret.clone().try_into().unwrap(),
            c: PublicKey::from_str(
                "02698c4e2b5f9534cd0687d87513c759790cf829aa5739184a3e3735471fbda904",
            )
            .unwrap(),
            witness: Some(Signatures { signatures: vec![] }),
            dleq: None,
        };

        proof.sign_p2pk(secret_key).unwrap();

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

        // Proof with onlt one of the required signatures
        let invalid_proof = r#"{"amount":0,"secret":"[\"P2PK\",{\"nonce\":\"0ed3fcb22c649dd7bbbdcca36e0c52d4f0187dd3b6a19efcc2bfbebb5f85b2a1\",\"data\":\"0249098aa8b9d2fbec49ff8598feb17b592b986e62319a4fa488a3dc36387157a7\",\"tags\":[[\"pubkeys\",\"0279be667ef9dcbbac55a06295ce870b07029bfcdb2dce28d959f2815b16f81798\",\"02142715675faf8da1ecc4d51e0b9e539fa0d52fdd96ed60dbe99adb15d6b05ad9\"],[\"n_sigs\",\"2\"],[\"sigflag\",\"SIG_INPUTS\"]]}]","C":"02698c4e2b5f9534cd0687d87513c759790cf829aa5739184a3e3735471fbda904","id":"009a1f293253e41e","witness":"{\"signatures\":[\"83564aca48c668f50d022a426ce0ed19d3a9bdcffeeaee0dc1e7ea7e98e9eff1840fcc821724f623468c94f72a8b0a7280fa9ef5a54a1b130ef3055217f467b3\"]}"}"#;

        let invalid_proof: Proof = serde_json::from_str(invalid_proof).unwrap();

        // Verification should fail without the requires signatures
        assert!(invalid_proof.verify_p2pk().is_err());
    }

    #[test]
    fn verify_refund() {
        let valid_proof = r#"{"amount":0,"secret":"[\"P2PK\",{\"nonce\":\"3eff971bb1ca70b16be3446a4d3feedf2f37f054c5c8621d832744df71b028f0\",\"data\":\"0249098aa8b9d2fbec49ff8598feb17b592b986e62319a4fa488a3dc36387157a7\",\"tags\":[[\"pubkeys\",\"0279be667ef9dcbbac55a06295ce870b07029bfcdb2dce28d959f2815b16f81798\",\"02142715675faf8da1ecc4d51e0b9e539fa0d52fdd96ed60dbe99adb15d6b05ad9\"],[\"locktime\",\"21\"],[\"n_sigs\",\"2\"],[\"refund\",\"49098aa8b9d2fbec49ff8598feb17b592b986e62319a4fa488a3dc36387157a7\"],[\"sigflag\",\"SIG_INPUTS\"]]}]","C":"02698c4e2b5f9534cd0687d87513c759790cf829aa5739184a3e3735471fbda904","id":"009a1f293253e41e","witness":"{\"signatures\":[\"94c6355461ca88e5d22c4e65e920b2e8253ccb4dd084675453a7bba7044e580246bd05e2520691afeccb2a88784cc56064353aec8b6a61e172727ba9cb3054a1\"]}"}"#;

        let valid_proof: Proof = serde_json::from_str(valid_proof).unwrap();
        assert!(valid_proof.verify_p2pk().is_ok());

        let invalid_proof = r#"{"amount":0,"secret":"[\"P2PK\",{\"nonce\":\"d14cf9be9d9438d548b6b9d29bf800611136d053421b0f48c38d1447a7a92fc8\",\"data\":\"0249098aa8b9d2fbec49ff8598feb17b592b986e62319a4fa488a3dc36387157a7\",\"tags\":[[\"pubkeys\",\"0279be667ef9dcbbac55a06295ce870b07029bfcdb2dce28d959f2815b16f81798\",\"02142715675faf8da1ecc4d51e0b9e539fa0d52fdd96ed60dbe99adb15d6b05ad9\"],[\"locktime\",\"2100000000000\"],[\"n_sigs\",\"2\"],[\"refund\",\"49098aa8b9d2fbec49ff8598feb17b592b986e62319a4fa488a3dc36387157a7\"],[\"sigflag\",\"SIG_INPUTS\"]]}]","C":"02698c4e2b5f9534cd0687d87513c759790cf829aa5739184a3e3735471fbda904","id":"009a1f293253e41e","witness":"{\"signatures\":[\"c3079dccf828e9d38bbbb17edf19c7915ee11920cf271c36b8780fdeb88b16fbfbe0328c7dcbe80e56cdc8f85c5831c79df77b27e81e5630a4dd392601fab9eb\"]}"}"#;

        let invalid_proof: Proof = serde_json::from_str(invalid_proof).unwrap();

        assert!(invalid_proof.verify_p2pk().is_err());
    }
}
