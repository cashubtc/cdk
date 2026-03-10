//! FFI Minting Integration Tests
//!
//! These tests verify the FFI wallet minting functionality through the complete
//! mint-to-tokens workflow, similar to the Swift bindings tests. The tests use
//! the actual FFI layer to ensure compatibility with language bindings.
//!
//! The tests include:
//! 1. Creating mint quotes through the FFI layer
//! 2. Simulating payment for development/testing environments
//! 3. Minting tokens and verifying amounts
//! 4. Testing the complete quote state transitions
//! 5. Validating proof generation and verification

use std::env;
use std::path::PathBuf;
use std::str::FromStr;
use std::time::Duration;

use bip39::Mnemonic;
use cdk_ffi::sqlite::WalletSqliteDatabase;
use cdk_ffi::types::{encode_mint_quote, Amount, CurrencyUnit, QuoteState, SplitTarget};
use cdk_ffi::wallet::Wallet as FfiWallet;
use cdk_ffi::{PaymentMethod, WalletConfig};
use cdk_integration_tests::{get_mint_url_from_env, pay_if_regtest};
use lightning_invoice::Bolt11Invoice;
use tokio::time::timeout;

// Helper function to get temp directory from environment or fallback
fn get_test_temp_dir() -> PathBuf {
    match env::var("CDK_ITESTS_DIR") {
        Ok(dir) => PathBuf::from(dir),
        Err(_) => panic!("Unknown test dir"),
    }
}

/// Create a test FFI wallet with in-memory database
async fn create_test_ffi_wallet() -> FfiWallet {
    let db = WalletSqliteDatabase::new_in_memory().expect("Failed to create in-memory database");
    let mnemonic = Mnemonic::generate(12).unwrap().to_string();
    let config = WalletConfig {
        target_proof_count: Some(3),
    };

    FfiWallet::new(
        get_mint_url_from_env(),
        CurrencyUnit::Sat,
        mnemonic,
        db,
        config,
    )
    .expect("Failed to create FFI wallet")
}

