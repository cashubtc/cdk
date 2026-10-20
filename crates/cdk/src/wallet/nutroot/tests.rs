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

#[test]
fn sign_quote_commits_the_draw_and_the_quote_state() {
    use std::str::FromStr;

    // The shared partial mint: an 8-sat quote with 3 issued, drawing 4.
    let vectors: serde_json::Value = serde_json::from_str(include_str!(
        "../../../../cashu/src/nuts/nut10/nutroot/nutroot_v3_vectors.json"
    ))
    .unwrap();
    let vector = &vectors["transcript"]["partial_mint"];
    let lock = &vectors["nut13_v3"]["quote_locks"][0];
    let key = SecretKey::from_hex(lock["privkey"].as_str().unwrap()).unwrap();
    let outputs: Vec<BlindedMessage> = vector["tx"]["blinded_outputs"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| BlindedMessage {
            amount: v["amount"].as_u64().unwrap().into(),
            keyset_id: Id::from_str(v["keyset_id"].as_str().unwrap()).unwrap(),
            blinded_secret: cdk_common::nuts::PublicKey::from_hex(v["B_"].as_str().unwrap())
                .unwrap(),
            witness: None,
        })
        .collect();
    let mut quote = cdk_common::wallet::MintQuote::new(
        "quote-mint-0004".to_string(),
        cdk_common::mint_url::MintUrl::from_str("https://mint.example").unwrap(),
        cdk_common::PaymentMethod::from_str("bolt11").unwrap(),
        Some(8.into()),
        cdk_common::nuts::CurrencyUnit::Sat,
        String::new(),
        0,
        Some(key.clone()),
    );
    quote.amount_issued = 3.into();

    let signature = super::sign_quote(&outputs, &[quote], None, "quote-mint-0004", &key).unwrap();
    let digest: [u8; 32] = cdk_common::util::hex::decode(vector["input_digest"].as_str().unwrap())
        .unwrap()
        .try_into()
        .unwrap();
    let verified = bitcoin::secp256k1::schnorr::Signature::from_str(&signature)
        .map(|sig| {
            cdk_common::SECP256K1
                .verify_schnorr(
                    &sig,
                    &bitcoin::secp256k1::Message::from_digest(digest),
                    &key.as_secp256k1()
                        .unwrap()
                        .x_only_public_key(&cdk_common::SECP256K1)
                        .0,
                )
                .is_ok()
        })
        .unwrap();
    assert!(verified);
}

#[test]
fn x_only_twin_derives_a_key_blinded_from_the_even_lift() {
    use bitcoin::secp256k1::{Parity, PublicKey, SecretKey};
    use cdk_common::nuts::nut10::nutroot::{receiver_key, sender_key};

    let secp = &cdk_common::SECP256K1;
    let receiver = (1u8..)
        .map(|i| SecretKey::from_slice(&[i; 32]).unwrap())
        .find(|key| key.x_only_public_key(secp).1 == Parity::Odd)
        .unwrap();
    // The sender holds only the x-only key (eg a Nostr npub) and lifts it to even Y.
    let lifted =
        PublicKey::from_x_only_public_key(receiver.x_only_public_key(secp).0, Parity::Even);
    let ephemeral = SecretKey::from_slice(&[9; 32]).unwrap();
    let blinded = sender_key(&lifted, &ephemeral, 1).unwrap();
    let ephemeral = PublicKey::from_secret_key(secp, &ephemeral);
    let derives = |keys: &[SecretKey]| {
        keys.iter().any(|key| {
            PublicKey::from_secret_key(secp, &receiver_key(key, &ephemeral, 1).unwrap()) == blinded
        })
    };
    assert!(!derives(&[receiver]));
    assert!(derives(&super::with_x_only_twins(vec![receiver])));
}
