//! End-to-end v3 (nutroot) flows between a wallet and an in-process mint
//!
//! Minting onto a v3 keyset locks the quote with a NUT-13 type `0x04` key
//! and signs the mint transcript; swaps, sends, receives and melts sign
//! every v3 input over its input digest.

use std::collections::HashMap;

use cdk::nuts::nut00::ProofsMethods;
use cdk::nuts::{CheckStateRequest, KeySetVersion, State, Witness};
use cdk::wallet::{ReceiveOptions, SendOptions};
use cdk::Amount;
use cdk_fake_wallet::create_fake_invoice;
use cdk_integration_tests::init_pure_tests::*;

#[tokio::test]
async fn v3_mint_send_receive_and_melt() {
    setup_tracing();
    let mint = create_and_start_v3_test_mint().await.expect("v3 mint");
    let info = mint.mint_info().await.unwrap();
    assert!(info.pubkey.is_some());
    assert!(info.nuts.nut10.supports_leaf("0001"));

    let alice = create_test_wallet_for_mint(mint.clone()).await.unwrap();
    let carol = create_test_wallet_for_mint(mint.clone()).await.unwrap();

    // Mint onto the v3 keyset: a locked quote signs the mint transcript.
    let funded = fund_wallet(alice.clone(), 64, None).await.expect("fund");
    assert_eq!(funded, Amount::from(64));
    let proofs = alice.get_unspent_proofs().await.unwrap();
    assert!(proofs
        .iter()
        .all(|p| p.keyset_id.get_version() == KeySetVersion::Version02));
    assert!(proofs.iter().all(|p| p.secret.v3_point().is_ok()));

    // Send 40: the swap signs its v3 inputs; the token carries bearer keys.
    let token = alice
        .prepare_send(Amount::from(40), SendOptions::default())
        .await
        .expect("prepare send")
        .confirm(None)
        .await
        .expect("send");
    let token_str = token.to_string();
    let keysets: Vec<cdk::nuts::KeySetInfo> = alice
        .keysets(Default::default())
        .await
        .unwrap()
        .iter()
        .map(|ks| cdk::nuts::KeySetInfo {
            id: ks.id,
            unit: ks.unit.clone(),
            active: ks.active.unwrap_or(true),
            input_fee_ppk: ks.input_fee_ppk,
            final_expiry: ks.final_expiry,
        })
        .collect();
    let sent = token.proofs(&keysets).unwrap();
    assert!(sent.iter().all(|p| p.spend_info.is_some()));
    assert!(sent.iter().all(|p| p.witness.is_none()));
    assert_eq!(alice.total_balance().await.unwrap(), Amount::from(24));

    // Carol receives: check 1 rebuilds each secret from the bearer key, and
    // the sweep signs with it.
    let received = carol
        .receive(&token_str, ReceiveOptions::default())
        .await
        .expect("receive");
    assert_eq!(received, Amount::from(40));

    // The swept proofs are spent, with their witnesses and input digests.
    let states = mint
        .check_state(&CheckStateRequest {
            ys: sent.ys().unwrap(),
        })
        .await
        .unwrap()
        .states;
    for state in &states {
        assert_eq!(state.state, State::Spent);
        assert!(matches!(state.witness, Some(Witness::Nutroot(_))));
        assert!(state.input_digest.is_some());
    }

    // Receiving the same token again fails.
    assert!(carol
        .receive(&token_str, ReceiveOptions::default())
        .await
        .is_err());

    // Carol melts: the melt transcript binds the quote amount plus reserve.
    let invoice = create_fake_invoice(10_000, String::new());
    let quote = carol
        .melt_quote(
            cdk::nuts::PaymentMethod::BOLT11,
            invoice.to_string(),
            None,
            None,
        )
        .await
        .expect("melt quote");
    let melted = carol
        .prepare_melt(&quote.id, HashMap::new())
        .await
        .expect("prepare melt")
        .confirm()
        .await
        .expect("melt");
    assert_eq!(melted.amount(), Amount::from(10));
    let left = carol.total_balance().await.unwrap();
    assert!(left < Amount::from(30), "balance {left}");
}

#[tokio::test]
async fn v3_restore_recovers_point_secrets() {
    setup_tracing();
    let mint = create_and_start_v3_test_mint().await.expect("v3 mint");
    let seed = bip39::Mnemonic::generate(12)
        .unwrap()
        .to_seed_normalized("");
    let wallet = create_test_wallet_for_mint_with_seed(mint.clone(), seed)
        .await
        .unwrap();
    fund_wallet(wallet.clone(), 32, None).await.expect("fund");

    let restored = create_test_wallet_for_mint_with_seed(mint.clone(), seed)
        .await
        .unwrap();
    restored.restore().await.expect("restore");
    assert_eq!(restored.total_balance().await.unwrap(), Amount::from(32));

    // The restored proofs spend: their keys re-derive from the seed.
    let token = restored
        .prepare_send(Amount::from(5), SendOptions::default())
        .await
        .expect("prepare send")
        .confirm(None)
        .await
        .expect("send");
    assert!(token.value().unwrap() >= Amount::from(5));
}

