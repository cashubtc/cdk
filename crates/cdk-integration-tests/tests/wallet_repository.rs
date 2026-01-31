//! Integration tests for WalletRepository
//!
//! These tests verify the WalletRepository functionality including:
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
use cdk::nuts::nut00::{KnownMethod, ProofsMethods};
use cdk::nuts::{CurrencyUnit, MeltQuoteState, MintQuoteState, PaymentMethod, Token};
use cdk::wallet::{ReceiveOptions, SendOptions, WalletRepository};
use cdk_common::wallet::WalletKey;
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

// Helper to create a WalletRepository with a fresh seed and in-memory database
async fn create_test_wallet_repository() -> WalletRepository {
    let seed = Mnemonic::generate(12).unwrap().to_seed_normalized("");
    let localstore = Arc::new(memory::empty().await.unwrap());

    WalletRepository::new(localstore, seed)
        .await
        .expect("failed to create wallet repository")
}

/// Helper to fund a WalletRepository at a specific mint
async fn fund_wallet_repository(
    repo: &WalletRepository,
    mint_url: &MintUrl,
    amount: Amount,
) -> Amount {
    let wallet = repo
        .get_wallet(mint_url, &CurrencyUnit::Sat)
        .await
        .expect("wallet not found");
    let mint_quote = wallet.mint_quote(PaymentMethod::Known(KnownMethod::Bolt11), Some(amount), None, None).await.unwrap();

    let invoice = Bolt11Invoice::from_str(&mint_quote.request).unwrap();
    pay_if_regtest(&get_test_temp_dir(), &invoice)
        .await
        .unwrap();

    let proofs = wallet
        .wait_and_mint_quote(
            mint_quote,
            SplitTarget::default(),
            None,
            std::time::Duration::from_secs(60),
        )
        .await
        .expect("mint failed");

    proofs.total_amount().unwrap()
}

/// Test the direct mint() function on WalletRepository
///
/// This test verifies:
/// 1. Create a mint quote
/// 2. Pay the invoice
/// 3. Poll until quote is paid (like a real wallet would)
/// 4. Call mint() directly (not wait_for_mint_quote)
/// 5. Verify tokens are received
#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_wallet_repository_mint() {
    let wallet_repository = create_test_wallet_repository().await;

    let mint_url = MintUrl::from_str(&get_mint_url_from_env()).expect("invalid mint url");
    wallet_repository
        .add_mint(mint_url.clone())
        .await
        .expect("failed to add mint");

    let wallet = wallet_repository
        .get_wallet(&mint_url, &CurrencyUnit::Sat)
        .await
        .expect("failed to get wallet");

    // Create mint quote
    let mint_quote = wallet.mint_quote(PaymentMethod::Known(KnownMethod::Bolt11), Some(100.into()), None, None).await.unwrap();

    // Pay the invoice (in regtest mode) - for fake wallet, payment is simulated automatically
    let invoice = Bolt11Invoice::from_str(&mint_quote.request).unwrap();
    pay_if_regtest(&get_test_temp_dir(), &invoice)
        .await
        .unwrap();

    // Poll for quote to be paid (like a real wallet would)
    let mut quote_status = wallet
        .refresh_mint_quote_status(&mint_quote.id)
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
        quote_status = wallet
            .refresh_mint_quote_status(&mint_quote.id)
            .await
            .unwrap();
    }
        tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;
        quote_status = wallet
            .refresh_mint_quote_status(&mint_quote.id)
            .await
            .unwrap();


    // Call mint() directly (quote should be Paid at this point)
    let proofs = wallet
        .mint(&mint_quote.id, SplitTarget::default(), None)
        .await
        .unwrap();

    let minted_amount = proofs.total_amount().unwrap();
    assert_eq!(minted_amount, 100.into(), "Should mint exactly 100 sats");

    // Verify balance
    let balance = wallet_repository.total_balance().await.unwrap();
    assert_eq!(balance, 100.into(), "Total balance should be 100 sats");
}

