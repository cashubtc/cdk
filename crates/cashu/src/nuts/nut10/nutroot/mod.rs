//! Nutroot secrets (NUT-10, v3 keysets)
//!
//! A v3 secret is a compressed secp256k1 point. A proof with spending
//! conditions commits them into the point, following BIP341's construction
//! with Cashu tags:
//!
//! ```text
//! secret = P = K + t*G,  t = tagged_hash("Cashu_NutrootTweak", K || merkle_root) mod n
//! ```
//!
//! `K` is a 33-byte compressed internal key (parity included, unlike
//! BIP341's x-only keys) and `merkle_root` commits to a tree of
//! [condition leaves](leaf). A secret spends by the key path, one BIP-340
//! signature by `P`'s scalar, or by the script path, which reveals one leaf
//! with its merkle path and satisfies it.

use thiserror::Error;

pub mod inputs;
pub mod internal_key;
pub mod leaf;
pub mod tree;
pub mod tweak;
pub mod verify;
pub mod witness;

pub use self::inputs::{
    sign_input_key_path, sign_proofs_key_path, verify_proof_input, verify_quote_input,
};
pub use self::internal_key::{nums_internal_key, NUMS_H};
pub use self::leaf::{Leaf, LeafBytes, LeafType, ParsedLeaf};
pub use self::tree::{merkle_path, merkle_root, MAX_LEAVES, MAX_PATH_LEN};
pub use self::tweak::{tweak, tweaked_key, tweaked_secret_key, Tweak};
pub use self::verify::{sign_digest, verify_witness};
pub use self::witness::{Control, NutrootWitness, RawNutrootWitness};

/// Nutroot error
#[derive(Debug, Error)]
pub enum Error {
    /// A serialized leaf is malformed
    #[error("Malformed nutroot leaf: {0}")]
    MalformedLeaf(&'static str),
    /// A leaf body exceeds 512 bytes
    #[error("Nutroot leaf body of {0} bytes exceeds 512")]
    LeafTooLong(usize),
    /// A leaf of an unknown version or type cannot be satisfied
    #[error("Unknown nutroot leaf version {version:#04x} type {leaf_type:#04x}")]
    UnknownLeaf {
        /// Leaf version
        version: u8,
        /// Leaf type
        leaf_type: u8,
    },
    /// A `commit` leaf was revealed in a witness
    #[error("A commit leaf cannot be spent")]
    CommitLeafRevealed,
    /// A tree has no leaves
    #[error("Nutroot tree has no leaves")]
    EmptyTree,
    /// A tree has more than 8 leaves
    #[error("Nutroot tree of {0} leaves exceeds 8")]
    TooManyLeaves(usize),
    /// A merkle path has more than 3 sibling hashes
    #[error("Nutroot merkle path of {0} hashes exceeds 3")]
    PathTooLong(usize),
    /// A leaf index is out of range
    #[error("No nutroot leaf at index {0}")]
    LeafIndex(usize),
    /// The tweaked key is the point at infinity, or the tweaked scalar zero
    #[error("Nutroot tweak produced an invalid key")]
    InvalidTweak,
    /// The witness does not commit to the secret
    #[error("Nutroot witness does not match the secret")]
    CommitmentMismatch,
    /// A witness is malformed
    #[error("Malformed nutroot witness: {0}")]
    MalformedWitness(&'static str),
    /// The leaf's locktime has not been reached
    #[error("Nutroot leaf locktime not reached")]
    LocktimeNotReached,
    /// The hashlock preimage is missing or wrong
    #[error("Nutroot hashlock preimage invalid")]
    InvalidPreimage,
    /// Too few distinct listed keys signed
    #[error("Nutroot signatures: {valid} valid of {required} required")]
    SignaturesNotMet {
        /// Distinct listed keys with a valid signature
        valid: usize,
        /// Signatures required
        required: usize,
    },
    /// The key-path signature is invalid
    #[error("Invalid nutroot key-path signature")]
    InvalidSignature,
    /// The input is not a v3 proof
    #[error("Not a v3 input")]
    NotV3,
    /// A v3 input carries no nutroot witness
    #[error("Missing nutroot witness")]
    MissingWitness,
    /// The transaction input does not match the proof or quote
    #[error("Transaction input {0} does not match")]
    InputMismatch(usize),
    /// Secret error
    #[error(transparent)]
    Secret(#[from] crate::secret::Error),
    /// Transaction error
    #[error(transparent)]
    Transaction(#[from] crate::nuts::nut00::transaction::Error),
    /// JSON error
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    /// TLV error
    #[error(transparent)]
    Tlv(#[from] crate::nuts::nut00::tlv::Error),
    /// secp256k1 error
    #[error(transparent)]
    Secp256k1(#[from] bitcoin::secp256k1::Error),
}
