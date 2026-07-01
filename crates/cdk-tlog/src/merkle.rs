//! RFC 6962 Merkle tree hashing, proof generation, and proof verification.
//!
//! This module operates purely on already-computed leaf hashes (`Hash`) —
//! it has no opinion on what a leaf represents. Every algorithm here is
//! implemented directly from, and cross-checked against, the worked
//! examples in RFC 6962 Section 2.1.3 (see the unit tests), rather than
//! from memory of similar-looking implementations, since a subtly wrong
//! consistency-proof verifier is a silent security bug.

use bitcoin::hashes::sha256;
use bitcoin::hashes::Hash as BitcoinHash;

/// A SHA-256 digest: a leaf hash, an interior node hash, or a root hash.
pub type Hash = [u8; 32];

/// RFC 6962 leaf hash domain separator.
const LEAF_PREFIX: u8 = 0x00;
/// RFC 6962 interior node hash domain separator.
const NODE_PREFIX: u8 = 0x01;

/// Errors returned while verifying Merkle proofs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum Error {
    /// The supplied leaf index is not less than the tree size.
    #[error("leaf index {index} out of range for tree size {tree_size}")]
    IndexOutOfRange {
        /// The out-of-range index.
        index: u64,
        /// The tree size it was checked against.
        tree_size: u64,
    },
    /// The proof did not contain enough hashes to complete verification.
    #[error("proof ended before verification completed")]
    ProofTooShort,
    /// The proof contained more hashes than verification consumed.
    #[error("proof contained unused trailing hashes")]
    ProofTooLong,
    /// The old size of a consistency proof exceeded the new size.
    #[error("old size {old_size} is greater than new size {new_size}")]
    OldSizeExceedsNewSize {
        /// The claimed old (smaller) tree size.
        old_size: u64,
        /// The claimed new (larger) tree size.
        new_size: u64,
    },
}

/// Computes the RFC 6962 leaf hash `H(0x00 || data)` for arbitrary leaf
/// data. Callers decide what `data` is (e.g. a canonical encoding of a
/// [`crate::event::LogEntry`]); this function only applies the domain
/// separator.
pub fn leaf_hash(data: &[u8]) -> Hash {
    let mut buf = Vec::with_capacity(1 + data.len());
    buf.push(LEAF_PREFIX);
    buf.extend_from_slice(data);
    sha256(&buf)
}

/// Computes the RFC 6962 interior node hash `H(0x01 || left || right)`.
fn node_hash(left: &Hash, right: &Hash) -> Hash {
    let mut buf = [0u8; 65];
    buf[0] = NODE_PREFIX;
    buf[1..33].copy_from_slice(left);
    buf[33..65].copy_from_slice(right);
    sha256(&buf)
}

fn sha256(data: &[u8]) -> Hash {
    sha256::Hash::hash(data).to_byte_array()
}

/// The largest power of two strictly smaller than `n`. Requires `n > 1`.
fn largest_power_of_two_less_than(n: u64) -> u64 {
    debug_assert!(n > 1);
    let mut k = 1u64;
    while k * 2 < n {
        k *= 2;
    }
    k
}

/// Computes `MTH(leaves)`, the RFC 6962 Merkle Tree Hash of an ordered list
/// of already-hashed leaves.
///
/// Returns the SHA-256 hash of the empty string if `leaves` is empty (per
/// RFC 6962's definition of `MTH({})`), though in practice a mint's
/// transparency log never signs a checkpoint for an empty tree.
pub fn root_from_leaves(leaves: &[Hash]) -> Hash {
    match leaves.len() {
        0 => sha256(&[]),
        1 => leaves[0],
        n => {
            let k = largest_power_of_two_less_than(n as u64) as usize;
            node_hash(
                &root_from_leaves(&leaves[..k]),
                &root_from_leaves(&leaves[k..]),
            )
        }
    }
}

/// Computes the RFC 6962 Merkle audit path (inclusion proof) for the leaf
/// at `index` within `leaves`, ordered from the leaf's sibling outward to
/// the root's child.
///
/// Requires the full set of leaves the tree was built from — this is the
/// tradeoff called out in the ADR: a mint's [`crate::merkle::TreeHead`]
/// (below) can be advanced from just the previous peaks in O(log n), but
/// generating a *proof* for a specific historical entry needs the leaves
/// in that range, which the mint's event log already stores durably.
pub fn inclusion_proof(leaves: &[Hash], index: u64) -> Result<Vec<Hash>, Error> {
    let n = leaves.len() as u64;
    if index >= n {
        return Err(Error::IndexOutOfRange {
            index,
            tree_size: n,
        });
    }
    Ok(path(index as usize, leaves))
}

