//! BIP340 tagged hashes (NUT-00)

use bitcoin::hashes::sha256::Hash as Sha256Hash;
use bitcoin::hashes::{Hash, HashEngine};

/// `tagged_hash(tag, message) = SHA256(SHA256(tag) || SHA256(tag) || message)`
///
/// BIP340's construction over the tag's UTF-8 bytes.
pub fn tagged_hash(tag: &str, message: &[u8]) -> [u8; 32] {
    tagged_hash_parts(tag, &[message])
}

/// [`tagged_hash`] over the concatenation of `parts`
pub fn tagged_hash_parts(tag: &str, parts: &[&[u8]]) -> [u8; 32] {
    let tag_hash = Sha256Hash::hash(tag.as_bytes()).to_byte_array();
    let mut engine = Sha256Hash::engine();
    engine.input(&tag_hash);
    engine.input(&tag_hash);
    for part in parts {
        engine.input(part);
    }
    Sha256Hash::from_engine(engine).to_byte_array()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::util::hex;

    #[test]
    fn tag_hashes_match_the_spec() {
        // tests/00-tests.md and tests/10-tests.md
        for (tag, expected) in [
            (
                "Cashu_TransactionInput",
                "4996fee585f625e6a33865ce975efc32c42d2b1b95328385ce45f5209a16e9ec",
            ),
            (
                "Cashu_NutrootLeaf",
                "e19ba80c5d6798399efd68b1d3b0e7ad57e79a2777310a9e6334934ee7a0b52b",
            ),
            (
                "Cashu_NutrootBranch",
                "f54194fd19dabbcca1474f329fe5ec065fc54d94063d14ae820788ba5bd8e55e",
            ),
            (
                "Cashu_NutrootTweak",
                "cc14d6872e6d0bc79a3dadb4c43f9362916bb6df512126dda139e65b812facd1",
            ),
        ] {
            assert_eq!(
                hex::encode(Sha256Hash::hash(tag.as_bytes()).to_byte_array()),
                expected
            );
        }
    }

    #[test]
    fn parts_concatenate() {
        assert_eq!(
            tagged_hash_parts("TapLeaf", &[b"ab", b"c"]),
            tagged_hash("TapLeaf", b"abc")
        );
        // Leaf hash of tests/10 `threshold_1of1_key3`, exercising the full construction.
        let leaf = hex::decode(
            "00010200010104002102f9308a019258c31049344f85f89d5229b531c845836f99b08601f113bce036f9",
        )
        .unwrap();
        assert_eq!(
            hex::encode(tagged_hash("Cashu_NutrootLeaf", &leaf)),
            "23e8ff1693496ecad495b7ed3cdd7f8595c52a3adc0b92475835b0fb839116cb"
        );
    }
}
