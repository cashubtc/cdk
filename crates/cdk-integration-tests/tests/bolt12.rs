use std::sync::Arc;

use anyhow::Result;
use bip39::Mnemonic;
use cashu::amount::SplitTarget;
use cashu::{Amount, CurrencyUnit, ProofsMethods};
use cdk::wallet::Wallet;
use cdk_integration_tests::init_regtest::get_cln_dir;
use cdk_integration_tests::{get_mint_url_from_env, wait_for_mint_to_be_paid};
use cdk_sqlite::wallet::memory;
use ln_regtest_rs::ln_client::ClnClient;

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_regtest_bolt12_mint() -> Result<()> {
    let wallet = Wallet::new(
        &get_mint_url_from_env(),
        CurrencyUnit::Sat,
        Arc::new(memory::empty().await?),
        &Mnemonic::generate(12)?.to_seed_normalized(""),
        None,
    )?;

    let mint_amount = Amount::from(100);

    let mint_quote = wallet
        .mint_bolt12_quote(Some(mint_amount), None, true, None)
        .await?;

    assert_eq!(mint_quote.amount, Some(mint_amount));

    let cln_one_dir = get_cln_dir("one");
    let cln_client = ClnClient::new(cln_one_dir.clone(), None).await?;
    cln_client
        .pay_bolt12_offer(None, mint_quote.request)
        .await?;

    let proofs = wallet
        .mint(&mint_quote.id, SplitTarget::default(), None)
        .await?;

    assert_eq!(proofs.total_amount().unwrap(), 100.into());

    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_regtest_bolt12_mint_multiple() -> Result<()> {
    let wallet = Wallet::new(
        &get_mint_url_from_env(),
        CurrencyUnit::Sat,
        Arc::new(memory::empty().await?),
        &Mnemonic::generate(12)?.to_seed_normalized(""),
        None,
    )?;

    let mint_quote = wallet.mint_bolt12_quote(None, None, false, None).await?;

    let cln_one_dir = get_cln_dir("one");
    let cln_client = ClnClient::new(cln_one_dir.clone(), None).await?;
    cln_client
        .pay_bolt12_offer(Some(10000), mint_quote.request.clone())
        .await
        .unwrap();

    wait_for_mint_to_be_paid(&wallet, &mint_quote.id, 60).await?;

    wallet.mint_bolt12_quote_state(&mint_quote.id).await?;

    let proofs = wallet
        .mint(&mint_quote.id, SplitTarget::default(), None)
        .await
        .unwrap();

    assert_eq!(proofs.total_amount().unwrap(), 10.into());

    cln_client
        .pay_bolt12_offer(Some(11_000), mint_quote.request)
        .await
        .unwrap();

    wait_for_mint_to_be_paid(&wallet, &mint_quote.id, 60).await?;

    wallet.mint_bolt12_quote_state(&mint_quote.id).await?;

    let proofs = wallet
        .mint(&mint_quote.id, SplitTarget::default(), None)
        .await
        .unwrap();

    assert_eq!(proofs.total_amount().unwrap(), 11.into());

    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_regtest_bolt12_attempt_doube_pay_single() -> Result<()> {
    Ok(())
}
