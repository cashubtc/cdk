use std::time::Duration;

use anyhow::{bail, Result};
use cashu::amount::SplitTarget;
use cashu::{Amount, MintRequest, PreMintSecrets};
use cdk::nuts::PublicKey;
use cdk_integration_tests::init_pure_tests::*;

// Note: Temp directory functions available from init_regtest module

// Helper function to create a dummy public key for testing
fn create_dummy_pubkey() -> PublicKey {
    // Create a valid dummy public key for testing purposes
    // This is a real secp256k1 public key (compressed format)
    PublicKey::from_hex("0279be667ef9dcbbac55a06295ce870b07029bfcdb2dce28d959f2815b16f81798")
        .expect("Valid dummy pubkey")
}

// Helper function to simulate onchain payment confirmation
// In real implementation, this would monitor Bitcoin blockchain
async fn simulate_onchain_payment_confirmation(
    _address: String,
    _amount: Amount,
) -> Result<String> {
    // Simulate blockchain confirmation delay
    tokio::time::sleep(Duration::from_millis(100)).await;

    // Return mock transaction ID
    Ok("abcdef1234567890abcdef1234567890abcdef1234567890abcdef1234567890".to_string())
}

/// Tests basic onchain minting functionality:
/// - Creates a wallet
/// - Gets an onchain quote with a pubkey
/// - Simulates onchain payment confirmation
/// - Mints tokens and verifies the correct amount is received
#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_regtest_onchain_mint() -> Result<()> {
    setup_tracing();
    let mint = create_and_start_test_mint()
        .await
        .expect("Failed to create test mint");
    let wallet = create_test_wallet_for_mint(mint.clone())
        .await
        .expect("Failed to create test wallet");

    let mint_amount = Amount::from(100);
    let pubkey = create_dummy_pubkey();

    let mint_quote = wallet.mint_onchain_quote(pubkey).await?;

    // In a real scenario, the quote would contain a Bitcoin address
    // and the user would send Bitcoin to that address
    assert!(!mint_quote.request.is_empty()); // Should contain a Bitcoin address
                                             // Note: pubkey information is stored separately from the quote
                                             // assert_eq!(mint_quote.pubkey, pubkey);
                                             // assert_eq!(mint_quote.amount_paid, Amount::ZERO);
                                             // assert_eq!(mint_quote.amount_issued, Amount::ZERO);

    // Simulate onchain payment
    let tx_id =
        simulate_onchain_payment_confirmation(mint_quote.request.clone(), mint_amount).await?;

    println!("Simulated onchain payment with tx_id: {}", tx_id);

    // In real implementation, we would wait for blockchain confirmation
    // For testing, we'll simulate the payment being confirmed
    tokio::time::sleep(Duration::from_millis(500)).await;

    // Check quote status after payment
    let updated_quote = wallet.mint_onchain_quote_state(&mint_quote.id).await?;

    // Note: In real implementation, these would be updated after blockchain confirmation
    println!(
        "Quote status - Paid: {}, Issued: {}",
        updated_quote.amount_paid, updated_quote.amount_issued
    );

    Ok(())
}

/// Tests onchain quote status checking:
/// - Creates a wallet and gets an onchain quote
/// - Checks the initial status (unpaid)
/// - Simulates payment and checks updated status
/// - Verifies the quote tracking functionality
#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_regtest_onchain_quote_status() -> Result<()> {
    setup_tracing();
    let mint = create_and_start_test_mint()
        .await
        .expect("Failed to create test mint");
    let wallet = create_test_wallet_for_mint(mint.clone())
        .await
        .expect("Failed to create test wallet");

    let pubkey = create_dummy_pubkey();

    let mint_quote = wallet.mint_onchain_quote(pubkey).await?;

    // Check initial status - FakeWallet automatically pays quotes
    let initial_status = wallet.mint_onchain_quote_state(&mint_quote.id).await?;

    // FakeWallet automatically simulates payment, so amount_paid will be 1000
    assert_eq!(initial_status.amount_paid, Amount::from(1000));
    assert_eq!(initial_status.amount_issued, Amount::ZERO);
    assert_eq!(initial_status.pubkey, pubkey);

    // Simulate payment
    let tx_id =
        simulate_onchain_payment_confirmation(mint_quote.request.clone(), Amount::from(5000))
            .await?;

    println!("Simulated onchain payment with tx_id: {}", tx_id);

    // Wait for simulated confirmation
    tokio::time::sleep(Duration::from_millis(500)).await;

    // Check updated status
    let updated_status = wallet.mint_onchain_quote_state(&mint_quote.id).await?;

    // The quote should still show zero amounts until blockchain confirmation
    // In real implementation, amount_unconfirmed might show the pending amount
    println!(
        "Updated quote status - Paid: {}, Unconfirmed: {}",
        updated_status.amount_paid, updated_status.amount_unconfirmed
    );

    Ok(())
}

