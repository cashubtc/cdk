//! Integration tests for WalletManager
//!
//! These tests verify the WalletManager functionality including:
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
use cdk::amount::Amount;
use cdk::mint_url::MintUrl;
use cdk::nuts::nut00::ProofsMethods;
use cdk::nuts::{CurrencyUnit, Token};
use cdk::wallet::history::HistoryQuery;
use cdk::wallet::mint::{MintRequest, MintState};
use cdk::wallet::operation::SyncPolicy;
use cdk::wallet::payment::{PaymentQuoteRequest, PaymentState, PaymentTarget};
use cdk::wallet::receive::ReceiveRequest;
use cdk::wallet::send::SendRequest;
use cdk::wallet::{RestoreRequest, WalletIdentity, WalletManager, WalletManagerBuilder};
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

// Helper to create a WalletManager with a fresh seed and in-memory database
async fn create_test_wallet_manager() -> cdk::wallet::WalletManager {
    let seed = Mnemonic::generate(12).unwrap().to_seed_normalized("");
    let localstore = Arc::new(memory::empty().await.unwrap());

    WalletManagerBuilder::new()
        .with_store(localstore)
        .with_seed(seed)
        .build()
        .await
        .expect("failed to create wallet manager")
}

/// Helper to fund a WalletManager at a specific mint
async fn fund_wallet_manager(
    manager: &WalletManager,
    mint_url: &MintUrl,
    amount: Amount,
) -> Amount {
    let wallet = manager
        .wallet(WalletIdentity::new(mint_url.clone(), CurrencyUnit::Sat))
        .await
        .expect("wallet not found");
    let session = wallet
        .request_mint(MintRequest::bolt11(amount))
        .await
        .unwrap();

    let invoice = Bolt11Invoice::from_str(&session.initial_state().payment_request).unwrap();
    pay_if_regtest(&get_test_temp_dir(), &invoice)
        .await
        .unwrap();

    let receipt = session
        .wait(std::time::Duration::from_secs(60))
        .await
        .expect("mint failed");

    receipt.amount
}

/// Test the incoming-payment workflow through `WalletManager`.
///
/// This test verifies:
/// 1. Create a mint quote
/// 2. Pay the invoice
/// 3. Poll until quote is paid (like a real wallet would)
/// 4. Claim the paid session
/// 5. Verify tokens are received
#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_wallet_manager_mint() {
    let wallet_manager = create_test_wallet_manager().await;

    let mint_url = MintUrl::from_str(&get_mint_url_from_env()).expect("invalid mint url");
    wallet_manager
        .register_mint(mint_url.clone())
        .await
        .expect("failed to add mint");

    let wallet = wallet_manager
        .wallet(WalletIdentity::new(mint_url.clone(), CurrencyUnit::Sat))
        .await
        .expect("failed to get wallet");

    // Create mint quote
    let session = wallet
        .request_mint(MintRequest::bolt11(100.into()))
        .await
        .unwrap();

    // Pay the invoice (in regtest mode) - for fake wallet, payment is simulated automatically
    let invoice = Bolt11Invoice::from_str(&session.initial_state().payment_request).unwrap();
    pay_if_regtest(&get_test_temp_dir(), &invoice)
        .await
        .unwrap();

    // Poll for quote to be paid (like a real wallet would)
    let mut quote_status = session.refresh().await.unwrap();

    let timeout = tokio::time::Duration::from_secs(30);
    let start = tokio::time::Instant::now();
    while quote_status.state != MintState::Paid && quote_status.state != MintState::Issued {
        if start.elapsed() > timeout {
            panic!(
                "Timeout waiting for quote to be paid, state: {:?}",
                quote_status.state
            );
        }
        tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;
        quote_status = session.refresh().await.unwrap();
    }
    tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;
    let _ = session.refresh().await.unwrap();

    // Claim the paid session.
    let minted_amount = session.claim().await.unwrap().amount;
    assert_eq!(minted_amount, 100.into(), "Should mint exactly 100 sats");

    // Verify balance
    let balances = wallet_manager.balance_totals().await.unwrap();
    let balance = balances
        .get(&CurrencyUnit::Sat)
        .copied()
        .unwrap_or(Amount::ZERO);
    assert_eq!(balance, 100.into(), "Total balance should be 100 sats");
}