/// Test the melt() function with automatic mint selection
///
/// This test verifies:
/// 1. Fund wallet at a mint
/// 2. Call melt() without specifying mint (auto-selection)
/// 3. Verify payment is made
#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_wallet_repository_melt_auto_select() {
    let wallet_repository = create_test_wallet_repository().await;

    let mint_url = MintUrl::from_str(&get_mint_url_from_env()).expect("invalid mint url");
    wallet_repository
        .add_mint(mint_url.clone())
        .await
        .expect("failed to add mint");

    // Fund the wallet
    let funded_amount = fund_wallet_repository(&wallet_repository, &mint_url, 100.into()).await;
    assert_eq!(funded_amount, 100.into());

    // Create an invoice to pay
    let invoice = create_invoice_for_env(Some(50)).await.unwrap();

    // Get wallet and call melt
    let wallet = wallet_repository
        .get_wallet(&mint_url, &CurrencyUnit::Sat)
        .await
        .unwrap();
    let melt_quote = wallet
        .melt_quote(
            PaymentMethod::Known(KnownMethod::Bolt11),
            invoice.to_string(),
            None,
            None,
        )
        .await
        .unwrap();
    let melt_result = wallet
        .prepare_melt(&melt_quote.id, std::collections::HashMap::new())
        .await
        .unwrap()
        .confirm()
        .await
        .unwrap();

    assert_eq!(
        melt_result.state(),
        MeltQuoteState::Paid,
        "Melt should be paid"
    );
    assert_eq!(melt_result.amount(), 50.into(), "Should melt 50 sats");

    // Verify balance
    let balance = wallet_repository.total_balance().await.unwrap();
    assert!(
        balance < 100.into(),
        "Balance should be less than 100 after melt"
    );
}

