//! NUT-10: Spending Conditions
//!
//! <https://github.com/cashubtc/nuts/blob/main/10.md>

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
                Some(pubkeys)
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
                    .cloned()
                    .map(Conditions::try_from)
                    .transpose()?,
            }),
            Kind::HTLC => Ok(Self::HTLCConditions {
                data: Sha256Hash::from_str(secret.secret_data().data())
                    .map_err(|_| Error::NUT14(nut14::Error::InvalidHash))?,
                conditions: secret
                    .secret_data()
                    .tags()
                    .cloned()
                    .map(Conditions::try_from)
                    .transpose()?,
            }),
        }
    }
}

/// The only door from an author-supplied lock to wire bytes, so the
/// construction rules are enforced here instead of at each caller.
impl TryFrom<SpendingConditions> for super::Secret {
    type Error = Error;
    fn try_from(conditions: SpendingConditions) -> Result<super::Secret, Self::Error> {
        conditions.validate()?;

        Ok(match conditions {
            SpendingConditions::P2PKConditions { data, conditions } => super::Secret::new(
                Kind::P2PK,
                super::SecretData::new(data.to_hex(), conditions),
            ),
            SpendingConditions::HTLCConditions { data, conditions } => super::Secret::new(
                Kind::HTLC,
                super::SecretData::new(data.to_string(), conditions),
            ),
        })
    }
}

impl TryFrom<SpendingConditions> for Secret {
    type Error = Error;
    fn try_from(conditions: SpendingConditions) -> Result<Secret, Self::Error> {
        Secret::try_from(Nut10Secret::try_from(conditions)?)
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
    fn validate(&self, primary_key_count: u64) -> Result<(), Error> {
        if let Some(n) = self.num_sigs {
            if n == 0 {
                return Err(Error::NUT11(crate::nut11::Error::ZeroSignaturesRequired));
            }

            let available_keys =
                primary_key_count + self.pubkeys.as_ref().map(Vec::len).unwrap_or(0) as u64;
            if n > available_keys {
                return Err(Error::NUT11(
                    crate::nut11::Error::ImpossibleMultisigConfiguration {
                        required: n,
                        available: available_keys,
                    },
                ));
            }
        }

        match (&self.refund_keys, self.num_sigs_refund) {
            (Some(_), Some(0)) => {
                return Err(Error::NUT11(crate::nut11::Error::ZeroSignaturesRequired));
            }
            (Some(refund_keys), Some(required)) if required > refund_keys.len() as u64 => {
                return Err(Error::NUT11(
                    crate::nut11::Error::ImpossibleRefundMultisigConfiguration {
                        required,
                        available: refund_keys.len() as u64,
                    },
                ));
            }
            (None, Some(0)) => {
                return Err(Error::NUT11(crate::nut11::Error::ZeroSignaturesRequired));
            }
            (None, Some(required)) => {
                return Err(Error::NUT11(
                    crate::nut11::Error::ImpossibleRefundMultisigConfiguration {
                        required,
                        available: 0,
                    },
                ));
            }
            (Some(refund_keys), None) if refund_keys.is_empty() => {
                return Err(Error::NUT11(
                    crate::nut11::Error::ImpossibleRefundMultisigConfiguration {
                        required: 1,
                        available: 0,
                    },
                ));
            }
            _ => {}
        }

        Ok(())
    }

    /// Create new Spending [`Conditions`]
    ///
    /// A key repeated within either list is rejected: verification counts keys
    /// by x-coordinate, so the repetition can never add a signer. On top of the
    /// protocol rules this constructor applies authoring policy, refusing a
    /// locktime already in the past and refund keys with no locktime, since
    /// both describe a branch the author cannot have meant to leave dead.
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

        if let Some(pubkeys) = pubkeys.as_deref() {
            super::check_duplicate_pubkeys(pubkeys)?;
        }
        if let Some(refund_keys) = refund_keys.as_deref() {
            super::check_duplicate_pubkeys(refund_keys)?;
        }

        let conditions = Self {
            locktime,
            pubkeys,
            refund_keys,
            num_sigs,
            sig_flag: sig_flag.unwrap_or_default(),
            num_sigs_refund,
        };
        conditions.validate(1)?;

        if conditions
            .refund_keys
            .as_ref()
            .is_some_and(|keys| !keys.is_empty())
            && conditions.locktime.is_none()
        {
            return Err(Error::NUT11(crate::nut11::Error::RefundKeysRequireLocktime));
        }

        Ok(conditions)
    }
}

