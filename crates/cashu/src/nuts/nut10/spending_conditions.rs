//! NUT-10: Spending Conditions
//!
//! <https://github.com/cashubtc/nuts/blob/main/10.md>

use std::collections::{HashMap, HashSet};
use std::str::FromStr;

use bitcoin::hashes::sha256::Hash as Sha256Hash;
use serde::{Deserialize, Serialize};

use crate::nut10::{Error, Tag, TagKind};
use crate::secret::Secret;
use crate::util::unix_time;
use crate::{ensure_cdk, nut14, Kind, Nut10Secret, PublicKey, SigFlag};

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

    /// Public keys of locked
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
                    .map_err(|_| Error::NUT14(nut14::Error::InvalidHash))?,
                conditions: secret
                    .secret_data()
                    .tags()
                    .and_then(|t| t.clone().try_into().ok()),
            }),
        }
    }
}

impl From<SpendingConditions> for super::Secret {
    fn from(conditions: SpendingConditions) -> super::Secret {
        match conditions {
            SpendingConditions::P2PKConditions { data, conditions } => super::Secret::new(
                Kind::P2PK,
                super::SecretData::new(data.to_hex(), conditions),
            ),
            SpendingConditions::HTLCConditions { data, conditions } => super::Secret::new(
                Kind::HTLC,
                super::SecretData::new(data.to_string(), conditions),
            ),
        }
    }
}

impl TryFrom<SpendingConditions> for Secret {
    type Error = Error;
    fn try_from(conditions: SpendingConditions) -> Result<Secret, Self::Error> {
        let secret: Nut10Secret = conditions.into();
        Secret::try_from(secret)
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
            ensure_cdk!(
                locktime.ge(&unix_time()),
                Error::NUT11(crate::nut11::Error::LocktimeInPast)
            );
        }

        if let Some(n) = num_sigs {
            let available_keys = 1 + pubkeys.as_ref().map(Vec::len).unwrap_or(0);
            if n > available_keys as u64 {
                return Err(Error::NUT11(
                    crate::nut11::Error::ImpossibleMultisigConfiguration {
                        required: n,
                        available: available_keys as u64,
                    },
                ));
            }
        }

        if let Some(n) = num_sigs_refund {
            let refund_key_count = refund_keys.as_ref().map(Vec::len).unwrap_or(0);
            if n > refund_key_count as u64 {
                return Err(Error::NUT11(
                    crate::nut11::Error::ImpossibleMultisigConfiguration {
                        required: n,
                        available: refund_key_count as u64,
                    },
                ));
            }
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
