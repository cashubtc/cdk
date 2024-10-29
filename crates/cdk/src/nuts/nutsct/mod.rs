//! NUT-SCT: Spending Condition Tree
//!  
//! https://github.com/cashubtc/nuts/blob/a86a4e8ce0b9a76ce9b242d6c2c2ab846b3e1955/sct.md

pub mod serde_sct_witness;

use bitcoin::hashes::sha256::Hash as Sha256Hash;
use bitcoin::hashes::Hash;
use serde::{Deserialize, Serialize};

use crate::secret::Secret;

use super::{Proof, Witness};

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
/// SCT Witness
pub struct SCTWitness {
    /// Leaf secret being proven
    leaf_secret: String,
    /// Merkle proof of the leaf secret
    merkle_proof: Vec<String>,
}

impl Proof {
    /// Add SCT witness to proof
    pub fn add_sct_witness(&mut self, leaf_secret: String, merkle_proof: Vec<String>) {
        self.witness = Some(Witness::SCTWitness(SCTWitness {
            leaf_secret,
            merkle_proof,
        }));
    }
}

/// see https://github.com/cashubtc/nuts/blob/a86a4e8ce0b9a76ce9b242d6c2c2ab846b3e1955/dlc.md#payout-structures
pub fn sorted_merkle_hash(left: &[u8], right: &[u8]) -> [u8; 32] {
    // sort the inputs
    let (left, right) = if left < right {
        (left, right)
    } else {
        (right, left)
    };

    // concatenate the inputs
    let mut to_hash = Vec::new();
    to_hash.extend_from_slice(left);
    to_hash.extend_from_slice(right);

    // hash the concatenated inputs
    Sha256Hash::hash(&to_hash).to_byte_array()
}

/// see https://github.com/cashubtc/nuts/blob/a86a4e8ce0b9a76ce9b242d6c2c2ab846b3e1955/sct.md#merkle_rootleaf_hashes-listbytes---bytes
pub fn merkle_root(leaf_hashes: &[[u8; 32]]) -> [u8; 32] {
    if leaf_hashes.is_empty() {
        return [0; 32];
    } else if leaf_hashes.len() == 1 {
        return leaf_hashes[0].to_owned();
    } else {
        let split = leaf_hashes.len() / 2; // TODO: will this round?
        let left = merkle_root(&leaf_hashes[..split]);
        let right = merkle_root(&leaf_hashes[split..]);
        sorted_merkle_hash(&left, &right)
    }
}

/// see https://github.com/cashubtc/nuts/blob/a86a4e8ce0b9a76ce9b242d6c2c2ab846b3e1955/sct.md#merkle_verifyroot-bytes-leaf_hash-bytes-proof-listbytes---bool
pub fn merkle_verify(root: &[u8; 32], leaf_hash: &[u8; 32], proof: &Vec<String>) -> bool {
    let mut current_hash = *leaf_hash;
    for branch_hash_hex in proof {
        let branch_hash = crate::util::hex::decode(branch_hash_hex).expect("Invalid hex string");
        current_hash = sorted_merkle_hash(&current_hash, &branch_hash);
    }

    current_hash == *root
}

/// see https://github.com/cashubtc/nuts/blob/a86a4e8ce0b9a76ce9b242d6c2c2ab846b3e1955/dlc.md#payout-structures
pub fn merkle_prove(leaf_hashes: Vec<[u8; 32]>, position: usize) -> Vec<[u8; 32]> {
    if leaf_hashes.len() <= 1 {
        return Vec::new();
    }
    let split = leaf_hashes.len() / 2;

    if position < split {
        let mut proof = merkle_prove(leaf_hashes[..split].to_vec(), position);
        proof.push(merkle_root(&leaf_hashes[split..]));
        return proof;
    } else {
        let mut proof = merkle_prove(leaf_hashes[split..].to_vec(), position - split);
        proof.push(merkle_root(&leaf_hashes[..split]));
        return proof;
    }
}

