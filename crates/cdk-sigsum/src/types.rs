//! Wire types for the Sigsum log server protocol.
//!
//! See <https://git.glasklar.is/sigsum/project/documentation/-/blob/main/log.md>
//! section 2.2 for the serialization formats these types mirror.

use crate::hashing::Hash;

/// A single leaf in a Sigsum log's Merkle tree.
///
/// Serialized as exactly 128 bytes: `checksum (32) || signature (64) ||
/// key_hash (32)`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TreeLeaf {
    /// `H(message)`, where `message` is the data the submitter wants
    /// logged. The log rejects anything that isn't exactly 32 bytes.
    pub checksum: Hash,
    /// Ed25519 signature over the namespaced checksum, made by the
    /// submitter's key identified by `key_hash`.
    pub signature: [u8; 64],
    /// `H(submitter_public_key)`.
    pub key_hash: Hash,
}

impl TreeLeaf {
    /// Serializes the leaf to its 128-byte on-the-wire representation.
    pub fn to_bytes(&self) -> [u8; 128] {
        let mut out = [0u8; 128];
        out[0..32].copy_from_slice(&self.checksum);
        out[32..96].copy_from_slice(&self.signature);
        out[96..128].copy_from_slice(&self.key_hash);
        out
    }
}

/// A witness's cosignature over a tree head, as returned by
/// `get-tree-head`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Cosignature {
    /// `H(witness_public_key)`.
    pub witness_key_hash: Hash,
    /// Seconds since the Unix epoch, covered by the signature.
    pub timestamp: u64,
    /// Ed25519 signature over the cosignature serialization (see
    /// `hashing::cosignature_signing_bytes`).
    pub signature: [u8; 64],
}

/// A log's signed tree head, plus any witness cosignatures it has
/// collected, as returned by `get-tree-head`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SignedTreeHead {
    /// Number of leaves in the tree.
    pub size: u64,
    /// Root hash of the tree at `size`.
    pub root_hash: Hash,
    /// The log's own Ed25519 signature over the tree head.
    pub signature: [u8; 64],
    /// Zero or more witness cosignatures collected for this tree head.
    pub cosignatures: Vec<Cosignature>,
}

/// An inclusion proof for one leaf under a tree of a given size, as
/// returned by `get-inclusion-proof`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InclusionProof {
    /// Zero-based index of the leaf within the tree.
    pub leaf_index: u64,
    /// RFC 6962 audit path, closest sibling first.
    pub node_hashes: Vec<Hash>,
}

/// A consistency proof between two tree sizes of the same log, as returned
/// by `get-consistency-proof`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConsistencyProof {
    /// RFC 6962 consistency proof nodes.
    pub node_hashes: Vec<Hash>,
}
