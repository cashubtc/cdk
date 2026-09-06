use std::env;
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{bail, Result};
use bip39::Mnemonic;
use cashu::amount::SplitTarget;
use cashu::{
    Amount, CurrencyUnit, MintRequest as ProtocolMintRequest, MintUrl, PaymentMethod,
    PreMintSecrets,
};
use cdk::wallet::advanced::{
    HttpClient, MintConnector, MintMetadataRequest, MintSessionFilter, WalletBuilder,
};
use cdk::wallet::mint::{MintRequest, MintSession, MintSessionState};
use cdk::wallet::operation::SyncPolicy;
use cdk::wallet::payment::{PaymentQuoteRequest, PaymentTarget};
use cdk::wallet::Wallet;
use cdk::StreamExt;
use cdk_common::database::WalletDatabase;
use cdk_integration_tests::get_mint_url_from_env;
use cdk_integration_tests::init_regtest::{get_cln_dir, get_temp_dir};
use cdk_integration_tests::ln_regtest::ln_client::ClnClient;
use cdk_sqlite::wallet::memory;

async fn create_wallet() -> Result<Wallet> {
    Ok(WalletBuilder::new()
        .with_mint_url(get_mint_url_from_env().parse()?)
        .with_unit(CurrencyUnit::Sat)
        .with_store(Arc::new(memory::empty().await?))
        .with_seed(Mnemonic::generate(12)?.to_seed_normalized(""))
        .build()?)
}

async fn create_wallet_with_store(
) -> Result<(Wallet, Arc<cdk_sqlite::wallet::WalletSqliteDatabase>)> {
    let store = Arc::new(memory::empty().await?);
    let wallet = WalletBuilder::new()
        .with_mint_url(get_mint_url_from_env().parse()?)
        .with_unit(CurrencyUnit::Sat)
        .with_store(store.clone())
        .with_seed(Mnemonic::generate(12)?.to_seed_normalized(""))
        .build()?;
    Ok((wallet, store))
}

