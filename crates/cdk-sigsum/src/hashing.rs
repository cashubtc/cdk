//! Hashing and namespacing helpers shared by every part of the Sigsum
//! protocol (leaf hashing, tree head signing, rate-limit tokens).
//!
//! All namespace strings and the double-hash recommendation for `message`
//! come directly from section 2.1/2.2 of the log server protocol spec.

use bitcoin::hashes::{sha256, Hash as BitcoinHash};
use ed25519_dalek::{Signer, SigningKey};

use crate::types::TreeLeaf;

/// A SHA-256 digest, used throughout the Sigsum protocol for checksums,
/// root hashes, and key hashes.
pub type Hash = [u8; 32];

/// Domain-separation prefix for leaf signatures.
pub const NAMESPACE_TREE_LEAF: &[u8] = b"sigsum.org/v1/tree-leaf";
/// Domain-separation prefix for tree head signatures/serialization.
pub const NAMESPACE_TREE: &[u8] = b"sigsum.org/v1/tree/";
/// Domain-separation prefix for rate-limit submit tokens.
pub const NAMESPACE_SUBMIT_TOKEN: &[u8] = b"sigsum.org/v1/submit-token";
/// Domain-separation prefix for witness cosignatures.
pub const NAMESPACE_COSIGNATURE: &[u8] = b"cosignature/v1";
/// RFC 6962 leaf hash domain separator.
const RFC6962_LEAF_PREFIX: u8 = 0x00;

/// Computes `SHA256(data)`.
pub fn sha256(data: &[u8]) -> Hash {
    sha256::Hash::hash(data).to_byte_array()
}

/// Computes `H(H(data))`, the recommended way to derive a Sigsum
/// `message` from arbitrary application data. The log only ever sees the
/// outer hash (`checksum`), never `data` itself.
pub fn checksum_of(data: &[u8]) -> Hash {
    sha256(&sha256(data))
}

/// Computes the RFC 6962 leaf hash `H(0x00 || tree_leaf)` used to address
/// a leaf in `get-inclusion-proof` requests.
pub fn leaf_hash(leaf: &TreeLeaf) -> Hash {
    let mut buf = Vec::with_capacity(1 + 128);
    buf.push(RFC6962_LEAF_PREFIX);
    buf.extend_from_slice(&leaf.to_bytes());
    sha256(&buf)
}

/// Builds the 56-octet message that a submitter signs to produce
/// `TreeLeaf::signature`: the tree-leaf namespace, a NUL separator, and
/// the checksum.
pub fn tree_leaf_signing_bytes(checksum: &Hash) -> Vec<u8> {
    let mut buf = Vec::with_capacity(NAMESPACE_TREE_LEAF.len() + 1 + 32);
    buf.extend_from_slice(NAMESPACE_TREE_LEAF);
    buf.push(0);
    buf.extend_from_slice(checksum);
    buf
}

/// Signs `checksum` as a submitter would, producing a ready-to-submit
/// [`TreeLeaf`] (with `key_hash` derived from `signing_key`'s public key).
pub fn sign_leaf(signing_key: &SigningKey, checksum: Hash) -> TreeLeaf {
    let signing_bytes = tree_leaf_signing_bytes(&checksum);
    let signature = signing_key.sign(&signing_bytes).to_bytes();
    let key_hash = sha256(signing_key.verifying_key().as_bytes());
    TreeLeaf {
        checksum,
        signature,
        key_hash,
    }
}

/// Builds the 3-line serialization of a tree head that both the log and
/// witnesses sign directly (section 2.2.2).
pub fn tree_head_signing_bytes(log_key_hash: &Hash, size: u64, root_hash: &Hash) -> Vec<u8> {
    use base64_line::encode_line;

    let mut buf = Vec::new();
    buf.extend_from_slice(NAMESPACE_TREE);
    buf.extend_from_slice(hex::encode(log_key_hash).as_bytes());
    buf.push(b'\n');
    buf.extend_from_slice(size.to_string().as_bytes());
    buf.push(b'\n');
    encode_line(root_hash, &mut buf);
    buf
}

