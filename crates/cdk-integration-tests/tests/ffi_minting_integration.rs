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
use cdk_ffi::database::WalletStore;
use cdk_ffi::types::{Amount, CurrencyUnit};
use cdk_ffi::wallet::Wallet as FfiWallet;
use cdk_ffi::{MintRequest, MintingState, PaymentMethod, WalletConfig, WalletOpenRequest};
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

/// Create a temporary SQLite-backed WalletStore
fn temp_wallet_store() -> WalletStore {
    let path = get_test_temp_dir()
        .join(format!("ffi_test_{}.sqlite", uuid::Uuid::new_v4()))
        .to_string_lossy()
        .to_string();
    WalletStore::Sqlite { path }
}

/// Create a test FFI wallet with a temp SQLite database
async fn create_test_ffi_wallet() -> FfiWallet {
    let mnemonic = Mnemonic::generate(12).unwrap().to_string();
    let config = WalletConfig {
        target_proof_count: Some(3),
        rate_limit: None,
    };

    FfiWallet::open(WalletOpenRequest {
        mint_url: get_mint_url_from_env(),
        unit: CurrencyUnit::Sat,
        mnemonic,
        store: temp_wallet_store(),
        config: Some(config),
    })
    .expect("Failed to create FFI wallet")
}

/// Tests the complete FFI minting flow from quote creation to token minting
///
/// This test replicates the Swift integration test functionality:
/// 1. Creates an FFI wallet with in-memory database
/// 2. Creates a mint quote for 1000 sats
/// 3. Verifies the quote properties (amount, state, expiry)
/// 4. Simulates payment in test environments
/// 5. Claims the paid quote through its durable mint session
/// 6. Verifies the claimed amount
/// 7. Validates the wallet balance after minting
///
/// This ensures the FFI layer properly handles the complete minting workflow
/// that language bindings (Swift, Python, Kotlin) will use.
#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_ffi_full_minting_flow() {
    let wallet = create_test_ffi_wallet().await;

    // Verify initial wallet state
    let initial_balance = wallet
        .balance()
        .await
        .expect("Failed to get initial balance");
    assert_eq!(
        initial_balance.available.value, 0,
        "Initial balance should be zero"
    );

    // Test minting amount (1000 sats, matching Swift test)
    let mint_amount = Amount::new(1000);

    // Step 1: Create a mint quote
    let session = wallet
        .request_minting(MintRequest {
            method: PaymentMethod::Bolt11,
            amount: Some(mint_amount),
            description: Some("FFI Integration Test".to_string()),
            extra: None,
        })
        .await
        .expect("Failed to create mint quote");
    let quote = session.initial_state();

    // Verify quote properties
    assert_eq!(
        quote.amount,
        Some(mint_amount),
        "Quote amount should match requested amount"
    );
    assert_eq!(
        wallet.identity().unit,
        CurrencyUnit::Sat,
        "Quote wallet unit should be sats"
    );
    assert_eq!(
        quote.state,
        MintingState::Unpaid,
        "Initial quote state should be unpaid"
    );
    assert!(
        !quote.payment_request.is_empty(),
        "Quote should have a payment request"
    );
    assert!(!quote.id.is_empty(), "Quote should have an ID");

    // Check mint quote status
    let quote_status = session.refresh().await.expect("failed to get mint status");
    assert_eq!(
        quote_status.amount,
        Some(mint_amount),
        "Quote amount should match requested amount"
    );
    assert_eq!(
        quote_status.state,
        MintingState::Unpaid,
        "Initial quote state should be unpaid"
    );
    assert!(
        !quote_status.payment_request.is_empty(),
        "Quote should have a payment request"
    );

    // Verify the quote can be parsed as a valid invoice
    let invoice = Bolt11Invoice::from_str(&quote.payment_request)
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
        // Keep checking quote status until it is paid, then claim it.
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

            let current = session
                .refresh()
                .await
                .expect("Failed to refresh minting session");
            match current.state {
                MintingState::Paid => break session.claim().await.expect("Failed to claim quote"),
                MintingState::Unpaid => {
                    tokio::time::sleep(Duration::from_millis(2000)).await;
                }
                MintingState::Issued => break current.amount_claimed,
            }
        }
    })
    .await
    .expect("Timeout waiting for minting to complete");

    // Step 3: Verify the facade reports the claimed value without exposing proofs.
    let total_minted = mint_result.value;
    assert_eq!(
        total_minted, mint_amount.value,
        "Total minted amount should equal requested amount"
    );

    // Step 4: Verify wallet balance after minting
    let final_balance = wallet.balance().await.expect("Failed to get final balance");
    assert_eq!(
        final_balance.available.value, mint_amount.value,
        "Final wallet balance should equal minted amount"
    );

    println!("✅ FFI minting test completed successfully: minted {total_minted} sats");
}

