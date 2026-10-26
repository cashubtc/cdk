//! Vectors from cashubtc/nuts PR #443 at 2df08274, tests/10-tests.md, and PR #464.
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
    for vector in &vectors[26..=30] {
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
    let tree = Tree::new(vec![
        Leaf::new(2, keys.to_vec(), Condition::Threshold).unwrap()
    ])
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
fn thresholds_skip_unmatched_and_repeated_signatures() {
    use bitcoin::secp256k1::SecretKey;
    let [one, two, three, outsider] =
        [1, 2, 3, 4].map(|b| SecretKey::from_slice(&[b; 32]).unwrap());
    let keys = [one, two, three].map(|k| PublicKey::from_secret_key(&crate::SECP256K1, &k));
    let tree = Tree::new(vec![
        Leaf::new(2, keys.to_vec(), Condition::Threshold).unwrap()
    ])
    .unwrap();
    let secret = tweaked_key(keys[0], Some(tree.root())).unwrap().to_string();
    let digest = [42; 32];
    // Within the entry bound, NUT-10 counts distinct satisfied keys only.
    for extra in [outsider, one] {
        let mut witness = Witness::script_path(&tree, 0, keys[0]).unwrap();
        for key in [one, two, extra] {
            witness
                .signatures
                .extend(Witness::key_path(&key, digest).signatures);
        }
        witness.verify(&secret, digest, 0).unwrap();
    }
    // Two valid entries by one key are one satisfied key.
    let mut witness = Witness::script_path(&tree, 0, keys[0]).unwrap();
    for key in [one, one, outsider] {
        witness
            .signatures
            .extend(Witness::key_path(&key, digest).signatures);
    }
    assert!(witness.verify(&secret, digest, 0).is_err());
}

#[test]
fn nut28_v3_slot_map_vectors() {
    use bitcoin::secp256k1::SecretKey;
    let shared: Value = serde_json::from_str(include_str!("nutroot_v3_vectors.json")).unwrap();
    let (refund, forms) = (&shared["receiver_keyed_refund"], &shared["leaf_forms"]);
    let secret = |v: &Value| SecretKey::from_slice(&bytes(v)).unwrap();
    let ephemeral = secret(&refund["ephemeral_priv"]);
    let e = key(&refund["ephemeral_pub"]);
    let carol = secret(&refund["carol_priv"]);
    let r = Scalar::from(secret(&vectors()[3]["slot0_r"]));
    assert_eq!(
        receiver_key(&carol, &e, 0).unwrap(),
        carol.add_tweak(&r).unwrap()
    );
    assert_eq!(
        sender_key(&key(&refund["carol_pub"]), &ephemeral, 0).unwrap(),
        key(&vectors()[3]["internal_key"])
    );
    let slot1 = sender_key(&key(&forms["blind_slot1_key"]), &ephemeral, 1).unwrap();
    assert_eq!(slot1, key(&forms["blind_slot1_result"]));
    let alice = secret(&refund["alice_refund_priv"]);
    assert_eq!(
        PublicKey::from_secret_key(&crate::SECP256K1, &receiver_key(&alice, &e, 1).unwrap()),
        slot1
    );
    let time = refund["refund_time"].as_u64().unwrap();
    let leaf = Leaf::new(1, vec![slot1], Condition::After(time)).unwrap();
    assert_eq!(leaf.to_bytes(), bytes(&forms["blind_slot1_leaf"]));
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
        Leaf::new(1, vec![internal], Condition::Hashlock(hash)).unwrap(),
        Leaf::new(0, vec![], Condition::Commit(hash)).unwrap(),
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
    witness.preimage = Some(hex::encode([42; 31]));
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
    )
    .is_err());
    assert!(Leaf::new(0, vec![k], Condition::Threshold).is_err());
    assert!(Leaf::new(2, vec![k], Condition::Threshold).is_err());
    assert!(Leaf::new(1, vec![k], Condition::After(1 << 53)).is_err());
    assert!(Leaf::new(0, vec![], Condition::Hashlock([0; 32])).is_err());
    let zero = Leaf::new(1, vec![k], Condition::After(0)).unwrap();
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
        witness.verify(secret, digest, 1755561600).unwrap();
        assert!(witness.verify(secret, [0; 32], 1755561600).is_err());
        if index == 5 {
            assert!(witness.verify(secret, digest, 1755561599).is_err());
        }
        witness.signatures.push(witness.signatures[0].clone());
        assert!(witness.verify(secret, digest, 1755561600).is_err());
    }
    let witness: Witness = serde_json::from_value(vectors[10].clone()).unwrap();
    witness
        .verify(
            vectors[8]["secret"].as_str().unwrap(),
            hash(&vectors[9]["input_digest"]),
            0,
        )
        .unwrap();
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
    assert_eq!(
        Transaction::from_bytes(transaction.as_bytes()).unwrap(),
        transaction
    );
    for index in 0..2 {
        assert_eq!(
            transaction.input_digest(index).unwrap(),
            hash(&vectors[18]["inputs"][index]["input_digest"])
        );
    }
}