/// Merkle root of SCT
pub fn sct_root(secrets: Vec<Secret>) -> [u8; 32] {
    let leaf_hashes: Vec<[u8; 32]> = secrets
        .iter()
        .map(|s| Sha256Hash::hash(&s.to_bytes()).to_byte_array())
        .collect();

    merkle_root(&leaf_hashes)
}

/// Hashes of SCT leaves
pub fn sct_leaf_hashes(secrets: Vec<Secret>) -> Vec<[u8; 32]> {
    secrets
        .iter()
        .map(|s| Sha256Hash::hash(&s.as_bytes()).to_byte_array())
        .collect()
}

#[cfg(test)]
mod tests {
    use std::str::FromStr;

    use crate::util::hex;

    use super::*;

    //https://github.com/cashubtc/nuts/blob/a86a4e8ce0b9a76ce9b242d6c2c2ab846b3e1955/tests/sct-tests.md.md
    #[test]
    fn test_secret_hash() {
        let s = "[\"P2PK\",{\"nonce\":\"ffd73b9125cc07cdbf2a750222e601200452316bf9a2365a071dd38322a098f0\",\"data\":\"028fab76e686161cc6daf78fea08ba29ce8895e34d20322796f35fec8e689854aa\",\"tags\":[[\"sigflag\",\"SIG_INPUTS\"]]}]";
        let secret = Secret::from_str(s).unwrap();
        println!("{:?}", secret.as_bytes());

        let hasher = Sha256Hash::hash(secret.as_bytes()).to_byte_array();

        let expected_hash: [u8; 32] =
            hex::decode("b43b79ed408d4cc0aa75ad0a97ab21e357ff7ee027300fb573833c568431e808")
                .unwrap()
                .try_into()
                .unwrap();

        assert_eq!(hasher, expected_hash)
    }

    #[test]
    fn test_sct_root() {
        let s1: [u8; 32] =
            hex::decode("b43b79ed408d4cc0aa75ad0a97ab21e357ff7ee027300fb573833c568431e808")
                .unwrap()
                .try_into()
                .unwrap();
        let s2: [u8; 32] =
            hex::decode("6bad0d7d596cb9048754ee75daf13ee7e204c6e408b83ee67514369e3f8f3f96")
                .unwrap()
                .try_into()
                .unwrap();
        let s3: [u8; 32] =
            hex::decode("8da10ed117cad5e89c6131198ffe271166d68dff9ce961ff117bd84297133b77")
                .unwrap()
                .try_into()
                .unwrap();
        let s4: [u8; 32] =
            hex::decode("7ec5a236d308d2c2bf800d81d3e3df89cc98f4f937d0788c302d2754ba28166a")
                .unwrap()
                .try_into()
                .unwrap();
        let s5: [u8; 32] =
            hex::decode("e19353a94d1aaf56b150b1399b33cd4ef4096b086665945fbe96bd72c22097a7")
                .unwrap()
                .try_into()
                .unwrap();
        let s6: [u8; 32] =
            hex::decode("cc655b7103c8b999b3fc292484bcb5a526e2d0cbf951f17fd7670fc05b1ff947")
                .unwrap()
                .try_into()
                .unwrap();
        let s7: [u8; 32] =
            hex::decode("009ea9fae527f7914096da1f1ce2480d2e4cfea62480afb88da9219f1c09767f")
                .unwrap()
                .try_into()
                .unwrap();

        let leaf_hashes = &[s1, s2, s3, s4, s5, s6, s7];

        let root = merkle_root(leaf_hashes);

        let expected_root: [u8; 32] =
            hex::decode("71655cac0c83c6949169bcd6c82b309810138895f83b967089ffd9f64d109306")
                .unwrap()
                .try_into()
                .unwrap();

        assert_eq!(root, expected_root);
    }

