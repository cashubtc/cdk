use std::assert_eq;

use cdk::cdk_database::TransactionDirection;
use cdk::nuts::nut00::ProofsMethods;
use cdk::wallet::ReceiveOptions;
use cdk::wallet::SendOptions;
use cdk::Amount;
use cdk_integration_tests::init_pure_tests::{
    create_and_start_test_mint, create_test_wallet_for_mint, fund_wallet,
};

#[tokio::test]
async fn test_swap_to_send() -> anyhow::Result<()> {
    let mint_bob = create_and_start_test_mint().await?;
    let wallet_alice = create_test_wallet_for_mint(mint_bob.clone())?;

    // Alice gets 64 sats
    fund_wallet(wallet_alice.clone(), 64).await?;
    let balance_alice = wallet_alice.total_balance().await?;
    assert_eq!(Amount::from(64), balance_alice);
    let alice_txs = wallet_alice
        .transaction_db
        .list_transactions(None, None, None, None)
        .await?;
    assert_eq!(1, alice_txs.len());
    let alice_tx = alice_txs
        .first()
        .ok_or(anyhow::anyhow!("No transaction found"))?;
    assert_eq!(Amount::from(64), alice_tx.amount);
    assert_eq!(TransactionDirection::Incoming, alice_tx.direction);
    assert_eq!(Amount::ZERO, alice_tx.fee);
    assert_eq!(wallet_alice.mint_url, alice_tx.mint_url);

    // Alice wants to send 40 sats, which internally swaps
    let token = wallet_alice
        .send(Amount::from(40), SendOptions::default())
        .await?;
    assert_eq!(Amount::from(40), token.proofs().total_amount()?);
    assert_eq!(Amount::from(24), wallet_alice.total_balance().await?);

    // Alice sends cashu, Carol receives
    let wallet_carol = create_test_wallet_for_mint(mint_bob.clone())?;
    let received_amount = wallet_carol
        .receive_proofs(token.proofs(), ReceiveOptions::default())
        .await?;

    assert_eq!(Amount::from(40), received_amount);
    assert_eq!(Amount::from(40), wallet_carol.total_balance().await?);
    let carol_txs = wallet_carol
        .transaction_db
        .list_transactions(None, None, None, None)
        .await?;
    assert_eq!(1, carol_txs.len());
    let carol_tx = carol_txs
        .first()
        .ok_or(anyhow::anyhow!("No transaction found"))?;
    assert_eq!(Amount::from(40), carol_tx.amount);
    assert_eq!(TransactionDirection::Incoming, carol_tx.direction);
    assert_eq!(Amount::ZERO, carol_tx.fee);
    assert_eq!(wallet_carol.mint_url, carol_tx.mint_url);

    Ok(())
}
