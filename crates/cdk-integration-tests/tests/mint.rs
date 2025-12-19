//! Mint Tests
//!
//! This file contains tests that focus on the mint's internal functionality without client interaction.
//! These tests verify the mint's behavior in isolation, such as keyset management, database operations,
//! and other mint-specific functionality that doesn't require wallet clients.
//!
//! Test Categories:
//! - Keyset rotation and management
//! - Database transaction handling
//! - Internal state transitions
//! - Fee calculation and enforcement
//! - Proof validation and state management

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use bip39::Mnemonic;
use cdk::mint::{MintBuilder, MintMeltLimits};
use cdk::nuts::{CurrencyUnit, PaymentMethod};
use cdk::types::{FeeReserve, QuoteTTL};
use cdk_fake_wallet::FakeWallet;
use cdk_sqlite::mint::memory;

pub const MINT_URL: &str = "http://127.0.0.1:8088";

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_correct_keyset() {
    let mnemonic = Mnemonic::generate(12).unwrap();
    let fee_reserve = FeeReserve {
        min_fee_reserve: 1.into(),
        percent_fee_reserve: 1.0,
    };

    let database = memory::empty().await.expect("valid db instance");

    let fake_wallet = FakeWallet::new(
        fee_reserve,
        HashMap::default(),
        HashSet::default(),
        0,
        CurrencyUnit::Sat,
    );

    let localstore = Arc::new(database);
    let mut mint_builder = MintBuilder::new(localstore.clone());

    mint_builder = mint_builder
        .with_name("regtest mint".to_string())
        .with_description("regtest mint".to_string());

    mint_builder
        .add_payment_processor(
            CurrencyUnit::Sat,
            PaymentMethod::Bolt11,
            MintMeltLimits::new(1, 5_000),
            Arc::new(fake_wallet),
        )
        .await
        .unwrap();
    // .with_seed(mnemonic.to_seed_normalized("").to_vec());

    let mint = mint_builder
        .build_with_seed(localstore.clone(), &mnemonic.to_seed_normalized(""))
        .await
        .unwrap();

    let quote_ttl = QuoteTTL::new(10000, 10000);
    mint.set_quote_ttl(quote_ttl).await.unwrap();

    let active = mint.get_active_keysets();

    let active = active
        .get(&CurrencyUnit::Sat)
        .expect("There is a keyset for unit");
    let old_keyset_info = mint.get_keyset_info(active).expect("There is keyset");

    mint.rotate_keyset(
        CurrencyUnit::Sat,
        cdk_integration_tests::standard_keyset_amounts(32),
        0,
    )
    .await
    .unwrap();

    let active = mint.get_active_keysets();

    let active = active
        .get(&CurrencyUnit::Sat)
        .expect("There is a keyset for unit");

    let keyset_info = mint.get_keyset_info(active).expect("There is keyset");

    assert_ne!(keyset_info.id, old_keyset_info.id);

    mint.rotate_keyset(
        CurrencyUnit::Sat,
        cdk_integration_tests::standard_keyset_amounts(32),
        0,
    )
    .await
    .unwrap();

    let active = mint.get_active_keysets();

    let active = active
        .get(&CurrencyUnit::Sat)
        .expect("There is a keyset for unit");

    let new_keyset_info = mint.get_keyset_info(active).expect("There is keyset");

    assert_ne!(new_keyset_info.id, keyset_info.id);
}

