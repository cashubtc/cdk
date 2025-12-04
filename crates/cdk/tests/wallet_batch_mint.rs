use std::sync::Arc;

use cdk::amount::{Amount, SplitTarget};
use cdk::nuts::SecretKey;
use cdk::nuts::SpendingConditions;
use cdk::wallet::Wallet;
use cdk_common::{MintQuoteState, PaymentMethod};
use cdk_sqlite::wallet::memory;

async fn store_test_quote(
    wallet: &Wallet,
    payment_method: PaymentMethod,
    unit: cdk::nuts::CurrencyUnit,
    amount: Amount,
    paid: bool,
    secret_key: Option<SecretKey>,
) -> String {
    let quote_id = format!("test-quote-{}", rand::random::<u64>());
    let mut quote = cdk_common::wallet::MintQuote::new(
        quote_id.clone(),
        wallet.mint_url.clone(),
        payment_method,
        Some(amount),
        unit,
        "lnbc-test".to_string(),
        1000,
        secret_key,
    );
    if paid {
        quote.state = MintQuoteState::Paid;
        quote.amount_paid = amount;
    }
    wallet
        .localstore
        .add_mint_quote(quote)
        .await
        .expect("quote stored");
    quote_id
}

// Validation tests for batch minting - detailed integration tests will be in cdk-integration-tests
#[tokio::test]
async fn test_wallet_batch_mint_validates_same_unit() -> anyhow::Result<()> {
    let seed = rand::random::<[u8; 64]>();
    let mint_url = "https://fake.thesimplekid.dev";
    let unit = cdk::nuts::CurrencyUnit::Sat;

    let localstore = memory::empty().await?;
    let wallet = Wallet::new(mint_url, unit.clone(), Arc::new(localstore), seed, None)?;

    // Create two quotes with different units
    let quote1_id = store_test_quote(
        &wallet,
        PaymentMethod::Bolt11,
        unit.clone(),
        Amount::from(100),
        true,
        None,
    )
    .await;
    let quote2_id = store_test_quote(
        &wallet,
        PaymentMethod::Bolt11,
        cdk::nuts::CurrencyUnit::Usd,
        Amount::from(200),
        true,
        None,
    )
    .await;

    // Try to mint batch with different units - should fail before HTTP call
    let quote_ids = vec![quote1_id.clone(), quote2_id.clone()];
    let result = wallet
        .mint_batch(
            quote_ids.clone(),
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

    // Create quotes with different payment methods
    let quote1_id = store_test_quote(
        &wallet,
        PaymentMethod::Bolt11,
        unit.clone(),
        Amount::from(100),
        true,
        None,
    )
    .await;
    let quote2_id = store_test_quote(
        &wallet,
        PaymentMethod::Bolt12,
        unit.clone(),
        Amount::from(200),
        true,
        None,
    )
    .await;

    let quote_ids = vec![quote1_id.clone(), quote2_id.clone()];

    // This should fail because quotes have different payment methods
    let result = wallet
        .mint_batch(
            quote_ids.clone(),
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
async fn test_wallet_batch_mint_unpaid_quote_error() -> anyhow::Result<()> {
    let seed = rand::random::<[u8; 64]>();
    let mint_url = "https://fake.thesimplekid.dev";
    let unit = cdk::nuts::CurrencyUnit::Sat;

    let localstore = memory::empty().await?;
    let wallet = Wallet::new(mint_url, unit.clone(), Arc::new(localstore), seed, None)?;

    // Create two quotes
    let quote1_id = store_test_quote(
        &wallet,
        PaymentMethod::Bolt11,
        unit.clone(),
        Amount::from(100),
        true,
        None,
    )
    .await;
    let quote2_id = store_test_quote(
        &wallet,
        PaymentMethod::Bolt11,
        unit.clone(),
        Amount::from(200),
        false,
        None,
    )
    .await;

    // Try to mint batch with one unpaid quote
    let quote_ids = vec![quote1_id.clone(), quote2_id.clone()];
    let result = wallet
        .mint_batch(
            quote_ids,
            SplitTarget::default(),
            None,
            PaymentMethod::Bolt11,
        )
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

    // Create single quote
    let amount = Amount::from(500);
    let quote_id = store_test_quote(
        &wallet,
        PaymentMethod::Bolt11,
        unit.clone(),
        amount,
        true,
        None,
    )
    .await;

    // Try to mint batch with single quote - will fail at HTTP level but validation should pass
    let quote_ids = vec![quote_id.clone()];
    let result = wallet
        .mint_batch(
            quote_ids,
            SplitTarget::default(),
            None,
            PaymentMethod::Bolt11,
        )
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
    let wallet = Wallet::new(mint_url, unit.clone(), Arc::new(localstore), seed, None)?;

    // Try to mint batch with empty list
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
    let wallet = Wallet::new(mint_url, unit.clone(), Arc::new(localstore), seed, None)?;

    // Try to mint batch with non-existent quote
    let result = wallet
        .mint_batch(
            vec!["nonexistent".to_string()],
            SplitTarget::default(),
            None,
            PaymentMethod::Bolt11,
        )
        .await;

    assert!(result.is_err());
    match result {
        Err(cdk::error::Error::UnknownQuote) => (),
        _ => panic!("Expected UnknownQuote error"),
    }

    Ok(())
}

#[tokio::test]
async fn test_wallet_batch_mint_bolt12_requires_spending_conditions() -> anyhow::Result<()> {
    let seed = rand::random::<[u8; 64]>();
    let mint_url = "https://fake.thesimplekid.dev";
    let unit = cdk::nuts::CurrencyUnit::Sat;

    let localstore = memory::empty().await?;
    let wallet = Wallet::new(mint_url, unit.clone(), Arc::new(localstore), seed, None)?;

    let secret_key = SecretKey::generate();
    let mut quote = cdk_common::wallet::MintQuote::new(
        "bolt12-quote".to_string(),
        wallet.mint_url.clone(),
        PaymentMethod::Bolt12,
        Some(Amount::from(100)),
        cdk::nuts::CurrencyUnit::Sat,
        "lnbc-bolt12".to_string(),
        1000,
        Some(secret_key.clone()),
    );
    quote.state = MintQuoteState::Paid;
    quote.amount_paid = Amount::from(100);
    wallet.localstore.add_mint_quote(quote.clone()).await?;

    let result = wallet
        .mint_batch(
            vec![quote.id.clone()],
            SplitTarget::default(),
            None,
            PaymentMethod::Bolt12,
        )
        .await;

    assert!(matches!(
        result,
        Err(cdk::error::Error::BatchBolt12RequiresSpendingConditions)
    ));

    // Ensure providing spending conditions moves past this error
    let spending_conditions =
        SpendingConditions::new_p2pk(secret_key.public_key(), None /* conditions */);

    let result = wallet
        .mint_batch(
            vec![quote.id.clone()],
            SplitTarget::default(),
            Some(spending_conditions),
            PaymentMethod::Bolt12,
        )
        .await;

    assert!(result.is_err());

    Ok(())
}

#[tokio::test]
async fn test_wallet_batch_mint_bolt12_requires_secret_keys() -> anyhow::Result<()> {
    let seed = rand::random::<[u8; 64]>();
    let mint_url = "https://fake.thesimplekid.dev";
    let unit = cdk::nuts::CurrencyUnit::Sat;

    let localstore = memory::empty().await?;
    let wallet = Wallet::new(mint_url, unit.clone(), Arc::new(localstore), seed, None)?;

    let mut quote = cdk_common::wallet::MintQuote::new(
        "bolt12-quote-missing-secret".to_string(),
        wallet.mint_url.clone(),
        PaymentMethod::Bolt12,
        Some(Amount::from(50)),
        cdk::nuts::CurrencyUnit::Sat,
        "lnbc-bolt12".to_string(),
        1000,
        None,
    );
    quote.state = MintQuoteState::Paid;
    quote.amount_paid = Amount::from(50);
    wallet.localstore.add_mint_quote(quote.clone()).await?;

    let recipient_key = SecretKey::generate();
    let spending_conditions =
        SpendingConditions::new_p2pk(recipient_key.public_key(), None /* conditions */);

    let result = wallet
        .mint_batch(
            vec![quote.id.clone()],
            SplitTarget::default(),
            Some(spending_conditions),
            PaymentMethod::Bolt12,
        )
        .await;

    assert!(matches!(
        result,
        Err(cdk::error::Error::BatchBolt12MissingSecretKey)
    ));

    Ok(())
}

#[tokio::test]
async fn test_wallet_batch_mint_rejects_empty_quotes() -> anyhow::Result<()> {
    let seed = rand::random::<[u8; 64]>();
    let mint_url = "https://fake.thesimplekid.dev";
    let unit = cdk::nuts::CurrencyUnit::Sat;

    let localstore = memory::empty().await?;
    let wallet = Wallet::new(mint_url, unit.clone(), Arc::new(localstore), seed, None)?;

    let result = wallet
        .mint_batch(vec![], SplitTarget::default(), None, PaymentMethod::Bolt11)
        .await;

    assert!(matches!(result, Err(cdk::error::Error::BatchEmpty)));
    Ok(())
}

#[tokio::test]
async fn test_wallet_batch_mint_rejects_oversized_batch() -> anyhow::Result<()> {
    let seed = rand::random::<[u8; 64]>();
    let mint_url = "https://fake.thesimplekid.dev";
    let unit = cdk::nuts::CurrencyUnit::Sat;

    let localstore = memory::empty().await?;
    let wallet = Wallet::new(mint_url, unit.clone(), Arc::new(localstore), seed, None)?;

    // Try to create a batch with 101 quotes
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

    // Create 101 quote IDs
    let quote_ids: Vec<String> = (0..101).map(|i| format!("quote_{}", i)).collect();

    // Try to mint batch with over 100 quotes
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
async fn test_wallet_batch_mint_requires_all_paid_state() -> anyhow::Result<()> {
    let seed = rand::random::<[u8; 64]>();
    let mint_url = "https://fake.thesimplekid.dev";
    let unit = cdk::nuts::CurrencyUnit::Sat;

    let localstore = memory::empty().await?;
    let wallet = Wallet::new(mint_url, unit.clone(), Arc::new(localstore), seed, None)?;

    let quote1_id = store_test_quote(
        &wallet,
        PaymentMethod::Bolt11,
        unit.clone(),
        Amount::from(100),
        true,
        None,
    )
    .await;
    let quote2_id = store_test_quote(
        &wallet,
        PaymentMethod::Bolt11,
        unit.clone(),
        Amount::from(200),
        false,
        None,
    )
    .await;

    // Try to mint batch with mixed states
    let quote_ids = vec![quote1_id.clone(), quote2_id.clone()];
    let result = wallet
        .mint_batch(
            quote_ids,
            SplitTarget::default(),
            None,
            PaymentMethod::Bolt11,
        )
        .await;

    // Should fail because quote2 is not paid
    assert!(matches!(result, Err(cdk::error::Error::UnpaidQuote)));

    Ok(())
}

#[tokio::test]
async fn test_batch_mint_payment_method_validation() -> anyhow::Result<()> {
    let seed = rand::random::<[u8; 64]>();
    let mint_url = "https://fake.thesimplekid.dev";
    let unit = cdk::nuts::CurrencyUnit::Sat;

    let localstore = memory::empty().await?;
    let wallet = Wallet::new(mint_url, unit.clone(), Arc::new(localstore), seed, None)?;

    // Create quotes for each payment method
    let quote1_id = store_test_quote(
        &wallet,
        PaymentMethod::Bolt11,
        unit.clone(),
        Amount::from(100),
        true,
        None,
    )
    .await;
    let quote2_id = store_test_quote(
        &wallet,
        PaymentMethod::Bolt12,
        unit.clone(),
        Amount::from(50),
        true,
        None,
    )
    .await;

    let quote_ids = vec![quote1_id.clone(), quote2_id.clone()];
    let result = wallet
        .mint_batch(
            quote_ids,
            SplitTarget::default(),
            None,
            PaymentMethod::Bolt11,
        )
        .await;

    // Should fail with BatchPaymentMethodMismatch
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

    // Create two Bolt12 quotes manually (not compatible with Bolt11 endpoint)
    let quote1 = cdk_common::wallet::MintQuote::new(
        "quote_bolt12_1".to_string(),
        "https://fake.thesimplekid.dev".parse()?,
        PaymentMethod::Bolt12,
        Some(Amount::from(100)),
        unit.clone(),
        "lnbc1000n...".to_string(),
        1000,
        None,
    );
    wallet.localstore.add_mint_quote(quote1.clone()).await?;

    let quote2 = cdk_common::wallet::MintQuote::new(
        "quote_bolt12_2".to_string(),
        "https://fake.thesimplekid.dev".parse()?,
        PaymentMethod::Bolt12,
        Some(Amount::from(200)),
        unit.clone(),
        "lnbc2000n...".to_string(),
        1000,
        None,
    );
    wallet.localstore.add_mint_quote(quote2.clone()).await?;

    // Mark both as paid
    for quote_id in [&quote1.id, &quote2.id] {
        let mut quote_info = wallet.localstore.get_mint_quote(quote_id).await?.unwrap();
        quote_info.state = MintQuoteState::Paid;
        wallet.localstore.add_mint_quote(quote_info).await?;
    }

    // Quotes are Bolt12 but endpoint is Bolt11
    let quote_ids = vec![quote1.id.clone(), quote2.id.clone()];
    let result = wallet
        .mint_batch(
            quote_ids,
            SplitTarget::default(),
            None,
            PaymentMethod::Bolt11,
        )
        .await;

    // Should fail with BatchPaymentMethodEndpointMismatch error
    assert!(matches!(
        result,
        Err(cdk::error::Error::BatchPaymentMethodEndpointMismatch)
    ));

    Ok(())
}
