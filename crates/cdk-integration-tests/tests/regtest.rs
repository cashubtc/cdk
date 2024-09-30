use std::{str::FromStr, sync::Arc, time::Duration};

use anyhow::{bail, Result};
use bip39::Mnemonic;
use cdk::{
    amount::{Amount, SplitTarget},
    cdk_database::WalletMemoryDatabase,
    nuts::{CurrencyUnit, MeltQuoteState, MintQuoteState, PreMintSecrets, State},
    wallet::{client::HttpClient, Wallet},
};
use cdk_integration_tests::init_regtest::{get_mint_url, init_cln_client, init_lnd_client};
use lightning_invoice::Bolt11Invoice;
use ln_regtest_rs::InvoiceStatus;
use tokio::time::sleep;

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_regtest_mint_melt_round_trip() -> Result<()> {
    let lnd_client = init_lnd_client().await.unwrap();

    let wallet = Wallet::new(
        &get_mint_url(),
        CurrencyUnit::Sat,
        Arc::new(WalletMemoryDatabase::default()),
        &Mnemonic::generate(12)?.to_seed_normalized(""),
        None,
    )?;

    let mint_quote = wallet.mint_quote(100.into(), None).await?;

    lnd_client.pay_invoice(mint_quote.request).await?;

    let mint_amount = wallet
        .mint(&mint_quote.id, SplitTarget::default(), None)
        .await?;

    assert!(mint_amount == 100.into());

    let invoice = lnd_client.create_invoice(50).await?;

    let melt = wallet.melt_quote(invoice, None).await?;

    let melt = wallet.melt(&melt.id).await.unwrap();

    assert!(melt.preimage.is_some());

    assert!(melt.state == MeltQuoteState::Paid);

    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_regtest_mint_melt() -> Result<()> {
    let lnd_client = init_lnd_client().await?;

    let wallet = Wallet::new(
        &get_mint_url(),
        CurrencyUnit::Sat,
        Arc::new(WalletMemoryDatabase::default()),
        &Mnemonic::generate(12)?.to_seed_normalized(""),
        None,
    )?;

    let mint_amount = Amount::from(100);

    let mint_quote = wallet.mint_quote(mint_amount, None).await?;

    assert_eq!(mint_quote.amount, mint_amount);

    lnd_client.pay_invoice(mint_quote.request).await?;

    let mint_amount = wallet
        .mint(&mint_quote.id, SplitTarget::default(), None)
        .await?;

    assert!(mint_amount == 100.into());

    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_restore() -> Result<()> {
    let lnd_client = init_lnd_client().await?;

    let seed = Mnemonic::generate(12)?.to_seed_normalized("");
    let wallet = Wallet::new(
        &get_mint_url(),
        CurrencyUnit::Sat,
        Arc::new(WalletMemoryDatabase::default()),
        &seed,
        None,
    )?;

    let mint_quote = wallet.mint_quote(100.into(), None).await?;

    lnd_client.pay_invoice(mint_quote.request).await?;

    let _mint_amount = wallet
        .mint(&mint_quote.id, SplitTarget::default(), None)
        .await?;

    assert!(wallet.total_balance().await? == 100.into());

    let wallet_2 = Wallet::new(
        &get_mint_url(),
        CurrencyUnit::Sat,
        Arc::new(WalletMemoryDatabase::default()),
        &seed,
        None,
    )?;

    assert!(wallet_2.total_balance().await? == 0.into());

    let restored = wallet_2.restore().await?;
    let proofs = wallet_2.get_proofs().await?;

    wallet_2
        .swap(None, SplitTarget::default(), proofs, None, false)
        .await?;

    assert!(restored == 100.into());

    assert!(wallet_2.total_balance().await? == 100.into());

    let proofs = wallet.get_proofs().await?;

    let states = wallet.check_proofs_spent(proofs).await?;

    for state in states {
        if state.state != State::Spent {
            bail!("All proofs should be spent");
        }
    }

    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_pay_invoice_twice() -> Result<()> {
    let lnd_client = init_lnd_client().await?;
    let seed = Mnemonic::generate(12)?.to_seed_normalized("");
    let wallet = Wallet::new(
        &get_mint_url(),
        CurrencyUnit::Sat,
        Arc::new(WalletMemoryDatabase::default()),
        &seed,
        None,
    )?;

    let mint_quote = wallet.mint_quote(100.into(), None).await?;

    lnd_client.pay_invoice(mint_quote.request).await?;

    let mint_amount = wallet
        .mint(&mint_quote.id, SplitTarget::default(), None)
        .await?;

    assert_eq!(mint_amount, 100.into());

    let invoice = lnd_client.create_invoice(10).await?;

    let melt_quote = wallet.melt_quote(invoice.clone(), None).await?;

    let melt = wallet.melt(&melt_quote.id).await.unwrap();

    let melt_two = wallet.melt_quote(invoice, None).await?;

    let melt_two = wallet.melt(&melt_two.id).await;

    match melt_two {
        Err(err) => match err {
            cdk::Error::RequestAlreadyPaid => (),
            _ => {
                bail!("Wrong invoice already paid");
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

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_internal_payment() -> Result<()> {
    let lnd_client = init_lnd_client().await?;

    let seed = Mnemonic::generate(12)?.to_seed_normalized("");
    let wallet = Wallet::new(
        &get_mint_url(),
        CurrencyUnit::Sat,
        Arc::new(WalletMemoryDatabase::default()),
        &seed,
        None,
    )?;

    let mint_quote = wallet.mint_quote(100.into(), None).await?;

    lnd_client.pay_invoice(mint_quote.request).await?;

    let _mint_amount = wallet
        .mint(&mint_quote.id, SplitTarget::default(), None)
        .await?;

    assert!(wallet.total_balance().await? == 100.into());

    let seed = Mnemonic::generate(12)?.to_seed_normalized("");

    let wallet_2 = Wallet::new(
        &get_mint_url(),
        CurrencyUnit::Sat,
        Arc::new(WalletMemoryDatabase::default()),
        &seed,
        None,
    )?;

    let mint_quote = wallet_2.mint_quote(10.into(), None).await?;

    let melt = wallet.melt_quote(mint_quote.request.clone(), None).await?;

    assert_eq!(melt.amount, 10.into());

    let _melted = wallet.melt(&melt.id).await.unwrap();

    let _wallet_2_mint = wallet_2
        .mint(&mint_quote.id, SplitTarget::default(), None)
        .await
        .unwrap();

    let cln_client = init_cln_client().await?;
    let payment_hash = Bolt11Invoice::from_str(&mint_quote.request)?;
    let check_paid = cln_client
        .check_incoming_invoice(payment_hash.payment_hash().to_string())
        .await?;

    match check_paid {
        InvoiceStatus::Unpaid => (),
        _ => {
            bail!("Invoice has incorrect status: {:?}", check_paid);
        }
    }

    let wallet_2_balance = wallet_2.total_balance().await?;

    assert!(wallet_2_balance == 10.into());

    let wallet_1_balance = wallet.total_balance().await?;

    assert!(wallet_1_balance == 90.into());

    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_cached_mint() -> Result<()> {
    let lnd_client = init_lnd_client().await.unwrap();

    let wallet = Wallet::new(
        &get_mint_url(),
        CurrencyUnit::Sat,
        Arc::new(WalletMemoryDatabase::default()),
        &Mnemonic::generate(12)?.to_seed_normalized(""),
        None,
    )?;

    let mint_amount = Amount::from(100);

    let quote = wallet.mint_quote(mint_amount, None).await?;
    lnd_client.pay_invoice(quote.request).await?;

    loop {
        let status = wallet.mint_quote_state(&quote.id).await.unwrap();

        println!("Quote status: {}", status.state);

        if status.state == MintQuoteState::Paid {
            break;
        }

        sleep(Duration::from_secs(5)).await;
    }

    let active_keyset_id = wallet.get_active_mint_keyset().await?.id;
    let http_client = HttpClient::new();
    let premint_secrets =
        PreMintSecrets::random(active_keyset_id, 31.into(), &SplitTarget::default()).unwrap();

    let response = http_client
        .post_mint(
            get_mint_url().as_str().parse()?,
            &quote.id,
            premint_secrets.clone(),
        )
        .await?;
    let response1 = http_client
        .post_mint(get_mint_url().as_str().parse()?, &quote.id, premint_secrets)
        .await?;

    assert!(response == response1);
    Ok(())
}