/// Test concurrent payment processing to verify race condition fix
///
/// This test simulates the real-world race condition where multiple concurrent
/// payment notifications arrive for the same payment_id. Before the fix, this
/// would cause "Payment ID already exists" errors. After the fix, all but one
/// should gracefully handle the duplicate and return a Duplicate error.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn test_concurrent_duplicate_payment_handling() {
    use cashu::PaymentMethod;
    use cdk::cdk_database::{MintDatabase, MintQuotesDatabase};
    use cdk::mint::MintQuote;
    use cdk::Amount;
    use cdk_common::payment::PaymentIdentifier;
    use tokio::task::JoinSet;

    // Create a test mint with in-memory database
    let mnemonic = Mnemonic::generate(12).unwrap();
    let fee_reserve = FeeReserve {
        min_fee_reserve: 1.into(),
        percent_fee_reserve: 1.0,
    };

    let database = Arc::new(memory::empty().await.expect("valid db instance"));

    let fake_wallet = FakeWallet::new(
        fee_reserve,
        HashMap::default(),
        HashSet::default(),
        0,
        CurrencyUnit::Sat,
    );

    let mut mint_builder = MintBuilder::new(database.clone());

    mint_builder = mint_builder
        .with_name("concurrent test mint".to_string())
        .with_description("testing concurrent payment handling".to_string());

    mint_builder
        .add_payment_processor(
            CurrencyUnit::Sat,
            PaymentMethod::Bolt11,
            MintMeltLimits::new(1, 5_000),
            Arc::new(fake_wallet),
        )
        .await
        .unwrap();

    let mint = mint_builder
        .build_with_seed(database.clone(), &mnemonic.to_seed_normalized(""))
        .await
        .unwrap();

    let quote_ttl = QuoteTTL::new(10000, 10000);
    mint.set_quote_ttl(quote_ttl).await.unwrap();

    // Create a mint quote
    let current_time = cdk::util::unix_time();
    let mint_quote = MintQuote::new(
        None,
        "concurrent_test_invoice".to_string(),
        CurrencyUnit::Sat,
        Some(Amount::from(1000)),
        current_time + 3600, // expires in 1 hour
        PaymentIdentifier::CustomId("test_lookup_id".to_string()),
        None,
        Amount::ZERO,
        Amount::ZERO,
        PaymentMethod::Bolt11,
        current_time,
        vec![],
        vec![],
    );

    // Add the quote to the database
    {
        let mut tx = MintDatabase::begin_transaction(&*database).await.unwrap();
        tx.add_mint_quote(mint_quote.clone()).await.unwrap();
        tx.commit().await.unwrap();
    }

    // Simulate 10 concurrent payment notifications with the SAME payment_id
    let payment_id = "duplicate_payment_test_12345";
    let mut join_set = JoinSet::new();

    for i in 0..10 {
        let db_clone = database.clone();
        let quote_id = mint_quote.id.clone();
        let payment_id_clone = payment_id.to_string();

        join_set.spawn(async move {
            let mut tx = MintDatabase::begin_transaction(&*db_clone).await.unwrap();
            let result = tx
                .increment_mint_quote_amount_paid(&quote_id, Amount::from(10), payment_id_clone)
                .await;

            if result.is_ok() {
                tx.commit().await.unwrap();
            }

            (i, result)
        });
    }

    // Collect results
    let mut success_count = 0;
    let mut duplicate_errors = 0;
    let mut other_errors = Vec::new();

    while let Some(result) = join_set.join_next().await {
        let (task_id, db_result) = result.unwrap();
        match db_result {
            Ok(_) => success_count += 1,
            Err(e) => {
                let err_str = format!("{:?}", e);
                if err_str.contains("Duplicate") {
                    duplicate_errors += 1;
                } else {
                    other_errors.push((task_id, err_str));
                }
            }
        }
    }

    // Verify results
    assert_eq!(
        success_count, 1,
        "Exactly one task should successfully process the payment (got {})",
        success_count
    );
    assert!(
        other_errors.is_empty(),
        "No unexpected errors should occur. Got: {:?}",
        other_errors
    );
    assert_eq!(
        duplicate_errors, 9,
        "Nine tasks should receive Duplicate error (got {})",
        duplicate_errors
    );

    // Verify the quote was incremented exactly once
    let final_quote = MintQuotesDatabase::get_mint_quote(&*database, &mint_quote.id)
        .await
        .unwrap()
        .expect("Quote should exist");

    assert_eq!(
        final_quote.amount_paid(),
        Amount::from(10),
        "Quote amount should be incremented exactly once"
    );
    assert_eq!(
        final_quote.payments.len(),
        1,
        "Should have exactly one payment recorded"
    );
    assert_eq!(
        final_quote.payments[0].payment_id, payment_id,
        "Payment ID should match"
    );
}