/// Builds the 5-line serialization a witness signs to cosign a tree head
/// (section 2.2.3).
pub fn cosignature_signing_bytes(
    timestamp: u64,
    log_key_hash: &Hash,
    size: u64,
    root_hash: &Hash,
) -> Vec<u8> {
    let mut buf = Vec::new();
    buf.extend_from_slice(NAMESPACE_COSIGNATURE);
    buf.push(b'\n');
    buf.extend_from_slice(format!("time {timestamp}").as_bytes());
    buf.push(b'\n');
    buf.extend_from_slice(&tree_head_signing_bytes(log_key_hash, size, root_hash));
    buf
}

/// Minimal single-purpose base64 line encoder, to avoid pulling in a
/// general-purpose base64 crate for one field.
mod base64_line {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

    /// Appends the base64 (with padding) encoding of `data` followed by a
    /// newline to `out`.
    pub fn encode_line(data: &[u8], out: &mut Vec<u8>) {
        let mut chunks = data.chunks_exact(3);
        for chunk in &mut chunks {
            let n = u32::from_be_bytes([0, chunk[0], chunk[1], chunk[2]]);
            out.push(TABLE[((n >> 18) & 0x3f) as usize]);
            out.push(TABLE[((n >> 12) & 0x3f) as usize]);
            out.push(TABLE[((n >> 6) & 0x3f) as usize]);
            out.push(TABLE[(n & 0x3f) as usize]);
        }
        let remainder = chunks.remainder();
        match remainder.len() {
            1 => {
                let n = u32::from_be_bytes([0, remainder[0], 0, 0]);
                out.push(TABLE[((n >> 18) & 0x3f) as usize]);
                out.push(TABLE[((n >> 12) & 0x3f) as usize]);
                out.push(b'=');
                out.push(b'=');
            }
            2 => {
                let n = u32::from_be_bytes([0, remainder[0], remainder[1], 0]);
                out.push(TABLE[((n >> 18) & 0x3f) as usize]);
                out.push(TABLE[((n >> 12) & 0x3f) as usize]);
                out.push(TABLE[((n >> 6) & 0x3f) as usize]);
                out.push(b'=');
            }
            _ => {}
        }
        out.push(b'\n');
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn leaf_hash_is_stable_for_same_input() {
        let leaf = TreeLeaf {
            checksum: [1u8; 32],
            signature: [2u8; 64],
            key_hash: [3u8; 32],
        };
        assert_eq!(leaf_hash(&leaf), leaf_hash(&leaf));
    }

    #[test]
    fn checksum_of_double_hashes() {
        let data = b"hello, sigsum";
        assert_eq!(checksum_of(data), sha256(&sha256(data)));
    }

    /// Reproduces the worked example from section 2.2.3 of the log server
    /// protocol spec byte-for-byte, to pin our hand-rolled base64 line
    /// encoder against the spec's own test vector rather than just our
    /// own round-trip.
    #[test]
    fn tree_head_signing_bytes_matches_spec_example() {
        let log_key_hash =
            hex_to_hash("3620c0d515f87e60959d29a4682fd1f0db984704981fda39b3e9ba0a44f57e2f");
        let root_hash =
            hex_to_hash("df525052af04c90c79969aad291aabc89cc0fbbed60f6c664f2b81e2e2255de1");

        let bytes = tree_head_signing_bytes(&log_key_hash, 15368405, &root_hash);
        let text = String::from_utf8(bytes).expect("ascii output");

        assert_eq!(
            text,
            "sigsum.org/v1/tree/3620c0d515f87e60959d29a4682fd1f0db984704981fda39b3e9ba0a44f57e2f\n\
             15368405\n\
             31JQUq8EyQx5lpqtKRqryJzA+77WD2xmTyuB4uIlXeE=\n"
        );
    }

    fn hex_to_hash(s: &str) -> Hash {
        hex::decode(s)
            .expect("valid hex in test vector")
            .try_into()
            .expect("32 bytes in test vector")
    }
}
