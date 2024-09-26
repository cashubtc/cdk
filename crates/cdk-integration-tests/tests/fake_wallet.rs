use std::{sync::Arc, time::Duration};

use anyhow::Result;
use bip39::Mnemonic;
use cdk::{
    amount::SplitTarget,
    cdk_database::WalletMemoryDatabase,
    nuts::{CurrencyUnit, MeltQuoteState, State},
    wallet::Wallet,
};
use cdk_fake_wallet::{create_fake_invoice, FakeInvoiceDescription};
use cdk_integration_tests::attempt_to_swap_pending;
use tokio::time::sleep;

const MINT_URL: &str = "http://127.0.0.1:8086";

// If both pay and check return pending input proofs should remain pending
#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_fake_tokens_pending() -> Result<()> {
    let wallet = Wallet::new(
        MINT_URL,
        CurrencyUnit::Sat,
        Arc::new(WalletMemoryDatabase::default()),
        &Mnemonic::generate(12)?.to_seed_normalized(""),
        None,
    )?;

    let mint_quote = wallet.mint_quote(100.into(), None).await?;

    sleep(Duration::from_secs(5)).await;

    let _mint_amount = wallet
        .mint(&mint_quote.id, SplitTarget::default(), None)
        .await?;

    let fake_description = FakeInvoiceDescription {
        pay_invoice_state: MeltQuoteState::Pending,
        check_payment_state: MeltQuoteState::Pending,
        pay_err: false,
        check_err: false,
    };

    let invoice = create_fake_invoice(1000, serde_json::to_string(&fake_description).unwrap());

    let melt_quote = wallet.melt_quote(invoice.to_string(), None).await?;

    let melt = wallet.melt(&melt_quote.id).await;

    assert!(melt.is_err());

    attempt_to_swap_pending(&wallet).await?;

    Ok(())
}

// If the pay error fails and the check returns unknown or failed
// The inputs proofs should be unset as spending
#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_fake_melt_payment_fail() -> Result<()> {
    let wallet = Wallet::new(
        MINT_URL,
        CurrencyUnit::Sat,
        Arc::new(WalletMemoryDatabase::default()),
        &Mnemonic::generate(12)?.to_seed_normalized(""),
        None,
    )?;

    let mint_quote = wallet.mint_quote(100.into(), None).await?;

    let _mint_amount = wallet
        .mint(&mint_quote.id, SplitTarget::default(), None)
        .await?;

    let fake_description = FakeInvoiceDescription {
        pay_invoice_state: MeltQuoteState::Unknown,
        check_payment_state: MeltQuoteState::Unknown,
        pay_err: true,
        check_err: false,
    };

    let invoice = create_fake_invoice(1000, serde_json::to_string(&fake_description).unwrap());

    let melt_quote = wallet.melt_quote(invoice.to_string(), None).await?;

    // The melt should error at the payment invoice command
    let melt = wallet.melt(&melt_quote.id).await;
    assert!(melt.is_err());

    let fake_description = FakeInvoiceDescription {
        pay_invoice_state: MeltQuoteState::Failed,
        check_payment_state: MeltQuoteState::Failed,
        pay_err: true,
        check_err: false,
    };

    let invoice = create_fake_invoice(1000, serde_json::to_string(&fake_description).unwrap());

    let melt_quote = wallet.melt_quote(invoice.to_string(), None).await?;

    // The melt should error at the payment invoice command
    let melt = wallet.melt(&melt_quote.id).await;
    assert!(melt.is_err());

    // The mint should have unset proofs from pending since payment failed
    let all_proof = wallet.get_proofs().await?;
    let states = wallet.check_proofs_spent(all_proof).await?;
    for state in states {
        assert!(state.state == State::Unspent);
    }

    let wallet_bal = wallet.total_balance().await?;
    assert!(wallet_bal == 100.into());

    Ok(())
}

// When both the pay_invoice and check_invoice both fail
// the proofs should remain as pending
#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_fake_melt_payment_fail_and_check() -> Result<()> {
    let wallet = Wallet::new(
        MINT_URL,
        CurrencyUnit::Sat,
        Arc::new(WalletMemoryDatabase::default()),
        &Mnemonic::generate(12)?.to_seed_normalized(""),
        None,
    )?;

    let mint_quote = wallet.mint_quote(100.into(), None).await?;

    let _mint_amount = wallet
        .mint(&mint_quote.id, SplitTarget::default(), None)
        .await?;

    let fake_description = FakeInvoiceDescription {
        pay_invoice_state: MeltQuoteState::Unknown,
        check_payment_state: MeltQuoteState::Unknown,
        pay_err: true,
        check_err: true,
    };

    let invoice = create_fake_invoice(7000, serde_json::to_string(&fake_description).unwrap());

    let melt_quote = wallet.melt_quote(invoice.to_string(), None).await?;

    // The melt should error at the payment invoice command
    let melt = wallet.melt(&melt_quote.id).await;
    assert!(melt.is_err());

    let pending = wallet
        .localstore
        .get_proofs(None, None, Some(vec![State::Pending]), None)
        .await?;

    assert!(!pending.is_empty());

    Ok(())
}

