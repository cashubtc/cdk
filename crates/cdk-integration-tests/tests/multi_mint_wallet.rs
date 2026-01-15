//! Integration tests for MultiMintWallet
//!
//! These tests verify the multi-mint wallet functionality including:
//! - Basic mint/melt operations across multiple mints
//! - Token receive and send operations
//! - Automatic mint selection for melts
//! - Cross-mint transfers
//!
//! Tests use the fake wallet backend for deterministic behavior.

use std::env;
use std::path::PathBuf;
use std::str::FromStr;
use std::sync::Arc;

use bip39::Mnemonic;
use cdk::amount::{Amount, SplitTarget};
use cdk::mint_url::MintUrl;
use cdk::nuts::nut00::ProofsMethods;
use cdk::nuts::{CurrencyUnit, MeltQuoteState, MintQuoteState, Token};
use cdk::wallet::{MultiMintReceiveOptions, MultiMintWallet, SendOptions};
use cdk_integration_tests::{create_invoice_for_env, get_mint_url_from_env, pay_if_regtest};
use cdk_sqlite::wallet::memory;
use lightning_invoice::Bolt11Invoice;

// Helper function to get temp directory from environment or fallback
fn get_test_temp_dir() -> PathBuf {
    match env::var("CDK_ITESTS_DIR") {
        Ok(dir) => PathBuf::from(dir),
        Err(_) => panic!("Unknown test dir"),
    }
}

/// Helper to create a MultiMintWallet with a fresh seed and in-memory database
async fn create_test_multi_mint_wallet() -> MultiMintWallet {
    let seed = Mnemonic::generate(12).unwrap().to_seed_normalized("");
    let localstore = Arc::new(memory::empty().await.unwrap());

    MultiMintWallet::new(localstore, seed, CurrencyUnit::Sat)
        .await
        .expect("failed to create multi mint wallet")
}

/// Helper to fund a MultiMintWallet at a specific mint
async fn fund_multi_mint_wallet(
    wallet: &MultiMintWallet,
    mint_url: &MintUrl,
    amount: Amount,
) -> Amount {
    let mint_quote = wallet.mint_quote(mint_url, amount, None).await.unwrap();

    let invoice = Bolt11Invoice::from_str(&mint_quote.request).unwrap();
    pay_if_regtest(&get_test_temp_dir(), &invoice)
        .await
        .unwrap();

    let proofs = wallet
        .wait_for_mint_quote(mint_url, &mint_quote.id, SplitTarget::default(), None, 60)
        .await
        .expect("mint failed");

    proofs.total_amount().unwrap()
}

/// Test the direct mint() function on MultiMintWallet
///
/// This test verifies:
/// 1. Create a mint quote
/// 2. Pay the invoice
/// 3. Poll until quote is paid (like a real wallet would)
/// 4. Call mint() directly (not wait_for_mint_quote)
/// 5. Verify tokens are received
#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_multi_mint_wallet_mint() {
    let multi_mint_wallet = create_test_multi_mint_wallet().await;

    let mint_url = MintUrl::from_str(&get_mint_url_from_env()).expect("invalid mint url");
    multi_mint_wallet
        .add_mint(mint_url.clone())
        .await
        .expect("failed to add mint");

    // Create mint quote
    let mint_quote = multi_mint_wallet
        .mint_quote(&mint_url, 100.into(), None)
        .await
        .unwrap();

    // Pay the invoice (in regtest mode) - for fake wallet, payment is simulated automatically
    let invoice = Bolt11Invoice::from_str(&mint_quote.request).unwrap();
    pay_if_regtest(&get_test_temp_dir(), &invoice)
        .await
        .unwrap();

    // Poll for quote to be paid (like a real wallet would)
    let mut quote_status = multi_mint_wallet
        .check_mint_quote(&mint_url, &mint_quote.id)
        .await
        .unwrap();

    let timeout = tokio::time::Duration::from_secs(30);
    let start = tokio::time::Instant::now();
    while quote_status.state != MintQuoteState::Paid && quote_status.state != MintQuoteState::Issued
    {
        if start.elapsed() > timeout {
            panic!(
                "Timeout waiting for quote to be paid, state: {:?}",
                quote_status.state
            );
        }
        tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;
        quote_status = multi_mint_wallet
            .check_mint_quote(&mint_url, &mint_quote.id)
            .await
            .unwrap();
    }

    // Call mint() directly (quote should be Paid at this point)
    let proofs = multi_mint_wallet
        .mint(&mint_url, &mint_quote.id, None)
        .await
        .unwrap();

    let minted_amount = proofs.total_amount().unwrap();
    assert_eq!(minted_amount, 100.into(), "Should mint exactly 100 sats");

    // Verify balance
    let balance = multi_mint_wallet.total_balance().await.unwrap();
    assert_eq!(balance, 100.into(), "Total balance should be 100 sats");
}