/// Reject a key set that P2BK cannot blind.
///
/// Only NUT-28 assigns slot indices, and it carries them in a single byte, so
/// the blinded `data` key plus the `pubkeys` and refund tags cannot occupy more
/// than [`MAX_LOCKING_SLOTS`] entries. Plain NUT-11 and NUT-14 locks have no
/// such limit, and verification never counts slots.
pub(crate) fn check_locking_slots(
    pubkeys: usize,
    refund_keys: Option<&[PublicKey]>,
) -> Result<(), Error> {
    let slots = 1 + pubkeys + refund_keys.map(<[PublicKey]>::len).unwrap_or(0);
    if slots > super::MAX_LOCKING_SLOTS {
        return Err(Error::NUT11(crate::nut11::Error::TooManyPubkeys { slots }));
    }
    Ok(())
}

/// Enforce the P2PK construction rules over a key set.
///
/// `data` and the `pubkeys` tag are one signing pathway, so a key repeated
/// across the two is a duplicate; the refund tag is checked as its own set.
/// Shared with the P2BK path, which has to reject before blinding: each key is
/// blinded under its own slot index, so a repeated key would blind into two
/// different keys and hide the duplicate.
pub(crate) fn validate_p2pk(data: PublicKey, conditions: Option<&Conditions>) -> Result<(), Error> {
    let Some(conditions) = conditions else {
        return Ok(());
    };

    let mut primary = vec![data];
    primary.extend(conditions.pubkeys.clone().unwrap_or_default());
    super::check_duplicate_pubkeys(&primary)?;

    if let Some(refund_keys) = conditions.refund_keys.as_deref() {
        super::check_duplicate_pubkeys(refund_keys)?;
    }

    conditions.validate(1)
}