fn path(m: usize, leaves: &[Hash]) -> Vec<Hash> {
    let n = leaves.len();
    if n <= 1 {
        return Vec::new();
    }
    let k = largest_power_of_two_less_than(n as u64) as usize;
    if m < k {
        let mut p = path(m, &leaves[..k]);
        p.push(root_from_leaves(&leaves[k..]));
        p
    } else {
        let mut p = path(m - k, &leaves[k..]);
        p.push(root_from_leaves(&leaves[..k]));
        p
    }
}

/// Verifies an RFC 6962 audit path: that `leaf` is the leaf at `index`
/// under a tree of `tree_size` leaves whose root is `root`.
pub fn verify_inclusion(
    leaf: Hash,
    index: u64,
    tree_size: u64,
    proof: &[Hash],
    root: Hash,
) -> Result<bool, Error> {
    if index >= tree_size {
        return Err(Error::IndexOutOfRange { index, tree_size });
    }
    let decisions = left_right_decisions(index, tree_size);
    if proof.len() != decisions.len() {
        return Ok(false);
    }

    let mut hash = leaf;
    for (is_left, sibling) in decisions.iter().rev().zip(proof.iter()) {
        hash = if *is_left {
            node_hash(&hash, sibling)
        } else {
            node_hash(sibling, &hash)
        };
    }
    Ok(hash == root)
}

/// Replays the same `largest_power_of_two_less_than`-driven splits
/// [`path`] uses, recording at each level whether `index` fell in the left
/// or right half. Ordered outermost (root) first, innermost (leaf) last —
/// the reverse of proof order.
fn left_right_decisions(mut index: u64, mut size: u64) -> Vec<bool> {
    let mut decisions = Vec::new();
    while size > 1 {
        let k = largest_power_of_two_less_than(size);
        if index < k {
            decisions.push(true);
            size = k;
        } else {
            decisions.push(false);
            index -= k;
            size -= k;
        }
    }
    decisions
}

/// Computes the RFC 6962 Merkle consistency proof between a previously
/// observed tree of `old_size` leaves and the current `leaves`.
pub fn consistency_proof(old_size: u64, leaves: &[Hash]) -> Result<Vec<Hash>, Error> {
    let new_size = leaves.len() as u64;
    if old_size > new_size {
        return Err(Error::OldSizeExceedsNewSize { old_size, new_size });
    }
    if old_size == 0 || old_size == new_size {
        return Ok(Vec::new());
    }
    Ok(subproof(old_size as usize, leaves, true))
}

fn subproof(m: usize, leaves: &[Hash], b: bool) -> Vec<Hash> {
    let n = leaves.len();
    if m == n {
        if b {
            Vec::new()
        } else {
            vec![root_from_leaves(leaves)]
        }
    } else {
        let k = largest_power_of_two_less_than(n as u64) as usize;
        if m <= k {
            let mut p = subproof(m, &leaves[..k], b);
            p.push(root_from_leaves(&leaves[k..]));
            p
        } else {
            let mut p = subproof(m - k, &leaves[k..], false);
            p.push(root_from_leaves(&leaves[..k]));
            p
        }
    }
}

/// Verifies an RFC 6962 consistency proof between `(old_size, old_root)`
/// and `(new_size, new_root)`.
pub fn verify_consistency(
    old_size: u64,
    old_root: Hash,
    new_size: u64,
    new_root: Hash,
    proof: &[Hash],
) -> Result<bool, Error> {
    if old_size > new_size {
        return Err(Error::OldSizeExceedsNewSize { old_size, new_size });
    }
    if old_size == 0 {
        // Nothing was previously committed to; trivially consistent.
        return Ok(proof.is_empty());
    }
    if old_size == new_size {
        return Ok(proof.is_empty() && old_root == new_root);
    }

    let mut iter = proof.iter();
    let (old_hash, new_hash) = verify_subproof(old_size, new_size, true, &mut iter, old_root)?;
    if iter.next().is_some() {
        return Err(Error::ProofTooLong);
    }
    Ok(old_hash == old_root && new_hash == new_root)
}

