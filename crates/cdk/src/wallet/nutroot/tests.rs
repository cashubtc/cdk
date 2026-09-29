use cdk_common::amount::SplitTarget;
use cdk_common::nuts::nut10::nutroot::{Condition, Leaf, NutrootOption};
use cdk_common::nuts::{
    BlindedMessage, Id, PreMintSecrets, Proof, SecretKey, SpendingConditionVerification,
    SwapRequest,
};

use crate::wallet::test_utils::{create_test_db, create_test_wallet};

#[tokio::test]
async fn wallet_signs_blinded_multisig_leaves_without_key_path_bypass() {
    let wallet = create_test_wallet(create_test_db().await).await;
    let keys = [SecretKey::generate(), SecretKey::generate()];
    let leaf = Leaf::new(
        2,
        keys.iter()
            .map(|key| *key.public_key().as_secp256k1().unwrap())
            .collect(),
        Condition::Threshold,
        false,
    )
    .unwrap();
    let locking = NutrootOption {
        key: super::nutroot::nums_point(),
        leaves: Some(vec![cdk_common::util::hex::encode(leaf.to_bytes())]),
        blind_keys: Some(leaf.keys().to_vec()),
    };
    let id = Id::from_bytes(&[vec![2], vec![1; 32]].concat()).unwrap();
    let premints = PreMintSecrets::with_nutroot(
        id,
        1.into(),
        &SplitTarget::None,
        &locking,
        &(0, vec![1]).into(),
    )
    .unwrap();
    let premint = &premints.secrets[0];
    let c = crate::nuts::nut01::BlsG1PublicKey::hash_to_curve(b"test signature").into();
    let mut proof = Proof::new(1.into(), id, premint.secret.clone(), c);
    proof.spend_info = premint.spend_info.clone();
    assert!(super::requires_swap_before_send(&proof));
    let output = BlindedMessage::new(1.into(), id, premint.blinded_message.blinded_secret);
    let request = SwapRequest::new(vec![proof], vec![output]);
    assert!(wallet
        .sign_nutroot_swap(&mut request.clone(), &keys[..1], &[])
        .await
        .is_err());
    let mut signed = request.clone();
    wallet
        .sign_nutroot_swap(&mut signed, &keys, &[])
        .await
        .unwrap();
    signed.verify_spending_conditions().unwrap();
    assert!(signed.inputs()[0].spend_info.is_none());
    // Replay retains noncanonical JSON bytes, but cannot bypass metadata checks.
    let transaction = super::Transaction::new(signed.inputs(), &[], signed.outputs(), &[]).unwrap();
    let digest = transaction.input_digest(0).unwrap();
    let now = cdk_common::util::unix_time();
    let mut replay = signed.inputs()[0].clone();
    let crate::nuts::Witness::NutrootWitness(raw) = replay.witness.as_ref().unwrap() else {
        panic!("expected Nutroot witness");
    };
    let witness: super::nutroot::Witness = serde_json::from_str(raw).unwrap();
    let raw = serde_json::to_string_pretty(&witness).unwrap();
    replay.witness = Some(crate::nuts::Witness::NutrootWitness(raw.clone()));
    replay.spend_info = request.inputs()[0].spend_info.clone();
    let signing_keys: Vec<_> = keys
        .iter()
        .map(|key| *key.as_secp256k1().unwrap())
        .collect();
    assert_eq!(
        super::input_witness(&replay, &signing_keys, &[], digest, now).unwrap(),
        raw
    );
    replay.spend_info.as_mut().unwrap().internal_key =
        Some(*SecretKey::generate().public_key().as_secp256k1().unwrap());
    assert!(super::input_witness(&replay, &signing_keys, &[], digest, now).is_err());
    replay.spend_info = None;
    assert_eq!(
        super::input_witness(&replay, &[], &[], digest, now).unwrap(),
        raw
    );
    let mut wrong_digest = digest;
    wrong_digest[0] ^= 1;
    assert!(super::input_witness(&replay, &[], &[], wrong_digest, now).is_err());

    let before = signed.inputs()[0].witness.clone();
    wallet
        .sign_nutroot_swap(&mut signed, &keys, &[])
        .await
        .unwrap();
    assert_eq!(signed.inputs()[0].witness, before);
    let ids = wallet.nutroot_receipt_ids().await.unwrap();
    assert_eq!(ids.len(), 1, "replay preserves the exact receipt opening");
    let bytes = wallet
        .localstore
        .kv_read(
            "nutroot_receipts",
            &wallet.nutroot_receipt_namespace(),
            &ids[0],
        )
        .await
        .unwrap()
        .unwrap();
    let receipt: super::nutroot::SpendReceipt = serde_json::from_slice(&bytes).unwrap();
    let transaction = super::Transaction::new(signed.inputs(), &[], signed.outputs(), &[]).unwrap();
    let mut state = cdk_common::nuts::ProofState::from((
        signed.inputs()[0].y().unwrap(),
        cdk_common::nuts::State::Spent,
    ));
    super::nutroot::SpendRecord::new(
        &signed.inputs()[0],
        &transaction,
        0,
        cdk_common::util::unix_time(),
    )
    .unwrap()
    .apply_to(&mut state);
    let keysets = vec![cdk_common::nuts::KeySetInfo {
        id,
        unit: wallet.unit.clone(),
        active: true,
        input_fee_ppk: 0,
        final_expiry: None,
    }];
    receipt
        .verify(&keysets, &[state], cdk_common::util::unix_time())
        .unwrap();
}