#[tokio::test]
async fn v3_nutroot_payment_request_flow() {
    use cdk::nuts::nut10::nutroot::Leaf;
    use cdk::nuts::nut18::NutrootOption;
    use cdk::nuts::SecretKey;

    setup_tracing();
    let mint = create_and_start_v3_test_mint().await.expect("v3 mint");
    let alice = create_test_wallet_for_mint(mint.clone()).await.unwrap();
    let carol = create_test_wallet_for_mint(mint.clone()).await.unwrap();
    fund_wallet(alice.clone(), 64, None).await.expect("fund");

    // Carol requests receiver-keyed outputs with a refund leaf for a
    // co-signer, whose key is tagged blind-me.
    let carol_key = SecretKey::generate();
    let cosigner = SecretKey::generate().public_key();
    let refund = Leaf::After {
        n: 1,
        keys: vec![cosigner],
        time: cdk::util::unix_time() + 30 * 24 * 3600,
    }
    .to_leaf_bytes()
    .unwrap();
    let request = NutrootOption {
        k: carol_key.public_key(),
        l: Some(vec![refund]),
        b: Some(vec![cosigner]),
    };

    // Alice pays: the send swaps into secrets built from the request.
    let token = alice
        .prepare_send(
            Amount::from(16),
            SendOptions {
                nutroot: Some(request.clone()),
                ..Default::default()
            },
        )
        .await
        .expect("prepare nutroot send")
        .confirm(None)
        .await
        .expect("nutroot send");
    let token_str = token.to_string();

    // A receiver without Carol's key, or with another request, refuses it.
    let wrong_request = NutrootOption {
        b: None,
        ..request.clone()
    };
    assert!(carol
        .receive(
            &token_str,
            ReceiveOptions {
                p2pk_signing_keys: vec![carol_key.clone()],
                v3_payment_request: Some(wrong_request),
                ..Default::default()
            },
        )
        .await
        .is_err());
    assert!(carol
        .receive(&token_str, ReceiveOptions::default())
        .await
        .is_err());

    // Carol accepts it against her request and sweeps it with the key
    // derived from E (slot 0) and the tweak.
    let received = carol
        .receive(
            &token_str,
            ReceiveOptions {
                p2pk_signing_keys: vec![carol_key],
                v3_payment_request: Some(request),
                ..Default::default()
            },
        )
        .await
        .expect("receive nutroot payment");
    assert_eq!(received, Amount::from(16));
}

/// A NUMS lock can only be swept by revealing a leaf. Exercise both plain
/// and NUT-28 blinded leaf keys, and refuse a hashlock without its preimage.
#[tokio::test]
async fn v3_nums_payment_sweeps_threshold_and_hashlock_leaves() {
    use bitcoin::hashes::{sha256, Hash};
    use cdk::nuts::nut10::nutroot::{Leaf, NUMS_H};
    use cdk::nuts::nut18::NutrootOption;
    use cdk::nuts::SecretKey;

    let mint = create_and_start_v3_test_mint().await.unwrap();
    let alice = create_test_wallet_for_mint(mint.clone()).await.unwrap();
    let carol = create_test_wallet_for_mint(mint.clone()).await.unwrap();
    fund_wallet(alice.clone(), 64, None).await.unwrap();

    let receiver = SecretKey::generate();
    let preimage = [42u8; 32];
    for blinded in [false, true] {
        let leaf = match blinded {
            false => Leaf::Threshold {
                n: 1,
                keys: vec![receiver.public_key()],
            },
            true => Leaf::Hashlock {
                n: 1,
                keys: vec![receiver.public_key()],
                hash: sha256::Hash::hash(&preimage).to_byte_array(),
            },
        };
        let request = NutrootOption {
            k: cashu::PublicKey::from_hex(NUMS_H).unwrap(),
            l: Some(vec![leaf.to_leaf_bytes().unwrap()]),
            b: blinded.then(|| vec![receiver.public_key()]),
        };
        let token = alice
            .prepare_send(
                Amount::from(16),
                SendOptions {
                    nutroot: Some(request.clone()),
                    ..Default::default()
                },
            )
            .await
            .unwrap()
            .confirm(None)
            .await
            .unwrap();
        let mut options = ReceiveOptions {
            p2pk_signing_keys: vec![receiver.clone()],
            v3_payment_request: Some(request),
            ..Default::default()
        };
        if blinded {
            assert!(carol
                .receive(&token.to_string(), options.clone())
                .await
                .is_err());
            options.preimages = vec![cashu::util::hex::encode(preimage)];
        }
        assert_eq!(
            carol.receive(&token.to_string(), options).await.unwrap(),
            Amount::from(16)
        );
    }
    assert_eq!(carol.total_balance().await.unwrap(), Amount::from(32));
    mint.stop().await.unwrap();
}

#[tokio::test]
async fn v3_nutroot_rejects_legacy_keysets() {
    use cdk::nuts::nut18::NutrootOption;
    use cdk::nuts::SecretKey;

    let mint = create_and_start_test_mint().await.unwrap();
    let wallet = create_test_wallet_for_mint(mint.clone()).await.unwrap();
    fund_wallet(wallet.clone(), 32, None).await.unwrap();
    let error = wallet
        .prepare_send(
            Amount::from(16),
            SendOptions {
                nutroot: Some(NutrootOption {
                    k: SecretKey::generate().public_key(),
                    l: None,
                    b: None,
                }),
                ..Default::default()
            },
        )
        .await
        .expect_err("legacy keysets are rejected before reservation");
    assert!(matches!(error, cdk::Error::NutrootRequiresV3Keyset));
    assert_eq!(wallet.total_balance().await.unwrap(), Amount::from(32));
    assert!(wallet.get_reserved_proofs().await.unwrap().is_empty());
    mint.stop().await.unwrap();
}
