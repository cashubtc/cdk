//! Nutroot wallet-to-mint flows without external services.
use cashu::nuts::nut10::nutroot::{nums_point, Condition, Leaf, NutrootOption};
use cashu::{Amount, CurrencyUnit, KeySetVersion, SecretKey};
use cdk::wallet::{ReceiveOptions, SendOptions};
use cdk_integration_tests::init_pure_tests::{
    create_and_start_test_mint, create_test_wallet_for_mint, fund_wallet,
};

#[tokio::test]
async fn nutroot_payment_and_receive() {
    let mint = create_and_start_test_mint().await.unwrap();
    assert_eq!(
        mint.get_active_keysets()[&CurrencyUnit::Sat].get_version(),
        KeySetVersion::Version02
    );
    let payer = create_test_wallet_for_mint(mint.clone()).await.unwrap();
    let receiver = create_test_wallet_for_mint(mint.clone()).await.unwrap();
    let key = SecretKey::generate();
    let public = *key.public_key().as_secp256k1().unwrap();
    fund_wallet(payer.clone(), 32, None).await.unwrap();

    for script_path in [false, true] {
        let policy = if script_path {
            let leaf = Leaf::new(1, vec![public], Condition::Threshold).unwrap();
            NutrootOption {
                key: nums_point(),
                leaves: Some(vec![cashu::util::hex::encode(leaf.to_bytes())]),
                blind_keys: Some(vec![public]),
            }
        } else {
            NutrootOption {
                key: public,
                leaves: None,
                blind_keys: None,
            }
        };
        let prepared = payer
            .prepare_send(
                8.into(),
                SendOptions {
                    nutroot: Some(policy.clone()),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        assert!(prepared.proofs_to_send().is_empty());
        let token = prepared.confirm(None).await.unwrap();
        let mut wrong_policy = policy.clone();
        wrong_policy.key = *SecretKey::generate().public_key().as_secp256k1().unwrap();
        assert!(receiver
            .receive(
                &token.to_string(),
                ReceiveOptions {
                    nutroot: Some(wrong_policy),
                    p2pk_signing_keys: vec![key.clone()],
                    ..Default::default()
                }
            )
            .await
            .is_err());
        let received = receiver
            .receive(
                &token.to_string(),
                ReceiveOptions {
                    nutroot: Some(policy),
                    p2pk_signing_keys: vec![key.clone()],
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        assert_eq!(received, Amount::from(8));
    }
    assert_eq!(payer.total_balance().await.unwrap(), Amount::from(16));
    assert_eq!(receiver.total_balance().await.unwrap(), Amount::from(16));
    mint.stop().await.unwrap();
}
