use std::collections::HashSet;

use bitcoin::hashes::Hash;
use bitcoin::secp256k1::{PublicKey, SecretKey};
use serde::{Deserialize, Serialize};

use super::{nums_key, sender_key, tweaked_key, Condition, Error, Leaf, SpendInfo, Tree};
use crate::nuts::nut10::SpendingConditions;
use crate::nuts::SigFlag;
use crate::util::hex;
use crate::SECP256K1;

/// Payment-request locking policy for version-02 outputs (NUT-18).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct NutrootOption {
    /// Static receiver key, or the NUMS point for script-only outputs.
    #[serde(rename = "k")]
    pub key: PublicKey,
    /// Serialized leaves in slot order.
    #[serde(rename = "l", default, skip_serializing_if = "Option::is_none")]
    pub leaves: Option<Vec<String>>,
    /// Leaf keys whose owners request NUT-28 blinding.
    #[serde(rename = "b", default, skip_serializing_if = "Option::is_none")]
    pub blind_keys: Option<Vec<PublicKey>>,
}

/// BIP341's NUMS point, used to request script-only locking.
pub fn nums_point() -> PublicKey {
    super::parse_secret("0250929b74c1a04954b78b4b6035e97a5e078a5a0f28ec96d547bfee9ace803ac0")
        .expect("fixed valid NUMS point")
}

impl NutrootOption {
    /// Validate leaves and the blind-me subset before constructing any outputs.
    pub fn validate(&self) -> Result<Option<Tree>, Error> {
        let tree = SpendInfo {
            tree: self.leaves.clone(),
            ..Default::default()
        }
        .parsed_tree()?;
        if self.key == nums_point() && tree.is_none() {
            return Err(Error::InvalidSpendInfo);
        }
        let keys: HashSet<_> = tree
            .as_ref()
            .into_iter()
            .flat_map(|tree| tree.leaves())
            .flat_map(|leaf| leaf.keys())
            .copied()
            .collect();
        let mut blind = HashSet::new();
        if self
            .blind_keys
            .as_deref()
            .unwrap_or_default()
            .iter()
            .any(|key| !keys.contains(key) || !blind.insert(*key))
        {
            return Err(Error::InvalidSpendInfo);
        }
        Ok(tree)
    }

    /// Construct one output. Callers must supply fresh scalars for every proof.
    pub fn create_output(
        &self,
        ephemeral: &SecretKey,
        offset: &SecretKey,
    ) -> Result<(String, SpendInfo), Error> {
        let tree = self.validate()?;
        let nums = self.key == nums_point();
        let blind = self.blind_keys.as_deref().unwrap_or_default();
        let uses_ephemeral = !nums || !blind.is_empty();
        let internal = if nums {
            nums_key(offset)?
        } else {
            sender_key(&self.key, ephemeral, 0)?
        };
        let mut slot = 0u8;
        let leaves = tree
            .map(|tree| {
                tree.leaves()
                    .iter()
                    .map(|leaf| {
                        let keys = leaf
                            .keys()
                            .iter()
                            .map(|key| {
                                slot = slot.checked_add(1).ok_or(Error::InvalidTree)?;
                                if blind.contains(key) {
                                    sender_key(key, ephemeral, slot)
                                } else {
                                    Ok(*key)
                                }
                            })
                            .collect::<Result<Vec<_>, Error>>()?;
                        Leaf::new(
                            leaf.threshold(),
                            keys,
                            leaf.condition().clone(),
                            leaf.disclosure(),
                        )
                    })
                    .collect::<Result<Vec<_>, Error>>()
            })
            .transpose()?;
        let tree = leaves.map(Tree::new).transpose()?;
        let secret = tweaked_key(internal, tree.as_ref().map(Tree::root))?.to_string();
        let info = SpendInfo {
            internal_key: Some(internal),
            ephemeral_key: uses_ephemeral
                .then(|| PublicKey::from_secret_key(&SECP256K1, ephemeral)),
            nums_offset: nums.then(|| (*offset).into()),
            tree: tree.map(|tree| {
                tree.leaves()
                    .iter()
                    .map(|leaf| hex::encode(leaf.to_bytes()))
                    .collect()
            }),
            bearer_key: None,
        };
        Ok((secret, info))
    }