/// Test the payment workflow with manager-selected wallet access.
///
/// This test verifies:
/// 1. Fund wallet at a mint
/// 2. Quote, prepare, and confirm the payment
/// 3. Verify payment is made
#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_wallet_manager_payment_workflow() {
    let wallet_manager = create_test_wallet_manager().await;

    let mint_url = MintUrl::from_str(&get_mint_url_from_env()).expect("invalid mint url");
    wallet_manager
        .register_mint(mint_url.clone())
        .await
        .expect("failed to add mint");

    // Fund the wallet
    let funded_amount = fund_wallet_manager(&wallet_manager, &mint_url, 100.into()).await;
    assert_eq!(funded_amount, 100.into());

    // Create an invoice to pay
    let invoice = create_invoice_for_env(Some(50)).await.unwrap();

    // Get the wallet and complete the payment workflow.
    let wallet = wallet_manager
        .wallet(WalletIdentity::new(mint_url.clone(), CurrencyUnit::Sat))
        .await
        .unwrap();
    let payment = wallet
        .quote_payment(PaymentQuoteRequest::new(PaymentTarget::bolt11(invoice)))
        .await
        .unwrap()
        .into_single()
        .unwrap();
    let melt_result = payment.prepare().await.unwrap().execute().await.unwrap();

    assert_eq!(melt_result.amount, 50.into(), "Should melt 50 sats");

    // Verify balance
    let balances = wallet_manager.balance_totals().await.unwrap();
    let balance = balances
        .get(&CurrencyUnit::Sat)
        .copied()
        .unwrap_or(Amount::ZERO);
    assert!(
        balance < 100.into(),
        "Balance should be less than 100 after melt"
    );
}

/// Test the receive() function on WalletManager
///
/// This test verifies:
/// 1. Create a token from a wallet
/// 2. Receive the token in a different WalletManager
/// 3. Verify the token value is received
#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_wallet_manager_receive() {
    // Create sender wallet and fund it
    let sender_manager = create_test_wallet_manager().await;
    let mint_url = MintUrl::from_str(&get_mint_url_from_env()).expect("invalid mint url");
    sender_manager
        .register_mint(mint_url.clone())
        .await
        .expect("failed to add mint");

    let funded_amount = fund_wallet_manager(&sender_manager, &mint_url, 100.into()).await;
    assert_eq!(funded_amount, 100.into());

    // Create a token to send
    let sender_wallet = sender_manager
        .wallet(WalletIdentity::new(mint_url.clone(), CurrencyUnit::Sat))
        .await
        .unwrap();
    let prepared_send = sender_wallet
        .plan_send(SendRequest::new(50.into()))
        .await
        .unwrap();

    let token = prepared_send.execute().await.unwrap().token;
    let token_string = token.to_string();

    // Create receiver wallet
    let receiver_manager = create_test_wallet_manager().await;
    // Add the same mint as trusted
    receiver_manager
        .register_mint(mint_url.clone())
        .await
        .expect("failed to add mint");

    // Receive the token
    let receiver_wallet = receiver_manager
        .wallet(WalletIdentity::new(mint_url.clone(), CurrencyUnit::Sat))
        .await
        .unwrap();
    let received_amount = receiver_wallet
        .receive(ReceiveRequest::new(token_string))
        .await
        .unwrap()
        .amount;

    // Note: received amount may be slightly less due to fees
    assert!(
        received_amount > Amount::ZERO,
        "Should receive some amount, got {:?}",
        received_amount
    );

    // Verify receiver balance
    let receiver_balances = receiver_manager.balance_totals().await.unwrap();
    let receiver_balance = receiver_balances
        .get(&CurrencyUnit::Sat)
        .copied()
        .unwrap_or(Amount::ZERO);
    assert!(
        receiver_balance > Amount::ZERO,
        "Receiver should have balance"
    );

    // Verify sender balance decreased
    let sender_balances = sender_manager.balance_totals().await.unwrap();
    let sender_balance = sender_balances
        .get(&CurrencyUnit::Sat)
        .copied()
        .unwrap_or(Amount::ZERO);
    assert!(
        sender_balance < 100.into(),
        "Sender balance should be less than 100 after send"
    );
}

