//! NUT-10: Spending Conditions
//!
//! <https://github.com/cashubtc/nuts/blob/main/10.md>

use std::{collections::HashSet, str::FromStr};

use crate::{nut10::Error, secret::Secret, util::hex, Kind, Nut10Secret};
use bitcoin::hashes::sha256::Hash as Sha256Hash;
use bitcoin::hashes::Hash;
use serde::{Deserialize, Serialize};

use crate::{Conditions, PublicKey};

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
                    .map_err(|_| Error::InvalidHash)?,
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
            SpendingConditions::P2PKConditions { data, conditions } => {
                super::Secret::new(Kind::P2PK, data.to_hex(), conditions)
            }
            SpendingConditions::HTLCConditions { data, conditions } => {
                super::Secret::new(Kind::HTLC, data.to_string(), conditions)
            }
        }
    }
}