#[tokio::test]
async fn nutroot_payment_outputs_keep_policy_and_separate_seed_change() {
    use crate::fees::ProofsFeeBreakdown;
    use crate::wallet::swap::ProofReservation;
    let wallet = create_test_wallet(create_test_db().await).await;
    let receiver = SecretKey::generate();
    let policy = NutrootOption {
        key: *receiver.public_key().as_secp256k1().unwrap(),
        leaves: None,
        blind_keys: None,
    };
    let id = Id::from_bytes(&[vec![2], vec![1; 32]].concat()).unwrap();
    let premints = PreMintSecrets::with_nutroot(
        id,
        8.into(),
        &SplitTarget::None,
        &policy,
        &(0, vec![1, 2, 4, 8]).into(),
    )
    .unwrap();
    let c = crate::nuts::nut01::BlsG1PublicKey::hash_to_curve(b"test signature").into();
    let proof = Proof::new(8.into(), id, premints.secrets[0].secret.clone(), c);
    let fees = ProofsFeeBreakdown {
        total: 0.into(),
        per_keyset: Default::default(),
    };
    let swap = wallet
        .create_swap(
            &uuid::Uuid::now_v7(),
            id,
            &(0, vec![1, 2, 4, 8]).into(),
            Some(4.into()),
            SplitTarget::None,
            vec![proof],
            None,
            Some(policy.clone()),
            false,
            false,
            &fees,
            ProofReservation::Skip,
        )
        .await
        .unwrap();
    let outputs = swap.pre_mint_secrets.secrets;
    assert!(outputs.len() >= 2);
    let locked = outputs
        .iter()
        .find(|p| {
            p.spend_info
                .as_ref()
                .is_some_and(|info| info.ephemeral_key.is_some())
        })
        .unwrap();
    policy
        .verify_output(
            &locked.secret.to_string(),
            locked.spend_info.as_ref().unwrap(),
            &[*receiver.as_secp256k1().unwrap()],
        )
        .unwrap();
    assert_eq!(locked.amount, 4.into());
    assert!(locked.derivation_index.is_none());
    let change = outputs
        .iter()
        .find(|p| p.derivation_index.is_some())
        .unwrap();
    assert!(change
        .spend_info
        .as_ref()
        .is_none_or(|info| info.ephemeral_key.is_none()));
    let change_amount: u64 = outputs
        .iter()
        .filter(|p| p.derivation_index.is_some())
        .map(|p| u64::from(p.amount))
        .sum();
    assert_eq!(change_amount, 4);
}