#[test]
fn token_spend_info_vectors_and_witness_stripping() {
    use std::str::FromStr;
    let vectors = vectors();
    for vector in &vectors[26..=30] {
        let mut token = crate::nuts::TokenV4::from_str(vector["token"].as_str().unwrap()).unwrap();
        let expected: SpendInfo = serde_json::from_value(vector["spend_info"].clone()).unwrap();
        assert_eq!(
            token.token[0].proofs[0].spend_info.as_ref(),
            Some(&expected)
        );
        // Same CBOR bytes; base64 padding is optional (NUT-00).
        assert_eq!(
            token.to_string().trim_end_matches('='),
            vector["token"].as_str().unwrap()
        );
        token.token[0].proofs[0].witness = Some(crate::nuts::Witness::NutrootWitness(
            "{\"signatures\":[]}".to_owned(),
        ));
        let encoded = token.to_string();
        let decoded = crate::nuts::TokenV4::from_str(&encoded).unwrap();
        assert!(decoded.token[0].proofs[0].witness.is_none());
        assert_eq!(
            decoded.token[0].proofs[0].spend_info.as_ref(),
            Some(&expected)
        );
        // A token received with a transaction witness must drop it too.
        let mut value = serde_json::to_value(&decoded).unwrap();
        value["t"][0]["p"][0]["witness"] = "{\"signatures\":[]}".into();
        let decoded: crate::nuts::TokenV4 = serde_json::from_value(value).unwrap();
        assert!(decoded.token[0].proofs[0].witness.is_none());
        let raw = decoded.to_raw_bytes().unwrap();
        let restored = crate::nuts::TokenV4::try_from(&raw).unwrap();
        assert_eq!(restored, decoded);
    }
}

#[test]
fn proof_preserves_exact_nutroot_witness() {
    let mut value = vectors()[14]["inputs"][0].clone();
    let witness = "{ \"signatures\": [] }";
    value["witness"] = witness.into();
    let proof: crate::Proof = serde_json::from_value(value).unwrap();
    assert_eq!(
        proof.witness,
        Some(crate::nuts::Witness::NutrootWitness(witness.to_owned()))
    );
    assert_eq!(serde_json::to_value(proof).unwrap()["witness"], witness);
}