/// Test receiving after explicitly registering the token's mint.
///
/// This test verifies:
/// 1. Create a token from a known mint
/// 2. Register that mint with a fresh manager
/// 3. Receive through the configured wallet
#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_wallet_manager_receive_after_registering_mint() {
    // Create sender wallet and fund it
    let sender_manager = create_test_wallet_manager().await;
    let mint_url = MintUrl::from_str(&get_mint_url_from_env()).expect("invalid mint url");
    sender_manager
        .register_mint(mint_url.clone())
        .await
        .expect("failed to add mint");

    let funded_amount = fund_wallet_manager(&sender_manager, &mint_url, 100.into()).await;
    assert_eq!(funded_amount, 100.into());

    // Create a token to send
    let sender_wallet = sender_manager
        .wallet(WalletIdentity::new(mint_url.clone(), CurrencyUnit::Sat))
        .await
        .unwrap();
    let prepared_send = sender_wallet
        .plan_send(SendRequest::new(50.into()))
        .await
        .unwrap();

    let token = prepared_send.execute().await.unwrap().token;
    let token_string = token.to_string();

    // Create a fresh receiver manager.
    let receiver_manager = create_test_wallet_manager().await;

    // Register the mint before accepting its token.
    receiver_manager
        .register_mint(mint_url.clone())
        .await
        .expect("failed to add mint");

    // Now receive
    let receiver_wallet = receiver_manager
        .wallet(WalletIdentity::new(mint_url.clone(), CurrencyUnit::Sat))
        .await
        .unwrap();
    let received_amount = receiver_wallet
        .receive(ReceiveRequest::new(token_string))
        .await
        .unwrap()
        .amount;

    assert!(received_amount > Amount::ZERO, "Should receive some amount");

    // Verify the mint is in the wallet
    assert!(
        receiver_manager.contains_mint(&mint_url).await,
        "Mint should be in wallet"
    );
}

/// Test the send-planning happy path.
///
/// This test verifies:
/// 1. Fund wallet
/// 2. Plan the send successfully
/// 3. Confirm the send and get a token
/// 4. Verify the token is valid
#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_wallet_manager_send_plan_happy_path() {
    let wallet_manager = create_test_wallet_manager().await;

    let mint_url = MintUrl::from_str(&get_mint_url_from_env()).expect("invalid mint url");
    wallet_manager
        .register_mint(mint_url.clone())
        .await
        .expect("failed to add mint");

    // Fund the wallet
    let funded_amount = fund_wallet_manager(&wallet_manager, &mint_url, 100.into()).await;
    assert_eq!(funded_amount, 100.into());

    // Plan the send.
    let wallet = wallet_manager
        .wallet(WalletIdentity::new(mint_url.clone(), CurrencyUnit::Sat))
        .await
        .unwrap();
    let prepared_send = wallet.plan_send(SendRequest::new(50.into())).await.unwrap();

    // Get the token
    let token = prepared_send.execute().await.unwrap().token;
    let token_string = token.to_string();

    // Verify the token can be parsed back
    let parsed_token = Token::from_str(&token_string).unwrap();
    let token_mint_url = parsed_token.mint_url().unwrap();
    assert_eq!(token_mint_url, mint_url, "Token mint URL should match");

    // Get token data to verify value
    let token_data = wallet_manager
        .advanced()
        .inspect_token(&parsed_token)
        .await
        .unwrap();
    assert_eq!(token_data.value, 50.into(), "Token value should be 50 sats");

    // Verify wallet balance decreased
    let balances = wallet_manager.balance_totals().await.unwrap();
    let balance = balances
        .get(&CurrencyUnit::Sat)
        .copied()
        .unwrap_or(Amount::ZERO);
    assert_eq!(balance, 50.into(), "Remaining balance should be 50 sats");
}

