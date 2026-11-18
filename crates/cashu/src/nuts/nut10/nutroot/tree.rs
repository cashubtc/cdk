//! The leaf tree
//!
//! ```text
//! leaf_hash = tagged_hash("Cashu_NutrootLeaf", serialized_leaf)
//! branch    = tagged_hash("Cashu_NutrootBranch", min(h1, h2) || max(h1, h2))
//! ```
//!
//! The root of one leaf is its leaf hash. For more, the leaf hashes are
//! sorted ascending once, then folded pairwise per level, an unpaired last
//! hash promoted unchanged, until one remains. Duplicates are kept.
//!
//! NUT-10 does not say whether later levels are re-sorted; the readings
//! differ from five leaves up. This follows cashu-ts and sorts once.

use super::leaf::LeafBytes;
use super::Error;
use crate::util::tagged_hash::{tagged_hash, tagged_hash_parts};

/// Tag of a leaf hash
pub const LEAF_TAG: &str = "Cashu_NutrootLeaf";
/// Tag of a branch hash
pub const BRANCH_TAG: &str = "Cashu_NutrootBranch";
/// Most leaves a tree may hold
pub const MAX_LEAVES: usize = 8;
/// Most sibling hashes a merkle path may hold
pub const MAX_PATH_LEN: usize = 3;

/// A 32-byte node hash
pub type NodeHash = [u8; 32];

/// `tagged_hash("Cashu_NutrootLeaf", leaf)`
pub fn leaf_hash(leaf: &[u8]) -> NodeHash {
    tagged_hash(LEAF_TAG, leaf)
}

/// `tagged_hash("Cashu_NutrootBranch", min(a, b) || max(a, b))`
pub fn branch_hash(a: &NodeHash, b: &NodeHash) -> NodeHash {
    let (lo, hi) = if a <= b { (a, b) } else { (b, a) };
    tagged_hash_parts(BRANCH_TAG, &[lo, hi])
}

fn check_count(count: usize) -> Result<(), Error> {
    match count {
        0 => Err(Error::EmptyTree),
        n if n > MAX_LEAVES => Err(Error::TooManyLeaves(n)),
        _ => Ok(()),
    }
}

/// Leaf hashes sorted ascending, each with its index in `leaves`
fn sorted_hashes(leaves: &[LeafBytes]) -> Vec<(NodeHash, usize)> {
    let mut hashes: Vec<(NodeHash, usize)> = leaves
        .iter()
        .enumerate()
        .map(|(index, leaf)| (leaf.hash(), index))
        .collect();
    hashes.sort();
    hashes
}

fn fold_level(level: &[NodeHash]) -> Vec<NodeHash> {
    let mut next = Vec::with_capacity(level.len().div_ceil(2));
    let mut hashes = level.iter();
    while let Some(a) = hashes.next() {
        next.push(match hashes.next() {
            Some(b) => branch_hash(a, b),
            None => *a,
        });
    }
    next
}

/// The merkle root of a tree of 1 to 8 leaves, in any order
pub fn merkle_root(leaves: &[LeafBytes]) -> Result<NodeHash, Error> {
    check_count(leaves.len())?;
    let mut level: Vec<NodeHash> = sorted_hashes(leaves).into_iter().map(|(h, _)| h).collect();
    while level.len() > 1 {
        level = fold_level(&level);
    }
    Ok(level[0])
}

/// The sibling hashes from `leaves[index]` to the root
pub fn merkle_path(leaves: &[LeafBytes], index: usize) -> Result<Vec<NodeHash>, Error> {
    check_count(leaves.len())?;
    let sorted = sorted_hashes(leaves);
    let mut position = sorted
        .iter()
        .position(|(_, i)| *i == index)
        .ok_or(Error::LeafIndex(index))?;
    let mut level: Vec<NodeHash> = sorted.into_iter().map(|(h, _)| h).collect();
    let mut path = Vec::new();
    while level.len() > 1 {
        if let Some(sibling) = level.get(position ^ 1) {
            path.push(*sibling);
        }
        level = fold_level(&level);
        position /= 2;
    }
    Ok(path)
}

/// Recompute the root from a leaf hash and its merkle path
pub fn root_from_path(leaf_hash: &NodeHash, path: &[NodeHash]) -> Result<NodeHash, Error> {
    if path.len() > MAX_PATH_LEN {
        return Err(Error::PathTooLong(path.len()));
    }
    Ok(path
        .iter()
        .fold(*leaf_hash, |node, sibling| branch_hash(&node, sibling)))
}