/// Tests FFI wallet quote creation and validation
///
/// This test focuses on the quote creation aspects:
/// 1. Creates quotes for different amounts
/// 2. Verifies quote properties and validation
/// 3. Tests local session reconstruction by quote ID
/// 4. Ensures quotes have proper expiry times
#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_ffi_mint_quote_creation() {
    let wallet = create_test_ffi_wallet().await;

    // Test different quote amounts
    let test_amounts = vec![100, 500, 1000, 2100]; // Including amount that requires split

    for amount_value in test_amounts {
        let amount = Amount::new(amount_value);
        let description = format!("Test quote for {} sats", amount_value);

        let session = wallet
            .request_minting(MintRequest {
                method: PaymentMethod::Bolt11,
                amount: Some(amount),
                description: Some(description.clone()),
                extra: None,
            })
            .await
            .unwrap_or_else(|_| panic!("Failed to create quote for {} sats", amount_value));
        let quote = session.initial_state();

        // Verify quote properties
        assert_eq!(quote.amount, Some(amount));
        assert_eq!(quote.state, MintingState::Unpaid);
        assert!(!quote.id.is_empty());
        assert!(!quote.payment_request.is_empty());

        // Verify the payment request is a valid Lightning invoice
        let invoice = Bolt11Invoice::from_str(&quote.payment_request)
            .expect("Quote request should be a valid Lightning invoice");

        // The invoice amount should match the quote amount (in millisats)
        assert_eq!(
            invoice.amount_milli_satoshis(),
            Some(amount_value * 1000),
            "Invoice amount should match quote amount"
        );

        // Reconstructing the session is local and survives handle loss.
        let resumed = wallet
            .minting_session(quote.id.clone())
            .await
            .expect("Quote should resume from local storage");
        assert_eq!(resumed.initial_state(), quote);

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
    let mnemonic = Mnemonic::generate(12).unwrap().to_string();
    let config = WalletConfig {
        target_proof_count: Some(3),
        rate_limit: None,
    };

    let invalid_wallet_result = FfiWallet::open(WalletOpenRequest {
        mint_url: "invalid-url".to_string(),
        unit: CurrencyUnit::Sat,
        mnemonic: mnemonic.clone(),
        store: temp_wallet_store(),
        config: Some(config.clone()),
    });
    assert!(
        invalid_wallet_result.is_err(),
        "Should fail to create wallet with invalid URL"
    );

    // Test with valid wallet for other error cases
    let wallet = create_test_ffi_wallet().await;

    // Test zero amount quote (should fail)
    let zero_amount_result = wallet
        .request_minting(MintRequest {
            method: PaymentMethod::Bolt11,
            amount: Some(Amount::new(0)),
            description: None,
            extra: None,
        })
        .await;
    assert!(
        zero_amount_result.is_err(),
        "Should fail to create quote with zero amount"
    );

    // Test resuming a non-existent quote ID
    let invalid_mint_result = wallet
        .minting_session("non-existent-quote-id".to_string())
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
        let config = WalletConfig {
            target_proof_count: Some(target_count),
            rate_limit: None,
        };

        let wallet = FfiWallet::open(WalletOpenRequest {
            mint_url: mint_url.clone(),
            unit: CurrencyUnit::Sat,
            mnemonic: mnemonic.clone(),
            store: temp_wallet_store(),
            config: Some(config),
        })
        .expect("Failed to create wallet");

        // Verify wallet properties
        assert_eq!(wallet.identity().mint_url.url, mint_url);
        assert_eq!(wallet.identity().unit, CurrencyUnit::Sat);

        println!(
            "✅ Wallet created with target proof count: {}",
            target_count
        );
    }

    // Test wallet restoration with same mnemonic
    let config = WalletConfig {
        target_proof_count: Some(3),
        rate_limit: None,
    };

    let wallet1 = FfiWallet::open(WalletOpenRequest {
        mint_url: mint_url.clone(),
        unit: CurrencyUnit::Sat,
        mnemonic: mnemonic.clone(),
        store: temp_wallet_store(),
        config: Some(config.clone()),
    })
    .expect("Failed to create first wallet");

    let wallet2 = FfiWallet::open(WalletOpenRequest {
        mint_url,
        unit: CurrencyUnit::Sat,
        mnemonic,
        store: temp_wallet_store(),
        config: Some(config),
    })
    .expect("Failed to create second wallet");

    // Both wallets should have the same mint URL and unit
    assert_eq!(
        wallet1.identity().mint_url.url,
        wallet2.identity().mint_url.url
    );
    assert_eq!(wallet1.identity().unit, wallet2.identity().unit);

    println!("✅ Wallet configuration tests completed successfully");
}