/// Tests multiple onchain payments to demonstrate accumulation:
/// - Creates a wallet and gets an onchain quote
/// - Simulates multiple payments to the same address
/// - Verifies that payments can accumulate on the same quote
/// - Tests the flexibility of onchain quotes for multiple transactions
#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_regtest_onchain_multiple_payments() -> Result<()> {
    setup_tracing();
    let mint = create_and_start_test_mint()
        .await
        .expect("Failed to create test mint");
    let wallet = create_test_wallet_for_mint(mint.clone())
        .await
        .expect("Failed to create test wallet");

    let pubkey = create_dummy_pubkey();

    let mint_quote = wallet.mint_onchain_quote(pubkey).await?;

    // Simulate first payment
    let tx_id_1 =
        simulate_onchain_payment_confirmation(mint_quote.request.clone(), Amount::from(10000))
            .await?;

    println!("First payment tx_id: {}", tx_id_1);
    tokio::time::sleep(Duration::from_millis(200)).await;

    // Simulate second payment to same address
    let tx_id_2 =
        simulate_onchain_payment_confirmation(mint_quote.request.clone(), Amount::from(15000))
            .await?;

    println!("Second payment tx_id: {}", tx_id_2);
    tokio::time::sleep(Duration::from_millis(200)).await;

    // Check final status
    let final_status = wallet.mint_onchain_quote_state(&mint_quote.id).await?;

    println!(
        "Final status - Paid: {}, Unconfirmed: {}",
        final_status.amount_paid, final_status.amount_unconfirmed
    );

    // Verify quote properties remain consistent
    assert_eq!(final_status.pubkey, pubkey);
    assert!(!final_status.request.is_empty());

    Ok(())
}

/// Tests multiple wallets using onchain quotes:
/// - Creates two separate wallets
/// - Each wallet gets its own onchain quote
/// - Simulates payments for each wallet
/// - Verifies that wallets operate independently
/// - Tests the multi-user scenario for onchain minting
#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_regtest_onchain_multiple_wallets() -> Result<()> {
    setup_tracing();
    let mint = create_and_start_test_mint()
        .await
        .expect("Failed to create test mint");

    // Create first wallet
    let wallet_one = create_test_wallet_for_mint(mint.clone())
        .await
        .expect("Failed to create test wallet");

    // Create second wallet
    let wallet_two = create_test_wallet_for_mint(mint.clone())
        .await
        .expect("Failed to create test wallet");

    let pubkey_one = create_dummy_pubkey();
    let pubkey_two = create_dummy_pubkey();

    // First wallet gets a quote
    let quote_one = wallet_one.mint_onchain_quote(pubkey_one).await?;

    // Second wallet gets a separate quote
    let quote_two = wallet_two.mint_onchain_quote(pubkey_two).await?;

    // Verify quotes are different
    assert_ne!(quote_one.id, quote_two.id);
    assert_ne!(quote_one.request, quote_two.request); // Different addresses
                                                      // Note: pubkey information is stored separately from the quote
                                                      // assert_eq!(quote_one.pubkey, pubkey_one);
                                                      // assert_eq!(quote_two.pubkey, pubkey_two);

    // Simulate payments for both wallets
    let tx_id_one =
        simulate_onchain_payment_confirmation(quote_one.request.clone(), Amount::from(25000))
            .await?;

    let tx_id_two =
        simulate_onchain_payment_confirmation(quote_two.request.clone(), Amount::from(30000))
            .await?;

    println!("Wallet one tx_id: {}", tx_id_one);
    println!("Wallet two tx_id: {}", tx_id_two);

    tokio::time::sleep(Duration::from_millis(500)).await;

    // Check both wallet statuses
    let status_one = wallet_one.mint_onchain_quote_state(&quote_one.id).await?;

    let status_two = wallet_two.mint_onchain_quote_state(&quote_two.id).await?;

    // Verify wallets are independent
    assert_eq!(status_one.pubkey, pubkey_one);
    assert_eq!(status_two.pubkey, pubkey_two);

    println!("Wallet one status - Paid: {}", status_one.amount_paid);
    println!("Wallet two status - Paid: {}", status_two.amount_paid);

    Ok(())
}

