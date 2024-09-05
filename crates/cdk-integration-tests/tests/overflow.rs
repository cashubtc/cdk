use std::time::Duration;

use anyhow::{bail, Result};
use cdk::amount::SplitTarget;
use cdk::dhke::construct_proofs;
use cdk::nuts::{CurrencyUnit, MintQuoteState, PreMintSecrets, SwapRequest};
use cdk::Amount;
use cdk::HttpClient;
use cdk_integration_tests::{create_backends_fake_wallet, start_mint, MINT_URL};
use tokio::time::sleep;

/// This attempts to swap for more outputs then inputs.
/// This will work if the mint does not check for outputs amounts overflowing
async fn attempt_to_swap_by_overflowing() -> Result<()> {
    let wallet_client = HttpClient::new();

    let mint_keys = wallet_client.get_mint_keys(MINT_URL.parse()?).await?;

    let mint_keys = mint_keys.first().unwrap();

    let keyset_id = mint_keys.id;

    let mint_quote = wallet_client
        .post_mint_quote(MINT_URL.parse()?, 100.into(), CurrencyUnit::Sat)
        .await?;

    loop {
        let status = wallet_client
            .get_mint_quote_status(MINT_URL.parse()?, &mint_quote.quote)
            .await?;

        if status.state == MintQuoteState::Paid {
            break;
        }
        println!("{:?}", status);

        sleep(Duration::from_secs(2)).await;
    }

    let premint_secrets = PreMintSecrets::random(keyset_id, 1.into(), &SplitTarget::default())?;

    let mint_response = wallet_client
        .post_mint(
            MINT_URL.parse()?,
            &mint_quote.quote,
            premint_secrets.clone(),
        )
        .await?;

    let pre_swap_proofs = construct_proofs(
        mint_response.signatures,
        premint_secrets.rs(),
        premint_secrets.secrets(),
        &mint_keys.clone().keys,
    )?;

    // Construct messages that will overflow

    let amount = 2_u64.pow(63);

    let pre_mint_amount =
        PreMintSecrets::random(keyset_id, amount.into(), &SplitTarget::default())?;
    let pre_mint_amount_two =
        PreMintSecrets::random(keyset_id, amount.into(), &SplitTarget::default())?;

    let mut pre_mint = PreMintSecrets::random(keyset_id, 1.into(), &SplitTarget::default())?;

    pre_mint.combine(pre_mint_amount);
    pre_mint.combine(pre_mint_amount_two);

    let swap_request = SwapRequest::new(pre_swap_proofs.clone(), pre_mint.blinded_messages());

    let swap_response = match wallet_client
        .post_swap(MINT_URL.parse()?, swap_request)
        .await
    {
        Ok(res) => res,
        // In the context of this test an error response here is good.
        // It means the mint does not allow us to swap for more then we should by overflowing
        Err(_err) => return Ok(()),
    };

    let post_swap_proofs = construct_proofs(
        swap_response.signatures,
        pre_mint.rs(),
        pre_mint.secrets(),
        &mint_keys.clone().keys,
    )?;

    println!(
        "Pre swap amount: {:?}",
        Amount::try_sum(pre_swap_proofs.iter().map(|p| p.amount)).expect("Amount overflowed")
    );
    println!(
        "Post swap amount: {:?}",
        Amount::try_sum(post_swap_proofs.iter().map(|p| p.amount)).expect("Amount Overflowed")
    );

    println!(
        "Pre swap amounts: {:?}",
        pre_swap_proofs
            .iter()
            .map(|p| p.amount)
            .collect::<Vec<Amount>>()
    );
    println!(
        "Post swap amounts: {:?}",
        post_swap_proofs
            .iter()
            .map(|p| p.amount)
            .collect::<Vec<Amount>>()
    );

    bail!("Should not have been able to swap")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
pub async fn test_overflow() -> Result<()> {
    tokio::spawn(async move {
        let ln_backends = create_backends_fake_wallet();

        start_mint(ln_backends).await.expect("Could not start mint")
    });

    // Wait for mint server to start
    tokio::time::sleep(Duration::from_millis(500)).await;

    let result = attempt_to_swap_by_overflowing().await;

    assert!(result.is_ok());

    Ok(())
}