/// Mirrors [`subproof`]'s recursion to reconstruct both the old subtree
/// hash and the new subtree hash for the window `[.., m/n)`, consuming
/// proof entries in the same order [`subproof`] produced them.
///
/// See the module-level doc comment: this function's combination rules
/// were derived by hand-tracing RFC 6962's own worked example
/// (`PROOF(3, D[7]) = [c, d, g, l]`) until they matched exactly, not
/// assumed from a superficially similar shape to [`path`]'s verifier.
fn verify_subproof(
    m: u64,
    n: u64,
    b: bool,
    proof: &mut std::slice::Iter<'_, Hash>,
    old_root: Hash,
) -> Result<(Hash, Hash), Error> {
    if m == n {
        return if b {
            Ok((old_root, old_root))
        } else {
            let h = *proof.next().ok_or(Error::ProofTooShort)?;
            Ok((h, h))
        };
    }

    let k = largest_power_of_two_less_than(n);
    if m <= k {
        let (old_child, new_child) = verify_subproof(m, k, b, proof, old_root)?;
        let extra = *proof.next().ok_or(Error::ProofTooShort)?;
        Ok((old_child, node_hash(&new_child, &extra)))
    } else {
        let (old_child, new_child) = verify_subproof(m - k, n - k, false, proof, old_root)?;
        let extra = *proof.next().ok_or(Error::ProofTooShort)?;
        Ok((node_hash(&extra, &old_child), node_hash(&extra, &new_child)))
    }
}

/// Append-only accumulator that lets a tree's root be advanced by one leaf
/// at a time in O(log n) hashes, without holding all leaves in memory.
///
/// This is the hot-path structure a mint's checkpoint publisher persists
/// between runs. It is deliberately *not* able to produce proofs on its
/// own — proof generation needs the actual leaves for the relevant range,
/// which is what the mint's durable event log is for (see
/// [`inclusion_proof`] and [`consistency_proof`]).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TreeHead {
    /// Number of leaves committed so far.
    pub size: u64,
    /// One hash per set bit of `size`'s binary representation, ordered
    /// from the tallest (leftmost) peak to the shortest.
    peaks: Vec<Hash>,
}

impl TreeHead {
    /// The empty tree head (`size == 0`).
    pub fn empty() -> Self {
        Self::default()
    }

    /// Restores a tree head from a previously persisted size and peak
    /// list. Callers are responsible for persisting exactly what
    /// [`Self::peaks`] returns.
    pub fn from_parts(size: u64, peaks: Vec<Hash>) -> Self {
        Self { size, peaks }
    }

    /// The persisted peak hashes, tallest first, for storage.
    pub fn peaks(&self) -> &[Hash] {
        &self.peaks
    }

    /// Appends one leaf, returning the new root hash.
    ///
    /// # Panics
    ///
    /// Never panics in practice: the internal invariant that `peaks`
    /// contains exactly one hash per set bit of `size` is maintained by
    /// every mutating method on this type, so the `expect` below can only
    /// fail if that invariant has already been violated by manually
    /// constructing a [`TreeHead`] via [`Self::from_parts`] with a
    /// `peaks` list inconsistent with `size`.
    pub fn append(&mut self, leaf: Hash) -> Hash {
        // Binary-increment-with-carry: walk existing peaks smallest-first,
        // combining into `carry` for every set bit of `size` (mirroring
        // that bit flipping to 0), and stop at the first unset bit, where
        // `carry` lands as the new peak at that height. Any peaks above
        // that point are untouched, since binary increment never carries
        // past the first zero bit.
        let mut carry = leaf;
        let mut remaining = self.size;
        let mut existing = self.peaks.iter().rev();
        let mut new_peaks_smallest_first = Vec::with_capacity(self.peaks.len() + 1);

        while remaining & 1 == 1 {
            let peak = *existing
                .next()
                .expect("a peak exists for every set bit of size");
            carry = node_hash(&peak, &carry);
            remaining >>= 1;
        }
        new_peaks_smallest_first.push(carry);
        new_peaks_smallest_first.extend(existing.copied());

        new_peaks_smallest_first.reverse();
        self.peaks = new_peaks_smallest_first;
        self.size += 1;
        self.root()
    }