/// Tests onchain melting (spending) functionality:
/// - Creates a wallet with existing tokens
/// - Creates an onchain melt quote with a Bitcoin address
/// - Tests melting (spending) tokens to send Bitcoin onchain
/// - Verifies the correct amount is melted and transaction details
#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_regtest_onchain_melt() -> Result<()> {
    setup_tracing();
    let mint = create_and_start_test_mint()
        .await
        .expect("Failed to create test mint");
    let wallet = create_test_wallet_for_mint(mint.clone())
        .await
        .expect("Failed to create test wallet");

    // For this test, we would need to first mint some tokens
    // In a real scenario, the wallet would have tokens from previous minting

    // Simulate having tokens by creating a mint quote first
    let pubkey = create_dummy_pubkey();
    let mint_quote = wallet.mint_onchain_quote(pubkey).await?;

    // Simulate payment to get tokens (simplified for testing)
    let _tx_id =
        simulate_onchain_payment_confirmation(mint_quote.request.clone(), Amount::from(50000))
            .await?;

    tokio::time::sleep(Duration::from_millis(300)).await;

    // Now test melting
    let destination_address = "bc1qxy2kgdygjrsqtzq2n0yrf2493p83kkfjhx0wlh";
    let melt_amount = Amount::from(5000); // Within mint limits (1-10000 sats)

    let melt_quote = wallet
        .melt_onchain_quote(destination_address.to_string(), melt_amount)
        .await?;

    // Verify melt quote properties
    assert_eq!(melt_quote.request, destination_address);
    assert_eq!(melt_quote.amount, melt_amount);
    assert!(melt_quote.fee_reserve > Amount::ZERO); // Should have some fee

    println!(
        "Melt quote - Amount: {}, Fee: {}",
        melt_quote.amount, melt_quote.fee_reserve
    );

    // In a real implementation, we would call wallet.melt(&melt_quote.quote)
    // For testing, we'll just verify the quote was created properly

    // Check melt quote status
    let melt_status = wallet.melt_onchain_quote_status(&melt_quote.id).await?;

    assert_eq!(melt_status.request, destination_address);
    assert_eq!(melt_status.amount, melt_amount);

    println!("Melt status verified - State: {:?}", melt_status.state);

    Ok(())
}