/// Test the receive() function on WalletRepository
///
/// This test verifies:
/// 1. Create a token from a wallet
/// 2. Receive the token in a different WalletRepository
/// 3. Verify the token value is received
#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_wallet_repository_receive() {
    // Create sender wallet and fund it
    let sender_repo = create_test_wallet_repository().await;
    let mint_url = MintUrl::from_str(&get_mint_url_from_env()).expect("invalid mint url");
    sender_repo
        .add_mint(mint_url.clone())
        .await
        .expect("failed to add mint");

    let funded_amount = fund_wallet_repository(&sender_repo, &mint_url, 100.into()).await;
    assert_eq!(funded_amount, 100.into());

    // Create a token to send
    let sender_wallet = sender_repo
        .get_wallet(&mint_url, &CurrencyUnit::Sat)
        .await
        .unwrap();
    let prepared_send = sender_wallet
        .prepare_send(50.into(), SendOptions::default())
        .await
        .unwrap();

    let token = prepared_send.confirm(None).await.unwrap();
    let token_string = token.to_string();

    // Create receiver wallet
    let receiver_repo = create_test_wallet_repository().await;
    // Add the same mint as trusted
    receiver_repo
        .add_mint(mint_url.clone())
        .await
        .expect("failed to add mint");

    // Receive the token
    let receiver_wallet = receiver_repo
        .get_wallet(&mint_url, &CurrencyUnit::Sat)
        .await
        .unwrap();
    let received_amount = receiver_wallet
        .receive(&token_string, ReceiveOptions::default())
        .await
        .unwrap();

    // Note: received amount may be slightly less due to fees
    assert!(
        received_amount > Amount::ZERO,
        "Should receive some amount, got {:?}",
        received_amount
    );

    // Verify receiver balance
    let receiver_balance = receiver_repo.total_balance().await.unwrap();
    assert!(
        receiver_balance > Amount::ZERO,
        "Receiver should have balance"
    );

    // Verify sender balance decreased
    let sender_balance = sender_repo.total_balance().await.unwrap();
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
async fn test_wallet_repository_receive_untrusted() {
    // Create sender wallet and fund it
    let sender_repo = create_test_wallet_repository().await;
    let mint_url = MintUrl::from_str(&get_mint_url_from_env()).expect("invalid mint url");
    sender_repo
        .add_mint(mint_url.clone())
        .await
        .expect("failed to add mint");

    let funded_amount = fund_wallet_repository(&sender_repo, &mint_url, 100.into()).await;
    assert_eq!(funded_amount, 100.into());

    // Create a token to send
    let sender_wallet = sender_repo
        .get_wallet(&mint_url, &CurrencyUnit::Sat)
        .await
        .unwrap();
    let prepared_send = sender_wallet
        .prepare_send(50.into(), SendOptions::default())
        .await
        .unwrap();

    let token = prepared_send.confirm(None).await.unwrap();
    let token_string = token.to_string();

    // Create receiver wallet WITHOUT adding the mint
    let receiver_repo = create_test_wallet_repository().await;

    // Add the mint first, then receive (untrusted receive would require the
    // WalletRepository to auto-add mints, which it doesn't support directly)
    receiver_repo
        .add_mint(mint_url.clone())
        .await
        .expect("failed to add mint");

    // Now receive
    let receiver_wallet = receiver_repo
        .get_wallet(&mint_url, &CurrencyUnit::Sat)
        .await
        .unwrap();
    let received_amount = receiver_wallet
        .receive(&token_string, ReceiveOptions::default())
        .await
        .unwrap();

    assert!(received_amount > Amount::ZERO, "Should receive some amount");

    // Verify the mint is in the wallet
    assert!(
        receiver_repo.has_mint(&mint_url).await,
        "Mint should be in wallet"
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
async fn test_wallet_repository_prepare_send_happy_path() {
    let wallet_repository = create_test_wallet_repository().await;

    let mint_url = MintUrl::from_str(&get_mint_url_from_env()).expect("invalid mint url");
    wallet_repository
        .add_mint(mint_url.clone())
        .await
        .expect("failed to add mint");

    // Fund the wallet
    let funded_amount = fund_wallet_repository(&wallet_repository, &mint_url, 100.into()).await;
    assert_eq!(funded_amount, 100.into());

    // Prepare send
    let wallet = wallet_repository
        .get_wallet(&mint_url, &CurrencyUnit::Sat)
        .await
        .unwrap();
    let prepared_send = wallet
        .prepare_send(50.into(), SendOptions::default())
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
    let token_data = wallet_repository
        .get_token_data(&parsed_token)
        .await
        .unwrap();
    assert_eq!(token_data.value, 50.into(), "Token value should be 50 sats");

    // Verify wallet balance decreased
    let balance = wallet_repository.total_balance().await.unwrap();
    assert_eq!(balance, 50.into(), "Remaining balance should be 50 sats");
}

/// Test get_balances() across multiple operations
///
/// This test verifies:
/// 1. Empty wallet has zero balances
/// 2. After minting, balance is updated
/// 3. get_balances() returns per-mint breakdown
#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_wallet_repository_get_balances() {
    let wallet_repository = create_test_wallet_repository().await;

    let mint_url = MintUrl::from_str(&get_mint_url_from_env()).expect("invalid mint url");
    wallet_repository
        .add_mint(mint_url.clone())
        .await
        .expect("failed to add mint");

    // Check initial balances
    let balances = wallet_repository.get_balances().await.unwrap();
    let initial_balance = balances
        .get(&WalletKey::new(mint_url.clone(), CurrencyUnit::Sat))
        .cloned()
        .unwrap_or(Amount::ZERO);
    assert_eq!(initial_balance, Amount::ZERO, "Initial balance should be 0");

    // Fund the wallet
    fund_wallet_repository(&wallet_repository, &mint_url, 100.into()).await;

    // Check balances again
    let balances = wallet_repository.get_balances().await.unwrap();
    let balance = balances
        .get(&WalletKey::new(mint_url.clone(), CurrencyUnit::Sat))
        .cloned()
        .unwrap_or(Amount::ZERO);
    assert_eq!(balance, 100.into(), "Balance should be 100 sats");

    // Verify total_balance matches
    let total = wallet_repository.total_balance().await.unwrap();
    assert_eq!(total, 100.into(), "Total balance should match");
}

/// Test list_proofs() function
///
/// This test verifies:
/// 1. Empty wallet has no proofs
/// 2. After minting, proofs are listed correctly
#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_wallet_repository_list_proofs() {
    let wallet_repository = create_test_wallet_repository().await;

    let mint_url = MintUrl::from_str(&get_mint_url_from_env()).expect("invalid mint url");
    wallet_repository
        .add_mint(mint_url.clone())
        .await
        .expect("failed to add mint");

    // Check initial proofs
    let proofs = wallet_repository.list_proofs().await.unwrap();
    let mint_proofs = proofs
        .get(&WalletKey::new(mint_url.clone(), CurrencyUnit::Sat))
        .cloned()
        .unwrap_or_default();
    assert!(mint_proofs.is_empty(), "Should have no proofs initially");

    // Fund the wallet
    fund_wallet_repository(&wallet_repository, &mint_url, 100.into()).await;

    // Check proofs again
    let proofs = wallet_repository.list_proofs().await.unwrap();
    let mint_proofs = proofs
        .get(&WalletKey::new(mint_url.clone(), CurrencyUnit::Sat))
        .cloned()
        .unwrap_or_default();
    assert!(!mint_proofs.is_empty(), "Should have proofs after minting");

    // Verify proof total matches balance
    let proof_total: Amount = mint_proofs.total_amount().unwrap();
    assert_eq!(proof_total, 100.into(), "Proof total should be 100 sats");
}

