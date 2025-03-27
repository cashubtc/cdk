use std::sync::Arc;

use anyhow::{bail, Result};
use bip39::Mnemonic;
use cashu::kvac::KvacCoin;
use cashu::{Amount, CurrencyUnit};
use cdk::nuts::{MeltQuoteState, State};
use cdk::wallet::Wallet;
use cdk_fake_wallet::{create_fake_invoice, FakeInvoiceDescription};
use cdk_integration_tests::wait_for_mint_to_be_paid;
use cdk_sqlite::wallet::memory;

// Get all pending from wallet and attempt to swap
// Will panic if there are no pending
// Will return Ok if swap fails as expected
pub async fn attempt_to_swap_kvac_pending(wallet: &Wallet) -> Result<()> {
    let pending = wallet
        .localstore
        .get_kvac_coins(None, None, Some(vec![State::Pending]), None)
        .await?;

    let pending_coins: Vec<KvacCoin> = pending.into_iter().map(|p| p.coin).collect();
    let amount = pending_coins.iter().fold(Amount::ZERO, |acc, c| acc + c.amount);
    let outputs = wallet.create_kvac_outputs(vec![Amount::ZERO, amount]).await?;

    assert!(!pending_coins.is_empty());

    let swap = wallet
        .kvac_swap(
            &pending_coins,
            &outputs,
        )
        .await;

    match swap {
        Ok(_swap) => {
            bail!("These proofs should be pending")
        }
        Err(err) => match err {
            cdk::error::Error::TokenPending => (),
            _ => {
                println!("{:?}", err);
                bail!("Wrong error")
            }
        },
    }

    Ok(())
}

const MINT_URL: &str = "http://127.0.0.1:8086";

// If both pay and check return pending input proofs should remain pending
#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_fake_coins_pending() -> Result<()> {
    let wallet = Wallet::new(
        MINT_URL,
        CurrencyUnit::Sat,
        Arc::new(memory::empty().await?),
        &Mnemonic::generate(12)?.to_seed_normalized(""),
        None,
    )?;

    let mint_quote = wallet.mint_quote(100.into(), None).await?;

    wait_for_mint_to_be_paid(&wallet, &mint_quote.id, 60).await?;

    let _coins = wallet
        .kvac_mint(&mint_quote.id, 100.into())
        .await?;

    let fake_description = FakeInvoiceDescription {
        pay_invoice_state: MeltQuoteState::Pending,
        check_payment_state: MeltQuoteState::Pending,
        pay_err: false,
        check_err: false,
    };

    let invoice = create_fake_invoice(1000, serde_json::to_string(&fake_description).unwrap());

    let melt_quote = wallet.melt_quote(invoice.to_string(), None).await?;

    let melt = wallet.kvac_melt(&melt_quote.id).await;

    assert!(melt.is_err());

    attempt_to_swap_kvac_pending(&wallet).await?;

    Ok(())
}