#[test]
fn locking_policies_preserve_thresholds_and_blind_all_slots() {
    use bitcoin::secp256k1::SecretKey;
    let one = SecretKey::from_slice(&[1; 32]).unwrap();
    let two = SecretKey::from_slice(&[2; 32]).unwrap();
    let ephemeral = SecretKey::from_slice(&[3; 32]).unwrap();
    let offset = SecretKey::from_slice(&[4; 32]).unwrap();
    let keys = [one, two].map(|key| PublicKey::from_secret_key(&crate::SECP256K1, &key));
    let leaf = Leaf::new(2, keys.to_vec(), Condition::Threshold).unwrap();
    let option = NutrootOption {
        key: nums_point(),
        leaves: Some(vec![hex::encode(leaf.to_bytes())]),
        blind_keys: Some(keys.to_vec()),
    };
    let (secret, info) = option.create_output(&ephemeral, &offset).unwrap();
    let internal = info.verify(&secret, None).unwrap();
    option.verify_output(&secret, &info, &[one, two]).unwrap();
    let mut wrong_policy = option.clone();
    wrong_policy.leaves = Some(vec![hex::encode(
        Leaf::new(1, keys.to_vec(), Condition::Threshold)
            .unwrap()
            .to_bytes(),
    )]);
    assert!(wrong_policy
        .verify_output(&secret, &info, &[one, two])
        .is_err());
    let mut wrong_ephemeral = info.clone();
    wrong_ephemeral.ephemeral_key = Some(keys[0]);
    assert!(option
        .verify_output(&secret, &wrong_ephemeral, &[one, two])
        .is_err());
    assert!(info.key_path_key(&secret, Some(&one)).is_err());
    let tree = info.parsed_tree().unwrap().unwrap();
    let mut witness = Witness::script_path(&tree, 0, internal).unwrap();
    let e = info.ephemeral_key.unwrap();
    let first = receiver_key(&one, &e, 1).unwrap();
    let second = receiver_key(&two, &e, 2).unwrap();
    witness
        .signatures
        .extend(Witness::key_path(&first, [42; 32]).signatures);
    assert!(witness.verify(&secret, [42; 32], 0).is_err());
    witness
        .signatures
        .extend(Witness::key_path(&second, [42; 32]).signatures);
    witness.verify(&secret, [42; 32], 0).unwrap();
    assert!(witness.verify(&secret, [43; 32], 0).is_err());
    assert_eq!(
        sender_key(&keys[0], &ephemeral, 255).unwrap(),
        PublicKey::from_secret_key(&crate::SECP256K1, &receiver_key(&one, &e, 255).unwrap())
    );
    let (_, another) = option.create_output(&ephemeral, &two).unwrap();
    assert_ne!(info.internal_key, another.internal_key);
}

#[test]
fn legacy_keyless_policies_are_not_silently_weakened_for_nutroot() {
    let key = crate::nuts::SecretKey::generate();
    let policy = crate::nuts::SpendingConditions::P2PKConditions {
        data: key.public_key(),
        conditions: Some(crate::nuts::Conditions {
            locktime: Some(100),
            ..Default::default()
        }),
    };
    assert!(NutrootOption::from_spending_conditions(&policy, false).is_err());
    let policy = crate::nuts::SpendingConditions::HTLCConditions {
        data: "00".repeat(32).parse().unwrap(),
        conditions: None,
    };
    assert!(NutrootOption::from_spending_conditions(&policy, false).is_err());
}

