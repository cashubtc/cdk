//! Fee tests for over and underpaying

use std::collections::HashMap;
use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{bail, Result};
use bip39::Mnemonic;
use cdk::cdk_database::WalletMemoryDatabase;
use cdk::mint_url::MintUrl;
use cdk::nuts::{CurrencyUnit, SwapRequest};
use cdk::wallet::client::HttpClient;
use cdk::Wallet;
use cdk::{amount::SplitTarget, nuts::PreMintSecrets};
use cdk_integration_tests::{create_backends_fake_wallet, start_mint, wallet_mint, MINT_URL};

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
pub async fn test_swap_overpay_underpay() -> Result<()> {
    tokio::spawn(async move {
        let ln_backends = create_backends_fake_wallet();

        let mut supported_units = HashMap::new();
        supported_units.insert(CurrencyUnit::Sat, (1, 32));

        start_mint(ln_backends, supported_units)
            .await
            .expect("Could not start mint")
    });

    tokio::time::sleep(Duration::from_millis(500)).await;

    let mnemonic = Mnemonic::generate(12)?;

    let wallet = Wallet::new(
        MINT_URL,
        CurrencyUnit::Sat,
        Arc::new(WalletMemoryDatabase::default()),
        &mnemonic.to_seed_normalized(""),
        None,
    )?;

    let wallet = Arc::new(wallet);

    wallet_mint(
        Arc::clone(&wallet),
        10000.into(),
        SplitTarget::Value(1.into()),
    )
    .await
    .unwrap();
    println!("Minted");

    let proofs = wallet
        .localstore
        .get_proofs(Some(MintUrl::from_str(MINT_URL)?), None, None, None)
        .await?;

    let proofs: Vec<cdk::nuts::Proof> = proofs.into_iter().map(|p| p.proof).collect();

    let keyset_id = proofs.first().unwrap().keyset_id;

    let pre_swap_proofs = proofs[..1000].to_vec();

    // Attempt to swap while overpaying fee

    let pre_swap_secret = PreMintSecrets::random(keyset_id, 450.into(), &SplitTarget::default())?;

    let swap_request =
        SwapRequest::new(pre_swap_proofs.clone(), pre_swap_secret.blinded_messages());

    let wallet_client = HttpClient::new();

    match wallet_client
        .post_swap(MINT_URL.parse()?, swap_request)
        .await
    {
        Ok(_res) => {
            bail!("Swap should not have succeeded");
        }
        Err(err) => match err {
            cdk::error::Error::TransactionUnbalanced(_, _, _) => (),
            _ => {
                println!("{:?}", err);
                bail!("Swap returned the wrong error when overpaying fee");
            }
        },
    };

    // Attempt to swap while underpaying fee

    let pre_swap_secret = PreMintSecrets::random(keyset_id, 1000.into(), &SplitTarget::default())?;
    let swap_request =
        SwapRequest::new(pre_swap_proofs.clone(), pre_swap_secret.blinded_messages());
    match wallet_client
        .post_swap(MINT_URL.parse()?, swap_request)
        .await
    {
        Ok(_res) => {
            bail!("Swap should not have succeeded");
        }
        // In the context of this test an error response here is good.
        // It means the mint does not allow us to swap for more then we should by overflowing
        Err(err) => match err {
            cdk::error::Error::TransactionUnbalanced(_, _, _) => (),
            _ => {
                println!("{:?}", err);
                bail!("Swap returned the wrong error when underpaying fee");
            }
        },
    };
    Ok(())
}