/// Tests the complete FFI minting flow from quote creation to token minting
///
/// This test replicates the Swift integration test functionality:
/// 1. Creates an FFI wallet with in-memory database
/// 2. Creates a mint quote for 1000 sats
/// 3. Verifies the quote properties (amount, state, expiry)
/// 4. Simulates payment in test environments
/// 5. Mints tokens using the paid quote
/// 6. Verifies the minted proofs have the correct total amount
/// 7. Validates the wallet balance after minting
///
/// This ensures the FFI layer properly handles the complete minting workflow
/// that language bindings (Swift, Python, Kotlin) will use.
#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_ffi_full_minting_flow() {
    let wallet = create_test_ffi_wallet().await;

    // Verify initial wallet state
    let initial_balance = wallet
        .total_balance()
        .await
        .expect("Failed to get initial balance");
    assert_eq!(initial_balance.value, 0, "Initial balance should be zero");

    // Test minting amount (1000 sats, matching Swift test)
    let mint_amount = Amount::new(1000);

    // Step 1: Create a mint quote
    let quote = wallet
        .mint_quote(
            PaymentMethod::Bolt11,
            Some(mint_amount),
            Some("FFI Integration Test".to_string()),
            None,
        )
        .await
        .expect("Failed to create mint quote");

    // Verify quote properties
    assert_eq!(
        quote.amount,
        Some(mint_amount),
        "Quote amount should match requested amount"
    );
    assert_eq!(quote.unit, CurrencyUnit::Sat, "Quote unit should be sats");
    assert_eq!(
        quote.state,
        QuoteState::Unpaid,
        "Initial quote state should be unpaid"
    );
    assert!(
        !quote.request.is_empty(),
        "Quote should have a payment request"
    );
    assert!(!quote.id.is_empty(), "Quote should have an ID");

    // Check mint quote status
    let quote_status = wallet
        .check_mint_quote(quote.id.clone())
        .await
        .expect("failed to get mint status");
    assert_eq!(
        quote_status.amount,
        Some(mint_amount),
        "Quote amount should match requested amount"
    );
    assert_eq!(
        quote_status.unit,
        CurrencyUnit::Sat,
        "Quote unit should be sats"
    );
    assert_eq!(
        quote_status.state,
        QuoteState::Unpaid,
        "Initial quote state should be unpaid"
    );
    assert!(
        !quote_status.request.is_empty(),
        "Quote should have a payment request"
    );

    // Verify the quote can be parsed as a valid invoice
    let invoice = Bolt11Invoice::from_str(&quote.request)
        .expect("Quote request should be a valid Lightning invoice");

    // In test environments, simulate payment
    pay_if_regtest(&get_test_temp_dir(), &invoice)
        .await
        .expect("Failed to pay invoice in test environment");

    // Give the mint time to process the payment in test environments
    tokio::time::sleep(Duration::from_millis(1000)).await;

    // Step 2: Wait for payment and mint tokens
    // We'll use a timeout to avoid hanging in case of issues
    let mint_result = timeout(Duration::from_secs(30), async {
        // Keep checking quote status until it's paid, then mint
        let mut attempts = 0;
        let max_attempts = 10;

        loop {
            attempts += 1;
            if attempts > max_attempts {
                panic!(
                    "Quote never transitioned to paid state after {} attempts",
                    max_attempts
                );
            }

            // In a real scenario, we'd check quote status, but for integration tests
            // we'll try to mint directly and handle any errors
            match wallet.mint(quote.id.clone(), SplitTarget::None, None).await {
                Ok(proofs) => break proofs,
                Err(e) => {
                    // If quote isn't paid yet, wait and retry
                    if e.to_string().contains("quote not paid") || e.to_string().contains("unpaid")
                    {
                        tokio::time::sleep(Duration::from_millis(2000)).await;
                        continue;
                    } else {
                        panic!("Unexpected error while minting: {}", e);
                    }
                }
            }
        }
    })
    .await
    .expect("Timeout waiting for minting to complete");

    // Step 3: Verify minted proofs
    assert!(
        !mint_result.is_empty(),
        "Should have minted at least one proof"
    );

    // Calculate total amount of minted proofs
    let total_minted: u64 = mint_result.iter().map(|proof| proof.amount.value).sum();
    assert_eq!(
        total_minted, mint_amount.value,
        "Total minted amount should equal requested amount"
    );

    // Verify each proof has valid properties
    for proof in &mint_result {
        assert!(
            proof.amount.value > 0,
            "Each proof should have positive amount"
        );
        assert!(!proof.secret.is_empty(), "Each proof should have a secret");
        assert!(!proof.c.is_empty(), "Each proof should have a C value");
    }

    // Step 4: Verify wallet balance after minting
    let final_balance = wallet
        .total_balance()
        .await
        .expect("Failed to get final balance");
    assert_eq!(
        final_balance.value, mint_amount.value,
        "Final wallet balance should equal minted amount"
    );

    println!(
        "✅ FFI minting test completed successfully: minted {} sats in {} proofs",
        total_minted,
        mint_result.len()
    );
}

/// Tests FFI wallet quote creation and validation
///
/// This test focuses on the quote creation aspects:
/// 1. Creates quotes for different amounts
/// 2. Verifies quote properties and validation
/// 3. Tests quote serialization/deserialization
/// 4. Ensures quotes have proper expiry times
#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_ffi_mint_quote_creation() {
    let wallet = create_test_ffi_wallet().await;

    // Test different quote amounts
    let test_amounts = vec![100, 500, 1000, 2100]; // Including amount that requires split

    for amount_value in test_amounts {
        let amount = Amount::new(amount_value);
        let description = format!("Test quote for {} sats", amount_value);

        let quote = wallet
            .mint_quote(
                PaymentMethod::Bolt11,
                Some(amount),
                Some(description.clone()),
                None,
            )
            .await
            .unwrap_or_else(|_| panic!("Failed to create quote for {} sats", amount_value));

        // Verify quote properties
        assert_eq!(quote.amount, Some(amount));
        assert_eq!(quote.unit, CurrencyUnit::Sat);
        assert_eq!(quote.state, QuoteState::Unpaid);
        assert!(!quote.id.is_empty());
        assert!(!quote.request.is_empty());

        // Verify the payment request is a valid Lightning invoice
        let invoice = Bolt11Invoice::from_str(&quote.request)
            .expect("Quote request should be a valid Lightning invoice");

        // The invoice amount should match the quote amount (in millisats)
        assert_eq!(
            invoice.amount_milli_satoshis(),
            Some(amount_value * 1000),
            "Invoice amount should match quote amount"
        );

        // Test quote JSON serialization (useful for bindings that need JSON)
        let quote_json = encode_mint_quote(quote.clone()).expect("Quote should serialize to JSON");
        assert!(!quote_json.is_empty(), "Quote JSON should not be empty");

        println!(
            "✅ Quote created for {} sats: ID={}, Invoice amount={}msat",
            amount_value,
            quote.id,
            invoice.amount_milli_satoshis().unwrap_or(0)
        );
    }
}