#[test]
fn signing_packages_merge_only_matching_transactions_and_distinct_signers() {
    use bitcoin::secp256k1::SecretKey;
    let vectors = vectors();
    let mut proofs: Vec<crate::Proof> =
        serde_json::from_value(vectors[14]["inputs"].clone()).unwrap();
    let outputs: Vec<crate::BlindedMessage> =
        serde_json::from_value(vectors[14]["outputs"].clone()).unwrap();
    let keys = [
        SecretKey::from_slice(&[1; 32]).unwrap(),
        SecretKey::from_slice(&[2; 32]).unwrap(),
    ];
    let ephemeral = SecretKey::from_slice(&[3; 32]).unwrap();
    let ephemeral_public = PublicKey::from_secret_key(&crate::SECP256K1, &ephemeral);
    let public = keys.map(|k| {
        sender_key(
            &PublicKey::from_secret_key(&crate::SECP256K1, &k),
            &ephemeral,
            17,
        )
        .unwrap()
    });
    let tree = Tree::new(vec![
        Leaf::new(2, public.to_vec(), Condition::Threshold).unwrap()
    ])
    .unwrap();
    let internal = nums_point();
    let secret = tweaked_key(internal, Some(tree.root()))
        .unwrap()
        .to_string();
    proofs[0].secret = secret.parse().unwrap();
    proofs[0].spend_info = Some(SpendInfo {
        bearer_key: Some(crate::nuts::SecretKey::from_slice(&[7; 32]).unwrap()),
        ..Default::default()
    });
    let witness = Witness::script_path(&tree, 0, internal).unwrap();
    let mut package = SigningPackage::new(
        &Transaction::new(&proofs, &[], &outputs, &[]).unwrap(),
        vec![SigningSpend {
            input: 0,
            secret,
            witness,
            ephemeral_key: Some(ephemeral_public),
            slots: Some(vec![1, 2]),
        }],
    )
    .unwrap();
    let encoded = package.encode().unwrap();
    assert_eq!(encoded.parse::<SigningPackage>().unwrap(), package);
    // A spend opens only the input it names.
    let mut pair = proofs.clone();
    pair.push(serde_json::from_value(vectors[17].clone()).unwrap());
    let pair = Transaction::new(&pair, &[], &outputs, &[]).unwrap();
    let mut misnamed = package.spends().to_vec();
    assert!(SigningPackage::new(&pair, misnamed.clone()).is_ok());
    misnamed[0].input = 1;
    assert!(SigningPackage::new(&pair, misnamed.clone()).is_err());
    misnamed[0].input = 0;
    misnamed.push(misnamed[0].clone());
    assert!(SigningPackage::new(&pair, misnamed).is_err());
    assert!(SigningPackage::new(&pair, vec![]).is_err());
    let mut with_preimage = package.spends().to_vec();
    with_preimage[0].witness.preimage = Some("00".repeat(32));
    assert!(SigningPackage::new(&pair, with_preimage).is_err());
    assert!(!serde_json::to_string(&package)
        .unwrap()
        .contains("spend_info"));
    let mut invalid = serde_json::to_value(&package).unwrap();
    invalid["spends"][0]["signatures"] = serde_json::json!(["invalid"]);
    let mut invalid: SigningPackage = serde_json::from_value(invalid).unwrap();
    let before = invalid.clone();
    assert!(invalid.sign(&keys).is_err());
    assert_eq!(invalid, before);
    let before = package.clone();
    assert!(package.merge(&invalid).is_err());
    assert_eq!(package, before);
    let mut second = package.clone();
    assert_eq!(package.sign(&keys[..1]).unwrap(), 1);
    assert!(package
        .apply(&mut proofs, &outputs, None, &Default::default(), 0)
        .is_err());
    assert!(proofs[0].witness.is_none());
    assert_eq!(package.sign(&keys[..1]).unwrap(), 0);
    assert_eq!(second.sign(&keys[1..]).unwrap(), 1);
    package.merge(&second).unwrap();
    package.merge(&second).unwrap();
    package
        .apply(&mut proofs, &outputs, None, &Default::default(), 0)
        .unwrap();
    let transaction = Transaction::new(&proofs, &[], &outputs, &[]).unwrap();
    SpendRecord::new(&proofs[0], &transaction, 0, 0).unwrap();
    let mut wrong_outputs = outputs.clone();
    wrong_outputs[0].amount = 99.into();
    assert!(package
        .apply(&mut proofs, &wrong_outputs, None, &Default::default(), 0)
        .is_err());
    let altered = SigningPackage::new(
        &Transaction::new(&proofs, &[], &wrong_outputs, &[]).unwrap(),
        second.spends().to_vec(),
    );
    assert!(altered.is_err());
}

#[test]
fn authorized_request_binds_exact_http_bytes() {
    let digest = authorized_request_digest("POST", "/v1/swap?x=1", b"{}").unwrap();
    assert_ne!(
        digest,
        authorized_request_digest("POST", "/v1/swap?x=1", b"{ }").unwrap()
    );
    assert_ne!(
        digest,
        authorized_request_digest("POST", "/v1/swap?x=2", b"{}").unwrap()
    );
    assert_ne!(
        digest,
        authorized_request_digest("GET", "/v1/swap?x=1", b"{}").unwrap()
    );
    assert!(authorized_request_digest("post", "/v1/swap", b"").is_err());
    assert!(authorized_request_digest("POST", "https://mint/v1/swap", b"").is_err());
}

#[test]
fn upstream_authorized_request_vectors() {
    let vectors: Vec<Value> = serde_json::from_str(include_str!("auth_vectors.json")).unwrap();
    for (vector, body) in vectors
        .iter()
        .zip([&b"illustrative request body"[..], &b""[..]])
    {
        let (method, target) = (
            vector["method"].as_str().unwrap(),
            vector["target"].as_str().unwrap(),
        );
        let transcript = transaction::authorized_request_transcript(method, target, body).unwrap();
        assert_eq!(transcript, bytes(&vector["transcript"]));
        assert_eq!(
            <[u8; 32]>::from(sha2::Sha256::digest(&transcript)),
            hash(&vector["request_digest"])
        );
        let digest = authorized_request_digest(method, target, body).unwrap();
        assert_eq!(digest, hash(&vector["digest"]));
        let witness: Witness = serde_json::from_value(vector["witness"].clone()).unwrap();
        witness
            .verify(vector["secret"].as_str().unwrap(), digest, 0)
            .unwrap();
    }
}

