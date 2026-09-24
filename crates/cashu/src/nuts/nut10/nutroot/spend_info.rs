use std::fmt;

use bitcoin::secp256k1::{PublicKey, Scalar, SecretKey};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::{parse_secret, tweak, tweaked_key, Error, Leaf, Tree};
use crate::util::hex;
use crate::SECP256K1;

/// Wallet data needed to reconstruct and spend a transferred Nutroot proof.
/// Reconstruction alone does not prove the receiver can spend the proof;
/// callers must also verify ownership and every leaf against their policy.
#[derive(Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SpendInfo {
    /// Bearer internal private key. Mutually exclusive with `ephemeral_key`.
    #[serde(rename = "k", default, skip_serializing_if = "Option::is_none")]
    pub bearer_key: Option<crate::nuts::SecretKey>,
    /// Sender's ephemeral compressed key for receiver-keyed blinding.
    #[serde(rename = "E", default, skip_serializing_if = "Option::is_none")]
    pub ephemeral_key: Option<PublicKey>,
    /// Disclosed internal compressed public key.
    #[serde(rename = "K", default, skip_serializing_if = "Option::is_none")]
    pub internal_key: Option<PublicKey>,
    /// Serialized leaves in hex, in transmitted slot order.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tree: Option<Vec<String>>,
    /// NUMS offset scalar proving that no key path exists.
    #[serde(rename = "u", default, skip_serializing_if = "Option::is_none")]
    pub nums_offset: Option<crate::nuts::SecretKey>,
}

impl fmt::Debug for SpendInfo {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SpendInfo")
            .field(
                "bearer_key",
                &self.bearer_key.as_ref().map(|_| "[REDACTED]"),
            )
            .field("ephemeral_key", &self.ephemeral_key)
            .field("internal_key", &self.internal_key)
            .field("tree", &self.tree)
            .field(
                "nums_offset",
                &self.nums_offset.as_ref().map(|_| "[REDACTED]"),
            )
            .finish()
    }
}

impl SpendInfo {
    /// Parse and validate all disclosed leaves.
    pub fn parsed_tree(&self) -> Result<Option<Tree>, Error> {
        self.tree
            .as_ref()
            .map(|leaves| {
                if leaves.is_empty() || leaves.len() > 8 {
                    return Err(Error::InvalidTree);
                }
                Tree::new(
                    leaves
                        .iter()
                        .map(|leaf| {
                            if leaf.len() > 1026 {
                                return Err(Error::InvalidLeaf);
                            }
                            Leaf::from_bytes(&hex::decode(leaf)?)
                        })
                        .collect::<Result<_, _>>()?,
                )
            })
            .transpose()
    }

    /// Reconstruct the secret and validate an optional NUMS opening.
    /// `receiver` is the receiver's static private key, required for `E`.
    /// Returns the authoritative internal key; callers still check spendability.
    pub fn verify(&self, secret: &str, receiver: Option<&SecretKey>) -> Result<PublicKey, Error> {
        if self.bearer_key.is_some() && self.ephemeral_key.is_some() {
            return Err(Error::InvalidSpendInfo);
        }
        let internal = match (&self.bearer_key, self.ephemeral_key) {
            (Some(key), None) => PublicKey::from_secret_key(
                &SECP256K1,
                key.as_secp256k1().map_err(|_| Error::InvalidSpendInfo)?,
            ),
            (None, Some(ephemeral)) => {
                // Script-only outputs may blind leaf keys but never their NUMS base.
                match &self.nums_offset {
                    Some(offset) => {
                        nums_key(offset.as_secp256k1().map_err(|_| Error::InvalidSpendInfo)?)?
                    }
                    None => PublicKey::from_secret_key(
                        &SECP256K1,
                        &receiver_key(receiver.ok_or(Error::InvalidSpendInfo)?, &ephemeral, 0)?,
                    ),
                }
            }
            (None, None) => self.internal_key.ok_or(Error::InvalidSpendInfo)?,
            _ => return Err(Error::InvalidSpendInfo),
        };
        if self.internal_key.is_some_and(|key| key != internal) {
            return Err(Error::InvalidSpendInfo);
        }
        let tree = self.parsed_tree()?;
        if let Some(offset) = &self.nums_offset {
            if tree.is_none()
                || self.bearer_key.is_some()
                || nums_key(offset.as_secp256k1().map_err(|_| Error::InvalidSpendInfo)?)?
                    != internal
            {
                return Err(Error::InvalidSpendInfo);
            }
        }
        let secret = parse_secret(secret)?;
        let matches = match tree {
            Some(tree) => tweaked_key(internal, Some(tree.root()))? == secret,
            None => internal == secret || tweaked_key(internal, None)? == secret,
        };
        if !matches {
            return Err(Error::InvalidSpendInfo);
        }
        Ok(internal)
    }

    /// Recover a bearer or receiver key-path scalar after reconstruction.
    /// A receiver-derived key must never be re-gifted as a bearer key.
    pub fn key_path_key(
        &self,
        secret: &str,
        receiver: Option<&SecretKey>,
    ) -> Result<SecretKey, Error> {
        let internal = self.verify(secret, receiver)?;
        if self.nums_offset.is_some() {
            return Err(Error::InvalidSpendInfo);
        }
        let key = match (&self.bearer_key, self.ephemeral_key) {
            (Some(key), None) => *key.as_secp256k1().map_err(|_| Error::InvalidSpendInfo)?,
            (None, Some(ephemeral)) => {
                receiver_key(receiver.ok_or(Error::InvalidSpendInfo)?, &ephemeral, 0)?
            }
            _ => return Err(Error::InvalidSpendInfo),
        };
        let tree = self.parsed_tree()?;
        if tree.is_none() && internal == parse_secret(secret)? {
            return Ok(key);
        }
        Ok(key.add_tweak(&tweak(&internal, tree.map(|tree| tree.root())))?)
    }
}

/// Offset BIP341's NUMS point by a fresh per-proof scalar.
pub fn nums_key(offset: &SecretKey) -> Result<PublicKey, Error> {
    let nums = parse_secret("0250929b74c1a04954b78b4b6035e97a5e078a5a0f28ec96d547bfee9ace803ac0")?;
    Ok(nums.add_exp_tweak(&SECP256K1, &Scalar::from(*offset))?)
}

/// Derive a receiver's signing key for any of Nutroot's 256 NUT-28 slots.
pub fn receiver_key(
    receiver: &SecretKey,
    ephemeral: &PublicKey,
    slot: u8,
) -> Result<SecretKey, Error> {
    let shared = ephemeral.mul_tweak(&SECP256K1, &Scalar::from(*receiver))?;
    let mut message = b"Cashu_P2BK_v1".to_vec();
    message.extend_from_slice(&shared.x_only_public_key().0.serialize());
    message.push(slot);
    let first: [u8; 32] = Sha256::digest(&message).into();
    let scalar = match SecretKey::from_slice(&first) {
        Ok(key) => key,
        Err(_) => {
            message.push(0xff);
            SecretKey::from_slice(&Sha256::digest(&message))?
        }
    };
    Ok(receiver.add_tweak(&Scalar::from(scalar))?)
}
