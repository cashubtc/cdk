use std::assert_eq;
use std::collections::HashSet;
use std::hash::RandomState;

use cdk::amount::SplitTarget;
use cdk::nuts::nut00::ProofsMethods;
use cdk::wallet::SendOptions;
use cdk::Amount;
use cdk_integration_tests::init_pure_tests::*;

#[tokio::test]
async fn test_swap_to_send() -> anyhow::Result<()> {
    setup_tracing();
    let mint_bob = create_and_start_test_mint().await?;
    let wallet_alice = create_test_wallet_arc_for_mint(mint_bob.clone()).await?;

    // Alice gets 64 sats
    fund_wallet(wallet_alice.clone(), 64).await?;
    let balance_alice = wallet_alice.total_balance().await?;
    assert_eq!(Amount::from(64), balance_alice);

    // Alice wants to send 40 sats, which internally swaps
    let prepared_send = wallet_alice
        .prepare_send(Amount::from(40), SendOptions::default())
        .await?;
    assert_eq!(
        HashSet::<_, RandomState>::from_iter(prepared_send.proofs().ys()?),
        HashSet::from_iter(wallet_alice.get_reserved_proofs().await?.ys()?)
    );
    let token = wallet_alice.send(prepared_send, None).await?;
    assert_eq!(Amount::from(40), token.proofs().total_amount()?);
    assert_eq!(Amount::from(24), wallet_alice.total_balance().await?);
    assert_eq!(
        HashSet::<_, RandomState>::from_iter(token.proofs().ys()?),
        HashSet::from_iter(wallet_alice.get_pending_spent_proofs().await?.ys()?)
    );

    // Alice sends cashu, Carol receives
    let wallet_carol = create_test_wallet_arc_for_mint(mint_bob.clone()).await?;
    let received_amount = wallet_carol
        .receive_proofs(token.proofs(), SplitTarget::None, &[], &[])
        .await?;

    assert_eq!(Amount::from(40), received_amount);
    assert_eq!(Amount::from(40), wallet_carol.total_balance().await?);

    Ok(())
}

/// Pure integration tests related to NUT-06 (Mint Information)
mod nut06 {
    use std::str::FromStr;
    use std::sync::Arc;

    use anyhow::Result;
    use cashu::mint_url::MintUrl;
    use cashu::Amount;
    use cdk_integration_tests::init_pure_tests::*;

    #[tokio::test]
    async fn test_swap_to_send() -> Result<()> {
        setup_tracing();
        let mint_bob = create_and_start_test_mint().await?;
        let wallet_alice_guard = create_test_wallet_arc_mut_for_mint(mint_bob.clone()).await?;
        let mut wallet_alice = wallet_alice_guard.lock().await;

        // Alice gets 64 sats
        fund_wallet(Arc::new(wallet_alice.clone()), 64).await?;
        let balance_alice = wallet_alice.total_balance().await?;
        assert_eq!(Amount::from(64), balance_alice);

        let initial_mint_url = wallet_alice.mint_url.clone();
        let mint_info_before = wallet_alice.get_mint_info().await?.unwrap();
        assert!(mint_info_before
            .urls
            .unwrap()
            .contains(&initial_mint_url.to_string()));

        // Wallet updates mint URL
        let new_mint_url = MintUrl::from_str("https://new-mint-url")?;
        wallet_alice.update_mint_url(new_mint_url.clone()).await?;

        // Check balance after mint URL was updated
        let balance_alice_after = wallet_alice.total_balance().await?;
        assert_eq!(Amount::from(64), balance_alice_after);

        Ok(())
    }
}
