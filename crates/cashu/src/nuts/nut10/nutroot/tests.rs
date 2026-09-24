//! Vectors from cashubtc/nuts PR #443 at 9fdc29b1, tests/10-tests.md.
use serde_json::Value;

use super::*;
use crate::util::hex;

fn vectors() -> Vec<Value> {
    serde_json::from_str(include_str!("test_vectors.json")).unwrap()
}

#[test]
fn framed_derivation_vectors() {
    let vectors: Vec<Value> =
        serde_json::from_str(include_str!("derivation_vectors.json")).unwrap();
    let seed = bytes(&vectors[8]["seed_hex"]);
    let id = vectors[8]["keyset_id"].as_str().unwrap().parse().unwrap();
    for vector in vectors[9].as_array().unwrap() {
        let counter = vector["counter"].as_u64().unwrap();
        let derived = derive_key(&seed, id, counter, KeyPurpose::Internal).unwrap();
        assert_eq!(derived.secret_bytes(), hash(&vector["secret_key"]));
        assert_eq!(
            PublicKey::from_secret_key(&crate::SECP256K1, &derived),
            key(&vector["secret"])
        );
        assert_eq!(
            derive_blinding_factor(&seed, id, counter)
                .unwrap()
                .to_bytes(),
            hash(&vector["blinding_factor"])
        );
        assert_eq!(
            derive_key(&seed, id, counter, KeyPurpose::NumsOffset)
                .unwrap()
                .secret_bytes(),
            hash(&vector["nums_offset"])
        );
    }
    for vector in vectors[10].as_array().unwrap() {
        let derived = derive_key(
            &seed,
            id,
            0,
            KeyPurpose::Leaf(vector["index"].as_u64().unwrap() as u32),
        )
        .unwrap();
        assert_eq!(derived.secret_bytes(), hash(&vector["privkey"]));
    }
    for vector in vectors[12].as_array().unwrap() {
        let derived = derive_quote_key(
            &seed,
            key(&vectors[11]["mint_pubkey"]),
            vector["counter"].as_u64().unwrap(),
        )
        .unwrap();
        assert_eq!(derived.secret_bytes(), hash(&vector["privkey"]));
    }
}

#[test]
fn spend_info_reconstruction_vectors() {
    let vectors = vectors();
    let receiver =
        bitcoin::secp256k1::SecretKey::from_slice(&[vec![0; 31], vec![3]].concat()).unwrap();
    for vector in &vectors[27..=31] {
        let info: SpendInfo = serde_json::from_value(vector["spend_info"].clone()).unwrap();
        let secret = vector["secret"].as_str().unwrap();
        info.verify(secret, Some(&receiver)).unwrap();
        if info.bearer_key.is_some() || (info.ephemeral_key.is_some() && info.nums_offset.is_none())
        {
            let signing = info.key_path_key(secret, Some(&receiver)).unwrap();
            assert_eq!(
                PublicKey::from_secret_key(&crate::SECP256K1, &signing),
                parse_secret(secret).unwrap()
            );
        }
        let mut corrupted = info.clone();
        corrupted.internal_key = Some(PublicKey::from_secret_key(&crate::SECP256K1, &receiver));
        assert!(corrupted.verify(secret, Some(&receiver)).is_err());
    }
}

#[test]
fn script_paths_require_distinct_keys_and_bind_compressed_secret() {
    use bitcoin::secp256k1::SecretKey;
    let one = SecretKey::from_slice(&[1; 32]).unwrap();
    let two = SecretKey::from_slice(&[2; 32]).unwrap();
    let keys = [one, two].map(|k| PublicKey::from_secret_key(&crate::SECP256K1, &k));
    let tree = Tree::new(vec![Leaf::new(
        2,
        keys.to_vec(),
        Condition::Threshold,
        false,
    )
    .unwrap()])
    .unwrap();
    let internal = keys[0];
    let secret = tweaked_key(internal, Some(tree.root())).unwrap();
    let digest = [42; 32];
    let mut witness = Witness::script_path(&tree, 0, internal).unwrap();
    // Two independently randomized signatures by the same key are one signer.
    witness
        .signatures
        .extend(Witness::key_path(&one, digest).signatures);
    witness
        .signatures
        .extend(Witness::key_path(&one, digest).signatures);
    assert!(witness.verify(&secret.to_string(), digest, 0).is_err());
    witness.signatures.pop();
    witness
        .signatures
        .extend(Witness::key_path(&two, digest).signatures);
    witness.verify(&secret.to_string(), digest, 0).unwrap();
    let negated = secret.negate(&crate::SECP256K1);
    assert!(witness.verify(&negated.to_string(), digest, 0).is_err());
    witness.control.as_mut().unwrap().path = vec![hex::encode([0; 32]); 4];
    assert!(witness.verify(&secret.to_string(), digest, 0).is_err());
}