/// Test the melt() function with automatic mint selection
///
/// This test verifies:
/// 1. Fund wallet at a mint
/// 2. Call melt() without specifying mint (auto-selection)
/// 3. Verify payment is made
#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_multi_mint_wallet_melt_auto_select() {
    let multi_mint_wallet = create_test_multi_mint_wallet().await;

    let mint_url = MintUrl::from_str(&get_mint_url_from_env()).expect("invalid mint url");
    multi_mint_wallet
        .add_mint(mint_url.clone())
        .await
        .expect("failed to add mint");

    // Fund the wallet
    let funded_amount = fund_multi_mint_wallet(&multi_mint_wallet, &mint_url, 100.into()).await;
    assert_eq!(funded_amount, 100.into());

    // Create an invoice to pay
    let invoice = create_invoice_for_env(Some(50)).await.unwrap();

    // Use melt() with auto-selection (no specific mint specified)
    let melt_result = multi_mint_wallet.melt(&invoice, None, None).await.unwrap();

    assert_eq!(
        melt_result.state(),
        MeltQuoteState::Paid,
        "Melt should be paid"
    );
    assert_eq!(melt_result.amount(), 50.into(), "Should melt 50 sats");

    // Verify balance decreased
    let balance = multi_mint_wallet.total_balance().await.unwrap();
    assert!(
        balance < 100.into(),
        "Balance should be less than 100 after melt"
    );
}

/// Test the receive() function on MultiMintWallet
///
/// This test verifies:
/// 1. Create a token from a wallet
/// 2. Receive the token in a different MultiMintWallet
/// 3. Verify the token value is received
#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_multi_mint_wallet_receive() {
    // Create sender wallet and fund it
    let sender_wallet = create_test_multi_mint_wallet().await;
    let mint_url = MintUrl::from_str(&get_mint_url_from_env()).expect("invalid mint url");
    sender_wallet
        .add_mint(mint_url.clone())
        .await
        .expect("failed to add mint");

    let funded_amount = fund_multi_mint_wallet(&sender_wallet, &mint_url, 100.into()).await;
    assert_eq!(funded_amount, 100.into());

    // Create a token to send
    let send_options = SendOptions::default();
    let prepared_send = sender_wallet
        .prepare_send(mint_url.clone(), 50.into(), send_options)
        .await
        .unwrap();

    let token = prepared_send.confirm(None).await.unwrap();
    let token_string = token.to_string();

    // Create receiver wallet
    let receiver_wallet = create_test_multi_mint_wallet().await;
    // Add the same mint as trusted
    receiver_wallet
        .add_mint(mint_url.clone())
        .await
        .expect("failed to add mint");

    // Receive the token
    let receive_options = MultiMintReceiveOptions::default();
    let received_amount = receiver_wallet
        .receive(&token_string, receive_options)
        .await
        .unwrap();

    // Note: received amount may be slightly less due to fees
    assert!(
        received_amount > Amount::ZERO,
        "Should receive some amount, got {:?}",
        received_amount
    );

    // Verify receiver balance
    let receiver_balance = receiver_wallet.total_balance().await.unwrap();
    assert!(
        receiver_balance > Amount::ZERO,
        "Receiver should have balance"
    );

    // Verify sender balance decreased
    let sender_balance = sender_wallet.total_balance().await.unwrap();
    assert!(
        sender_balance < 100.into(),
        "Sender balance should be less than 100 after send"
    );
}