#[test]
fn shared_mint_quote_input_vectors() {
    use std::str::FromStr;
    let shared: Value = serde_json::from_str(include_str!("nutroot_v3_vectors.json")).unwrap();
    let list = |v: &Value| v.as_array().cloned().unwrap_or_default();
    let transaction = |tx: &Value| {
        let quotes: Vec<_> = list(&tx["mint_quote_inputs"])
            .iter()
            .map(|v| MintQuoteInput {
                amount: v["amount"].as_u64().unwrap().into(),
                id: v["quote_id"].as_str().unwrap().to_owned(),
                pubkey: crate::nuts::PublicKey::from_hex(v["lock_pubkey"].as_str().unwrap())
                    .unwrap(),
                issued: v["issued"].as_u64().unwrap().into(),
            })
            .collect();
        let outputs: Vec<_> = list(&tx["blinded_outputs"])
            .iter()
            .map(|v| crate::BlindedMessage {
                amount: v["amount"].as_u64().unwrap().into(),
                keyset_id: crate::nuts::Id::from_str(v["keyset_id"].as_str().unwrap()).unwrap(),
                blinded_secret: crate::nuts::PublicKey::from_hex(v["B_"].as_str().unwrap())
                    .unwrap(),
                witness: None,
            })
            .collect();
        Transaction::new(&[], &quotes, &outputs, &[]).unwrap()
    };
    for name in ["mint", "partial_mint", "batch_mint"] {
        let case = &shared["transcript"][name];
        let transaction = transaction(&case["tx"]);
        assert_eq!(transaction.as_bytes(), bytes(&case["transcript"]), "{name}");
        assert_eq!(transaction.digest(), hash(&case["digest"]), "{name}");
        assert_eq!(
            Transaction::from_bytes(transaction.as_bytes()).unwrap(),
            transaction,
            "{name}"
        );
        let inputs: Vec<(usize, &Value)> = match case.get("inputs") {
            Some(inputs) => inputs.as_array().unwrap().iter().enumerate().collect(),
            None => vec![(0, case)],
        };
        for (index, input) in inputs {
            assert_eq!(
                transaction.input_id(index).unwrap(),
                hash(&input["input_id"]),
                "{name}"
            );
            let digest = transaction.input_digest(index).unwrap();
            assert_eq!(digest, hash(&input["input_digest"]), "{name}");
            let Some(signature) = input["signature"].as_str() else {
                continue;
            };
            let key =
                crate::nuts::PublicKey::from_hex(input["lock_pubkey"].as_str().unwrap()).unwrap();
            let witness = Witness {
                signatures: vec![signature.to_owned()],
                leaf: None,
                control: None,
                preimage: None,
            };
            witness.verify(&key.to_string(), digest, 0).unwrap();
        }
    }
    // The partial mint's signature covers its amount and `issued` (tests/00-tests.md).
    let partial = &shared["transcript"]["partial_mint"];
    for (field, value) in [("amount", 8), ("issued", 0)] {
        let mut tx = partial["tx"].clone();
        tx["mint_quote_inputs"][0][field] = value.into();
        assert_ne!(
            transaction(&tx).input_digest(0).unwrap(),
            hash(&partial["input_digest"]),
            "{field}"
        );
    }
    // A quote input without its lock key is not a transcript this mint accepts.
    let transcript = bytes(&shared["transcript"]["mint"]["transcript"]);
    let mut stripped = transcript[..3].to_vec();
    stripped.extend_from_slice(&transcript[3..transcript.len() - 35]);
    assert!(Transaction::from_bytes(&stripped).is_err());
}