/// Test manager balance summaries across multiple operations.
///
/// This test verifies:
/// 1. Empty wallet has zero balances
/// 2. After minting, balance is updated
/// 3. `available_balances` returns a per-wallet breakdown
#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_wallet_manager_balances() {
    let wallet_manager = create_test_wallet_manager().await;

    let mint_url = MintUrl::from_str(&get_mint_url_from_env()).expect("invalid mint url");
    wallet_manager
        .register_mint(mint_url.clone())
        .await
        .expect("failed to add mint");

    // Check initial balances
    let balances = wallet_manager.available_balances().await.unwrap();
    let initial_balance = balances
        .get(&WalletIdentity::new(mint_url.clone(), CurrencyUnit::Sat))
        .cloned()
        .unwrap_or(Amount::ZERO);
    assert_eq!(initial_balance, Amount::ZERO, "Initial balance should be 0");

    // Fund the wallet
    fund_wallet_manager(&wallet_manager, &mint_url, 100.into()).await;

    // Check balances again
    let balances = wallet_manager.available_balances().await.unwrap();
    let balance = balances
        .get(&WalletIdentity::new(mint_url.clone(), CurrencyUnit::Sat))
        .cloned()
        .unwrap_or(Amount::ZERO);
    assert_eq!(balance, 100.into(), "Balance should be 100 sats");

    // Verify total_balance matches
    let total_balances = wallet_manager.balance_totals().await.unwrap();
    let total = total_balances
        .get(&CurrencyUnit::Sat)
        .copied()
        .unwrap_or(Amount::ZERO);
    assert_eq!(total, 100.into(), "Total balance should match");
}

/// Test the advanced proof-record inventory.
///
/// This test verifies:
/// 1. Empty wallet has no proofs
/// 2. After minting, proofs are listed correctly
#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_wallet_manager_proof_records() {
    let wallet_manager = create_test_wallet_manager().await;

    let mint_url = MintUrl::from_str(&get_mint_url_from_env()).expect("invalid mint url");
    wallet_manager
        .register_mint(mint_url.clone())
        .await
        .expect("failed to add mint");

    // Check initial proofs
    let proofs = wallet_manager.advanced().proof_records().await.unwrap();
    let mint_proofs = proofs
        .get(&WalletIdentity::new(mint_url.clone(), CurrencyUnit::Sat))
        .cloned()
        .unwrap_or_default();
    assert!(mint_proofs.is_empty(), "Should have no proofs initially");

    // Fund the wallet
    fund_wallet_manager(&wallet_manager, &mint_url, 100.into()).await;

    // Check proofs again
    let proofs = wallet_manager.advanced().proof_records().await.unwrap();
    let mint_proofs = proofs
        .get(&WalletIdentity::new(mint_url.clone(), CurrencyUnit::Sat))
        .cloned()
        .unwrap_or_default();
    assert!(!mint_proofs.is_empty(), "Should have proofs after minting");

    // Verify proof total matches balance
    let proofs: cashu::Proofs = mint_proofs.into_iter().map(|record| record.proof).collect();
    let proof_total = proofs.total_amount().expect("valid proof total");
    assert_eq!(proof_total, 100.into(), "Proof total should be 100 sats");
}

/// Test mint registration and wallet removal.
///
/// This test verifies:
/// 1. `contains_mint` returns false for unknown mints
/// 2. `register_mint` adds the mint's wallets
/// 3. `contains_mint` returns true after registration
/// 4. `forget_wallet` removes each configured unit
/// 5. `contains_mint` returns false after removal
#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_wallet_manager_mint_management() {
    let wallet_manager = create_test_wallet_manager().await;

    let mint_url = MintUrl::from_str(&get_mint_url_from_env()).expect("invalid mint url");

    // Initially mint should not be in wallet
    assert!(
        !wallet_manager.contains_mint(&mint_url).await,
        "Mint should not be in wallet initially"
    );

    // Add the mint
    wallet_manager
        .register_mint(mint_url.clone())
        .await
        .expect("failed to add mint");

    // Now mint should be in wallet
    assert!(
        wallet_manager.contains_mint(&mint_url).await,
        "Mint should be in wallet after adding"
    );

    // Get wallets should include this mint
    let wallets = wallet_manager.wallets().await;
    assert!(!wallets.is_empty(), "Should have at least one wallet");

    // Get specific wallet
    let wallet = wallet_manager
        .wallet(WalletIdentity::new(mint_url.clone(), CurrencyUnit::Sat))
        .await;
    assert!(wallet.is_ok(), "Should be able to get wallet for mint");

    // Get wallets for this mint
    let mint_wallets = wallet_manager.wallets_for_mint(&mint_url).await;

    // Remove all wallets for the mint
    for wallet in mint_wallets {
        wallet_manager
            .forget_wallet(wallet.identity())
            .await
            .unwrap();
    }

    // Now mint should not be in wallet
    assert!(
        !wallet_manager.contains_mint(&mint_url).await,
        "Mint should not be in wallet after removal"
    );
}