    #[test]
    fn test_basic_merkle_proof() {
        // Test merkle proof for tree with two nodes.  Proof should be other hash.
        let hash1: [u8; 32] = [9; 32];
        let hash2: [u8; 32] = [8; 32];
        let leaf_hashes = vec![hash1, hash2];

        let position = 0;
        let proof = merkle_prove(leaf_hashes.clone(), position);
        let expected_proof = vec![hash2];
        assert_eq!(proof, expected_proof);

        let position = 1;
        let proof = merkle_prove(leaf_hashes.clone(), position);
        let expected_proof = vec![hash1];
        assert_eq!(proof, expected_proof);

        let proof = proof
            .iter()
            .map(|h| hex::encode(h))
            .collect::<Vec<String>>();

        let root = merkle_root(&leaf_hashes);

        let valid = merkle_verify(&root, &leaf_hashes[1], &proof);
        assert!(valid);
    }

    #[test]
    fn test_complex_merkle_proof() {
        let s1: [u8; 32] =
            hex::decode("b43b79ed408d4cc0aa75ad0a97ab21e357ff7ee027300fb573833c568431e808")
                .unwrap()
                .try_into()
                .unwrap();
        let s2: [u8; 32] =
            hex::decode("6bad0d7d596cb9048754ee75daf13ee7e204c6e408b83ee67514369e3f8f3f96")
                .unwrap()
                .try_into()
                .unwrap();
        let s3: [u8; 32] =
            hex::decode("8da10ed117cad5e89c6131198ffe271166d68dff9ce961ff117bd84297133b77")
                .unwrap()
                .try_into()
                .unwrap();
        let s4: [u8; 32] =
            hex::decode("7ec5a236d308d2c2bf800d81d3e3df89cc98f4f937d0788c302d2754ba28166a")
                .unwrap()
                .try_into()
                .unwrap();
        let s5: [u8; 32] =
            hex::decode("e19353a94d1aaf56b150b1399b33cd4ef4096b086665945fbe96bd72c22097a7")
                .unwrap()
                .try_into()
                .unwrap();
        let s6: [u8; 32] =
            hex::decode("cc655b7103c8b999b3fc292484bcb5a526e2d0cbf951f17fd7670fc05b1ff947")
                .unwrap()
                .try_into()
                .unwrap();
        let s7: [u8; 32] =
            hex::decode("009ea9fae527f7914096da1f1ce2480d2e4cfea62480afb88da9219f1c09767f")
                .unwrap()
                .try_into()
                .unwrap();

        let s8: [u8; 32] =
            hex::decode("7a56977edf9c299c1cfb14dfbeb2ab28d7b3d44b3c9cc6b7854f8a58acb3407d")
                .unwrap()
                .try_into()
                .unwrap();
        let s9: [u8; 32] =
            hex::decode("7de4c7c75c8082ed9a2124ce8f027ed9a60f2236b6f50c62748a220086ed367b")
                .unwrap()
                .try_into()
                .unwrap();

        let s10: [u8; 32] =
            hex::decode("b43b79ed408d4cc0aa75ad0a97ab21e357ff7ee027300fb573833c568431e808")
                .unwrap()
                .try_into()
                .unwrap();
        let s11: [u8; 32] =
            hex::decode("7de4c7c75c8082ed9a2124ce8f027ed9a60f2236b6f50c62748a220086ed367b")
                .unwrap()
                .try_into()
                .unwrap();

        let leaf_hashes = &[s1, s2, s3, s4, s5, s6, s7];

        let position = 0;
        let proofs = merkle_prove(leaf_hashes.to_vec(), position);
        let expected_proofs = [s8, s9].to_vec();
        assert_eq!(proofs, expected_proofs);

        let position = 1;
        let expected_proofs = [s3, s10, s11];
        let proofs = merkle_prove(leaf_hashes.to_vec(), position);
        assert_eq!(proofs, expected_proofs);

        let position = 2;
        let expected_proofs = [s2, s10, s11];
        let proofs = merkle_prove(leaf_hashes.to_vec(), position);
        assert_eq!(proofs, expected_proofs);
        assert_eq!(proofs, expected_proofs);

        assert_eq!(proofs, expected_proofs);
    }

