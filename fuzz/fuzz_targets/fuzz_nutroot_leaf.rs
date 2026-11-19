#![no_main]

//! Nutroot leaves and trees (NUT-10, v3): parsing fails closed and a parsed
//! core leaf re-serializes to its exact bytes; any tree of 1 to 8 leaves
//! folds, and every leaf's path reaches the root.

use cashu::nuts::nut10::nutroot::tree::root_from_path;
use cashu::nuts::nut10::nutroot::{merkle_path, merkle_root, LeafBytes, ParsedLeaf};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let leaves: Vec<LeafBytes> = data
        .split(|byte| *byte == 0xfe)
        .take(9)
        .filter_map(|chunk| LeafBytes::from_bytes(chunk.to_vec()).ok())
        .collect();

    for leaf in &leaves {
        if let Ok(ParsedLeaf::Known(parsed)) = leaf.parse() {
            let bytes = parsed.to_leaf_bytes().expect("a parsed leaf re-serializes");
            assert_eq!(&bytes, leaf, "a parsed leaf keeps its exact bytes");
        }
    }

    if let Ok(root) = merkle_root(&leaves) {
        for (index, leaf) in leaves.iter().enumerate() {
            let path = merkle_path(&leaves, index).expect("path for a tree leaf");
            assert_eq!(
                root_from_path(&leaf.hash(), &path).expect("path fits"),
                root
            );
        }
    }
});