/// Test the receive() function with allow_untrusted option
///
/// This test verifies:
/// 1. Create a token from a known mint
/// 2. Receive with a wallet that doesn't have the mint added
/// 3. With allow_untrusted=true, the mint should be added automatically
#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_multi_mint_wallet_receive_untrusted() {
    // Create sender wallet and fund it
    let sender_wallet = create_test_multi_mint_wallet().await;
    let mint_url = MintUrl::from_str(&get_mint_url_from_env()).expect("invalid mint url");
    sender_wallet
        .add_mint(mint_url.clone())
        .await
        .expect("failed to add mint");

    let funded_amount = fund_multi_mint_wallet(&sender_wallet, &mint_url, 100.into()).await;
    assert_eq!(funded_amount, 100.into());

    // Create a token to send
    let send_options = SendOptions::default();
    let prepared_send = sender_wallet
        .prepare_send(mint_url.clone(), 50.into(), send_options)
        .await
        .unwrap();

    let token = prepared_send.confirm(None).await.unwrap();
    let token_string = token.to_string();

    // Create receiver wallet WITHOUT adding the mint
    let receiver_wallet = create_test_multi_mint_wallet().await;

    // First, verify that receiving without allow_untrusted fails
    let receive_options = MultiMintReceiveOptions::default();
    let result = receiver_wallet
        .receive(&token_string, receive_options)
        .await;
    assert!(result.is_err(), "Should fail without allow_untrusted");

    // Now receive with allow_untrusted=true
    let receive_options = MultiMintReceiveOptions::default().allow_untrusted(true);
    let received_amount = receiver_wallet
        .receive(&token_string, receive_options)
        .await
        .unwrap();

    assert!(received_amount > Amount::ZERO, "Should receive some amount");

    // Verify the mint was added to the wallet
    assert!(
        receiver_wallet.has_mint(&mint_url).await,
        "Mint should be added to wallet"
    );
}

/// Test prepare_send() happy path
///
/// This test verifies:
/// 1. Fund wallet
/// 2. Call prepare_send() successfully
/// 3. Confirm the send and get a token
/// 4. Verify the token is valid
#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_multi_mint_wallet_prepare_send_happy_path() {
    let multi_mint_wallet = create_test_multi_mint_wallet().await;

    let mint_url = MintUrl::from_str(&get_mint_url_from_env()).expect("invalid mint url");
    multi_mint_wallet
        .add_mint(mint_url.clone())
        .await
        .expect("failed to add mint");

    // Fund the wallet
    let funded_amount = fund_multi_mint_wallet(&multi_mint_wallet, &mint_url, 100.into()).await;
    assert_eq!(funded_amount, 100.into());

    // Prepare send
    let send_options = SendOptions::default();
    let prepared_send = multi_mint_wallet
        .prepare_send(mint_url.clone(), 50.into(), send_options)
        .await
        .unwrap();

    // Get the token
    let token = prepared_send.confirm(None).await.unwrap();
    let token_string = token.to_string();

    // Verify the token can be parsed back
    let parsed_token = Token::from_str(&token_string).unwrap();
    let token_mint_url = parsed_token.mint_url().unwrap();
    assert_eq!(token_mint_url, mint_url, "Token mint URL should match");

    // Get token data to verify value
    let token_data = multi_mint_wallet
        .get_token_data(&parsed_token)
        .await
        .unwrap();
    assert_eq!(token_data.value, 50.into(), "Token value should be 50 sats");

    // Verify wallet balance decreased
    let balance = multi_mint_wallet.total_balance().await.unwrap();
    assert_eq!(balance, 50.into(), "Remaining balance should be 50 sats");
}

