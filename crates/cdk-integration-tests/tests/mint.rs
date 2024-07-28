//! Mint integration tests

use std::sync::Arc;
use std::time::Duration;

use anyhow::{bail, Result};
use bip39::Mnemonic;
use cdk::amount::SplitTarget;
use cdk::cdk_database::WalletMemoryDatabase;
use cdk::nuts::CurrencyUnit;
use cdk::wallet::error::Error;
use cdk::wallet::SendKind;
use cdk::Wallet;
use cdk_integration_tests::{create_backends_fake_wallet, start_mint, wallet_mint, MINT_URL};

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
pub async fn test_mint_double_receive() -> Result<()> {
    tokio::spawn(async move {
        let ln_backends = create_backends_fake_wallet();

        start_mint(ln_backends).await.expect("Could not start mint")
    });

    tokio::time::sleep(Duration::from_millis(500)).await;

    let mnemonic = Mnemonic::generate(12)?;

    let wallet = Wallet::new(
        &MINT_URL,
        CurrencyUnit::Sat,
        Arc::new(WalletMemoryDatabase::default()),
        &mnemonic.to_seed_normalized(""),
        None,
    );

    let wallet = Arc::new(wallet);

    wallet_mint(Arc::clone(&wallet), 100.into()).await?;

    let token = wallet
        .send(
            10.into(),
            None,
            None,
            &SplitTarget::default(),
            &SendKind::default(),
            false,
        )
        .await?;

    let mnemonic = Mnemonic::generate(12)?;

    let wallet_two = Wallet::new(
        &MINT_URL,
        CurrencyUnit::Sat,
        Arc::new(WalletMemoryDatabase::default()),
        &mnemonic.to_seed_normalized(""),
        None,
    );

    let rec = wallet_two
        .receive(&token.to_string(), SplitTarget::default(), &[], &[])
        .await?;
    println!("Received: {}", rec);

    // Attempt to receive again
    if let Err(err) = wallet
        .receive(&token.to_string(), SplitTarget::default(), &[], &[])
        .await
    {
        match err {
            Error::TokenAlreadySpent => (),
            _ => {
                bail!("Expected an already spent error");
            }
        }
    }

    Ok(())
}