async fn wait_for_payment(session: &MintSession, timeout: Duration) -> Result<MintSessionState> {
    tokio::time::timeout(timeout, async {
        loop {
            let state = session.refresh().await?;
            if state.amount_paid > state.amount_claimed {
                return Ok(state);
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    })
    .await
    .map_err(|_| cdk::Error::Timeout)?
}

// Helper function to get temp directory from environment or fallback
fn get_test_temp_dir() -> PathBuf {
    match env::var("CDK_ITESTS_DIR") {
        Ok(dir) => PathBuf::from(dir),
        Err(_) => get_temp_dir(), // fallback to default
    }
}

fn is_ldk_mint() -> bool {
    get_mint_url_from_env() == "http://127.0.0.1:8089"
}

fn get_cln_payer_dir(work_dir: &Path) -> PathBuf {
    let node = if is_ldk_mint() { "one" } else { "two" };
    get_cln_dir(work_dir, node)
}

// Helper function to create CLN client with retries
async fn create_cln_client_with_retry(cln_dir: PathBuf) -> Result<ClnClient> {
    let mut retries = 0;
    let max_retries = 10;
    loop {
        match ClnClient::new(cln_dir.clone(), None).await {
            Ok(client) => return Ok(client),
            Err(e) => {
                retries += 1;
                if retries >= max_retries {
                    bail!(
                        "Could not connect to CLN client after {} retries: {}",
                        max_retries,
                        e
                    );
                }
                println!(
                    "Failed to connect to CLN (attempt {}/{}): {}. Retrying in 7 seconds...",
                    retries, max_retries, e
                );
                tokio::time::sleep(tokio::time::Duration::from_secs(7)).await;
            }
        }
    }
}

/// Tests basic BOLT12 minting functionality:
/// - Creates a wallet
/// - Gets a BOLT12 quote for a specific amount (100 sats)
/// - Pays the quote using Core Lightning
/// - Mints tokens and verifies the correct amount is received
#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_regtest_bolt12_mint() {
    let wallet = create_wallet().await.unwrap();

    let mint_amount = Amount::from(100);

    let session = wallet
        .request_mint(MintRequest::new(PaymentMethod::BOLT12, Some(mint_amount)))
        .await
        .unwrap();

    assert_eq!(session.initial_state().amount, Some(mint_amount));

    let work_dir = get_test_temp_dir();
    let cln_dir = get_cln_payer_dir(&work_dir);
    let cln_client = create_cln_client_with_retry(cln_dir).await.unwrap();
    cln_client
        .pay_bolt12_offer(None, session.initial_state().payment_request.clone())
        .await
        .unwrap();

    let receipt = session.wait(Duration::from_secs(60)).await.unwrap();

    assert_eq!(receipt.amount, 100.into());
}

/// Tests multiple payments to a single BOLT12 quote:
/// - Creates a wallet and gets a BOLT12 quote without specifying amount
/// - Makes two separate payments (10,000 sats and 11,000 sats) to the same quote
/// - Verifies that each payment can be minted separately and correctly
/// - Tests the functionality of reusing a quote for multiple payments
#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_regtest_bolt12_mint_multiple() -> Result<()> {
    let mint_url = MintUrl::from_str(&get_mint_url_from_env())?;

    let wallet = WalletBuilder::new()
        .with_mint_url(mint_url)
        .with_unit(CurrencyUnit::Sat)
        .with_store(Arc::new(memory::empty().await?))
        .with_seed(Mnemonic::generate(12)?.to_seed_normalized(""))
        .with_target_proof_count(3)
        .with_http_subscription()
        .build()?;

    let session = wallet
        .request_mint(MintRequest::new(PaymentMethod::BOLT12, None))
        .await?;
    let mut receipts = Box::pin(session.receipts(Default::default()));

    let work_dir = get_test_temp_dir();
    let cln_dir = get_cln_payer_dir(&work_dir);
    let cln_client = create_cln_client_with_retry(cln_dir).await?;
    cln_client
        .pay_bolt12_offer(Some(10000), session.initial_state().payment_request.clone())
        .await
        .unwrap();

    let receipt = tokio::time::timeout(Duration::from_secs(60), receipts.next())
        .await?
        .expect("mint receipt")?;

    assert_eq!(receipt.amount, 10.into());

    cln_client
        .pay_bolt12_offer(
            Some(11_000),
            session.initial_state().payment_request.clone(),
        )
        .await
        .unwrap();

    let receipt = tokio::time::timeout(Duration::from_secs(60), receipts.next())
        .await?
        .expect("mint receipt")?;

    assert_eq!(receipt.amount, 11.into());

    Ok(())
}

/// Tests that multiple wallets can pay the same BOLT12 offer:
/// - Creates a BOLT12 offer through CLN that both wallets will pay
/// - Creates two separate wallets with different minting amounts
/// - Has each wallet get their own quote and make payments
/// - Verifies both wallets can successfully mint their tokens
#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_regtest_bolt12_multiple_wallets() -> Result<()> {
    // Create first wallet
    let wallet_one = create_wallet().await?;

    // Create second wallet
    let wallet_two = create_wallet().await?;

    // Create a BOLT12 offer that both wallets will use
    let work_dir = get_test_temp_dir();
    let cln_dir = get_cln_payer_dir(&work_dir);
    let cln_client = create_cln_client_with_retry(cln_dir).await?;
    // First wallet payment
    let session_one = wallet_one
        .request_mint(MintRequest::new(PaymentMethod::BOLT12, Some(10_000.into())))
        .await?;
    cln_client
        .pay_bolt12_offer(None, session_one.initial_state().payment_request.clone())
        .await?;

    let receipt_one = session_one.wait(Duration::from_secs(60)).await?;

    assert_eq!(receipt_one.amount, 10_000.into());

    // Second wallet payment
    let session_two = wallet_two
        .request_mint(MintRequest::new(PaymentMethod::BOLT12, Some(15_000.into())))
        .await?;
    cln_client
        .pay_bolt12_offer(None, session_two.initial_state().payment_request.clone())
        .await?;

    let receipt_two = session_two.wait(Duration::from_secs(60)).await?;

    assert_eq!(receipt_two.amount, 15_000.into());

    if is_ldk_mint() {
        return Ok(());
    }

    let offer = cln_client
        .get_bolt12_offer(None, false, "test_multiple_wallets".to_string())
        .await?;

    let payment_one = wallet_one
        .quote_payment(PaymentQuoteRequest::new(PaymentTarget::bolt12_amountless(
            offer.to_string(),
            100_000.into(),
        )))
        .await?
        .into_single()?;

    let payment_two = wallet_two
        .quote_payment(PaymentQuoteRequest::new(PaymentTarget::bolt12_amountless(
            offer.to_string(),
            110_000.into(),
        )))
        .await?
        .into_single()?;

    let melted = payment_one.prepare().await?.execute().await?;

    assert!(melted.payment_proof.is_some());

    let melted_two = payment_two.prepare().await?.execute().await?;

    assert!(melted_two.payment_proof.is_some());

    Ok(())
}