/// Test get_balances() across multiple operations
///
/// This test verifies:
/// 1. Empty wallet has zero balances
/// 2. After minting, balance is updated
/// 3. get_balances() returns per-mint breakdown
#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_multi_mint_wallet_get_balances() {
    let multi_mint_wallet = create_test_multi_mint_wallet().await;

    let mint_url = MintUrl::from_str(&get_mint_url_from_env()).expect("invalid mint url");
    multi_mint_wallet
        .add_mint(mint_url.clone())
        .await
        .expect("failed to add mint");

    // Check initial balances
    let balances = multi_mint_wallet.get_balances().await.unwrap();
    let initial_balance = balances.get(&mint_url).cloned().unwrap_or(Amount::ZERO);
    assert_eq!(initial_balance, Amount::ZERO, "Initial balance should be 0");

    // Fund the wallet
    fund_multi_mint_wallet(&multi_mint_wallet, &mint_url, 100.into()).await;

    // Check balances again
    let balances = multi_mint_wallet.get_balances().await.unwrap();
    let balance = balances.get(&mint_url).cloned().unwrap_or(Amount::ZERO);
    assert_eq!(balance, 100.into(), "Balance should be 100 sats");

    // Verify total_balance matches
    let total = multi_mint_wallet.total_balance().await.unwrap();
    assert_eq!(total, 100.into(), "Total balance should match");
}

/// Test list_proofs() function
///
/// This test verifies:
/// 1. Empty wallet has no proofs
/// 2. After minting, proofs are listed correctly
#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_multi_mint_wallet_list_proofs() {
    let multi_mint_wallet = create_test_multi_mint_wallet().await;

    let mint_url = MintUrl::from_str(&get_mint_url_from_env()).expect("invalid mint url");
    multi_mint_wallet
        .add_mint(mint_url.clone())
        .await
        .expect("failed to add mint");

    // Check initial proofs
    let proofs = multi_mint_wallet.list_proofs().await.unwrap();
    let mint_proofs = proofs.get(&mint_url).cloned().unwrap_or_default();
    assert!(mint_proofs.is_empty(), "Should have no proofs initially");

    // Fund the wallet
    fund_multi_mint_wallet(&multi_mint_wallet, &mint_url, 100.into()).await;

    // Check proofs again
    let proofs = multi_mint_wallet.list_proofs().await.unwrap();
    let mint_proofs = proofs.get(&mint_url).cloned().unwrap_or_default();
    assert!(!mint_proofs.is_empty(), "Should have proofs after minting");

    // Verify proof total matches balance
    let proof_total: Amount = mint_proofs.total_amount().unwrap();
    assert_eq!(proof_total, 100.into(), "Proof total should be 100 sats");
}

/// Test mint management functions (add_mint, remove_mint, has_mint)
///
/// This test verifies:
/// 1. has_mint returns false for unknown mints
/// 2. add_mint adds the mint
/// 3. has_mint returns true after adding
/// 4. remove_mint removes the mint
/// 5. has_mint returns false after removal
#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_multi_mint_wallet_mint_management() {
    let multi_mint_wallet = create_test_multi_mint_wallet().await;

    let mint_url = MintUrl::from_str(&get_mint_url_from_env()).expect("invalid mint url");

    // Initially mint should not be in wallet
    assert!(
        !multi_mint_wallet.has_mint(&mint_url).await,
        "Mint should not be in wallet initially"
    );

    // Add the mint
    multi_mint_wallet
        .add_mint(mint_url.clone())
        .await
        .expect("failed to add mint");

    // Now mint should be in wallet
    assert!(
        multi_mint_wallet.has_mint(&mint_url).await,
        "Mint should be in wallet after adding"
    );

    // Get wallets should include this mint
    let wallets = multi_mint_wallet.get_wallets().await;
    assert!(!wallets.is_empty(), "Should have at least one wallet");

    // Get specific wallet
    let wallet = multi_mint_wallet.get_wallet(&mint_url).await;
    assert!(wallet.is_some(), "Should be able to get wallet for mint");

    // Remove the mint
    multi_mint_wallet.remove_mint(&mint_url).await;

    // Now mint should not be in wallet
    assert!(
        !multi_mint_wallet.has_mint(&mint_url).await,
        "Mint should not be in wallet after removal"
    );
}