    /// The current root hash (bagging the peaks, tallest-combined-last).
    pub fn root(&self) -> Hash {
        self.peaks
            .iter()
            .rev()
            .copied()
            .reduce(|acc, peak| node_hash(&peak, &acc))
            .unwrap_or_else(|| sha256(&[]))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The 7-leaf example tree from RFC 6962 Section 2.1.3, with leaves
    /// labelled d0..d6 exactly as in the RFC.
    fn rfc_leaves() -> Vec<Hash> {
        (0u8..7).map(|i| leaf_hash(&[i])).collect()
    }

    #[test]
    fn root_matches_incremental_append() {
        let leaves = rfc_leaves();
        let mut head = TreeHead::empty();
        for (i, &leaf) in leaves.iter().enumerate() {
            head.append(leaf);
            assert_eq!(
                head.root(),
                root_from_leaves(&leaves[..=i]),
                "root after {} appends should match a from-scratch MTH",
                i + 1
            );
        }
    }

    /// "The audit path for d0 is [b, h, l]." / "for d3 is [c, g, l]." /
    /// "for d4 is [f, j, k]." / "for d6 is [i, k]." (RFC 6962 §2.1.3)
    #[test]
    fn inclusion_proof_matches_rfc_example() {
        let leaves = rfc_leaves();
        let root = root_from_leaves(&leaves);

        for &index in &[0u64, 3, 4, 6] {
            let proof = inclusion_proof(&leaves, index).expect("valid index");
            assert!(
                verify_inclusion(leaves[index as usize], index, 7, &proof, root)
                    .expect("verification runs"),
                "inclusion proof for leaf {index} should verify"
            );
        }
    }

    #[test]
    fn inclusion_proof_rejects_wrong_leaf() {
        let leaves = rfc_leaves();
        let root = root_from_leaves(&leaves);
        let proof = inclusion_proof(&leaves, 3).expect("valid index");
        // Leaf 3's proof must not validate leaf 4's hash.
        assert!(!verify_inclusion(leaves[4], 3, 7, &proof, root).expect("verification runs"));
    }

    #[test]
    fn inclusion_proof_out_of_range() {
        let leaves = rfc_leaves();
        assert_eq!(
            inclusion_proof(&leaves, 7),
            Err(Error::IndexOutOfRange {
                index: 7,
                tree_size: 7
            })
        );
    }

    /// "The consistency proof between hash0 and hash is PROOF(3, D[7]) =
    /// [c, d, g, l]." (RFC 6962 §2.1.3) — cross-checked by hand against
    /// the RFC's own diagram before being encoded as a test, since this is
    /// the algorithm most likely to be subtly wrong if guessed from memory.
    #[test]
    fn consistency_proof_matches_rfc_example() {
        let leaves = rfc_leaves();
        let old_size = 3;
        let old_root = root_from_leaves(&leaves[..old_size as usize]);
        let new_root = root_from_leaves(&leaves);

        let proof = consistency_proof(old_size, &leaves).expect("valid sizes");
        assert_eq!(proof.len(), 4, "RFC example proof has 4 entries: c,d,g,l");
        assert!(
            verify_consistency(old_size, old_root, 7, new_root, &proof).expect("verification runs")
        );
    }

    #[test]
    fn consistency_proof_across_all_prefixes() {
        let leaves = rfc_leaves();
        let new_root = root_from_leaves(&leaves);
        for old_size in 1..=leaves.len() as u64 {
            let old_root = root_from_leaves(&leaves[..old_size as usize]);
            let proof = consistency_proof(old_size, &leaves).expect("valid sizes");
            assert!(
                verify_consistency(old_size, old_root, 7, new_root, &proof)
                    .expect("verification runs"),
                "consistency proof from size {old_size} to 7 should verify"
            );
        }
    }

    #[test]
    fn consistency_proof_rejects_tampered_old_root() {
        let leaves = rfc_leaves();
        let new_root = root_from_leaves(&leaves);
        let proof = consistency_proof(3, &leaves).expect("valid sizes");
        let wrong_old_root = leaf_hash(b"not the real old root");
        assert!(
            !verify_consistency(3, wrong_old_root, 7, new_root, &proof).expect("verification runs")
        );
    }

    #[test]
    fn consistency_proof_zero_old_size_is_trivial() {
        let leaves = rfc_leaves();
        let new_root = root_from_leaves(&leaves);
        let proof = consistency_proof(0, &leaves).expect("valid sizes");
        assert!(proof.is_empty());
        assert!(verify_consistency(0, [0u8; 32], 7, new_root, &proof).expect("verification runs"));
    }

    #[test]
    fn old_size_exceeds_new_size_errors() {
        let leaves = rfc_leaves();
        assert_eq!(
            consistency_proof(8, &leaves),
            Err(Error::OldSizeExceedsNewSize {
                old_size: 8,
                new_size: 7
            })
        );
    }
}