/// Tests the BOLT12 melting (spending) functionality:
/// - Creates a wallet and mints 20,000 sats using BOLT12
/// - Creates a BOLT12 offer for 100 sats
/// - Tests melting (spending) tokens using the BOLT12 offer
/// - Verifies the correct amount is melted
#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_regtest_bolt12_melt() -> Result<()> {
    let wallet = create_wallet().await?;

    let mint_amount = Amount::from(20_000);

    // Create a single-use BOLT12 quote
    let mint_session = wallet
        .request_mint(MintRequest::new(PaymentMethod::BOLT12, Some(mint_amount)))
        .await?;

    assert_eq!(mint_session.initial_state().amount, Some(mint_amount));
    // Pay the quote
    let work_dir = get_test_temp_dir();
    let cln_dir = get_cln_payer_dir(&work_dir);
    let cln_client = create_cln_client_with_retry(cln_dir).await?;
    cln_client
        .pay_bolt12_offer(None, mint_session.initial_state().payment_request.clone())
        .await?;

    mint_session.wait(Duration::from_secs(60)).await?;

    let max_attempts = if is_ldk_mint() { 3 } else { 1 };
    let mut attempt = 1;
    loop {
        let offer = cln_client
            .get_bolt12_offer(
                Some(100_000),
                true,
                format!("test_regtest_bolt12_melt_{attempt}"),
            )
            .await?;

        let payment = wallet
            .quote_payment(PaymentQuoteRequest::new(PaymentTarget::bolt12(
                offer.to_string(),
            )))
            .await?
            .into_single()?;

        match payment.prepare().await?.execute().await {
            Ok(melt) => {
                assert_eq!(melt.amount, 100.into());
                break;
            }
            Err(err) if attempt < max_attempts => {
                tracing::warn!(
                    "BOLT12 melt attempt {attempt}/{max_attempts} failed; retrying with a fresh offer: {err}"
                );
                tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;
                attempt += 1;
            }
            Err(err) => return Err(err.into()),
        }
    }

    Ok(())
}

