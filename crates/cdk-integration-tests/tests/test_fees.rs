use std::str::FromStr;
use std::sync::Arc;

use anyhow::Result;
use bip39::Mnemonic;
use cashu::{Bolt11Invoice, ProofsMethods};
use cdk::amount::{Amount, SplitTarget};
use cdk::nuts::CurrencyUnit;
use cdk::wallet::{ReceiveOptions, SendKind, SendOptions, Wallet};
use cdk_integration_tests::{
    create_invoice_for_env, get_mint_url_from_env, pay_if_regtest, wait_for_mint_to_be_paid,
};
use cdk_sqlite::wallet::memory;

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_swap() -> Result<()> {
    let seed = Mnemonic::generate(12)?.to_seed_normalized("");
    let wallet = Wallet::new(
        &get_mint_url_from_env(),
        CurrencyUnit::Sat,
        Arc::new(memory::empty().await?),
        &seed,
        None,
    )?;

    let mint_quote = wallet.mint_quote(100.into(), None).await?;

    let invoice = Bolt11Invoice::from_str(&mint_quote.request)?;
    pay_if_regtest(&invoice).await?;

    let _mint_amount = wallet
        .mint(&mint_quote.id, SplitTarget::default(), None)
        .await?;

    let proofs: Vec<Amount> = wallet
        .get_unspent_proofs()
        .await?
        .iter()
        .map(|p| p.amount)
        .collect();

    println!("{:?}", proofs);

    let send = wallet
        .prepare_send(
            4.into(),
            SendOptions {
                send_kind: SendKind::OfflineExact,
                ..Default::default()
            },
        )
        .await?;

    let proofs = send.proofs();

    let fee = wallet.get_proofs_fee(&proofs).await?;

    assert_eq!(fee, 1.into());

    let send = wallet.send(send, None).await?;

    let rec_amount = wallet
        .receive(&send.to_string(), ReceiveOptions::default())
        .await?;

    assert_eq!(rec_amount, 3.into());

    let wallet_balance = wallet.total_balance().await?;

    assert_eq!(wallet_balance, 99.into());

    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_fake_melt_change_in_quote() -> Result<()> {
    let wallet = Wallet::new(
        &get_mint_url_from_env(),
        CurrencyUnit::Sat,
        Arc::new(memory::empty().await?),
        &Mnemonic::generate(12)?.to_seed_normalized(""),
        None,
    )?;

    let mint_quote = wallet.mint_quote(100.into(), None).await?;

    let bolt11 = Bolt11Invoice::from_str(&mint_quote.request)?;

    pay_if_regtest(&bolt11).await?;

    wait_for_mint_to_be_paid(&wallet, &mint_quote.id, 60).await?;

    let _mint_amount = wallet
        .mint(&mint_quote.id, SplitTarget::default(), None)
        .await?;

    let invoice_amount = 9;

    let invoice = create_invoice_for_env(Some(invoice_amount)).await?;

    let melt_quote = wallet.melt_quote(invoice.to_string(), None).await?;

    let proofs = wallet.get_unspent_proofs().await?;

    let proofs_total = proofs.total_amount().unwrap();

    let fee = wallet.get_proofs_fee(&proofs).await?;
    let melt = wallet.melt_proofs(&melt_quote.id, proofs.clone()).await?;
    let change = melt.change.unwrap().total_amount().unwrap();
    let idk = proofs.total_amount()? - Amount::from(invoice_amount) - change;

    println!("{}", idk);
    println!("{}", fee);
    println!("{}", proofs_total);
    println!("{}", change);

    let ln_fee = 1;

    assert_eq!(
        wallet.total_balance().await?,
        Amount::from(100 - invoice_amount - u64::from(fee) - ln_fee)
    );

    Ok(())
}