    //https://github.com/cashubtc/nuts/blob/a86a4e8ce0b9a76ce9b242d6c2c2ab846b3e1955/tests/sct-tests.md.md#proofs
    #[test]
    //test vector from docs
    fn test_valid_sct() {
        let s = "9becd3a8ce24b53beaf8ffb20a497b683b55f87ef87e3814be43a5768bcfe69fj";

        let s1 = String::from("009ea9fae527f7914096da1f1ce2480d2e4cfea62480afb88da9219f1c09767f");
        let s2 = String::from("2628c9759f0cecbb43b297b6eb0c268573d265730c2c9f6e194b4948f43d669d");
        let s3 = String::from("7ea48b9a4ad58f92c4cfa8e006afa98b2b05ac1b4de481e13088d26f672d8edc");

        let merkle_proof = vec![s1, s2, s3];

        let root: [u8; 32] =
            hex::decode("71655cac0c83c6949169bcd6c82b309810138895f83b967089ffd9f64d109306")
                .unwrap()
                .try_into()
                .unwrap();

        let leaf_hash = Sha256Hash::hash(s.as_bytes()).to_byte_array();

        let b = merkle_verify(&root, &leaf_hash, &merkle_proof);
        println!("{b}");

        assert!(b);
    }

    #[test]
    //test from SCT our program created
    fn test_our_valid_sct() {
        let s = "[\"DLC\",{\"nonce\":\"aea22dd7c80f0fc87b3ab66b7c910d21d5f27d63f0f0f8164e3dbceed25c7447\",\"data\":\"2c5da07a0542ef3731e254c006d1ecfea7cd951c11cea1c065a12c39e3b1f1a2\",\"tags\":[[\"sigflag\",\"SIG_INPUTS\"],[\"threshold\",\"1\"]]}]";

        let s1 = String::from("80ebc929bcb51d0ac6ed24d9f9bbb6897494c5bf8c4a4dadad6dca772a1d865a");

        let merkle_proof = vec![s1];

        let root: [u8; 32] =
            hex::decode("09682b8e375979e68189ff293cbe09038de1d67b5b5fa46961814dc8747d8a7b")
                .unwrap()
                .try_into()
                .unwrap();

        let leaf_hash = Sha256Hash::hash(s.as_bytes()).to_byte_array();

        let b = merkle_verify(&root, &leaf_hash, &merkle_proof);

        assert!(b);
    }

    //https://github.com/cashubtc/nuts/blob/a86a4e8ce0b9a76ce9b242d6c2c2ab846b3e1955/tests/sct-tests.md.md#invalid

    #[test]
    //test vector from docs
    fn test_invalid_sct() {
        let s = "9becd3a8ce24b53beaf8ffb20a497b683b55f87ef87e3814be43a5768bcfe69fj";

        let s1 = String::from("db7a191c4f3c112d7eb3ae9ee8fa9bd940fc4fed6ada9ba9ab2f102c3e3bbe80");
        let s2 = String::from("2628c9759f0cecbb43b297b6eb0c268573d265730c2c9f6e194b4948f43d669d");
        let s3 = String::from("7ea48b9a4ad58f92c4cfa8e006afa98b2b05ac1b4de481e13088d26f672d8edc");

        let merkle_proof = vec![s1, s2, s3];

        let root: [u8; 32] =
            hex::decode("71655cac0c83c6949169bcd6c82b309810138895f83b967089ffd9f64d109306")
                .unwrap()
                .try_into()
                .unwrap();

        let leaf_hash = Sha256Hash::hash(s.as_bytes()).to_byte_array();

        let b = merkle_verify(&root, &leaf_hash, &merkle_proof);

        assert_ne!(b, true);
    }