#[cfg(test)]
mod tests {
    use std::str::FromStr;

    use super::*;
    use crate::util::hex;

    const THRESHOLD_1OF1_KEY3: &str =
        "00010200010104002102f9308a019258c31049344f85f89d5229b531c845836f99b08601f113bce036f9";
    const AFTER_1OF1_KEY4: &str = "00020200010104002102e493dbf1c10d80f3581e4904930b1404cc6c13900ee0758474fa94abe8c4cd1306000468a3be80";
    const HASHLOCK_1OF1_KEY3: &str = "00030200010104002102f9308a019258c31049344f85f89d5229b531c845836f99b08601f113bce036f9080020a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1";

    fn leaf(hex: &str) -> LeafBytes {
        LeafBytes::from_str(hex).unwrap()
    }

    /// tests/10-tests.md "The tree fold": three leaves, odd hash promoted
    #[test]
    fn three_leaf_vector() {
        let leaves = [
            leaf(THRESHOLD_1OF1_KEY3),
            leaf(AFTER_1OF1_KEY4),
            leaf(HASHLOCK_1OF1_KEY3),
        ];
        let root = merkle_root(&leaves).unwrap();
        assert_eq!(
            hex::encode(root),
            "3d4fbecf46f5c716d7cebd48863f3c4ab89e675e3beb4b3a28f3bd13b49d43ad"
        );
        let path = merkle_path(&leaves, 2).unwrap();
        assert_eq!(
            path.iter().map(hex::encode).collect::<Vec<_>>(),
            vec![
                "23e8ff1693496ecad495b7ed3cdd7f8595c52a3adc0b92475835b0fb839116cb",
                "9ed9c0b8907f7af4fce51cbeac218907bbf80ba40f3342df2406bce30616589a",
            ]
        );
        // Every leaf's path reaches the root, in any transmitted order.
        for index in 0..leaves.len() {
            let path = merkle_path(&leaves, index).unwrap();
            assert_eq!(root_from_path(&leaves[index].hash(), &path).unwrap(), root);
        }
        let mut permuted = leaves.clone();
        permuted.reverse();
        assert_eq!(merkle_root(&permuted).unwrap(), root);
    }

    /// tests/10-tests.md: two copies of one leaf are not deduplicated
    #[test]
    fn duplicate_leaf_vector() {
        let leaves = [leaf(THRESHOLD_1OF1_KEY3), leaf(THRESHOLD_1OF1_KEY3)];
        let h = leaves[0].hash();
        assert_eq!(
            hex::encode(h),
            "23e8ff1693496ecad495b7ed3cdd7f8595c52a3adc0b92475835b0fb839116cb"
        );
        let root = merkle_root(&leaves).unwrap();
        assert_eq!(
            hex::encode(root),
            "1eaf291448e2f3c3a4fc00bfd591917bbb807e63af0fb905d054002bddd2cbc6"
        );
        assert_eq!(root, branch_hash(&h, &h));
        for index in 0..2 {
            assert_eq!(merkle_path(&leaves, index).unwrap(), vec![h]);
        }
    }

    #[test]
    fn single_leaf_root_is_leaf_hash() {
        let leaves = [leaf(AFTER_1OF1_KEY4)];
        assert_eq!(
            hex::encode(merkle_root(&leaves).unwrap()),
            "9ed9c0b8907f7af4fce51cbeac218907bbf80ba40f3342df2406bce30616589a"
        );
        assert!(merkle_path(&leaves, 0).unwrap().is_empty());
    }

    #[test]
    fn tree_limits() {
        assert!(matches!(merkle_root(&[]), Err(Error::EmptyTree)));
        let nine = vec![leaf(THRESHOLD_1OF1_KEY3); 9];
        assert!(matches!(merkle_root(&nine), Err(Error::TooManyLeaves(9))));
        let eight = vec![leaf(THRESHOLD_1OF1_KEY3); 8];
        for index in 0..8 {
            assert_eq!(merkle_path(&eight, index).unwrap().len(), MAX_PATH_LEN);
        }
        assert!(matches!(merkle_path(&eight, 8), Err(Error::LeafIndex(8))));
        assert!(matches!(
            root_from_path(&[0; 32], &[[0; 32]; 4]),
            Err(Error::PathTooLong(4))
        ));
    }
}