#[test]
fn hashlock_and_commit_satisfaction() {
    use bitcoin::secp256k1::SecretKey;
    use sha2::{Digest, Sha256};
    let key = SecretKey::from_slice(&[1; 32]).unwrap();
    let internal = PublicKey::from_secret_key(&crate::SECP256K1, &key);
    let preimage = [42; 32];
    let hash = Sha256::digest(preimage).into();
    let tree = Tree::new(vec![
        Leaf::new(1, vec![internal], Condition::Hashlock(hash), false).unwrap(),
        Leaf::new(0, vec![], Condition::Commit(hash), false).unwrap(),
    ])
    .unwrap();
    let secret = tweaked_key(internal, Some(tree.root()))
        .unwrap()
        .to_string();
    let digest = [7; 32];
    let mut witness = Witness::script_path(&tree, 0, internal).unwrap();
    witness.signatures = Witness::key_path(&key, digest).signatures;
    assert!(witness.verify(&secret, digest, 0).is_err());
    witness.preimage = Some(hex::encode(preimage));
    witness.verify(&secret, digest, 0).unwrap();
    witness.preimage = Some(hex::encode([42; 33]));
    assert!(witness.verify(&secret, digest, 0).is_err());
    assert!(Witness::script_path(&tree, 1, internal).is_err());
    witness.leaf = Some(hex::encode(tree.leaves()[1].to_bytes()));
    witness.control.as_mut().unwrap().path =
        tree.path(1).unwrap().into_iter().map(hex::encode).collect();
    assert!(witness.verify(&secret, digest, 0).is_err());
}

fn bytes(value: &Value) -> Vec<u8> {
    hex::decode(value.as_str().unwrap()).unwrap()
}
fn hash(value: &Value) -> [u8; 32] {
    bytes(value).try_into().unwrap()
}
fn key(value: &Value) -> PublicKey {
    parse_secret(value.as_str().unwrap()).unwrap()
}
fn leaf(value: &Value) -> Leaf {
    Leaf::from_bytes(&bytes(value)).unwrap()
}

#[test]
fn canonical_leaves_and_rejections() {
    let vectors = vectors();
    for (name, value) in vectors[0].as_object().unwrap() {
        if name == "hashlock_hash" {
            continue;
        }
        assert_eq!(leaf(value).to_bytes(), bytes(value));
    }
    for index in [11, 12] {
        for value in vectors[index].as_object().unwrap().values() {
            assert!(Leaf::from_bytes(&bytes(value)).is_err());
        }
    }
    let good = bytes(&vectors[0]["threshold_1of1_key3"]);
    for len in 0..good.len() {
        assert!(Leaf::from_bytes(&good[..len]).is_err());
    }
    let mut unknown = good.clone();
    unknown[0] = 1;
    assert!(Leaf::from_bytes(&unknown).is_err());
    unknown[0] = 0;
    unknown[1] = 5;
    assert!(Leaf::from_bytes(&unknown).is_err());
    let k = leaf(&vectors[0]["threshold_1of1_key3"]).keys()[0];
    assert!(Leaf::new(
        1,
        vec![k, k.negate(&crate::SECP256K1)],
        Condition::Threshold,
        false
    )
    .is_err());
    assert!(Leaf::new(0, vec![k], Condition::Threshold, false).is_err());
    assert!(Leaf::new(2, vec![k], Condition::Threshold, false).is_err());
    assert!(Leaf::new(1, vec![k], Condition::After(1 << 53), false).is_err());
    assert!(Leaf::new(0, vec![], Condition::Hashlock([0; 32]), false).is_err());
    let zero = Leaf::new(1, vec![k], Condition::After(0), false).unwrap();
    assert_eq!(Leaf::from_bytes(&zero.to_bytes()).unwrap(), zero);
    let mut padded = zero.to_bytes();
    *padded.last_mut().unwrap() = 1;
    padded.push(0);
    assert!(Leaf::from_bytes(&padded).is_err());
}