/// Test check_all_mint_quotes() function
///
/// This test verifies:
/// 1. Create a mint quote
/// 2. Pay the quote
/// 3. Poll until quote is paid (like a real wallet would)
/// 4. check_all_mint_quotes() processes paid quotes
#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_multi_mint_wallet_check_all_mint_quotes() {
    let multi_mint_wallet = create_test_multi_mint_wallet().await;

    let mint_url = MintUrl::from_str(&get_mint_url_from_env()).expect("invalid mint url");
    multi_mint_wallet
        .add_mint(mint_url.clone())
        .await
        .expect("failed to add mint");

    // Create a mint quote
    let mint_quote = multi_mint_wallet
        .mint_quote(&mint_url, 100.into(), None)
        .await
        .unwrap();

    // Pay the invoice (in regtest mode) - for fake wallet, payment is simulated automatically
    let invoice = Bolt11Invoice::from_str(&mint_quote.request).unwrap();
    pay_if_regtest(&get_test_temp_dir(), &invoice)
        .await
        .unwrap();

    // Poll for quote to be paid (like a real wallet would)
    let mut quote_status = multi_mint_wallet
        .check_mint_quote(&mint_url, &mint_quote.id)
        .await
        .unwrap();

    let timeout = tokio::time::Duration::from_secs(30);
    let start = tokio::time::Instant::now();
    while quote_status.state != MintQuoteState::Paid {
        if start.elapsed() > timeout {
            panic!(
                "Timeout waiting for quote to be paid, state: {:?}",
                quote_status.state
            );
        }
        tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;
        quote_status = multi_mint_wallet
            .check_mint_quote(&mint_url, &mint_quote.id)
            .await
            .unwrap();
    }

    // Check all mint quotes - this should find the paid quote and mint
    let minted_amount = multi_mint_wallet
        .check_all_mint_quotes(Some(mint_url.clone()))
        .await
        .unwrap();

    assert_eq!(
        minted_amount,
        100.into(),
        "Should mint 100 sats from paid quote"
    );

    // Verify balance
    let balance = multi_mint_wallet.total_balance().await.unwrap();
    assert_eq!(balance, 100.into(), "Balance should be 100 sats");
}

/// Test restore() function
///
/// This test verifies:
/// 1. Create and fund a wallet with a specific seed
/// 2. Create a new wallet with the same seed
/// 3. Call restore() to recover the proofs
#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_multi_mint_wallet_restore() {
    let seed = Mnemonic::generate(12).unwrap().to_seed_normalized("");
    let mint_url = MintUrl::from_str(&get_mint_url_from_env()).expect("invalid mint url");

    // Create first wallet and fund it
    {
        let localstore = Arc::new(memory::empty().await.unwrap());
        let wallet1 = MultiMintWallet::new(localstore, seed, CurrencyUnit::Sat)
            .await
            .expect("failed to create wallet");

        wallet1
            .add_mint(mint_url.clone())
            .await
            .expect("failed to add mint");

        let funded = fund_multi_mint_wallet(&wallet1, &mint_url, 100.into()).await;
        assert_eq!(funded, 100.into());
    }
    // wallet1 goes out of scope

    // Create second wallet with same seed but fresh storage
    let localstore2 = Arc::new(memory::empty().await.unwrap());
    let wallet2 = MultiMintWallet::new(localstore2, seed, CurrencyUnit::Sat)
        .await
        .expect("failed to create wallet");

    wallet2
        .add_mint(mint_url.clone())
        .await
        .expect("failed to add mint");

    // Initially should have no balance
    let balance_before = wallet2.total_balance().await.unwrap();
    assert_eq!(balance_before, Amount::ZERO, "Should start with no balance");

    // Restore from mint
    let restored = wallet2.restore(&mint_url).await.unwrap();
    assert_eq!(restored, 100.into(), "Should restore 100 sats");
}

