use std::sync::Arc;

use anyhow::{bail, Result};
use bip39::Mnemonic;
use cashu::amount::SplitTarget;
use cashu::{Amount, CurrencyUnit, MintRequest, PreMintSecrets, ProofsMethods};
use cdk::wallet::{HttpClient, MintConnector, Wallet};
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
        .mint_bolt12(&mint_quote.id, None, SplitTarget::default(), None)
        .await
        .unwrap();

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
        .mint_bolt12(&mint_quote.id, None, SplitTarget::default(), None)
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
        .mint_bolt12(&mint_quote.id, None, SplitTarget::default(), None)
        .await
        .unwrap();

    assert_eq!(proofs.total_amount().unwrap(), 11.into());

    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_regtest_bolt12_melt() -> Result<()> {
    let wallet = Wallet::new(
        &get_mint_url_from_env(),
        CurrencyUnit::Sat,
        Arc::new(memory::empty().await?),
        &Mnemonic::generate(12)?.to_seed_normalized(""),
        None,
    )?;

    wallet.get_mint_info().await?;

    let mint_amount = Amount::from(20_000);

    // Create a single-use BOLT12 quote
    let mint_quote = wallet
        .mint_bolt12_quote(Some(mint_amount), None, true, None)
        .await?;

    assert_eq!(mint_quote.amount, Some(mint_amount));
    // Pay the quote
    let cln_one_dir = get_cln_dir("one");
    let cln_client = ClnClient::new(cln_one_dir.clone(), None).await?;
    cln_client
        .pay_bolt12_offer(None, mint_quote.request.clone())
        .await?;

    // Wait for payment to be processed
    wait_for_mint_to_be_paid(&wallet, &mint_quote.id, 60).await?;

    let offer = cln_client
        .get_bolt12_offer(Some(10_000), true, "hhhhhhhh".to_string())
        .await?;

    let _proofs = wallet
        .mint_bolt12(&mint_quote.id, None, SplitTarget::default(), None)
        .await
        .unwrap();

    let quote = wallet.melt_bolt12_quote(offer.to_string(), None).await?;

    let melt = wallet.melt(&quote.id).await?;

    assert_eq!(melt.amount, 10.into());

    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_regtest_bolt12_mint_extra() -> Result<()> {
    let wallet = Wallet::new(
        &get_mint_url_from_env(),
        CurrencyUnit::Sat,
        Arc::new(memory::empty().await?),
        &Mnemonic::generate(12)?.to_seed_normalized(""),
        None,
    )?;

    wallet.get_mint_info().await?;

    // Create a single-use BOLT12 quote
    let mint_quote = wallet.mint_bolt12_quote(None, None, false, None).await?;

    let state = wallet.mint_bolt12_quote_state(&mint_quote.id).await?;

    assert_eq!(state.amount_paid, Amount::ZERO);
    assert_eq!(state.amount_issued, Amount::ZERO);

    let active_keyset_id = wallet.get_active_mint_keyset().await?.id;

    let pay_amount_msats = 10_000;

    let cln_one_dir = get_cln_dir("one");
    let cln_client = ClnClient::new(cln_one_dir.clone(), None).await?;
    cln_client
        .pay_bolt12_offer(Some(pay_amount_msats), mint_quote.request.clone())
        .await?;

    wait_for_mint_to_be_paid(&wallet, &mint_quote.id, 60).await?;

    let state = wallet.mint_bolt12_quote_state(&mint_quote.id).await?;

    assert_eq!(state.amount_paid, (pay_amount_msats / 1_000).into());
    assert_eq!(state.amount_issued, Amount::ZERO);

    let pre_mint = PreMintSecrets::random(active_keyset_id, 500.into(), &SplitTarget::None)?;

    let quote_info = wallet
        .localstore
        .get_mint_quote(&mint_quote.id)
        .await?
        .expect("there is a quote");

    let mut mint_request = MintRequest {
        quote: mint_quote.id,
        outputs: pre_mint.blinded_messages(),
        signature: None,
    };

    if let Some(secret_key) = quote_info.secret_key {
        mint_request.sign(secret_key)?;
    }

    let http_client = HttpClient::new(get_mint_url_from_env().parse().unwrap(), None);

    let response = http_client.post_mint(mint_request.clone()).await;

    match response {
        Err(err) => match err {
            cdk::Error::TransactionUnbalanced(_, _, _) => (),
            err => {
                bail!("Wrong mint error returned: {}", err.to_string());
            }
        },
        Ok(_) => {
            bail!("Should not have allowed second payment");
        }
    }

    Ok(())
}