impl SpendingConditions {
    /// Enforce the NUT-10/11 construction rules, rejecting duplicate keys.
    ///
    /// Keys are compared by x-coordinate within a signing pathway, the way
    /// verification compares them, so a lock that validates here is one the
    /// mint will accept. The duplicate check runs before the signature
    /// thresholds so a repeated key always reports the repetition. Only
    /// protocol rules are enforced; the authoring policy in
    /// [`Conditions::new`] is not applied here.
    pub fn validate(&self) -> Result<(), Error> {
        match self {
            Self::P2PKConditions { data, conditions } => validate_p2pk(*data, conditions.as_ref()),
            Self::HTLCConditions { conditions, .. } => {
                let Some(conditions) = conditions else {
                    return Ok(());
                };

                if let Some(pubkeys) = conditions.pubkeys.as_deref() {
                    super::check_duplicate_pubkeys(pubkeys)?;
                }
                if let Some(refund_keys) = conditions.refund_keys.as_deref() {
                    super::check_duplicate_pubkeys(refund_keys)?;
                }

                conditions.validate(0)
            }
        }
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

        if let Some(refund_keys) = refund_keys.filter(|keys| !keys.is_empty()) {
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

        if let Some(0) = num_sigs {
            return Err(Error::NUT11(crate::nut11::Error::ZeroSignaturesRequired));
        }
        if let Some(0) = num_sigs_refund {
            return Err(Error::NUT11(crate::nut11::Error::ZeroSignaturesRequired));
        }

        if let Some(refund_keys) = &refund_keys {
            let required = num_sigs_refund.unwrap_or(1);
            if required > refund_keys.len() as u64 {
                return Err(Error::NUT11(
                    crate::nut11::Error::ImpossibleRefundMultisigConfiguration {
                        required,
                        available: refund_keys.len() as u64,
                    },
                ));
            }
        } else if let Some(required) = num_sigs_refund {
            return Err(Error::NUT11(
                crate::nut11::Error::ImpossibleRefundMultisigConfiguration {
                    required,
                    available: 0,
                },
            ));
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

    #[test]
    fn test_empty_refund_tag_is_rejected() {
        let tags = vec![
            vec!["refund".to_string()],
            vec!["sigflag".to_string(), "SIG_INPUTS".to_string()],
        ];

        let result = Conditions::try_from(tags);

        assert!(matches!(
            result,
            Err(Error::NUT11(
                crate::nut11::Error::ImpossibleRefundMultisigConfiguration {
                    required: 1,
                    available: 0
                }
            ))
        ));
    }

    #[test]
    fn test_empty_refund_keys_are_not_serialized() {
        let conditions = Conditions {
            locktime: Some(1),
            pubkeys: None,
            refund_keys: Some(vec![]),
            num_sigs: None,
            sig_flag: crate::SigFlag::default(),
            num_sigs_refund: None,
        };

        let tags = Vec::<Vec<String>>::from(conditions);

        assert!(!tags
            .iter()
            .any(|tag| tag.first() == Some(&"refund".to_string())));
    }

    #[test]
    fn test_n_sigs_refund_without_refund_keys_is_rejected() {
        let tags = vec![
            vec!["n_sigs_refund".to_string(), "1".to_string()],
            vec!["sigflag".to_string(), "SIG_INPUTS".to_string()],
        ];

        let result = Conditions::try_from(tags);

        assert!(matches!(
            result,
            Err(Error::NUT11(
                crate::nut11::Error::ImpossibleRefundMultisigConfiguration {
                    required: 1,
                    available: 0
                }
            ))
        ));
    }

    #[test]
    fn test_spending_conditions_try_from_propagates_invalid_tags() {
        let pubkey = PublicKey::from_str(
            "026562efcfadc8e86d44da6a8adf80633d974302e62c850774db1fb36ff4cc7198",
        )
        .unwrap();
        let nut10_secret = Nut10Secret::new(
            Kind::P2PK,
            crate::nuts::nut10::SecretData::new(
                pubkey.to_string(),
                Some(vec![vec!["n_sigs".to_string(), "0".to_string()]]),
            ),
        );

        let result = SpendingConditions::try_from(nut10_secret);

        assert!(matches!(
            result,
            Err(Error::NUT11(crate::nut11::Error::ZeroSignaturesRequired))
        ));
    }

    #[test]
    fn test_num_sigs_accessor_preserves_configured_value() {
        let conditions = SpendingConditions::P2PKConditions {
            data: PublicKey::from_str(
                "026562efcfadc8e86d44da6a8adf80633d974302e62c850774db1fb36ff4cc7198",
            )
            .unwrap(),
            conditions: Some(Conditions {
                num_sigs: Some(3),
                ..Default::default()
            }),
        };

        assert_eq!(conditions.num_sigs(), Some(3));
    }

    #[test]
    fn test_conditions_reject_zero_signature_thresholds() {
        let pubkey = PublicKey::from_str(
            "026562efcfadc8e86d44da6a8adf80633d974302e62c850774db1fb36ff4cc7198",
        )
        .unwrap();

        let result = Conditions {
            pubkeys: Some(vec![pubkey]),
            num_sigs: Some(0),
            ..Default::default()
        }
        .validate(1);

        assert!(matches!(
            result,
            Err(Error::NUT11(crate::nut11::Error::ZeroSignaturesRequired))
        ));

        let result = Conditions {
            refund_keys: Some(vec![pubkey]),
            num_sigs_refund: Some(0),
            ..Default::default()
        }
        .validate(1);

        assert!(matches!(
            result,
            Err(Error::NUT11(crate::nut11::Error::ZeroSignaturesRequired))
        ));

        let result = Conditions {
            num_sigs_refund: Some(0),
            ..Default::default()
        }
        .validate(1);

        assert!(matches!(
            result,
            Err(Error::NUT11(crate::nut11::Error::ZeroSignaturesRequired))
        ));
    }

    /// Without a locktime the refund branch never opens, but the primary
    /// pathway stays spendable, so NUT-11 still accepts the lock. Only
    /// `Conditions::new` refuses it.
    #[test]
    fn test_refund_keys_without_locktime_build_a_secret() {
        let key = PublicKey::from_str(
            "033281c37677ea273eb7183b783067f5244933ef78d8c3f15b1a77cb246099c26e",
        )
        .unwrap();
        let data = PublicKey::from_str(
            "026562efcfadc8e86d44da6a8adf80633d974302e62c850774db1fb36ff4cc7198",
        )
        .unwrap();

        let conditions = SpendingConditions::P2PKConditions {
            data,
            conditions: Some(Conditions {
                refund_keys: Some(vec![key]),
                locktime: None,
                ..Default::default()
            }),
        };

        let result: Result<Secret, _> = conditions.try_into();

        assert!(
            result.is_ok(),
            "refund keys without a locktime should build: {:?}",
            result.err()
        );
    }

    /// Wire bytes are only reachable through this conversion, so a lock
    /// verification would refuse can never be serialized.
    #[test]
    fn test_nut10_secret_rejects_duplicate_p2pk_key() {
        let key = PublicKey::from_str(
            "026562efcfadc8e86d44da6a8adf80633d974302e62c850774db1fb36ff4cc7198",
        )
        .unwrap();

        let result = Nut10Secret::try_from(SpendingConditions::P2PKConditions {
            data: key,
            conditions: Some(Conditions {
                pubkeys: Some(vec![key]),
                ..Default::default()
            }),
        });

        assert!(matches!(
            result,
            Err(Error::NUT11(crate::nut11::Error::DuplicatePubkey))
        ));
    }

    #[test]
    fn test_conditions_reject_empty_refund_keys_with_locktime() {
        let result = Conditions {
            locktime: Some(100),
            refund_keys: Some(vec![]),
            ..Default::default()
        }
        .validate(1);

        assert!(matches!(
            result,
            Err(Error::NUT11(
                crate::nut11::Error::ImpossibleRefundMultisigConfiguration {
                    required: 1,
                    available: 0
                }
            ))
        ));
    }
}
