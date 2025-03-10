//! Tests where we expect the payment processor to respond with an error or pass

use std::env;
use std::sync::Arc;

use anyhow::{bail, Result};
use bip39::Mnemonic;
use cdk::amount::{Amount, SplitTarget};
use cdk::nuts::nut00::ProofsMethods;
use cdk::nuts::CurrencyUnit;
use cdk::wallet::Wallet;
use cdk_fake_wallet::create_fake_invoice;
use cdk_integration_tests::init_regtest::{get_lnd_dir, get_mint_url, LND_RPC_ADDR};
use cdk_integration_tests::wait_for_mint_to_be_paid;
use cdk_sqlite::wallet::memory;
use ln_regtest_rs::ln_client::{LightningClient, LndClient};

// This is the ln wallet we use to send/receive ln payements as the wallet
async fn init_lnd_client() -> LndClient {
    let lnd_dir = get_lnd_dir("one");
    let cert_file = lnd_dir.join("tls.cert");
    let macaroon_file = lnd_dir.join("data/chain/bitcoin/regtest/admin.macaroon");
    LndClient::new(
        format!("https://{}", LND_RPC_ADDR),
        cert_file,
        macaroon_file,
    )
    .await
    .unwrap()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_regtest_mint() -> Result<()> {
    let wallet = Wallet::new(
        &get_mint_url("0"),
        CurrencyUnit::Sat,
        Arc::new(memory::empty().await?),
        &Mnemonic::generate(12)?.to_seed_normalized(""),
        None,
    )?;

    let mint_amount = Amount::from(100);

    let mint_quote = wallet.mint_quote(mint_amount, None).await?;

    assert_eq!(mint_quote.amount, mint_amount);

    let ln_backend = env::var("LN_BACKEND")?;

    if ln_backend != "FAKEWALLET" {
        let lnd_client = init_lnd_client().await;

        lnd_client.pay_invoice(mint_quote.request).await?;
    }

    wait_for_mint_to_be_paid(&wallet, &mint_quote.id, 60).await?;

    let proofs = wallet
        .mint(&mint_quote.id, SplitTarget::default(), None)
        .await?;

    let mint_amount = proofs.total_amount()?;

    assert!(mint_amount == 100.into());

    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_regtest_mint_melt() -> Result<()> {
    let wallet = Wallet::new(
        &get_mint_url("0"),
        CurrencyUnit::Sat,
        Arc::new(memory::empty().await?),
        &Mnemonic::generate(12)?.to_seed_normalized(""),
        None,
    )?;

    let mint_amount = Amount::from(100);

    let mint_quote = wallet.mint_quote(mint_amount, None).await?;

    assert_eq!(mint_quote.amount, mint_amount);

    let ln_backend = env::var("LN_BACKEND")?;
    if ln_backend != "FAKEWALLET" {
        let lnd_client = init_lnd_client().await;

        lnd_client.pay_invoice(mint_quote.request).await?;
    }

    wait_for_mint_to_be_paid(&wallet, &mint_quote.id, 60).await?;

    let proofs = wallet
        .mint(&mint_quote.id, SplitTarget::default(), None)
        .await?;

    let mint_amount = proofs.total_amount()?;

    assert!(mint_amount == 100.into());

    let invoice = if ln_backend != "FAKEWALLET" {
        let lnd_client = init_lnd_client().await;
        lnd_client.create_invoice(Some(50)).await?
    } else {
        create_fake_invoice(50000, "".to_string()).to_string()
    };

    let melt_quote = wallet.melt_quote(invoice, None).await?;

    wallet.melt(&melt_quote.id).await?;

    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_pay_invoice_twice() -> Result<()> {
    let ln_backend = env::var("LN_BACKEND")?;
    if ln_backend == "FAKEWALLET" {
        // We can only preform this test on regtest backends as fake wallet just marks the quote as paid
        return Ok(());
    }

    let seed = Mnemonic::generate(12)?.to_seed_normalized("");
    let wallet = Wallet::new(
        &get_mint_url("0"),
        CurrencyUnit::Sat,
        Arc::new(memory::empty().await?),
        &seed,
        None,
    )?;

    let mint_quote = wallet.mint_quote(100.into(), None).await?;

    let lnd_client = init_lnd_client().await;

    lnd_client.pay_invoice(mint_quote.request).await?;

    wait_for_mint_to_be_paid(&wallet, &mint_quote.id, 60).await?;

    let proofs = wallet
        .mint(&mint_quote.id, SplitTarget::default(), None)
        .await?;

    let mint_amount = proofs.total_amount()?;

    assert_eq!(mint_amount, 100.into());

    let invoice = lnd_client.create_invoice(Some(25)).await?;

    let melt_quote = wallet.melt_quote(invoice.clone(), None).await?;

    let melt = wallet.melt(&melt_quote.id).await.unwrap();

    let melt_two = wallet.melt_quote(invoice, None).await?;

    let melt_two = wallet.melt(&melt_two.id).await;

    match melt_two {
        Err(err) => match err {
            cdk::Error::RequestAlreadyPaid => (),
            err => {
                bail!("Wrong invoice already paid: {}", err.to_string());
            }
        },
        Ok(_) => {
            bail!("Should not have allowed second payment");
        }
    }

    let balance = wallet.total_balance().await?;

    assert_eq!(balance, (Amount::from(100) - melt.fee_paid - melt.amount));

    Ok(())
}
