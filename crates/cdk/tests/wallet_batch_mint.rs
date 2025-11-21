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

    let quote_ids = vec![quote1.id.clone(), quote2.id.clone()];
    let result = wallet
        .mint_batch(
            quote_ids,
            SplitTarget::default(),
            None,
            PaymentMethod::Bolt11,
        )
        .await;

    assert!(matches!(
        result,
        Err(cdk::error::Error::BatchCurrencyUnitMismatch)
    ));

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

    let result = wallet
        .mint_batch(
            vec![quote1.id.clone(), quote2.id.clone()],
            SplitTarget::default(),
            None,
            PaymentMethod::Bolt11,
        )
        .await;

    assert!(matches!(
        result,
        Err(cdk::error::Error::BatchPaymentMethodMismatch)
    ));

    Ok(())
}

#[tokio::test]
async fn test_wallet_batch_mint_requires_all_paid_state() -> anyhow::Result<()> {
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

    let result = wallet
        .mint_batch(
            vec![quote1.id.clone(), quote2.id.clone()],
            SplitTarget::default(),
            None,
            PaymentMethod::Bolt11,
        )
        .await;

    assert!(matches!(result, Err(cdk::error::Error::UnpaidQuote)));

    Ok(())
}

#[tokio::test]
async fn test_wallet_batch_mint_single_quote_validation() -> anyhow::Result<()> {
    let seed = rand::random::<[u8; 64]>();
    let mint_url = "https://fake.thesimplekid.dev";
    let unit = cdk::nuts::CurrencyUnit::Sat;

    let localstore = memory::empty().await?;
    let wallet = Wallet::new(mint_url, unit.clone(), Arc::new(localstore), seed, None)?;

    let quote = store_test_quote(
        &wallet,
        mint_url,
        "single",
        Amount::from(500),
        unit,
        PaymentMethod::Bolt11,
        MintQuoteState::Paid,
    )
    .await?;

    let result = wallet
        .mint_batch(
            vec![quote.id.clone()],
            SplitTarget::default(),
            None,
            PaymentMethod::Bolt11,
        )
        .await;

    assert!(result.is_err());

    Ok(())
}

#[tokio::test]
async fn test_wallet_batch_mint_rejects_empty_quotes() -> anyhow::Result<()> {
    let seed = rand::random::<[u8; 64]>();
    let mint_url = "https://fake.thesimplekid.dev";
    let unit = cdk::nuts::CurrencyUnit::Sat;

    let localstore = memory::empty().await?;
    let wallet = Wallet::new(mint_url, unit, Arc::new(localstore), seed, None)?;

    let result = wallet
        .mint_batch(vec![], SplitTarget::default(), None, PaymentMethod::Bolt11)
        .await;

    assert!(matches!(result, Err(cdk::error::Error::BatchEmpty)));

    Ok(())
}

#[tokio::test]
async fn test_wallet_batch_mint_unknown_quote_error() -> anyhow::Result<()> {
    let seed = rand::random::<[u8; 64]>();
    let mint_url = "https://fake.thesimplekid.dev";
    let unit = cdk::nuts::CurrencyUnit::Sat;

    let localstore = memory::empty().await?;
    let wallet = Wallet::new(mint_url, unit, Arc::new(localstore), seed, None)?;

    let result = wallet
        .mint_batch(
            vec!["nonexistent".to_string()],
            SplitTarget::default(),
            None,
            PaymentMethod::Bolt11,
        )
        .await;

    assert!(matches!(result, Err(cdk::error::Error::UnknownQuote)));

    Ok(())
}

#[tokio::test]
async fn test_wallet_batch_mint_rejects_oversized_batch() -> anyhow::Result<()> {
    let seed = rand::random::<[u8; 64]>();
    let mint_url = "https://fake.thesimplekid.dev";
    let unit = cdk::nuts::CurrencyUnit::Sat;

    let localstore = memory::empty().await?;
    let wallet = Wallet::new(mint_url, unit.clone(), Arc::new(localstore), seed, None)?;

    let quote_ids: Vec<String> = (0..101).map(|i| format!("quote_{}", i)).collect();
    let result = wallet
        .mint_batch(
            quote_ids,
            SplitTarget::default(),
            None,
            PaymentMethod::Bolt11,
        )
        .await;

    assert!(matches!(result, Err(cdk::error::Error::BatchSizeExceeded)));
    Ok(())
}

