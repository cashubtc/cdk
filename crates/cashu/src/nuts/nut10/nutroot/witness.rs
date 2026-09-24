use bitcoin::secp256k1::schnorr::Signature;
use bitcoin::secp256k1::{Keypair, Message, PublicKey, SecretKey};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::{branch, parse_secret, tweaked_key, Condition, Error, Leaf, Tree};
use crate::util::hex;

/// Script-path commitment opening.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ControlBlock {
    /// Compressed internal public key in hex.
    #[serde(rename = "K")]
    pub internal_key: String,
    /// Sibling hashes from the leaf to the root, in hex.
    pub path: Vec<String>,
}

/// Nutroot witness, represented as an object inside the wire witness string.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Witness {
    /// BIP340 signatures over this input's digest.
    pub signatures: Vec<String>,
    /// Serialized leaf in hex; its presence selects the script path.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub leaf: Option<String>,
    /// Script-path commitment opening.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub control: Option<ControlBlock>,
    /// Hashlock preimage in hex.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preimage: Option<String>,
}

impl Witness {
    /// Sign a key-path input with a bare or already-tweaked private key.
    pub fn key_path(key: &SecretKey, input_digest: [u8; 32]) -> Self {
        let keypair = Keypair::from_secret_key(&crate::SECP256K1, key);
        let signature =
            crate::SECP256K1.sign_schnorr(&Message::from_digest(input_digest), &keypair);
        Self {
            signatures: vec![signature.to_string()],
            leaf: None,
            control: None,
            preimage: None,
        }
    }

    /// Open a tree leaf for collecting script-path signatures.
    pub fn script_path(tree: &Tree, index: usize, internal_key: PublicKey) -> Result<Self, Error> {
        let leaf = tree.leaves().get(index).ok_or(Error::InvalidTree)?;
        if matches!(leaf.condition(), Condition::Commit(_)) {
            return Err(Error::InvalidWitness);
        }
        Ok(Self {
            signatures: vec![],
            leaf: Some(hex::encode(leaf.to_bytes())),
            control: Some(ControlBlock {
                internal_key: internal_key.to_string(),
                path: tree.path(index)?.into_iter().map(hex::encode).collect(),
            }),
            preimage: None,
        })
    }

    /// Verify against an input digest and the verifier's Unix clock.
    /// Returns whether the exercised path requires public disclosure.
    pub fn verify(&self, secret: &str, input_digest: [u8; 32], now: u64) -> Result<bool, Error> {
        let secret = parse_secret(secret)?;
        let message = Message::from_digest(input_digest);
        // Every valid leaf has at most 15 keys. Bound work before parsing signatures.
        if self.signatures.len() > 15 {
            return Err(Error::InvalidWitness);
        }
        let signatures = self
            .signatures
            .iter()
            .map(|sig| {
                if sig.len() != 128 {
                    return Err(Error::InvalidWitness);
                }
                Signature::from_slice(&hex::decode(sig)?).map_err(|_| Error::InvalidWitness)
            })
            .collect::<Result<Vec<_>, _>>()?;
        let Some(leaf) = &self.leaf else {
            if signatures.len() != 1 || self.control.is_some() || self.preimage.is_some() {
                return Err(Error::InvalidWitness);
            }
            verify_signature(&signatures[0], &message, &secret)?;
            return Ok(false);
        };
        let control = self.control.as_ref().ok_or(Error::InvalidWitness)?;
        if control.path.len() > 3 || leaf.len() > 1026 {
            return Err(Error::InvalidWitness);
        }
        let leaf = Leaf::from_bytes(&hex::decode(leaf)?)?;
        let mut root = leaf.hash();
        for sibling in &control.path {
            if sibling.len() != 64 {
                return Err(Error::InvalidWitness);
            }
            let sibling: [u8; 32] = hex::decode(sibling)?
                .try_into()
                .map_err(|_| Error::InvalidWitness)?;
            root = branch(root, sibling);
        }
        if tweaked_key(parse_secret(&control.internal_key)?, Some(root))? != secret
            || signatures.len() > leaf.keys().len()
        {
            return Err(Error::InvalidWitness);
        }
        match leaf.condition() {
            Condition::Commit(_) => return Err(Error::InvalidWitness),
            Condition::After(time) if now < *time => return Err(Error::InvalidWitness),
            Condition::Hashlock(hash) => {
                let preimage = self.preimage.as_ref().ok_or(Error::InvalidWitness)?;
                if preimage.len() > 64 {
                    return Err(Error::InvalidWitness);
                }
                let actual: [u8; 32] = Sha256::digest(hex::decode(preimage)?).into();
                if actual != *hash {
                    return Err(Error::InvalidWitness);
                }
            }
            _ => {}
        }
        if !matches!(leaf.condition(), Condition::Hashlock(_)) && self.preimage.is_some() {
            return Err(Error::InvalidWitness);
        }
        let satisfied = leaf
            .keys()
            .iter()
            .filter(|key| {
                signatures
                    .iter()
                    .any(|sig| verify_signature(sig, &message, key).is_ok())
            })
            .count();
        if satisfied < usize::from(leaf.threshold()) {
            return Err(Error::InvalidWitness);
        }
        Ok(leaf.disclosure())
    }
}

fn verify_signature(
    signature: &Signature,
    message: &Message,
    key: &PublicKey,
) -> Result<(), Error> {
    crate::SECP256K1
        .verify_schnorr(signature, message, &key.x_only_public_key().0)
        .map_err(|_| Error::InvalidWitness)
}