    #[test]
    //test from SCT our program created
    fn test_nutshell_info() {
        let s = "[\"DLC\",{\"nonce\":\"54d5263c9282f22c494b38f2967c23ac54de26502606f2a98b734b318c115250\",\"data\":\"d87010e7e82070c94c28b5e2aedff3275e452b93e6af1fd74e4f4d535e1e35a3\",\"tags\":[[\"sigflag\",\"SIG_INPUTS\"],[\"threshold\",\"1\"]]}]";

        let s1 = String::from("f737a46eaa37450285f9f9c7bafb653d9f6074614a9339a64a6307bd878b748c");

        let merkle_proof = vec![s1];

        let root: [u8; 32] =
            hex::decode("345af1eee507016d86d66d022bde5225ab3ac15a183fbb64d8780ef394b2fcc1")
                .unwrap()
                .try_into()
                .unwrap();

        let leaf_hash = Sha256Hash::hash(s.as_bytes()).to_byte_array();

        let b = merkle_verify(&root, &leaf_hash, &merkle_proof);

        assert!(b);
    }
}

/*
Proof we created to test

[


Proof { amount: Amount(1),

keyset_id: Id { version: Version00, id: [255, 212, 139, 143, 94, 207, 128] },

secret: Secret("[\"SCT\",{\"nonce\":\"bebc21ceaccd4aa59c5f19ee98373f88916dc79e204979f0aee043ce0943e05c\",\"data\":\"09682b8e375979e68189ff293cbe09038de1d67b5b5fa46961814dc8747d8a7b\"}]"),

c: PublicKey { inner: PublicKey(8a4fe273c7ddc7c25a0aeb52039cada076ae928ab04cbfbc1350d6702d7b2b05275ab6e3f3ad091057a2b7436931ad5802b82dced2b675a15025b09e9a878833) }, witness: Some(SCTWitness(SCTWitness { leaf_secret: "[\"DLC\",{\"nonce\":\"aea22dd7c80f0fc87b3ab66b7c910d21d5f27d63f0f0f8164e3dbceed25c7447\",\"data\":\"2c5da07a0542ef3731e254c006d1ecfea7cd951c11cea1c065a12c39e3b1f1a2\",\"tags\":[[\"sigflag\",\"SIG_INPUTS\"],[\"threshold\",\"1\"]]}]",

merkle_proof: ["80ebc929bcb51d0ac6ed24d9f9bbb6897494c5bf8c4a4dadad6dca772a1d865a"] })),

dleq: Some(ProofDleq { e: SecretKey { inner: SecretKey(#7564a3ed9461cbba) },

s: SecretKey { inner: SecretKey(#4c853763f86d0058) },

r: SecretKey { inner: SecretKey(#edb5053fca96a7f9) } }) },






Proof {

amount: Amount(4),


keyset_id: Id { version: Version00, id: [255, 212, 139, 143, 94, 207, 128] },

secret: Secret("[\"SCT\",{\"nonce\":\"d58333d05c1b0d6cd86c93a4b0aa54ba44488fee915439f861befd53bcdc5d6d\",\"data\":\"09682b8e375979e68189ff293cbe09038de1d67b5b5fa46961814dc8747d8a7b\"}]"),

c: PublicKey { inner: PublicKey(21de97e2fbc742501fc20d79fa900a733c74f38f6298f2f78b8d71bf337d7d7042e99162fac27506ef040a10e9a9b76578f807d5a13c0090e5b0ced0483e1b8d) }, witness: Some(SCTWitness(SCTWitness { leaf_secret: "[\"DLC\",{\"nonce\":\"aea22dd7c80f0fc87b3ab66b7c910d21d5f27d63f0f0f8164e3dbceed25c7447\",\"data\":\"2c5da07a0542ef3731e254c006d1ecfea7cd951c11cea1c065a12c39e3b1f1a2\",\"tags\":[[\"sigflag\",\"SIG_INPUTS\"],[\"threshold\",\"1\"]]}]",

merkle_proof: ["80ebc929bcb51d0ac6ed24d9f9bbb6897494c5bf8c4a4dadad6dca772a1d865a"] })),

dleq: Some(ProofDleq { e: SecretKey { inner: SecretKey(#1aa35cc207c967ae) },

s: SecretKey { inner: SecretKey(#1426c306e96c16a8) },

r: SecretKey { inner: SecretKey(#ed2a1fc21398d714) } }) }


]



*/