/// Test melt_with_mint() with explicit mint selection
///
/// This test verifies:
/// 1. Fund wallet
/// 2. Create melt quote at specific mint
/// 3. Execute melt_with_mint()
/// 4. Verify payment succeeded
#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_multi_mint_wallet_melt_with_mint() {
    let multi_mint_wallet = create_test_multi_mint_wallet().await;

    let mint_url = MintUrl::from_str(&get_mint_url_from_env()).expect("invalid mint url");
    multi_mint_wallet
        .add_mint(mint_url.clone())
        .await
        .expect("failed to add mint");

    // Fund the wallet
    fund_multi_mint_wallet(&multi_mint_wallet, &mint_url, 100.into()).await;

    // Create an invoice to pay
    let invoice = create_invoice_for_env(Some(50)).await.unwrap();

    // Create melt quote at specific mint
    let melt_quote = multi_mint_wallet
        .melt_quote(&mint_url, invoice, None)
        .await
        .unwrap();

    // Execute melt with specific mint
    let melt_result = multi_mint_wallet
        .melt_with_mint(&mint_url, &melt_quote.id)
        .await
        .unwrap();

    assert_eq!(
        melt_result.state(),
        MeltQuoteState::Paid,
        "Melt should be paid"
    );

    // Check melt quote status
    let quote_status = multi_mint_wallet
        .check_melt_quote(&mint_url, &melt_quote.id)
        .await
        .unwrap();

    assert_eq!(
        quote_status.state,
        MeltQuoteState::Paid,
        "Quote status should be paid"
    );
}

/// Test unit() function returns correct currency unit
#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_multi_mint_wallet_unit() {
    let seed = Mnemonic::generate(12).unwrap().to_seed_normalized("");
    let localstore = Arc::new(memory::empty().await.unwrap());

    let wallet = MultiMintWallet::new(localstore, seed, CurrencyUnit::Sat)
        .await
        .expect("failed to create wallet");

    assert_eq!(wallet.unit(), &CurrencyUnit::Sat, "Unit should be Sat");
}

/// Test list_transactions() function
///
/// This test verifies:
/// 1. Initially no transactions
/// 2. After minting, transaction is recorded
/// 3. After melting, transaction is recorded
#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_multi_mint_wallet_list_transactions() {
    let multi_mint_wallet = create_test_multi_mint_wallet().await;

    let mint_url = MintUrl::from_str(&get_mint_url_from_env()).expect("invalid mint url");
    multi_mint_wallet
        .add_mint(mint_url.clone())
        .await
        .expect("failed to add mint");

    // Fund the wallet (this creates a mint transaction)
    fund_multi_mint_wallet(&multi_mint_wallet, &mint_url, 100.into()).await;

    // List all transactions
    let transactions = multi_mint_wallet.list_transactions(None).await.unwrap();
    assert!(
        !transactions.is_empty(),
        "Should have at least one transaction after minting"
    );

    // Create an invoice and melt (this creates a melt transaction)
    let invoice = create_invoice_for_env(Some(50)).await.unwrap();
    let melt_quote = multi_mint_wallet
        .melt_quote(&mint_url, invoice, None)
        .await
        .unwrap();
    multi_mint_wallet
        .melt_with_mint(&mint_url, &melt_quote.id)
        .await
        .unwrap();

    // List transactions again
    let transactions_after = multi_mint_wallet.list_transactions(None).await.unwrap();
    assert!(
        transactions_after.len() > transactions.len(),
        "Should have more transactions after melt"
    );
}
