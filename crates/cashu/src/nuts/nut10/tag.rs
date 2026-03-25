//! NUT-10: Spending Conditions tags

use std::fmt;
use std::str::FromStr;

use serde::de::Error as DeserializerError;
use serde::ser::SerializeSeq;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::nut01::PublicKey;
use crate::nut10::Error;
use crate::SigFlag;

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
            TagKind::SigFlag => Ok(Tag::SigFlag(SigFlag::from_str(
                tag.get(1).ok_or(Error::TagValueNotFound)?.as_ref(),
            )?)),
            TagKind::NSigs => Ok(Tag::NSigs(
                tag.get(1)
                    .ok_or(Error::TagValueNotFound)?
                    .as_ref()
                    .parse()?,
            )),
            TagKind::Locktime => Ok(Tag::LockTime(
                tag.get(1)
                    .ok_or(Error::TagValueNotFound)?
                    .as_ref()
                    .parse()?,
            )),
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
            TagKind::NSigsRefund => Ok(Tag::NSigsRefund(
                tag.get(1)
                    .ok_or(Error::TagValueNotFound)?
                    .as_ref()
                    .parse()?,
            )),
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