/// Tests security validation for BOLT12 minting to prevent overspending:
/// - Creates a wallet and gets an open-ended BOLT12 quote
/// - Makes a payment of 10,000 millisats
/// - Attempts to mint more tokens (500 sats) than were actually paid for
/// - Verifies that the mint correctly rejects the oversized mint request
/// - Ensures proper error handling with TransactionUnbalanced error
/// This test is crucial for ensuring the economic security of the minting process
/// by preventing users from minting more tokens than they have paid for.
#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_regtest_bolt12_mint_extra() -> Result<()> {
    let (wallet, store) = create_wallet_with_store().await?;

    // Create a single-use BOLT12 quote
    let session = wallet
        .request_mint(MintRequest::new(PaymentMethod::BOLT12, None))
        .await?;

    let state = session.refresh().await?;

    assert_eq!(state.amount_paid, Amount::ZERO);
    assert_eq!(state.amount_claimed, Amount::ZERO);

    let active_keyset_id = wallet
        .advanced()
        .mint_metadata(MintMetadataRequest::default())
        .await?
        .active_keyset()
        .expect("active keyset")
        .id;

    let pay_amount_msats = 10_000;

    let work_dir = get_test_temp_dir();
    let cln_dir = get_cln_payer_dir(&work_dir);
    let cln_client = create_cln_client_with_retry(cln_dir).await?;
    cln_client
        .pay_bolt12_offer(
            Some(pay_amount_msats),
            session.initial_state().payment_request.clone(),
        )
        .await?;

    let state = wait_for_payment(&session, Duration::from_secs(15)).await?;
    let payment = state.amount_paid;

    assert_eq!(payment, state.amount_paid);
    assert_eq!(state.amount_paid, (pay_amount_msats / 1_000).into());
    assert_eq!(state.amount_claimed, Amount::ZERO);

    let fee_and_amounts = (0, ((0..32).map(|x| 2u64.pow(x)).collect::<Vec<_>>())).into();

    let pre_mint = PreMintSecrets::random(
        active_keyset_id,
        500.into(),
        &SplitTarget::None,
        &fee_and_amounts,
    )?;

    let quote_info = store
        .get_mint_quote(session.id().as_str())
        .await?
        .expect("there is a quote");

    let mut mint_request = ProtocolMintRequest {
        quote: session.id().to_string(),
        outputs: pre_mint.blinded_messages(),
        signature: None,
    };

    if let Some(secret_key) = quote_info.secret_key {
        mint_request.sign(&secret_key)?;
    }

    let http_client = HttpClient::new(get_mint_url_from_env().parse().unwrap(), None);

    let response = http_client
        .post_mint(&PaymentMethod::BOLT12, mint_request.clone())
        .await;

    match response {
        Err(err) => match err {
            cdk::Error::TransactionUnbalanced(_, _, _) => (),
            err => {
                bail!("Wrong mint error returned: {}", err);
            }
        },
        Ok(_) => {
            bail!("Should not have allowed second payment");
        }
    }

    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_attempt_to_mint_unpaid() {
    let (wallet, store) = create_wallet_with_store().await.unwrap();

    let mint_amount = Amount::from(100);

    let session = wallet
        .request_mint(MintRequest::new(PaymentMethod::BOLT12, Some(mint_amount)))
        .await
        .unwrap();

    assert_eq!(session.initial_state().amount, Some(mint_amount));

    let mut mint_quote = store
        .get_mint_quote(session.id().as_str())
        .await
        .unwrap()
        .unwrap();
    // Since the wallet checks how much it can mint
    // we manually set it in the db to fake like it was paid to the wallet
    // so it tries to mint
    mint_quote.amount_paid = mint_amount;
    store.add_mint_quote(mint_quote).await.unwrap();

    let result = session.claim().await;

    match result {
        Err(err) => {
            if !matches!(err, cdk::Error::UnpaidQuote) {
                panic!("Wrong error quote should be unpaid: {}", err);
            }
        }
        Ok(_) => {
            panic!("Minting should not be allowed");
        }
    }

    let session = wallet
        .request_mint(MintRequest::new(PaymentMethod::BOLT12, Some(mint_amount)))
        .await
        .unwrap();

    let state = session.refresh().await.unwrap();

    assert!(state.amount_paid == Amount::ZERO);
    let mut mint_quote = store
        .get_mint_quote(session.id().as_str())
        .await
        .unwrap()
        .unwrap();
    // Since the wallet checks how much it can mint
    // we manually set it in the db to fake like it was paid to the wallet
    // so it tries to mint
    mint_quote.amount_paid = mint_amount;
    store.add_mint_quote(mint_quote).await.unwrap();

    let result = session.claim().await;

    match result {
        Err(err) => {
            if !matches!(err, cdk::Error::UnpaidQuote) {
                panic!("Wrong error quote should be unpaid: {}", err);
            }
        }
        Ok(_) => {
            panic!("Minting should not be allowed");
        }
    }
}