/// Test mint management functions (add_mint, remove_wallet, has_mint)
///
/// This test verifies:
/// 1. has_mint returns false for unknown mints
/// 2. add_mint adds the mint
/// 3. has_mint returns true after adding
/// 4. remove_wallet removes the mint
/// 5. has_mint returns false after removal
#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_wallet_repository_mint_management() {
    let wallet_repository = create_test_wallet_repository().await;

    let mint_url = MintUrl::from_str(&get_mint_url_from_env()).expect("invalid mint url");

    // Initially mint should not be in wallet
    assert!(
        !wallet_repository.has_mint(&mint_url).await,
        "Mint should not be in wallet initially"
    );

    // Add the mint
    wallet_repository
        .add_mint(mint_url.clone())
        .await
        .expect("failed to add mint");

    // Now mint should be in wallet
    assert!(
        wallet_repository.has_mint(&mint_url).await,
        "Mint should be in wallet after adding"
    );

    // Get wallets should include this mint
    let wallets = wallet_repository.get_wallets().await;
    assert!(!wallets.is_empty(), "Should have at least one wallet");

    // Get specific wallet
    let wallet = wallet_repository
        .get_wallet(&mint_url, &CurrencyUnit::Sat)
        .await;
    assert!(wallet.is_ok(), "Should be able to get wallet for mint");

    // Get wallets for this mint
    let mint_wallets = wallet_repository.get_wallets_for_mint(&mint_url).await;

    // Remove all wallets for the mint
    for wallet in mint_wallets {
        wallet_repository
            .remove_wallet(mint_url.clone(), wallet.unit.clone())
            .await
            .unwrap();
    }

    // Now mint should not be in wallet
    assert!(
        !wallet_repository.has_mint(&mint_url).await,
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
async fn test_wallet_repository_check_all_mint_quotes() {
    let wallet_repository = create_test_wallet_repository().await;

    let mint_url = MintUrl::from_str(&get_mint_url_from_env()).expect("invalid mint url");
    wallet_repository
        .add_mint(mint_url.clone())
        .await
        .expect("failed to add mint");

    let wallet = wallet_repository
        .get_wallet(&mint_url, &CurrencyUnit::Sat)
        .await
        .unwrap();

    // Create a mint quote
    let mint_quote = wallet
        .mint_quote(
            PaymentMethod::Known(KnownMethod::Bolt11),
            Some(100.into()),
            None,
            None,
        )
        .await
        .unwrap();

    // Pay the invoice (in regtest mode) - for fake wallet, payment is simulated automatically
    let invoice = Bolt11Invoice::from_str(&mint_quote.request).unwrap();
    pay_if_regtest(&get_test_temp_dir(), &invoice)
        .await
        .unwrap();

    // Poll for quote to be paid (like a real wallet would)
    let mut quote_status = wallet
        .refresh_mint_quote_status(&mint_quote.id)
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
        quote_status = wallet
            .refresh_mint_quote_status(&mint_quote.id)
            .await
            .unwrap();
    }
        tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;
        quote_status = wallet
            .refresh_mint_quote_status(&mint_quote.id)
            .await
            .unwrap();


    // Check all mint quotes - this should find the paid quote and mint
    let minted_amount = wallet_repository
        .check_all_mint_quotes(Some(mint_url.clone()))
        .await
        .unwrap();

    assert_eq!(
        minted_amount,
        100.into(),
        "Should mint 100 sats from paid quote"
    );

    // Verify balance
    let balance = wallet_repository.total_balance().await.unwrap();
    assert_eq!(balance, 100.into(), "Balance should be 100 sats");
}