/// NUT-XX: a mint quote paying a melt keeps the mint vector's quote input bytes.
#[test]
fn shared_mint_quote_to_melt_vector() {
    let shared: Value = serde_json::from_str(include_str!("nutroot_v3_vectors.json")).unwrap();
    let case = &shared["transcript"]["mint_quote_to_melt"];
    let quote = &case["tx"]["mint_quote_inputs"][0];
    let melt = &case["tx"]["melt_quote_outputs"][0];
    let transaction = Transaction::new(
        &[],
        &[MintQuoteInput {
            amount: quote["amount"].as_u64().unwrap().into(),
            id: quote["quote_id"].as_str().unwrap().to_owned(),
            pubkey: crate::nuts::PublicKey::from_hex(quote["lock_pubkey"].as_str().unwrap())
                .unwrap(),
            issued: quote["issued"].as_u64().unwrap().into(),
        }],
        &[],
        &[Quote {
            amount: melt["amount"].as_u64().unwrap().into(),
            id: melt["quote_id"].as_str().unwrap().to_owned(),
        }],
    )
    .unwrap();
    assert_eq!(transaction.as_bytes(), bytes(&case["transcript"]));
    assert_eq!(transaction.digest(), hash(&case["digest"]));
    assert_eq!(transaction.input_id(0).unwrap(), hash(&case["input_id"]));
    assert_eq!(
        transaction.input_id(0).unwrap(),
        hash(&shared["transcript"]["mint"]["input_id"])
    );
    assert_eq!(
        transaction.input_digest(0).unwrap(),
        hash(&case["input_digest"])
    );
}

#[test]
fn shared_proof_transcript_vectors() {
    let shared: Value = serde_json::from_str(include_str!("nutroot_v3_vectors.json")).unwrap();
    let list = |v: &Value| v.as_array().cloned().unwrap_or_default();
    for name in [
        "swap",
        "melt",
        "melt_with_change",
        "multi_input",
        "mixed_keysets",
    ] {
        let case = &shared["transcript"][name];
        let tx = &case["tx"];
        let proofs: Vec<crate::Proof> = list(&tx["proof_inputs"])
            .iter()
            .map(|v| {
                let proof = serde_json::json!({
                    "amount": v["amount"], "id": v["keyset_id"], "secret": v["secret"], "C": v["C"]
                });
                serde_json::from_value(proof).unwrap()
            })
            .collect();
        let outputs: Vec<crate::BlindedMessage> = list(&tx["blinded_outputs"])
            .iter()
            .map(|v| {
                let output =
                    serde_json::json!({ "amount": v["amount"], "id": v["keyset_id"], "B_": v["B_"] });
                serde_json::from_value(output).unwrap()
            })
            .collect();
        let melts: Vec<Quote> = list(&tx["melt_quote_outputs"])
            .iter()
            .map(|v| Quote {
                amount: v["amount"].as_u64().unwrap().into(),
                id: v["quote_id"].as_str().unwrap().to_owned(),
            })
            .collect();
        let transaction = Transaction::new(&proofs, &[], &outputs, &melts).unwrap();
        assert_eq!(transaction.as_bytes(), bytes(&case["transcript"]), "{name}");
        assert_eq!(transaction.digest(), hash(&case["digest"]), "{name}");
        assert_eq!(
            Transaction::from_bytes(transaction.as_bytes()).unwrap(),
            transaction,
            "{name}"
        );
        let inputs: Vec<(usize, &Value)> = match case.get("inputs") {
            Some(inputs) => inputs.as_array().unwrap().iter().enumerate().collect(),
            None => vec![(0, case)],
        };
        for (index, input) in inputs {
            assert_eq!(
                transaction.input_id(index).unwrap(),
                hash(&input["input_id"]),
                "{name}"
            );
            // A pre-v3 input has an id but signs nothing.
            let Some(expected) = input.get("input_digest") else {
                continue;
            };
            let digest = transaction.input_digest(index).unwrap();
            assert_eq!(digest, hash(expected), "{name}");
            if let Some(signature) = input["signature"].as_str() {
                let witness = Witness {
                    signatures: vec![signature.to_owned()],
                    leaf: None,
                    control: None,
                    preimage: None,
                };
                witness
                    .verify(&proofs[index].secret.to_string(), digest, 0)
                    .unwrap();
            }
        }
    }
}

