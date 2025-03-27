//! Test that are meant to be run agiast the nutshell mint

use std::sync::Arc;

use anyhow::Result;
use bip39::Mnemonic;
use cdk::amount::SplitTarget;
use cdk::nuts::CurrencyUnit;
use cdk::wallet::{SendOptions, Wallet};
use cdk_integration_tests::wait_for_mint_to_be_paid;
use cdk_sqlite::wallet::memory;

const MINT_URL: &str = "http://127.0.0.1:3338";

/// Tests that change outputs in a melt quote are correctly handled
#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_fake_nutshell_mint() -> Result<()> {
    let wallet = Wallet::new(
        MINT_URL,
        CurrencyUnit::Sat,
        Arc::new(memory::empty().await?),
        &Mnemonic::generate(12)?.to_seed_normalized(""),
        None,
    )?;

    let mint_quote = wallet.mint_quote(100.into(), None).await?;

    wait_for_mint_to_be_paid(&wallet, &mint_quote.id, 60).await?;

    let _mint_amount = wallet
        .mint(&mint_quote.id, SplitTarget::default(), None)
        .await?;

    Ok(())
}

/// Tests that change outputs in a melt quote are correctly handled
#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_fake_nutshell_swap() -> Result<()> {
    let wallet = Wallet::new(
        MINT_URL,
        CurrencyUnit::Sat,
        Arc::new(memory::empty().await?),
        &Mnemonic::generate(12)?.to_seed_normalized(""),
        None,
    )?;

    let mint_quote = wallet.mint_quote(100.into(), None).await?;

    wait_for_mint_to_be_paid(&wallet, &mint_quote.id, 60).await?;

    let _mint_amount = wallet
        .mint(&mint_quote.id, SplitTarget::default(), None)
        .await?;

    // Send the token
    let prepared_send = wallet
        .prepare_send(10.into(), SendOptions::default())
        .await?;
    let token = wallet.send(prepared_send, None).await?;

    let r = wallet
        .receive(&token.to_string(), SplitTarget::default(), &[], &[])
        .await?;

    assert!(r == 10.into());

    Ok(())
}