#[tokio::test]
async fn test_wallet_batch_mint_rejects_over_limit() -> anyhow::Result<()> {
    let seed = rand::random::<[u8; 64]>();
    let mint_url = "https://fake.thesimplekid.dev";
    let unit = cdk::nuts::CurrencyUnit::Sat;

    let localstore = memory::empty().await?;
    let wallet = Wallet::new(mint_url, unit.clone(), Arc::new(localstore), seed, None)?;

    let quote_ids = (0..101)
        .map(|i| format!("quote_over_limit_{}", i))
        .collect::<Vec<_>>();
    for quote_id in &quote_ids {
        store_test_quote(
            &wallet,
            mint_url,
            quote_id,
            Amount::from(1),
            unit.clone(),
            PaymentMethod::Bolt11,
            MintQuoteState::Paid,
        )
        .await?;
    }

    let result = wallet
        .mint_batch(quote_ids, SplitTarget::default(), None, PaymentMethod::Bolt11)
        .await;

    assert!(matches!(result, Err(cdk::error::Error::BatchSizeExceeded)));
    Ok(())
}

#[tokio::test]
async fn test_batch_mint_payment_method_validation() -> anyhow::Result<()> {
    let seed = rand::random::<[u8; 64]>();
    let mint_url = "https://fake.thesimplekid.dev";
    let unit = cdk::nuts::CurrencyUnit::Sat;

    let localstore = memory::empty().await?;
    let wallet = Wallet::new(mint_url, unit.clone(), Arc::new(localstore), seed, None)?;

    let quote1 = store_test_quote(
        &wallet,
        mint_url,
        "bolt11_quote",
        Amount::from(100),
        unit.clone(),
        PaymentMethod::Bolt11,
        MintQuoteState::Paid,
    )
    .await?;
    let quote2 = store_test_quote(
        &wallet,
        mint_url,
        "bolt12_quote",
        Amount::from(50),
        unit,
        PaymentMethod::Bolt12,
        MintQuoteState::Paid,
    )
    .await?;

    let result = wallet
        .mint_batch(
            vec![quote1.id.clone(), quote2.id.clone()],
            SplitTarget::default(),
            None,
            PaymentMethod::Bolt11,
        )
        .await;

    assert!(matches!(
        result,
        Err(cdk::error::Error::BatchPaymentMethodMismatch)
    ));

    Ok(())
}

#[tokio::test]
async fn test_batch_mint_enforces_url_payment_method() -> anyhow::Result<()> {
    let seed = rand::random::<[u8; 64]>();
    let mint_url = "https://fake.thesimplekid.dev";
    let unit = cdk::nuts::CurrencyUnit::Sat;

    let localstore = memory::empty().await?;
    let wallet = Wallet::new(mint_url, unit.clone(), Arc::new(localstore), seed, None)?;

    let quote1 = store_test_quote(
        &wallet,
        mint_url,
        "bolt12_1",
        Amount::from(100),
        unit.clone(),
        PaymentMethod::Bolt12,
        MintQuoteState::Paid,
    )
    .await?;
    let quote2 = store_test_quote(
        &wallet,
        mint_url,
        "bolt12_2",
        Amount::from(200),
        unit,
        PaymentMethod::Bolt12,
        MintQuoteState::Paid,
    )
    .await?;

    let result = wallet
        .mint_batch(
            vec![quote1.id.clone(), quote2.id.clone()],
            SplitTarget::default(),
            None,
            PaymentMethod::Bolt11,
        )
        .await;

    assert!(matches!(
        result,
        Err(cdk::error::Error::BatchPaymentMethodEndpointMismatch)
    ));

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