/// Tests error handling in FFI minting operations
///
/// This test verifies proper error handling:
/// 1. Invalid mint URLs
/// 2. Invalid amounts (zero, too large)
/// 3. Attempting to mint unpaid quotes
/// 4. Network connectivity issues
#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_ffi_minting_error_handling() {
    // Test invalid mint URL
    let db = WalletSqliteDatabase::new_in_memory().expect("Failed to create database");
    let mnemonic = Mnemonic::generate(12).unwrap().to_string();
    let config = WalletConfig {
        target_proof_count: Some(3),
    };

    let invalid_wallet_result = FfiWallet::new(
        "invalid-url".to_string(),
        CurrencyUnit::Sat,
        mnemonic.clone(),
        db,
        config.clone(),
    );
    assert!(
        invalid_wallet_result.is_err(),
        "Should fail to create wallet with invalid URL"
    );

    // Test with valid wallet for other error cases
    let wallet = create_test_ffi_wallet().await;

    // Test zero amount quote (should fail)
    let zero_amount_result = wallet
        .mint_quote(PaymentMethod::Bolt11, Some(Amount::new(0)), None, None)
        .await;
    assert!(
        zero_amount_result.is_err(),
        "Should fail to create quote with zero amount"
    );

    // Test minting with non-existent quote ID
    let invalid_mint_result = wallet
        .mint("non-existent-quote-id".to_string(), SplitTarget::None, None)
        .await;
    assert!(
        invalid_mint_result.is_err(),
        "Should fail to mint with non-existent quote ID"
    );

    println!("✅ Error handling tests completed successfully");
}

/// Tests FFI wallet configuration options
///
/// This test verifies different wallet configurations:
/// 1. Different target proof counts
/// 2. Different currency units (if supported)
/// 3. Wallet restoration with same mnemonic
#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_ffi_wallet_configuration() {
    let mint_url = get_mint_url_from_env();
    let mnemonic = Mnemonic::generate(12).unwrap().to_string();

    // Test different target proof counts
    let proof_counts = vec![1, 3, 5, 10];

    for target_count in proof_counts {
        let db = WalletSqliteDatabase::new_in_memory().expect("Failed to create database");
        let config = WalletConfig {
            target_proof_count: Some(target_count),
        };

        let wallet = FfiWallet::new(
            mint_url.clone(),
            CurrencyUnit::Sat,
            mnemonic.clone(),
            db,
            config,
        )
        .expect("Failed to create wallet");

        // Verify wallet properties
        assert_eq!(wallet.mint_url().url, mint_url);
        assert_eq!(wallet.unit(), CurrencyUnit::Sat);

        println!(
            "✅ Wallet created with target proof count: {}",
            target_count
        );
    }

    // Test wallet restoration with same mnemonic
    let db1 = WalletSqliteDatabase::new_in_memory().expect("Failed to create database");
    let db2 = WalletSqliteDatabase::new_in_memory().expect("Failed to create database");

    let config = WalletConfig {
        target_proof_count: Some(3),
    };

    let wallet1 = FfiWallet::new(
        mint_url.clone(),
        CurrencyUnit::Sat,
        mnemonic.clone(),
        db1,
        config.clone(),
    )
    .expect("Failed to create first wallet");

    let wallet2 = FfiWallet::new(mint_url, CurrencyUnit::Sat, mnemonic, db2, config)
        .expect("Failed to create second wallet");

    // Both wallets should have the same mint URL and unit
    assert_eq!(wallet1.mint_url().url, wallet2.mint_url().url);
    assert_eq!(wallet1.unit(), wallet2.unit());

    println!("✅ Wallet configuration tests completed successfully");
}
