//! Nutroot wallet-to-mint flows without external services.
use cashu::nuts::nut10::nutroot::{nums_point, Condition, Leaf, NutrootOption};
use cashu::{Amount, CurrencyUnit, KeySetVersion, SecretKey};
use cdk::wallet::{ReceiveOptions, SendOptions};
use cdk_integration_tests::init_pure_tests::{
    create_and_start_test_mint, create_test_wallet_for_mint, fund_wallet,
};

#[tokio::test]
async fn nutroot_payment_receive_and_receipt_export() {
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
            let leaf = Leaf::new(1, vec![public], Condition::Threshold, true).unwrap();
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
    let receipt_ids = receiver.nutroot_receipt_ids().await.unwrap();
    assert_eq!(receipt_ids.len(), 2);
    for id in receipt_ids {
        let receipt = receiver.export_nutroot_receipt(&id).await.unwrap();
        payer.verify_nutroot_receipt(&receipt).await.unwrap();
    }
    mint.stop().await.unwrap();
}

#[tokio::test]
async fn nutroot_receipt_survives_mixed_legacy_redemption_without_legacy_dleq() {
    use cashu::{KeySetInfo, Token};
    use cdk_common::wallet::KeysetLoadPolicy;
    let mint = create_and_start_test_mint().await.unwrap();
    let amounts = vec![1, 2, 4, 8, 16, 32];
    mint.rotate_keyset_by_version(
        CurrencyUnit::Sat,
        amounts.clone(),
        0,
        KeySetVersion::Version01,
        None,
    )
    .await
    .unwrap();
    let wallet = create_test_wallet_for_mint(mint.clone()).await.unwrap();
    fund_wallet(wallet.clone(), 8, None).await.unwrap();
    mint.rotate_keyset_by_version(
        CurrencyUnit::Sat,
        amounts,
        0,
        KeySetVersion::Version02,
        None,
    )
    .await
    .unwrap();
    wallet.keysets(KeysetLoadPolicy::Refresh).await.unwrap();
    fund_wallet(wallet.clone(), 8, None).await.unwrap();
    let token = wallet
        .prepare_send(
            12.into(),
            SendOptions {
                nutroot: Some(NutrootOption {
                    key: *SecretKey::generate().public_key().as_secp256k1().unwrap(),
                    leaves: None,
                    blind_keys: None,
                }),
                ..Default::default()
            },
        )
        .await
        .unwrap()
        .confirm(None)
        .await
        .unwrap();
    assert_eq!(token.value().unwrap(), Amount::from(12));
    let ids = wallet.nutroot_receipt_ids().await.unwrap();
    assert_eq!(ids.len(), 1);
    let encoded = wallet.export_nutroot_receipt(&ids[0]).await.unwrap();
    let mut receipt: cashu::nuts::nut10::nutroot::SpendReceipt = encoded.parse().unwrap();
    let spent: Token = receipt.token.parse().unwrap();
    let keysets: Vec<_> = wallet
        .keysets(Default::default())
        .await
        .unwrap()
        .into_iter()
        .map(|keys| KeySetInfo {
            id: keys.id,
            unit: keys.unit,
            active: keys.active.unwrap_or(false),
            input_fee_ppk: keys.input_fee_ppk,
            final_expiry: keys.final_expiry,
        })
        .collect();
    let mut proofs = spent.proofs(&keysets).unwrap();
    assert!(proofs
        .iter()
        .any(|proof| proof.keyset_id.get_version() == KeySetVersion::Version01));
    assert!(proofs
        .iter()
        .any(|proof| proof.keyset_id.get_version() == KeySetVersion::Version02));
    for proof in &mut proofs {
        proof.dleq = None;
    }
    receipt.token =
        Token::new(spent.mint_url().unwrap(), proofs, None, CurrencyUnit::Sat).to_string();
    wallet
        .verify_nutroot_receipt(&receipt.encode().unwrap())
        .await
        .unwrap();
    mint.stop().await.unwrap();
}
