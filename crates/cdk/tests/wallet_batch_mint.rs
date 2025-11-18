use std::sync::Arc;

use cdk::amount::{Amount, SplitTarget};
use cdk::util::unix_time;
use cdk::wallet::Wallet;
use cdk_common::{MintQuoteState, PaymentMethod};
use cdk_sqlite::wallet::memory;

// Validation tests for batch minting - detailed integration tests will be in cdk-integration-tests
#[tokio::test]
async fn test_wallet_batch_mint_validates_same_unit() -> anyhow::Result<()> {
    let seed = rand::random::<[u8; 64]>();
    let mint_url = "https://fake.thesimplekid.dev";
    let unit = cdk::nuts::CurrencyUnit::Sat;

    let localstore = memory::empty().await?;
    let wallet = Wallet::new(mint_url, unit.clone(), Arc::new(localstore), seed, None)?;

    let quote1 = store_test_quote(
        &wallet,
        mint_url,
        "quote1",
        Amount::from(100),
        cdk::nuts::CurrencyUnit::Sat,
        PaymentMethod::Bolt11,
        MintQuoteState::Paid,
    )
    .await?;
    let quote2 = store_test_quote(
        &wallet,
        mint_url,
        "quote2",
        Amount::from(200),
        cdk::nuts::CurrencyUnit::Usd,
        PaymentMethod::Bolt11,
        MintQuoteState::Paid,
    )
    .await?;

    // Try to mint batch with different units - should fail before HTTP call
    let quote_ids = vec![quote1.id.clone(), quote2.id.clone()];
    let result = wallet
        .mint_batch(quote_ids.clone(), SplitTarget::default(), None)
        .await;

    assert!(result.is_err());
    match result {
        Err(cdk::error::Error::UnsupportedUnit) => (),
        _ => panic!("Expected UnsupportedUnit error"),
    }

    Ok(())
}

#[tokio::test]
async fn test_wallet_batch_mint_mixed_payment_methods_error() -> anyhow::Result<()> {
    let seed = rand::random::<[u8; 64]>();
    let mint_url = "https://fake.thesimplekid.dev";
    let unit = cdk::nuts::CurrencyUnit::Sat;

    let localstore = memory::empty().await?;
    let wallet = Wallet::new(mint_url, unit.clone(), Arc::new(localstore), seed, None)?;

    let quote1 = store_test_quote(
        &wallet,
        mint_url,
        "quote1",
        Amount::from(100),
        unit.clone(),
        PaymentMethod::Bolt11,
        MintQuoteState::Paid,
    )
    .await?;
    let quote2 = store_test_quote(
        &wallet,
        mint_url,
        "quote2",
        Amount::from(200),
        unit.clone(),
        PaymentMethod::Bolt12,
        MintQuoteState::Paid,
    )
    .await?;

    let quote_ids = vec![quote1.id.clone(), quote2.id.clone()];

    // This should fail because quotes have different payment methods
    let result = wallet
        .mint_batch(quote_ids.clone(), SplitTarget::default(), None)
        .await;

    assert!(result.is_err());
    match result {
        Err(cdk::error::Error::UnsupportedPaymentMethod) => (),
        _ => panic!("Expected UnsupportedPaymentMethod error"),
    }

    Ok(())
}

#[tokio::test]
async fn test_wallet_batch_mint_unpaid_quote_error() -> anyhow::Result<()> {
    let seed = rand::random::<[u8; 64]>();
    let mint_url = "https://fake.thesimplekid.dev";
    let unit = cdk::nuts::CurrencyUnit::Sat;

    let localstore = memory::empty().await?;
    let wallet = Wallet::new(mint_url, unit.clone(), Arc::new(localstore), seed, None)?;

    let quote1 = store_test_quote(
        &wallet,
        mint_url,
        "quote1",
        Amount::from(100),
        unit.clone(),
        PaymentMethod::Bolt11,
        MintQuoteState::Paid,
    )
    .await?;
    let quote2 = store_test_quote(
        &wallet,
        mint_url,
        "quote2",
        Amount::from(200),
        unit,
        PaymentMethod::Bolt11,
        MintQuoteState::Unpaid,
    )
    .await?;

    // Try to mint batch with one unpaid quote
    let quote_ids = vec![quote1.id.clone(), quote2.id.clone()];
    let result = wallet
        .mint_batch(quote_ids, SplitTarget::default(), None)
        .await;

    // Should fail because quote2 is not paid
    assert!(result.is_err());

    Ok(())
}