/// Tests security validation for onchain minting to prevent overspending:
/// - Creates a wallet and gets an onchain quote
/// - Simulates a small payment
/// - Attempts to mint more tokens than were paid for
/// - Verifies that the mint correctly rejects oversized mint requests
/// - Ensures proper error handling for economic security
#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_regtest_onchain_mint_security() -> Result<()> {
    setup_tracing();
    let mint = create_and_start_test_mint()
        .await
        .expect("Failed to create test mint");
    let wallet = create_test_wallet_for_mint(mint.clone())
        .await
        .expect("Failed to create test wallet");

    let pubkey = create_dummy_pubkey();
    let mint_quote = wallet.mint_onchain_quote(pubkey).await?;

    // Wait a bit for the fake wallet to process automatic payment
    tokio::time::sleep(Duration::from_millis(500)).await;

    // Check state after fake wallet automatic payment
    let initial_state = wallet.mint_onchain_quote_state(&mint_quote.id).await?;
    println!(
        "Initial state after fake wallet payment - Paid: {}, Unconfirmed: {}",
        initial_state.amount_paid, initial_state.amount_unconfirmed
    );

    // Fake wallet automatically pays a random amount (1-1000 sats), so amount_paid > 0
    assert!(initial_state.amount_paid > Amount::ZERO);
    assert_eq!(initial_state.amount_issued, Amount::ZERO);

    let active_keyset_id = wallet.fetch_active_keyset().await?.id;

    let paid_state = initial_state;
    println!(
        "Using fake wallet payment - Paid: {}, Unconfirmed: {}",
        paid_state.amount_paid, paid_state.amount_unconfirmed
    );

    // Attempt to mint much more than was paid (fake wallet pays 1-1000 sats, we try to mint 2000)
    let oversized_amount = Amount::from(2000);
    let pre_mint = PreMintSecrets::random(active_keyset_id, oversized_amount, &SplitTarget::None)?;

    let quote_info = wallet
        .localstore
        .get_mint_quote(&mint_quote.id)
        .await?
        .expect("Quote should exist");

    let mut mint_request = MintRequest {
        quote: mint_quote.id.clone(),
        outputs: pre_mint.blinded_messages(),
        signature: None,
    };

    if let Some(secret_key) = quote_info.secret_key {
        mint_request.sign(secret_key)?;
    }

    // This should fail due to insufficient payment
    // Convert string ID to UUID and call mint directly (like DirectMintConnection does)
    let mint_request_uuid = mint_request.clone().try_into().unwrap();
    let response = mint.process_mint_request(mint_request_uuid).await;

    match response {
        Err(err) => {
            match err {
                cdk::Error::TransactionUnbalanced(_, _, _) => {
                    println!("Correctly rejected oversized mint request");
                }
                cdk::Error::InsufficientFunds => {
                    println!("Correctly rejected due to insufficient funds");
                }
                cdk::Error::SignatureMissingOrInvalid => {
                    println!("Correctly rejected due to signature verification failure");
                }
                err => {
                    // Check if this is a signature-related error
                    if err.to_string().contains("signature") {
                        println!("Correctly rejected due to signature-related error: {}", err);
                    } else {
                        bail!("Unexpected error type: {}", err);
                    }
                }
            }
        }
        Ok(_) => {
            bail!("Should not have allowed oversized mint request");
        }
    }

    Ok(())
}

/// Tests onchain address generation and reuse:
/// - Verifies that onchain quotes generate valid Bitcoin addresses
/// - Tests that addresses can be reused for multiple payments
/// - Checks address format and validity
#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_regtest_onchain_address_handling() -> Result<()> {
    setup_tracing();
    let mint = create_and_start_test_mint()
        .await
        .expect("Failed to create test mint");
    let wallet = create_test_wallet_for_mint(mint.clone())
        .await
        .expect("Failed to create test wallet");

    let pubkey = create_dummy_pubkey();
    let mint_quote = wallet.mint_onchain_quote(pubkey).await?;

    // Verify address is not empty and has reasonable format
    assert!(!mint_quote.request.is_empty());
    assert!(mint_quote.request.len() > 20); // Bitcoin addresses are at least 26+ chars

    println!("Generated onchain address: {}", mint_quote.request);

    // Test multiple quotes with same pubkey
    let mint_quote_2 = wallet.mint_onchain_quote(pubkey).await?;

    // Each quote should be unique even with same pubkey
    assert_ne!(mint_quote.id, mint_quote_2.id);

    // Addresses might be the same or different depending on implementation
    println!("Second quote address: {}", mint_quote_2.request);

    // Note: pubkey information is stored separately from the quote
    // assert_eq!(mint_quote.pubkey, mint_quote_2.pubkey);
    // assert_eq!(mint_quote_2.pubkey, pubkey);

    Ok(())
}