/// Test manager-wide synchronization of paid mint sessions.
///
/// This test verifies:
/// 1. Create a mint quote
/// 2. Pay the quote
/// 3. Poll until quote is paid (like a real wallet would)
/// 4. `synchronize_all` claims the paid session
#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_wallet_manager_synchronize_all() {
    let wallet_manager = create_test_wallet_manager().await;

    let mint_url = MintUrl::from_str(&get_mint_url_from_env()).expect("invalid mint url");
    wallet_manager
        .register_mint(mint_url.clone())
        .await
        .expect("failed to add mint");

    let wallet = wallet_manager
        .wallet(WalletIdentity::new(mint_url.clone(), CurrencyUnit::Sat))
        .await
        .unwrap();

    // Create a mint quote
    let session = wallet
        .request_mint(MintRequest::bolt11(100.into()))
        .await
        .unwrap();

    // Pay the invoice (in regtest mode) - for fake wallet, payment is simulated automatically
    let invoice = Bolt11Invoice::from_str(&session.initial_state().payment_request).unwrap();
    pay_if_regtest(&get_test_temp_dir(), &invoice)
        .await
        .unwrap();

    // Poll for quote to be paid (like a real wallet would)
    let mut quote_status = session.refresh().await.unwrap();

    let timeout = tokio::time::Duration::from_secs(30);
    let start = tokio::time::Instant::now();
    while quote_status.state != MintState::Paid && quote_status.state != MintState::Issued {
        if start.elapsed() > timeout {
            panic!(
                "Timeout waiting for quote to be paid, state: {:?}",
                quote_status.state
            );
        }
        tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;
        quote_status = session.refresh().await.unwrap();
    }
    tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;
    let _ = session.refresh().await.unwrap();

    // Synchronize every wallet; this should find and claim the paid session.
    let reports = wallet_manager
        .synchronize_all(SyncPolicy::Online)
        .await
        .unwrap();
    let minted_amount = reports
        .iter()
        .fold(Amount::ZERO, |total, report| total + report.claimed_amount);

    assert_eq!(
        minted_amount,
        100.into(),
        "Should mint 100 sats from paid quote"
    );

    // Verify balance
    let balances = wallet_manager.balance_totals().await.unwrap();
    let balance = balances
        .get(&CurrencyUnit::Sat)
        .copied()
        .unwrap_or(Amount::ZERO);
    assert_eq!(balance, 100.into(), "Balance should be 100 sats");
}

/// Test seed restoration through a manager-provided wallet.
///
/// This test verifies:
/// 1. Create and fund a wallet with a specific seed
/// 2. Create a new wallet with the same seed
/// 3. Call `restore_from_seed` to recover the proofs
#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_wallet_manager_restore() {
    let seed = Mnemonic::generate(12).unwrap().to_seed_normalized("");
    let mint_url = MintUrl::from_str(&get_mint_url_from_env()).expect("invalid mint url");

    // Create first wallet and fund it
    {
        let localstore = Arc::new(memory::empty().await.unwrap());
        let wallet1 = WalletManagerBuilder::new()
            .with_store(localstore)
            .with_seed(seed)
            .build()
            .await
            .expect("failed to create wallet");

        wallet1
            .register_mint(mint_url.clone())
            .await
            .expect("failed to add mint");

        let funded = fund_wallet_manager(&wallet1, &mint_url, 100.into()).await;
        assert_eq!(funded, 100.into());
    }
    // wallet1 goes out of scope

    // Create second wallet with same seed but fresh storage
    let localstore2 = Arc::new(memory::empty().await.unwrap());
    let wallet2 = WalletManagerBuilder::new()
        .with_store(localstore2)
        .with_seed(seed)
        .build()
        .await
        .expect("failed to create wallet");

    wallet2
        .register_mint(mint_url.clone())
        .await
        .expect("failed to add mint");

    // Initially should have no balance
    let balances_before = wallet2.balance_totals().await.unwrap();
    let balance_before = balances_before
        .get(&CurrencyUnit::Sat)
        .copied()
        .unwrap_or(Amount::ZERO);
    assert_eq!(balance_before, Amount::ZERO, "Should start with no balance");

    // Restore from mint using the individual wallet
    let wallet = wallet2
        .wallet(WalletIdentity::new(mint_url.clone(), CurrencyUnit::Sat))
        .await
        .unwrap();
    let restored = wallet
        .restore_from_seed(RestoreRequest::default())
        .await
        .unwrap();
    assert_eq!(restored.unspent, 100.into(), "Should restore 100 sats");
}