#[test]
fn commit_leaf_and_unknown_sibling_vectors() {
    let shared: Value = serde_json::from_str(include_str!("nutroot_v3_vectors.json")).unwrap();
    let vector = &shared["commit_leaf"];
    let offset = bitcoin::secp256k1::SecretKey::from_slice(&bytes(&vector["u"])).unwrap();
    let internal = nums_key(&offset).unwrap();
    assert_eq!(internal, key(&vector["K"]));
    let threshold = leaf(&vector["leaf_threshold"]);
    let commit = leaf(&vector["leaf_commit"]);
    assert_eq!(
        commit.condition(),
        &Condition::Commit(hash(&vector["hash"]))
    );
    assert_eq!(threshold.hash(), hash(&vector["leaf_hash_threshold"]));
    assert_eq!(commit.hash(), hash(&vector["leaf_hash_commit"]));
    let tree = Tree::new(vec![threshold, commit]).unwrap();
    assert_eq!(tree.root(), hash(&vector["merkle_root"]));
    assert_eq!(
        tree.path(0).unwrap(),
        vec![hash(&vector["leaf_hash_commit"])]
    );
    assert_eq!(
        tweak(&internal, Some(tree.root())).to_be_bytes(),
        hash(&vector["tweak"])
    );
    assert_eq!(
        tweaked_key(internal, Some(tree.root())).unwrap(),
        key(&vector["secret"])
    );

    // An unknown leaf type still hashes as a sibling, though it never parses as a spend path.
    let vector = &vectors()[6];
    let unknown = bytes(&vector["leaf_0_unknown_type"]);
    assert!(Leaf::from_bytes(&unknown).is_err());
    let unknown_hash = tagged_hash("Cashu_NutrootLeaf", &unknown);
    assert_eq!(unknown_hash, hash(&vector["leaf_hash_0"]));
    let after = leaf(&vector["leaf_1_after"]);
    assert_eq!(after.hash(), hash(&vector["leaf_hash_1"]));
    let root = branch(unknown_hash, after.hash());
    assert_eq!(root, hash(&vector["merkle_root"]));
    let internal = key(&vector["internal_key"]);
    assert_eq!(
        tweak(&internal, Some(root)).to_be_bytes(),
        hash(&vector["tweak"])
    );
    assert_eq!(
        tweaked_key(internal, Some(root)).unwrap(),
        key(&vector["secret"])
    );
}

#[test]
fn shared_signing_package_vectors() {
    let shared: Value = serde_json::from_str(include_str!("nutroot_v3_vectors.json")).unwrap();
    let vector = &shared["signing_package"];
    let unsigned: SigningPackage = vector["package_string"].as_str().unwrap().parse().unwrap();
    let signed: SigningPackage = vector["signed_string"].as_str().unwrap().parse().unwrap();
    // A signer reviews what it signs: the mint vector's quote input under the lock, and its output.
    let tx = &shared["transcript"]["mint"]["tx"];
    let (quote, output) = (&tx["mint_quote_inputs"][0], &tx["blinded_outputs"][0]);
    let b_ = hex::decode(output["B_"].as_str().unwrap()).unwrap();
    assert_eq!(
        unsigned.transaction().unwrap().elements().unwrap(),
        vec![
            TransactionElement::MintQuoteInput(MintQuoteInput {
                amount: quote["amount"].as_u64().unwrap().into(),
                id: quote["quote_id"].as_str().unwrap().to_owned(),
                pubkey: crate::nuts::PublicKey::from_hex(vector["lock_key"].as_str().unwrap())
                    .unwrap(),
                issued: quote["issued"].as_u64().unwrap().into(),
            }),
            TransactionElement::BlindedOutput {
                amount: output["amount"].as_u64().unwrap().into(),
                keyset_id: output["keyset_id"].as_str().unwrap().parse().unwrap(),
                b_hash: sha2::Sha256::digest(&b_).into(),
            },
        ]
    );
    let melt = &shared["transcript"]["melt"];
    let elements = Transaction::from_bytes(&bytes(&melt["transcript"]))
        .unwrap()
        .elements()
        .unwrap();
    let proof = &melt["tx"]["proof_inputs"][0];
    assert!(
        matches!(&elements[0], TransactionElement::ProofInput { amount, keyset_id, y }
        if *amount == proof["amount"].as_u64().unwrap().into()
            && keyset_id.to_string() == proof["keyset_id"].as_str().unwrap()
            && matches!(y, crate::nuts::PublicKey::BlsG1(_)))
    );
    assert_eq!(
        elements[1],
        TransactionElement::MeltQuoteOutput(Quote {
            amount: melt["tx"]["melt_quote_outputs"][0]["amount"]
                .as_u64()
                .unwrap()
                .into(),
            id: "quote-melt-0001".to_owned(),
        })
    );
    assert_eq!(
        unsigned.transaction().unwrap().input_digest(0).unwrap(),
        hash(&vector["input_digest"])
    );
    assert_eq!(serde_json::to_value(&unsigned).unwrap(), vector["package"]);
    assert!(unsigned.spends()[0].witness.signatures.is_empty());
    assert_eq!(signed.spends()[0].witness.signatures.len(), 1);
    let mut merged = unsigned.clone();
    merged.merge(&signed).unwrap();
    assert_eq!(merged, signed);
    // The quote input's witness, as its mint request `signature` carries it.
    let transaction = signed.transaction().unwrap();
    let witness = signed.witness(&transaction, 0, None, 0).unwrap();
    assert_eq!(
        serde_json::from_str::<Value>(&witness).unwrap(),
        vector["witness"]
    );
    assert!(unsigned.witness(&transaction, 0, None, 0).is_err());
    assert!(signed.witness(&transaction, 1, None, 0).is_err());
    assert_eq!(
        signed.encode().unwrap().parse::<SigningPackage>().unwrap(),
        signed
    );

    // A spend must name an input of the transcript.
    let mut value = serde_json::to_value(&unsigned).unwrap();
    value["spends"][0]["input"] = 1.into();
    let misnamed: SigningPackage = serde_json::from_value(value).unwrap();
    assert!(misnamed.validate().is_err());
    // Inputs travel only as transcript containers.
    assert!(!serde_json::to_string(&unsigned).unwrap().contains("\"C\""));
}

