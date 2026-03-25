//! P2PK Auto-Sign Integration Tests
//!
//! Tests that stored P2PK signing keys are automatically used when receiving
//! tokens locked to those keys, without requiring manual key provision.

use anyhow::Result;
use cdk::nuts::SpendingConditions;
use cdk::wallet::{ReceiveOptions, SendOptions};
use cdk::Amount;
use cdk_integration_tests::init_pure_tests::*;

/// Tests the full P2PK auto-sign flow:
/// 1. Receiver generates and stores a P2PK key
/// 2. Sender sends tokens locked to that key
/// 3. Receiver receives tokens WITHOUT providing signing keys manually
/// 4. Verifies balance increased (auto-sign worked)
#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_p2pk_autosign_receive() -> Result<()> {
    setup_tracing();
    let mint = create_and_start_test_mint().await?;

    let sender = create_test_wallet_for_mint(mint.clone()).await?;
    let receiver = create_test_wallet_for_mint(mint.clone()).await?;

    // Fund sender
    let fund_amount = 1000_u64;
    fund_wallet(sender.clone(), fund_amount, None).await?;

    // Receiver generates and stores a P2PK key
    let receiver_pubkey = receiver.generate_p2pk_key().await?;

    // Sender sends tokens locked to receiver's pubkey
    let send_amount = Amount::from(500_u64);
    let conditions = SpendingConditions::new_p2pk(receiver_pubkey, None);
    let prepared = sender
        .prepare_send(
            send_amount,
            SendOptions {
                conditions: Some(conditions),
                ..Default::default()
            },
        )
        .await?;
    let token = prepared.confirm(None).await?;

    // Receiver receives WITHOUT providing signing keys — should auto-sign
    let received = receiver
        .receive(&token.to_string(), ReceiveOptions::default())
        .await?;

    assert!(received > Amount::ZERO, "Should have received some amount");

    // Verify receiver balance
    let balance = receiver.total_balance().await?;
    assert!(
        balance > Amount::ZERO,
        "Receiver balance should be positive"
    );

    Ok(())
}

/// Tests that receive fails when token is locked to an unknown key
/// and no signing keys are provided (negative case)
#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_p2pk_receive_fails_without_stored_key() -> Result<()> {
    setup_tracing();
    let mint = create_and_start_test_mint().await?;

    let sender = create_test_wallet_for_mint(mint.clone()).await?;
    let receiver = create_test_wallet_for_mint(mint.clone()).await?;

    // Fund sender
    fund_wallet(sender.clone(), 1000, None).await?;

    // Generate a key but DON'T store it in the receiver wallet
    let secret_key = cdk::nuts::SecretKey::generate();
    let pubkey = secret_key.public_key();

    // Send locked to that key
    let conditions = SpendingConditions::new_p2pk(pubkey, None);
    let prepared = sender
        .prepare_send(
            Amount::from(500_u64),
            SendOptions {
                conditions: Some(conditions),
                ..Default::default()
            },
        )
        .await?;
    let token = prepared.confirm(None).await?;

    // Receiver tries to receive without any signing key — should fail
    let result = receiver
        .receive(&token.to_string(), ReceiveOptions::default())
        .await;

    assert!(result.is_err(), "Should fail without signing key");

    Ok(())
}
