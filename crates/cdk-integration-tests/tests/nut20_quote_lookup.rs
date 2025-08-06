use std::collections::HashMap;

use cdk::nuts::SecretKey;
use cdk::nuts::{CurrencyUnit, MintQuoteBolt11Request};
use cdk::Amount;
use cdk_integration_tests::init_pure_tests::create_and_start_test_mint;

#[tokio::test]
async fn test_nut20_quote_lookup() -> anyhow::Result<()> {
    let mint = create_and_start_test_mint().await?;

    // Generate a test key pair for locking the quote
    let secret_key = SecretKey::generate();
    let pubkey = secret_key.public_key();

    // Create a mint quote with the locking pubkey
    let quote_request = MintQuoteBolt11Request {
        amount: Amount::from(100),
        unit: CurrencyUnit::Sat,
        description: Some("Test quote for NUT-20".to_string()),
        pubkey: Some(pubkey),
    };

    // Request the mint quote directly from the mint
    let quote_response = mint.get_mint_quote(quote_request.into()).await?;
    let quote_id = quote_response.quote_id();

    // Now test the lookup functionality directly
    let lookup_response = mint.lookup_mint_quotes_by_pubkeys(&[pubkey]).await?;

    // Verify the response
    assert_eq!(lookup_response.len(), 1);

    let found_quote = &lookup_response[0];
    assert_eq!(found_quote.pubkey, pubkey);
    assert_eq!(found_quote.quote, quote_id);
    assert_eq!(found_quote.amount, Amount::from(100));
    assert_eq!(found_quote.unit, CurrencyUnit::Sat);

    println!("✅ NUT-20 quote lookup test passed!");
    Ok(())
}

#[tokio::test]
async fn test_nut20_quote_lookup_multiple_keys() -> anyhow::Result<()> {
    let mint = create_and_start_test_mint().await?;

    // Generate multiple test key pairs
    let secret_key1 = SecretKey::generate();
    let pubkey1 = secret_key1.public_key();

    let secret_key2 = SecretKey::generate();
    let pubkey2 = secret_key2.public_key();

    // Create quotes with different pubkeys
    let _quote1 = mint
        .get_mint_quote(
            MintQuoteBolt11Request {
                amount: Amount::from(50),
                unit: CurrencyUnit::Sat,
                description: Some("Quote 1".to_string()),
                pubkey: Some(pubkey1),
            }
            .into(),
        )
        .await?;

    let _quote2 = mint
        .get_mint_quote(
            MintQuoteBolt11Request {
                amount: Amount::from(75),
                unit: CurrencyUnit::Sat,
                description: Some("Quote 2".to_string()),
                pubkey: Some(pubkey2),
            }
            .into(),
        )
        .await?;

    // Lookup both quotes
    let lookup_response = mint
        .lookup_mint_quotes_by_pubkeys(&[pubkey1, pubkey2])
        .await?;

    // Should find both quotes
    assert_eq!(lookup_response.len(), 2);

    let mut found_amounts = HashMap::new();
    for quote in &lookup_response {
        found_amounts.insert(quote.pubkey, quote.amount);
    }

    assert_eq!(found_amounts[&pubkey1], Amount::from(50));
    assert_eq!(found_amounts[&pubkey2], Amount::from(75));

    println!("✅ NUT-20 multiple key lookup test passed!");
    Ok(())
}

#[tokio::test]
async fn test_nut20_quote_lookup_empty_result() -> anyhow::Result<()> {
    let mint = create_and_start_test_mint().await?;

    // Generate a key that has no associated quotes
    let secret_key = SecretKey::generate();
    let pubkey = secret_key.public_key();

    let lookup_response = mint.lookup_mint_quotes_by_pubkeys(&[pubkey]).await?;

    // Should return empty array
    assert_eq!(lookup_response.len(), 0);

    println!("✅ NUT-20 empty lookup test passed!");
    Ok(())
}