// In the case that the ln backend returns a failed status but does not error
// The mint should do a second check, then remove proofs from pending
#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_fake_melt_payment_return_fail_status() -> Result<()> {
    let wallet = Wallet::new(
        MINT_URL,
        CurrencyUnit::Sat,
        Arc::new(WalletMemoryDatabase::default()),
        &Mnemonic::generate(12)?.to_seed_normalized(""),
        None,
    )?;

    let mint_quote = wallet.mint_quote(100.into(), None).await?;

    let _mint_amount = wallet
        .mint(&mint_quote.id, SplitTarget::default(), None)
        .await?;

    let fake_description = FakeInvoiceDescription {
        pay_invoice_state: MeltQuoteState::Failed,
        check_payment_state: MeltQuoteState::Failed,
        pay_err: false,
        check_err: false,
    };

    let invoice = create_fake_invoice(7000, serde_json::to_string(&fake_description).unwrap());

    let melt_quote = wallet.melt_quote(invoice.to_string(), None).await?;

    // The melt should error at the payment invoice command
    let melt = wallet.melt(&melt_quote.id).await;
    assert!(melt.is_err());

    let fake_description = FakeInvoiceDescription {
        pay_invoice_state: MeltQuoteState::Unknown,
        check_payment_state: MeltQuoteState::Unknown,
        pay_err: false,
        check_err: false,
    };

    let invoice = create_fake_invoice(7000, serde_json::to_string(&fake_description).unwrap());

    let melt_quote = wallet.melt_quote(invoice.to_string(), None).await?;

    // The melt should error at the payment invoice command
    let melt = wallet.melt(&melt_quote.id).await;
    assert!(melt.is_err());

    let pending = wallet
        .localstore
        .get_proofs(None, None, Some(vec![State::Pending]), None)
        .await?;

    assert!(pending.is_empty());

    Ok(())
}

// In the case that the ln backend returns a failed status but does not error
// The mint should do a second check, then remove proofs from pending
#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_fake_melt_payment_error_unknown() -> Result<()> {
    let wallet = Wallet::new(
        MINT_URL,
        CurrencyUnit::Sat,
        Arc::new(WalletMemoryDatabase::default()),
        &Mnemonic::generate(12)?.to_seed_normalized(""),
        None,
    )?;

    let mint_quote = wallet.mint_quote(100.into(), None).await?;

    let _mint_amount = wallet
        .mint(&mint_quote.id, SplitTarget::default(), None)
        .await?;

    let fake_description = FakeInvoiceDescription {
        pay_invoice_state: MeltQuoteState::Failed,
        check_payment_state: MeltQuoteState::Unknown,
        pay_err: true,
        check_err: false,
    };

    let invoice = create_fake_invoice(7000, serde_json::to_string(&fake_description).unwrap());

    let melt_quote = wallet.melt_quote(invoice.to_string(), None).await?;

    // The melt should error at the payment invoice command
    let melt = wallet.melt(&melt_quote.id).await;
    assert!(melt.is_err());

    let fake_description = FakeInvoiceDescription {
        pay_invoice_state: MeltQuoteState::Unknown,
        check_payment_state: MeltQuoteState::Unknown,
        pay_err: true,
        check_err: false,
    };

    let invoice = create_fake_invoice(7000, serde_json::to_string(&fake_description).unwrap());

    let melt_quote = wallet.melt_quote(invoice.to_string(), None).await?;

    // The melt should error at the payment invoice command
    let melt = wallet.melt(&melt_quote.id).await;
    assert!(melt.is_err());

    let pending = wallet
        .localstore
        .get_proofs(None, None, Some(vec![State::Pending]), None)
        .await?;

    assert!(pending.is_empty());

    Ok(())
}

// In the case that the ln backend returns an err
// The mint should do a second check, that returns paid
// Proofs should remain pending
#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_fake_melt_payment_err_paid() -> Result<()> {
    let wallet = Wallet::new(
        MINT_URL,
        CurrencyUnit::Sat,
        Arc::new(WalletMemoryDatabase::default()),
        &Mnemonic::generate(12)?.to_seed_normalized(""),
        None,
    )?;

    let mint_quote = wallet.mint_quote(100.into(), None).await?;

    let _mint_amount = wallet
        .mint(&mint_quote.id, SplitTarget::default(), None)
        .await?;

    let fake_description = FakeInvoiceDescription {
        pay_invoice_state: MeltQuoteState::Failed,
        check_payment_state: MeltQuoteState::Paid,
        pay_err: true,
        check_err: false,
    };

    let invoice = create_fake_invoice(7000, serde_json::to_string(&fake_description).unwrap());

    let melt_quote = wallet.melt_quote(invoice.to_string(), None).await?;

    // The melt should error at the payment invoice command
    let melt = wallet.melt(&melt_quote.id).await;
    assert!(melt.is_err());

    attempt_to_swap_pending(&wallet).await?;

    Ok(())
}
