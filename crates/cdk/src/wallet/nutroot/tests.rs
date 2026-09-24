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
    let before = signed.inputs()[0].witness.clone();
    wallet
        .sign_nutroot_swap(&mut signed, &keys, &[])
        .await
        .unwrap();
    assert_eq!(signed.inputs()[0].witness, before);
}
