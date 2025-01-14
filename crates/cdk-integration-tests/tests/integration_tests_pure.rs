use std::assert_eq;

use cdk::amount::SplitTarget;
use cdk::nuts::nut00::ProofsMethods;
use cdk::wallet::SendKind;
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

    // Alice wants to send 40 sats, which internally swaps
    let token = wallet_alice
        .send(
            Amount::from(40),
            None,
            None,
            &SplitTarget::None,
            &SendKind::OnlineExact,
            false,
        )
        .await?;
    assert_eq!(Amount::from(40), token.proofs().total_amount()?);
    assert_eq!(Amount::from(24), wallet_alice.total_balance().await?);

    // Alice sends cashu, Carol receives
    let wallet_carol = create_test_wallet_for_mint(mint_bob.clone())?;
    let received_amount = wallet_carol
        .receive_proofs(token.proofs(), SplitTarget::None, &[], &[])
        .await?;

    assert_eq!(Amount::from(40), received_amount);
    assert_eq!(Amount::from(40), wallet_carol.total_balance().await?);

    Ok(())
}