#[tokio::test]
async fn test_wallet_batch_mint_single_quote_validation() -> anyhow::Result<()> {
    let seed = rand::random::<[u8; 64]>();
    let mint_url = "https://fake.thesimplekid.dev";
    let unit = cdk::nuts::CurrencyUnit::Sat;

    let localstore = memory::empty().await?;
    let wallet = Wallet::new(mint_url, unit.clone(), Arc::new(localstore), seed, None)?;

    let amount = Amount::from(500);
    let quote = store_test_quote(
        &wallet,
        mint_url,
        "single",
        amount,
        unit,
        PaymentMethod::Bolt11,
        MintQuoteState::Paid,
    )
    .await?;

    // Try to mint batch with single quote - will fail at HTTP level but validation should pass
    let quote_ids = vec![quote.id.clone()];
    let result = wallet
        .mint_batch(quote_ids, SplitTarget::default(), None)
        .await;

    // Should fail due to HTTP communication, not validation
    assert!(result.is_err());

    Ok(())
}

#[tokio::test]
async fn test_wallet_batch_mint_empty_list_error() -> anyhow::Result<()> {
    let seed = rand::random::<[u8; 64]>();
    let mint_url = "https://fake.thesimplekid.dev";
    let unit = cdk::nuts::CurrencyUnit::Sat;

    let localstore = memory::empty().await?;
    let wallet = Wallet::new(mint_url, unit, Arc::new(localstore), seed, None)?;

    // Try to mint batch with empty list
    let result = wallet
        .mint_batch(vec![], SplitTarget::default(), None)
        .await;

    assert!(result.is_err());
    match result {
        Err(cdk::error::Error::AmountUndefined) => (),
        _ => panic!("Expected AmountUndefined error"),
    }

    Ok(())
}

#[tokio::test]
async fn test_wallet_batch_mint_unknown_quote_error() -> anyhow::Result<()> {
    let seed = rand::random::<[u8; 64]>();
    let mint_url = "https://fake.thesimplekid.dev";
    let unit = cdk::nuts::CurrencyUnit::Sat;

    let localstore = memory::empty().await?;
    let wallet = Wallet::new(mint_url, unit, Arc::new(localstore), seed, None)?;

    // Try to mint batch with non-existent quote
    let result = wallet
        .mint_batch(
            vec!["nonexistent".to_string()],
            SplitTarget::default(),
            None,
        )
        .await;

    assert!(result.is_err());
    match result {
        Err(cdk::error::Error::UnknownQuote) => (),
        _ => panic!("Expected UnknownQuote error"),
    }

    Ok(())
}

async fn store_test_quote(
    wallet: &Wallet,
    mint_url: &str,
    id: &str,
    amount: Amount,
    unit: cdk::nuts::CurrencyUnit,
    payment_method: PaymentMethod,
    state: MintQuoteState,
) -> anyhow::Result<cdk_common::wallet::MintQuote> {
    let mut quote = cdk_common::wallet::MintQuote::new(
        id.to_string(),
        mint_url.parse()?,
        payment_method,
        Some(amount),
        unit,
        "lnbc2000n1ps0qqqqpp5qqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqc8md94k6ar0da6gur0d3shg2zkyypqsp5qqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqhp58yjmyan4xq28guqq3c0sd5zyab0duulfr60v2n9qfv33zsrxqsp5qqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqhp4qqzj3u8ysyg8u0yy"
            .to_string(),
        unix_time() + 3600,
        None,
    );
    quote.state = state;
    quote.amount_paid = match state {
        MintQuoteState::Paid | MintQuoteState::Issued => amount,
        _ => Amount::ZERO,
    };
    wallet.localstore.add_mint_quote(quote.clone()).await?;
    Ok(quote)
}
