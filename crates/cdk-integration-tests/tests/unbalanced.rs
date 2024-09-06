//! Test that if a wallet attempts to swap for less outputs then inputs correct error is returned

use std::time::Duration;

use anyhow::{bail, Result};
use cdk::amount::SplitTarget;
use cdk::nuts::{PreMintSecrets, SwapRequest};
use cdk::Error;
use cdk::HttpClient;
use cdk_integration_tests::{create_backends_fake_wallet, mint_proofs, start_mint, MINT_URL};

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
pub async fn test_unbalanced_swap() -> Result<()> {
    tokio::spawn(async move {
        let ln_backends = create_backends_fake_wallet();

        start_mint(ln_backends).await.expect("Could not start mint")
    });

    // Wait for mint server to start
    tokio::time::sleep(Duration::from_millis(500)).await;

    let wallet_client = HttpClient::new();
    let mint_keys = wallet_client.get_mint_keys(MINT_URL.parse()?).await?;

    let mint_keys = mint_keys.first().unwrap();

    let keyset_id = mint_keys.id;

    let pre_swap_proofs = mint_proofs(MINT_URL, 10.into(), keyset_id, mint_keys).await?;

    let pre_mint = PreMintSecrets::random(keyset_id, 9.into(), &SplitTarget::default())?;

    let swap_request = SwapRequest::new(pre_swap_proofs.clone(), pre_mint.blinded_messages());

    let _swap_response = match wallet_client
        .post_swap(MINT_URL.parse()?, swap_request)
        .await
    {
        Ok(res) => res,
        // In the context of this test an error response here is good.
        // It means the mint does not allow us to swap for more then we should by overflowing
        Err(err) => match err {
            Error::TransactionUnbalanced(_, _, _) => {
                return Ok(());
            }
            _ => {
                println!("{}", err);
                bail!("Wrong error code returned");
            }
        },
    };

    bail!("Transaction should not have succeeded")
}
