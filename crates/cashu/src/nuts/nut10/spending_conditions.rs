//! NUT-10: Spending Conditions
//!
//! <https://github.com/cashubtc/nuts/blob/main/10.md>

use std::collections::HashSet;
use std::str::FromStr;

use bitcoin::hashes::sha256::Hash as Sha256Hash;
use serde::{Deserialize, Serialize};

use crate::nut10::{Error, Tag};
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
        let mut locktime = None;
        let mut pubkeys = None;
        let mut refund_keys = None;
        let mut sig_flag = None;
        let mut num_sigs = None;
        let mut num_sigs_refund = None;

        for tag_vec in tags {
            let tag = Tag::try_from(tag_vec)?;
            match tag {
                Tag::LockTime(lt) => {
                    if locktime.is_none() {
                        locktime = Some(lt);
                    }
                }
                Tag::PubKeys(pks) => {
                    if pubkeys.is_none() {
                        pubkeys = Some(pks);
                    }
                }
                Tag::Refund(keys) => {
                    if refund_keys.is_none() {
                        refund_keys = Some(keys);
                    }
                }
                Tag::SigFlag(sf) => {
                    if sig_flag.is_none() {
                        sig_flag = Some(sf);
                    }
                }
                Tag::NSigs(sigs) => {
                    if num_sigs.is_none() {
                        num_sigs = Some(sigs);
                    }
                }
                Tag::NSigsRefund(sigs) => {
                    if num_sigs_refund.is_none() {
                        num_sigs_refund = Some(sigs);
                    }
                }
                Tag::Custom(_, _) => {}
            }
        }

        Ok(Conditions {
            locktime,
            pubkeys,
            refund_keys,
            num_sigs,
            sig_flag: sig_flag.unwrap_or_default(),
            num_sigs_refund,
        })
    }
}

#[cfg(test)]
mod tests {
    use std::str::FromStr;

    use super::*;
    use crate::nut01::PublicKey;

    #[test]
    fn test_duplicate_tags_first_match() {
        let pk1 = "026562efcfadc8e86d44da6a8adf80633d974302e62c850774db1fb36ff4cc7198";
        let pk2 = "02a4ed09e9b22c0563f2043593902973d040054ff03be93c990264177d65123982";

        let tags = vec![
            vec!["locktime".to_string(), "100".to_string()],
            vec!["locktime".to_string(), "1".to_string()],
            vec!["n_sigs".to_string(), "2".to_string()],
            vec!["n_sigs".to_string(), "1".to_string()],
            vec!["sigflag".to_string(), "SIG_ALL".to_string()],
            vec!["sigflag".to_string(), "SIG_INPUTS".to_string()],
            vec!["pubkeys".to_string(), pk1.to_string()],
            vec!["pubkeys".to_string(), pk2.to_string()],
            vec!["refund".to_string(), pk1.to_string()],
            vec!["refund".to_string(), pk2.to_string()],
        ];

        let conditions = Conditions::try_from(tags).unwrap();

        // Verify first-match semantics
        assert_eq!(conditions.locktime, Some(100));
        assert_eq!(conditions.num_sigs, Some(2));
        assert_eq!(conditions.sig_flag, crate::SigFlag::SigAll);
        assert_eq!(
            conditions.pubkeys,
            Some(vec![PublicKey::from_str(pk1).unwrap()])
        );
        assert_eq!(
            conditions.refund_keys,
            Some(vec![PublicKey::from_str(pk1).unwrap()])
        );
    }
}