/// Test payment status with an explicitly selected mint wallet.
///
/// This test verifies:
/// 1. Fund wallet
/// 2. Quote a payment at that mint
/// 3. Prepare and confirm the payment
/// 4. Verify payment succeeded
#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_wallet_manager_payment_status() {
    let wallet_manager = create_test_wallet_manager().await;

    let mint_url = MintUrl::from_str(&get_mint_url_from_env()).expect("invalid mint url");
    wallet_manager
        .register_mint(mint_url.clone())
        .await
        .expect("failed to add mint");

    // Fund the wallet
    fund_wallet_manager(&wallet_manager, &mint_url, 100.into()).await;

    // Create an invoice to pay
    let invoice = create_invoice_for_env(Some(50)).await.unwrap();

    // Get wallet for operations
    let wallet = wallet_manager
        .wallet(WalletIdentity::new(mint_url.clone(), CurrencyUnit::Sat))
        .await
        .unwrap();

    // Create melt quote at specific mint
    let payment = wallet
        .quote_payment(PaymentQuoteRequest::new(PaymentTarget::bolt11(invoice)))
        .await
        .unwrap()
        .into_single()
        .unwrap();
    let melt_result = payment.prepare().await.unwrap().execute().await.unwrap();

    assert_eq!(melt_result.amount, 50.into(), "Payment should be paid");

    // Check melt quote status
    let quote_status = payment.refresh().await.unwrap();

    assert_eq!(
        quote_status.state,
        PaymentState::Paid,
        "Quote status should be paid"
    );
}

/// Test manager-wide application history.
///
/// This test verifies:
/// 1. Initially no transactions
/// 2. After minting, transaction is recorded
/// 3. After melting, transaction is recorded
#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_wallet_manager_history() {
    let wallet_manager = create_test_wallet_manager().await;

    let mint_url = MintUrl::from_str(&get_mint_url_from_env()).expect("invalid mint url");
    wallet_manager
        .register_mint(mint_url.clone())
        .await
        .expect("failed to add mint");

    // Fund the wallet (this creates a mint transaction)
    fund_wallet_manager(&wallet_manager, &mint_url, 100.into()).await;

    // List all transactions
    let transactions = wallet_manager
        .history_all(HistoryQuery::default())
        .await
        .unwrap();
    assert!(
        !transactions.is_empty(),
        "Should have at least one transaction after minting"
    );

    // Get wallet for melt operations
    let wallet = wallet_manager
        .wallet(WalletIdentity::new(mint_url.clone(), CurrencyUnit::Sat))
        .await
        .unwrap();

    // Create an invoice and melt (this creates a melt transaction)
    let invoice = create_invoice_for_env(Some(50)).await.unwrap();
    wallet
        .quote_payment(PaymentQuoteRequest::new(PaymentTarget::bolt11(invoice)))
        .await
        .unwrap()
        .into_single()
        .unwrap()
        .prepare()
        .await
        .unwrap()
        .execute()
        .await
        .unwrap();

    // List transactions again
    let transactions_after = wallet_manager
        .history_all(HistoryQuery::default())
        .await
        .unwrap();
    assert!(
        transactions_after.len() > transactions.len(),
        "Should have more transactions after melt"
    );
}