/// Tests the check_all_mint_quotes functionality for Bolt12 quotes
///
/// This test verifies that:
/// 1. Paid Bolt12 quotes are automatically minted when check_all_mint_quotes is called
/// 2. The method correctly handles the Bolt12-specific logic (amount_paid > amount_issued)
/// 3. Quote state is properly updated after minting
#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_check_all_mint_quotes_bolt12() -> Result<()> {
    let wallet = create_wallet().await?;

    let mint_amount = Amount::from(100);

    // Create a Bolt12 quote
    let session = wallet
        .request_mint(MintRequest::new(PaymentMethod::BOLT12, Some(mint_amount)))
        .await?;

    assert_eq!(session.initial_state().amount, Some(mint_amount));

    // Verify the quote is in unissued quotes before payment
    let unissued_before = wallet
        .advanced()
        .mint_sessions(MintSessionFilter::Unissued)
        .await?;
    assert!(
        unissued_before
            .iter()
            .any(|candidate| candidate.id() == session.id()),
        "Bolt12 quote should be in unissued quotes before payment"
    );

    // Pay the quote
    let work_dir = get_test_temp_dir();
    let cln_dir = get_cln_payer_dir(&work_dir);
    let cln_client = create_cln_client_with_retry(cln_dir).await?;
    cln_client
        .pay_bolt12_offer(None, session.initial_state().payment_request.clone())
        .await?;

    // Wait for payment to be recognized
    wait_for_payment(&session, Duration::from_secs(30)).await?;

    // Verify initial balance is zero
    assert_eq!(wallet.balance().await?.available, Amount::ZERO);

    // Synchronization claims paid quotes and recovers any interrupted operation.
    let report = wallet.synchronize(SyncPolicy::Online).await?;

    // Verify the amount minted is correct
    assert_eq!(
        report.claimed_amount, mint_amount,
        "synchronization should have claimed the Bolt12 quote"
    );

    // Verify wallet balance matches
    assert_eq!(wallet.balance().await?.available, mint_amount);

    // A second synchronization is idempotent.
    let second_check = wallet.synchronize(SyncPolicy::Online).await?;
    assert_eq!(
        second_check.claimed_amount,
        Amount::ZERO,
        "Second check should return 0 as quote is fully issued"
    );

    Ok(())
}

/// Tests that Bolt12 quote state (amount_issued) is properly updated after minting
///
/// This test verifies that:
/// 1. amount_issued starts at 0
/// 2. amount_issued is updated after minting
/// 3. The quote correctly tracks issued vs paid amounts
#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_bolt12_quote_amount_issued_tracking() -> Result<()> {
    let wallet = create_wallet().await?;

    // Create an open-ended Bolt12 quote (no amount specified)
    let session = wallet
        .request_mint(MintRequest::new(PaymentMethod::BOLT12, None))
        .await?;

    // Verify initial state
    let state_before = session.refresh().await?;
    assert_eq!(state_before.amount_paid, Amount::ZERO);
    assert_eq!(state_before.amount_claimed, Amount::ZERO);

    // Pay the quote with a specific amount
    let pay_amount_msats = 50_000; // 50 sats
    let work_dir = get_test_temp_dir();
    let cln_dir = get_cln_payer_dir(&work_dir);
    let cln_client = create_cln_client_with_retry(cln_dir).await?;
    cln_client
        .pay_bolt12_offer(
            Some(pay_amount_msats),
            session.initial_state().payment_request.clone(),
        )
        .await?;

    // Wait for payment
    let state_after_payment = wait_for_payment(&session, Duration::from_secs(30)).await?;
    let payment = state_after_payment.amount_paid;

    // Check state after payment but before minting
    assert_eq!(
        state_after_payment.amount_paid,
        Amount::from(pay_amount_msats / 1000)
    );
    assert_eq!(
        state_after_payment.amount_claimed,
        Amount::ZERO,
        "amount_issued should still be 0 before minting"
    );

    // Now mint the tokens
    let minted_amount = session.claim().await?.amount;
    assert_eq!(minted_amount, payment);

    // Check state after minting
    let state_after_mint = session.refresh().await?;
    assert_eq!(
        state_after_mint.amount_claimed, minted_amount,
        "amount_issued should be updated after minting"
    );
    assert_eq!(
        state_after_mint.amount_paid, state_after_mint.amount_claimed,
        "For a single payment, amount_paid should equal amount_issued after minting"
    );

    Ok(())
}
