//! Focused test: auto-sign P2PK receive using SQLite memory wallet DB

use cdk::amount::SplitTarget;
use cdk::nuts::{SecretKey, SpendingConditions};
use cdk::wallet::{ReceiveOptions, SendOptions};
use cdk_integration_tests::init_pure_tests::{
    create_and_start_test_mint, create_test_wallet_for_mint, fund_wallet, setup_tracing,
};

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_autosign_receive_with_sqlite_memory_db() {
    setup_tracing();

    // Create in-process mint and wallet (wallet DB type provided via CDK_TEST_DB_TYPE)
    let mint = create_and_start_test_mint()
        .await
        .expect("Failed to create test mint");
    let wallet = create_test_wallet_for_mint(mint)
        .await
        .expect("Failed to create test wallet");

    // Fund wallet with some amount to create a P2PK-locked token to self
    let _ = fund_wallet(wallet.clone(), 100, Some(SplitTarget::default()))
        .await
        .expect("Failed to fund wallet");

    // Generate and store a P2PK signing key in the wallet DB
    // Keep the secret locally to craft the spending condition
    let signing_sk = SecretKey::generate();
    let signing_pk = signing_sk.public_key();
    wallet
        .add_p2pk_signing_key(signing_sk.clone())
        .await
        .expect("Failed to store P2PK signing key");

    // Create a token locked to the stored P2PK public key
    let spending = SpendingConditions::new_p2pk(signing_pk, None);
    let prepared = wallet
        .prepare_send(
            10u64.into(),
            SendOptions {
                conditions: Some(spending),
                include_fee: true,
                ..Default::default()
            },
        )
        .await
        .expect("Failed to prepare send");
    let expected_received = 10u64 - u64::from(prepared.fee());
    let token = prepared
        .confirm(None)
        .await
        .expect("Failed to finalize send");

    // Receive without providing any p2pk_signing_keys in options.
    // This should auto-sign using the key stored in the wallet DB.
    let received = wallet
        .receive(&token.to_string(), ReceiveOptions::default())
        .await
        .expect("Receive should auto-sign and succeed");

    assert_eq!(u64::from(received), expected_received);
}

/// Negative case: receive should fail when no signing key is provided or stored
#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_receive_fails_without_signing_key() {
    setup_tracing();

    // Fresh mint and wallet; do NOT store any P2PK signing keys
    let mint = create_and_start_test_mint()
        .await
        .expect("Failed to create test mint");
    let wallet = create_test_wallet_for_mint(mint)
        .await
        .expect("Failed to create test wallet");

    // Fund wallet and create a token locked to a brand new key (unknown to wallet DB)
    let _ = fund_wallet(wallet.clone(), 100, Some(SplitTarget::default()))
        .await
        .expect("Failed to fund wallet");

    let locking_sk = SecretKey::generate();
    let spending = SpendingConditions::new_p2pk(locking_sk.public_key(), None);

    let prepared = wallet
        .prepare_send(
            10u64.into(),
            SendOptions {
                conditions: Some(spending),
                include_fee: true,
                ..Default::default()
            },
        )
        .await
        .expect("Failed to prepare send");
    let token = prepared
        .confirm(None)
        .await
        .expect("Failed to finalize send");

    // Attempt to receive without providing any keys and none stored → should fail
    let res = wallet
        .receive(&token.to_string(), ReceiveOptions::default())
        .await;

    match res {
        Ok(_) => panic!("Receive unexpectedly succeeded without signing key"),
        Err(e) => match e {
            // Expect signature-related error at the mint
            cdk::Error::NUT11(cdk::nuts::nut11::Error::SignaturesNotProvided) => (),
            other => panic!("Unexpected error: {:?}", other),
        },
    }
}
