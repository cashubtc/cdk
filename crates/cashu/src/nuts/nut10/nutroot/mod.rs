//! Nutroot primitives for version `02` keysets.
//!
//! Implements the draft in NUTs PR #443, revision
//! `9fdc29b104fd703402e98aab130303a685ef9b41`.
//!
//! CDK uses these primitives for version-02 transaction authorization, payment
//! policies, signing packages, receipts, and request-bound blind authentication.
//!
//! [`Transaction`] binds quote amounts supplied by its caller; mints must load
//! those amounts from their own quote state. [`SpendInfo::verify`] checks the
//! secret commitment, not ownership or a receiver's acceptance policy.

use bitcoin::secp256k1::{PublicKey, Scalar};
use sha2::{Digest, Sha256};
use thiserror::Error;

mod derivation;
mod leaf;
mod locking;
mod receipt;
pub(crate) mod serde_spend_info;
mod signing;
mod spend;
mod spend_info;
mod transaction;
mod tree;
mod witness;

#[cfg(test)]
mod tests;

pub use self::derivation::{derive_blinding_factor, derive_key, derive_quote_key, KeyPurpose};
pub use self::leaf::{Condition, Leaf};
pub use self::locking::{nums_point, NutrootOption};
pub use self::receipt::{ReceiptOpening, SpendReceipt};
pub use self::signing::{SigningInput, SigningKind, SigningPackage, SigningSpend};
pub use self::spend::{spend_commitment, SpendRecord};
pub use self::spend_info::{nums_key, receiver_key, sender_key, SpendInfo};
pub use self::transaction::{authorized_request_digest, Quote, Transaction};
pub use self::tree::Tree;
pub use self::witness::{ControlBlock, Witness};

/// Nutroot validation error.
#[derive(Debug, Error)]
pub enum Error {
    /// Noncanonical or unsupported leaf encoding.
    #[error("Invalid nutroot leaf")]
    InvalidLeaf,
    /// A tree must contain between one and eight leaves.
    #[error("Invalid nutroot tree")]
    InvalidTree,
    /// Secret must be a lowercase compressed secp256k1 point.
    #[error("Invalid nutroot secret")]
    InvalidSecret,
    /// Witness does not satisfy the committed condition.
    #[error("Invalid nutroot witness")]
    InvalidWitness,
    /// A transcript is empty, repeats an input, or exceeds a TLV length.
    #[error("Invalid nutroot transaction")]
    InvalidTransaction,
    /// Key derivation requires a version-02 keyset.
    #[error("Nutroot derivation requires a version-02 keyset")]
    InvalidKeyset,
    /// All retry counters were exhausted.
    #[error("Nutroot key derivation exhausted its retry counter")]
    DerivationExhausted,
    /// Spend information does not reconstruct the secret.
    #[error("Invalid nutroot spend information")]
    InvalidSpendInfo,
    /// Invalid secp256k1 operation.
    #[error(transparent)]
    Secp256k1(#[from] bitcoin::secp256k1::Error),
    /// Invalid hex encoding.
    #[error(transparent)]
    Hex(#[from] crate::util::hex::Error),
}

/// BIP340 tagged SHA-256 using a Cashu domain.
pub fn tagged_hash(tag: &str, message: &[u8]) -> [u8; 32] {
    let tag = Sha256::digest(tag.as_bytes());
    let mut hash = Sha256::new();
    hash.update(tag);
    hash.update(tag);
    hash.update(message);
    hash.finalize().into()
}

/// Parse the canonical wire encoding of a Nutroot secret.
pub fn parse_secret(secret: &str) -> Result<PublicKey, Error> {
    if secret.len() != 66
        || !secret
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        return Err(Error::InvalidSecret);
    }
    let bytes = crate::util::hex::decode(secret)?;
    PublicKey::from_slice(&bytes).map_err(|_| Error::InvalidSecret)
}

/// Compute the Nutroot tweak, reducing modulo the secp256k1 order.
pub fn tweak(internal_key: &PublicKey, root: Option<[u8; 32]>) -> Scalar {
    let mut message = internal_key.serialize().to_vec();
    if let Some(root) = root {
        message.extend_from_slice(&root);
    }
    reduce_scalar(tagged_hash("Cashu_NutrootTweak", &message))
}

fn reduce_scalar(mut bytes: [u8; 32]) -> Scalar {
    const ORDER: [u8; 32] = [
        0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff,
        0xfe, 0xba, 0xae, 0xdc, 0xe6, 0xaf, 0x48, 0xa0, 0x3b, 0xbf, 0xd2, 0x5e, 0x8c, 0xd0, 0x36,
        0x41, 0x41,
    ];
    if bytes >= ORDER {
        let mut borrow = 0u16;
        for i in (0..32).rev() {
            let sub = u16::from(ORDER[i]) + borrow;
            let value = u16::from(bytes[i]);
            bytes[i] = value.wrapping_sub(sub) as u8;
            borrow = u16::from(value < sub);
        }
    }
    Scalar::from_be_bytes(bytes).expect("digest reduced modulo secp256k1 order")
}

/// Commit an internal key to a tree root, or apply the empty tweak.
pub fn tweaked_key(internal_key: PublicKey, root: Option<[u8; 32]>) -> Result<PublicKey, Error> {
    Ok(internal_key.add_exp_tweak(&crate::SECP256K1, &tweak(&internal_key, root))?)
}

pub(super) fn branch(a: [u8; 32], b: [u8; 32]) -> [u8; 32] {
    let mut message = [0; 64];
    message[..32].copy_from_slice(&a.min(b));
    message[32..].copy_from_slice(&a.max(b));
    tagged_hash("Cashu_NutrootBranch", &message)
}