#[test]
fn signing_package_hashlock_takes_its_preimage_at_apply() {
    use bitcoin::hashes::{sha256, Hash};
    use bitcoin::secp256k1::SecretKey;
    let vectors = vectors();
    let mut proofs: Vec<crate::Proof> =
        serde_json::from_value(vectors[14]["inputs"].clone()).unwrap();
    let outputs: Vec<crate::BlindedMessage> =
        serde_json::from_value(vectors[14]["outputs"].clone()).unwrap();
    let key = SecretKey::from_slice(&[1; 32]).unwrap();
    let ephemeral = SecretKey::from_slice(&[3; 32]).unwrap();
    let signer = sender_key(
        &PublicKey::from_secret_key(&crate::SECP256K1, &key),
        &ephemeral,
        1,
    )
    .unwrap();
    let preimage = [5u8; 32];
    let hash = sha256::Hash::hash(&preimage).to_byte_array();
    let tree = Tree::new(vec![
        Leaf::new(1, vec![signer], Condition::Hashlock(hash)).unwrap()
    ])
    .unwrap();
    let internal = nums_point();
    let secret = tweaked_key(internal, Some(tree.root()))
        .unwrap()
        .to_string();
    proofs[0].secret = secret.parse().unwrap();
    let mut package = SigningPackage::new(
        &Transaction::new(&proofs, &[], &outputs, &[]).unwrap(),
        vec![SigningSpend {
            input: 0,
            secret: secret.clone(),
            witness: Witness::script_path(&tree, 0, internal).unwrap(),
            ephemeral_key: Some(PublicKey::from_secret_key(&crate::SECP256K1, &ephemeral)),
            slots: None,
        }],
    )
    .unwrap();
    assert_eq!(package.sign(&[key]).unwrap(), 1);
    assert!(package
        .apply(&mut proofs, &outputs, None, &Default::default(), 0)
        .is_err());
    let preimages = [(secret, hex::encode(preimage))].into();
    package
        .apply(&mut proofs, &outputs, None, &preimages, 0)
        .unwrap();
    let Some(crate::nuts::Witness::NutrootWitness(raw)) = &proofs[0].witness else {
        panic!("expected a nutroot witness");
    };
    assert!(raw.contains(&hex::encode(preimage)));
}