#[test]
fn normative_tree_fold_and_tweaks() {
    let vectors = vectors();
    for (index, leaves_field, root_field) in
        [(1, "three_leaf_tree", "root"), (2, "leaves", "merkle_root")]
    {
        let vector = &vectors[index];
        let leaves: Vec<_> = vector[leaves_field]
            .as_array()
            .unwrap()
            .iter()
            .map(leaf)
            .collect();
        let tree = Tree::new(leaves.clone()).unwrap();
        assert_eq!(tree.root(), hash(&vector[root_field]));
        assert_eq!(
            tweaked_key(key(&vector["internal_key"]), Some(tree.root())).unwrap(),
            key(&vector["secret"])
        );
        for (i, leaf) in leaves.iter().enumerate() {
            let root = tree.path(i).unwrap().into_iter().fold(leaf.hash(), branch);
            assert_eq!(root, tree.root());
        }
        let reversed = Tree::new(leaves.into_iter().rev().collect()).unwrap();
        assert_eq!(tree.root(), reversed.root());
    }
    let leaf = leaf(&vectors[0]["threshold_1of1_key3"]);
    assert!(Tree::new(vec![]).is_err());
    assert!(Tree::new(vec![leaf.clone(); 9]).is_err());
    for count in 1..=8 {
        let tree = Tree::new(vec![leaf.clone(); count]).unwrap();
        for index in 0..count {
            assert!(tree.path(index).unwrap().len() <= 3);
        }
    }
    let vector = &vectors[13];
    let internal = key(&vector["internal_key"]);
    assert_eq!(tweak(&internal, None).to_be_bytes(), hash(&vector["tweak"]));
    assert_eq!(tweaked_key(internal, None).unwrap(), key(&vector["secret"]));
    assert_eq!(
        reduce_scalar([0xff; 32]).to_be_bytes(),
        hex::decode("000000000000000000000000000000014551231950b75fc4402da1732fc9bebe")
            .unwrap()
            .as_slice()
    );
}

#[test]
fn key_and_script_witness_vectors() {
    let vectors = vectors();
    let secret = vectors[3]["secret"].as_str().unwrap();
    let digest = hex::decode("e1d7170b89a2b6eedec90453e32b6c320dfadd590e6a6454bddec95a0e3834cd")
        .unwrap()
        .try_into()
        .unwrap();
    for index in [4, 5] {
        let mut witness: Witness = serde_json::from_value(vectors[index].clone()).unwrap();
        assert!(!witness.verify(secret, digest, 1755561600).unwrap());
        assert!(witness.verify(secret, [0; 32], 1755561600).is_err());
        if index == 5 {
            assert!(witness.verify(secret, digest, 1755561599).is_err());
        }
        witness.signatures.push(witness.signatures[0].clone());
        assert!(witness.verify(secret, digest, 1755561600).is_err());
    }
    let witness: Witness = serde_json::from_value(vectors[10].clone()).unwrap();
    assert!(witness
        .verify(
            vectors[8]["secret"].as_str().unwrap(),
            hash(&vectors[9]["input_digest"]),
            0
        )
        .unwrap());
}

#[test]
fn transaction_and_input_digest_vectors() {
    let vectors = vectors();
    let proofs: Vec<crate::Proof> = serde_json::from_value(vectors[14]["inputs"].clone()).unwrap();
    let outputs =
        serde_json::from_value::<Vec<crate::BlindedMessage>>(vectors[14]["outputs"].clone())
            .unwrap();
    let transaction = Transaction::new(&proofs, &[], &outputs, &[]).unwrap();
    assert_eq!(transaction.as_bytes(), bytes(&vectors[15]["transcript"]));
    assert_eq!(transaction.digest(), hash(&vectors[15]["digest"]));
    assert_eq!(
        transaction.input_id(0).unwrap(),
        hash(&vectors[15]["input_id"])
    );
    assert_eq!(
        transaction.input_digest(0).unwrap(),
        hash(&vectors[15]["input_digest"])
    );
    let witness: Witness = serde_json::from_value(vectors[16].clone()).unwrap();
    witness
        .verify(
            &proofs[0].secret.to_string(),
            transaction.input_digest(0).unwrap(),
            0,
        )
        .unwrap();
    assert!(Transaction::new(&[proofs[0].clone(), proofs[0].clone()], &[], &outputs, &[]).is_err());
    assert!(Transaction::new(&proofs, &[], &[], &[]).is_err());
    assert!(Transaction::new(&[], &[], &outputs, &[]).is_err());
    assert!(transaction.input_digest(1).is_err());
    let mut multiple = proofs;
    multiple.push(serde_json::from_value(vectors[17].clone()).unwrap());
    let mut outputs = outputs;
    outputs[0].amount = 8.into();
    let transaction = Transaction::new(&multiple, &[], &outputs, &[]).unwrap();
    assert_eq!(transaction.digest(), hash(&vectors[18]["digest"]));
    for index in 0..2 {
        assert_eq!(
            transaction.input_digest(index).unwrap(),
            hash(&vectors[18]["inputs"][index]["input_digest"])
        );
    }
}