    /// Verify a received output against this exact requested policy.
    /// Keys not owned by this receiver are checked structurally; each key owner
    /// must verify its own NUT-28 substitutions before signing.
    pub fn verify_output(
        &self,
        secret: &str,
        info: &SpendInfo,
        receiver_keys: &[SecretKey],
    ) -> Result<(), Error> {
        let requested = self.validate()?;
        let nums = self.key == nums_point();
        let blind = self.blind_keys.as_deref().unwrap_or_default();
        let uses_ephemeral = !nums || !blind.is_empty();
        if info.bearer_key.is_some()
            || info.ephemeral_key.is_some() != uses_ephemeral
            || info.nums_offset.is_some() != nums
        {
            return Err(Error::InvalidSpendInfo);
        }
        let receiver = receiver_keys
            .iter()
            .find(|key| PublicKey::from_secret_key(&SECP256K1, key) == self.key);
        let internal = info.verify(secret, receiver)?;
        let actual = info.parsed_tree()?;
        if tweaked_key(internal, actual.as_ref().map(Tree::root))? != super::parse_secret(secret)? {
            return Err(Error::InvalidSpendInfo);
        }
        let requested_leaves = requested.as_ref().map(Tree::leaves).unwrap_or_default();
        let actual_leaves = actual.as_ref().map(Tree::leaves).unwrap_or_default();
        if requested_leaves.len() != actual_leaves.len() {
            return Err(Error::InvalidSpendInfo);
        }
        let slots: usize = actual_leaves.iter().map(|leaf| leaf.keys().len()).sum();
        let mut unmatched: Vec<_> = actual_leaves.iter().collect();
        for requested in requested_leaves {
            let matching =
                unmatched
                    .iter()
                    .position(|actual| {
                        requested.threshold() == actual.threshold()
                            && requested.condition() == actual.condition()
                            && requested.disclosure() == actual.disclosure()
                            && requested.keys().len() == actual.keys().len()
                            && requested.keys().iter().zip(actual.keys()).all(
                                |(key, substituted)| {
                                    if !blind.contains(key) {
                                        return key == substituted;
                                    }
                                    let owner = receiver_keys.iter().find(|private| {
                                        PublicKey::from_secret_key(&SECP256K1, private) == *key
                                    });
                                    match owner {
                                        Some(owner) => (1..=slots).any(|slot| {
                                            let Some(ephemeral) = &info.ephemeral_key else {
                                                return false;
                                            };
                                            super::receiver_key(owner, ephemeral, slot as u8)
                                                .is_ok_and(|derived| {
                                                    PublicKey::from_secret_key(&SECP256K1, &derived)
                                                        == *substituted
                                                })
                                        }),
                                        None => true,
                                    }
                                },
                            )
                    })
                    .ok_or(Error::InvalidSpendInfo)?;
            unmatched.remove(matching);
        }
        Ok(())
    }

    /// Translate policies with equivalent signature, hashlock and refund semantics.
    /// Keyless hashlocks/refunds and legacy SIG_ALL options are rejected.
    pub fn from_spending_conditions(
        policy: &SpendingConditions,
        blind: bool,
    ) -> Result<Self, Error> {
        let conditions = match policy {
            SpendingConditions::P2PKConditions { conditions, .. }
            | SpendingConditions::HTLCConditions { conditions, .. } => conditions.as_ref(),
        };
        if conditions.is_some_and(|c| c.sig_flag == SigFlag::SigAll) {
            return Err(Error::InvalidLeaf);
        }
        let keys = policy
            .pubkeys()
            .unwrap_or_default()
            .iter()
            .map(|key| key.as_secp256k1().copied().map_err(|_| Error::InvalidLeaf))
            .collect::<Result<Vec<_>, _>>()?;
        let condition = match policy {
            SpendingConditions::P2PKConditions { .. } => Condition::Threshold,
            SpendingConditions::HTLCConditions { data, .. } => {
                Condition::Hashlock(data.to_byte_array())
            }
        };
        let threshold =
            u8::try_from(policy.num_sigs().unwrap_or(1)).map_err(|_| Error::InvalidLeaf)?;
        let mut leaves = vec![Leaf::new(threshold, keys, condition, false)?];
        match (policy.locktime(), policy.refund_keys()) {
            (Some(time), Some(keys)) if !keys.is_empty() => {
                let threshold =
                    u8::try_from(conditions.and_then(|c| c.num_sigs_refund).unwrap_or(1))
                        .map_err(|_| Error::InvalidLeaf)?;
                let keys = keys
                    .iter()
                    .map(|key| key.as_secp256k1().copied().map_err(|_| Error::InvalidLeaf))
                    .collect::<Result<Vec<_>, _>>()?;
                leaves.push(Leaf::new(threshold, keys, Condition::After(time), false)?);
            }
            (None, None) => {}
            _ => return Err(Error::InvalidLeaf),
        }
        let blind_keys = blind.then(|| {
            let mut keys = vec![];
            for key in leaves.iter().flat_map(|leaf| leaf.keys()) {
                if !keys.contains(key) {
                    keys.push(*key);
                }
            }
            keys
        });
        Ok(Self {
            key: nums_point(),
            leaves: Some(
                leaves
                    .iter()
                    .map(|leaf| hex::encode(leaf.to_bytes()))
                    .collect(),
            ),
            blind_keys,
        })
    }
}