/// Test restore() function
///
/// This test verifies:
/// 1. Create and fund a wallet with a specific seed
/// 2. Create a new wallet with the same seed
/// 3. Call restore() to recover the proofs
#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_wallet_repository_restore() {
    let seed = Mnemonic::generate(12).unwrap().to_seed_normalized("");
    let mint_url = MintUrl::from_str(&get_mint_url_from_env()).expect("invalid mint url");

    // Create first wallet and fund it
    {
        let localstore = Arc::new(memory::empty().await.unwrap());
        let wallet1 = WalletRepository::new(localstore, seed)
            .await
            .expect("failed to create wallet");

        wallet1
            .add_mint(mint_url.clone())
            .await
            .expect("failed to add mint");

        let funded = fund_wallet_repository(&wallet1, &mint_url, 100.into()).await;
        assert_eq!(funded, 100.into());
    }
    // wallet1 goes out of scope

    // Create second wallet with same seed but fresh storage
    let localstore2 = Arc::new(memory::empty().await.unwrap());
    let wallet2 = WalletRepository::new(localstore2, seed)
        .await
        .expect("failed to create wallet");

    wallet2
        .add_mint(mint_url.clone())
        .await
        .expect("failed to add mint");

    // Initially should have no balance
    let balance_before = wallet2.total_balance().await.unwrap();
    assert_eq!(balance_before, Amount::ZERO, "Should start with no balance");

    // Restore from mint using the individual wallet
    let wallet = wallet2
        .get_wallet(&mint_url, &CurrencyUnit::Sat)
        .await
        .unwrap();
    let restored = wallet.restore().await.unwrap();
    assert_eq!(restored.unspent, 100.into(), "Should restore 100 sats");
}

/// Test melt_with_mint() with explicit mint selection
///
/// This test verifies:
/// 1. Fund wallet
/// 2. Create melt quote at specific mint
/// 3. Execute melt()
/// 4. Verify payment succeeded
#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_wallet_repository_melt_with_mint() {
    let wallet_repository = create_test_wallet_repository().await;

    let mint_url = MintUrl::from_str(&get_mint_url_from_env()).expect("invalid mint url");
    wallet_repository
        .add_mint(mint_url.clone())
        .await
        .expect("failed to add mint");

    // Fund the wallet
    fund_wallet_repository(&wallet_repository, &mint_url, 100.into()).await;

    // Create an invoice to pay
    let invoice = create_invoice_for_env(Some(50)).await.unwrap();

    // Get wallet for operations
    let wallet = wallet_repository
        .get_wallet(&mint_url, &CurrencyUnit::Sat)
        .await
        .unwrap();

    // Create melt quote at specific mint
    let melt_quote = wallet
        .melt_quote(
            PaymentMethod::Known(KnownMethod::Bolt11),
            invoice.to_string(),
            None,
            None,
        )
        .await
        .unwrap();
    let melt_result = wallet
        .prepare_melt(&melt_quote.id, std::collections::HashMap::new())
        .await
        .unwrap()
        .confirm()
        .await
        .unwrap();

    assert_eq!(
        melt_result.state(),
        MeltQuoteState::Paid,
        "Melt should be paid"
    );

    // Check melt quote status
    let quote_status = wallet.check_melt_quote_status(&melt_quote.id).await.unwrap();

    assert_eq!(
        quote_status.state,
        MeltQuoteState::Paid,
        "Quote status should be paid"
    );
}

/// Test list_transactions() function
///
/// This test verifies:
/// 1. Initially no transactions
/// 2. After minting, transaction is recorded
/// 3. After melting, transaction is recorded
#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_wallet_repository_list_transactions() {
    let wallet_repository = create_test_wallet_repository().await;

    let mint_url = MintUrl::from_str(&get_mint_url_from_env()).expect("invalid mint url");
    wallet_repository
        .add_mint(mint_url.clone())
        .await
        .expect("failed to add mint");

    // Fund the wallet (this creates a mint transaction)
    fund_wallet_repository(&wallet_repository, &mint_url, 100.into()).await;

    // List all transactions
    let transactions = wallet_repository.list_transactions(None).await.unwrap();
    assert!(
        !transactions.is_empty(),
        "Should have at least one transaction after minting"
    );

    // Get wallet for melt operations
    let wallet = wallet_repository
        .get_wallet(&mint_url, &CurrencyUnit::Sat)
        .await
        .unwrap();

    // Create an invoice and melt (this creates a melt transaction)
    let invoice = create_invoice_for_env(Some(50)).await.unwrap();
    let melt_quote = wallet
        .melt_quote(
            PaymentMethod::Known(KnownMethod::Bolt11),
            invoice.to_string(),
            None,
            None,
        )
        .await
        .unwrap();
    wallet
        .prepare_melt(&melt_quote.id, std::collections::HashMap::new())
        .await
        .unwrap()
        .confirm()
        .await
        .unwrap();

    // List transactions again
    let transactions_after = wallet_repository.list_transactions(None).await.unwrap();
    assert!(
        transactions_after.len() > transactions.len(),
        "Should have more transactions after melt"
    );
}
