//! Integration tests for mint-wallet interactions that should work across all mint implementations
//!
//! These tests verify the core functionality of the wallet-mint interaction protocol,
//! including minting, melting, and wallet restoration. They are designed to be
//! implementation-agnostic and should pass against any compliant Cashu mint,
//! including Nutshell, CDK, and other implementations that follow the Cashu NUTs.
//!
//! The tests use environment variables to determine which mint to connect to and
//! whether to use real Lightning Network payments (regtest mode) or simulated payments.

use core::panic;
use std::collections::HashMap;
use std::env;
use std::fmt::Debug;
use std::path::PathBuf;
use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;

use bip39::Mnemonic;
use cashu::{MeltRequest, PreMintSecrets};
use cdk::amount::{Amount, SplitTarget};
use cdk::mint_url::MintUrl;
use cdk::nuts::nut00::KnownMethod;
use cdk::nuts::nut17::{deserialize_payload_for_kind, Kind};
use cdk::nuts::{CurrencyUnit, MeltQuoteState, NotificationPayload, PaymentMethod, State};
use cdk::wallet::advanced::{
    FeeEstimateRequest, HttpClient, MetadataSource, MintClaimOptions, MintConnector,
    MintMetadataRequest, ProofCheckRequest, ProofQuery, ReissueFeePolicy, ReissueProtection,
    ReissueRequest,
};
use cdk::wallet::history::HistoryQuery;
use cdk::wallet::mint::MintRequest as WalletMintRequest;
use cdk::wallet::payment::{PaymentQuoteRequest, PaymentState, PaymentTarget};
use cdk::wallet::{
    RestoreRequest as WalletRestoreRequest, Wallet, WalletIdentity, WalletManagerBuilder,
};
use cdk_common::database::WalletDatabase;
use cdk_integration_tests::{create_invoice_for_env, get_mint_url_from_env, pay_if_regtest};
use cdk_sqlite::wallet::memory;
use futures::{SinkExt, StreamExt};
use lightning_invoice::Bolt11Invoice;
use serde_json::json;
use tokio::time::timeout;
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::protocol::Message;

// Helper function to get temp directory from environment or fallback
fn get_test_temp_dir() -> PathBuf {
    match env::var("CDK_ITESTS_DIR") {
        Ok(dir) => PathBuf::from(dir),
        Err(_) => panic!("Unknown test dir"),
    }
}

async fn mint_to_wallet(wallet: &Wallet, amount: Amount, split: SplitTarget) -> Amount {
    let session = wallet
        .request_mint(WalletMintRequest::bolt11(amount))
        .await
        .expect("mint session");
    let invoice =
        Bolt11Invoice::from_str(&session.initial_state().payment_request).expect("BOLT11 invoice");
    pay_if_regtest(&get_test_temp_dir(), &invoice)
        .await
        .expect("pay invoice");
    session
        .wait_with(
            MintClaimOptions {
                amount_split_target: split,
                conditions: None,
            },
            Duration::from_secs(120),
        )
        .await
        .expect("mint receipt")
        .amount
}

async fn unspent_proofs(wallet: &Wallet) -> cdk::nuts::Proofs {
    wallet
        .advanced()
        .proofs(ProofQuery::default())
        .await
        .expect("proofs")
        .into_iter()
        .map(|record| record.proof)
        .collect()
}

async fn reissue_all(wallet: &Wallet, proofs: cdk::nuts::Proofs) {
    wallet
        .advanced()
        .reissue(ReissueRequest {
            proofs,
            amount: None,
            amount_split_target: SplitTarget::default(),
            conditions: None,
            fee_policy: ReissueFeePolicy::Deduct,
            protection: ReissueProtection::Plain,
        })
        .await
        .expect("reissue proofs");
}

async fn get_notifications<T: StreamExt<Item = Result<Message, E>> + Unpin, E: Debug>(
    reader: &mut T,
    kind: &Kind,
    timeout_to_wait: Duration,
    total: usize,
) -> Vec<(String, NotificationPayload<String>)> {
    let mut results = Vec::new();
    for _ in 0..total {
        let msg = timeout(timeout_to_wait, reader.next())
            .await
            .expect("timeout")
            .unwrap()
            .unwrap();

        let mut response: serde_json::Value =
            serde_json::from_str(msg.to_text().unwrap()).expect("valid json");

        let mut params_raw = response
            .as_object_mut()
            .expect("object")
            .remove("params")
            .expect("valid params");

        let params_map = params_raw.as_object_mut().expect("params is object");

        results.push((
            params_map
                .remove("subId")
                .unwrap()
                .as_str()
                .unwrap()
                .to_string(),
            deserialize_payload_for_kind::<String, serde_json::Error>(
                kind,
                params_map.remove("payload").unwrap(),
            )
            .unwrap(),
        ))
    }
    results
}

/// Tests a complete mint-melt round trip with WebSocket notifications
///
/// This test verifies the full lifecycle of tokens:
/// 1. Creates a mint quote and pays the invoice
/// 2. Mints tokens and verifies the correct amount
/// 3. Creates a melt quote to spend tokens
/// 4. Subscribes to WebSocket notifications for the melt process
/// 5. Executes the melt and verifies the payment was successful
/// 6. Validates all WebSocket notifications received during the process
///
/// This ensures the entire mint-melt flow works correctly and that
/// WebSocket notifications are properly sent at each state transition.
#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_happy_mint_melt_round_trip() {
    let wallet = cdk_integration_tests::open_test_wallet(
        &get_mint_url_from_env(),
        CurrencyUnit::Sat,
        Arc::new(memory::empty().await.unwrap()),
        Mnemonic::generate(12).unwrap().to_seed_normalized(""),
        None,
    )
    .expect("failed to create new wallet");

    let (ws_stream, _) = connect_async(format!(
        "{}/v1/ws",
        get_mint_url_from_env().replace("http", "ws")
    ))
    .await
    .expect("Failed to connect");
    let (mut write, mut reader) = ws_stream.split();

    let mint_amount = mint_to_wallet(&wallet, 100.into(), SplitTarget::default()).await;

    assert!(mint_amount == 100.into());

    let invoice = create_invoice_for_env(Some(50)).await.unwrap();

    let metadata = HashMap::from([("test".to_string(), "value".to_string())]);
    let mut payment_request = PaymentQuoteRequest::new(PaymentTarget::bolt11(invoice));
    payment_request.metadata = metadata.clone();
    let payment = wallet
        .quote_payment(payment_request)
        .await
        .unwrap()
        .into_single()
        .unwrap();
    let melt = payment.quote();

    write
        .send(Message::Text(
            serde_json::to_string(&json!({
                    "jsonrpc": "2.0",
                    "id": 2,
                    "method": "subscribe",
                    "params": {
                      "kind": "bolt11_melt_quote",
                      "filters": [
                        melt.id.to_string(),
                      ],
                      "subId": "test-sub",
                    }

            }))
            .unwrap()
            .into(),
        ))
        .await
        .unwrap();

    // Parse both JSON strings to objects and compare them instead of comparing strings directly
    let binding = reader.next().await.unwrap().unwrap();
    let response_str = binding.to_text().unwrap();

    let response_json: serde_json::Value =
        serde_json::from_str(response_str).expect("Valid JSON response");
    let expected_json: serde_json::Value = serde_json::from_str(
        r#"{"jsonrpc":"2.0","result":{"status":"OK","subId":"test-sub"},"id":2}"#,
    )
    .expect("Valid JSON expected");

    assert_eq!(response_json, expected_json);

    // Read the initial state notification before starting the melt to ensure we capture Unpaid
    let initial_notification = get_notifications(
        &mut reader,
        &Kind::Bolt11MeltQuote,
        Duration::from_millis(15000),
        1,
    )
    .await;
    let (sub_id, payload) = &initial_notification[0];
    assert_eq!("test-sub", sub_id);
    let initial_melt = match payload {
        NotificationPayload::MeltQuoteBolt11Response(m) => m,
        _ => panic!("Wrong payload"),
    };
    assert_eq!(initial_melt.state, MeltQuoteState::Unpaid);
    assert_eq!(initial_melt.quote.to_string(), melt.id.to_string());

    // Now start the melt
    let melt_response = payment.prepare().await.unwrap().execute().await.unwrap();
    assert!(melt_response.payment_proof.is_some());

    let txs = wallet.history(HistoryQuery::default()).await.unwrap();
    let tx = txs
        .into_iter()
        .find(|tx| tx.quote_id.as_deref() == Some(melt.id.as_str()))
        .unwrap();
    assert_eq!(tx.amount, melt.amount);
    assert_eq!(tx.metadata, metadata);

    // Read remaining notifications (Pending -> Paid)
    let notifications = get_notifications(
        &mut reader,
        &Kind::Bolt11MeltQuote,
        Duration::from_millis(15000),
        2,
    )
    .await;

    let (sub_id, payload) = &notifications[0];
    assert_eq!("test-sub", sub_id);
    let pending_melt = match payload {
        NotificationPayload::MeltQuoteBolt11Response(m) => m,
        _ => panic!("Wrong payload"),
    };
    assert_eq!(pending_melt.state, MeltQuoteState::Pending);
    assert_eq!(pending_melt.quote.to_string(), melt.id.to_string());

    let (sub_id, payload) = &notifications[1];
    assert_eq!("test-sub", sub_id);
    let final_melt = match payload {
        NotificationPayload::MeltQuoteBolt11Response(m) => m,
        _ => panic!("Wrong payload"),
    };
    assert_eq!(final_melt.state, MeltQuoteState::Paid);
    assert_eq!(final_melt.amount, 50.into());
    assert_eq!(final_melt.quote.to_string(), melt.id.to_string());
}

/// Tests basic minting functionality with payment verification
///
/// This test focuses on the core minting process:
/// 1. Creates a mint quote for a specific amount (100 sats)
/// 2. Verifies the quote has the correct amount
/// 3. Pays the invoice (or simulates payment in non-regtest environments)
/// 4. Waits for the mint to recognize the payment
/// 5. Mints tokens and verifies the correct amount was received
///
/// This ensures the basic minting flow works correctly from quote to token issuance.
#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_happy_mint() {
    let wallet = cdk_integration_tests::open_test_wallet(
        &get_mint_url_from_env(),
        CurrencyUnit::Sat,
        Arc::new(memory::empty().await.unwrap()),
        Mnemonic::generate(12).unwrap().to_seed_normalized(""),
        None,
    )
    .expect("failed to create new wallet");

    let mint_amount = Amount::from(100);

    let mint_amount = mint_to_wallet(&wallet, mint_amount, SplitTarget::default()).await;

    assert!(mint_amount == 100.into());
}

/// Tests wallet restoration and proof state verification
///
/// This test verifies the wallet restoration process:
/// 1. Creates a wallet with a specific seed and mints tokens
/// 2. Verifies the wallet has the expected balance
/// 3. Creates a new wallet instance with the same seed but empty storage
/// 4. Confirms the new wallet starts with zero balance
/// 5. Restores the wallet state from the mint
/// 6. Swaps the proofs to ensure they're valid
/// 7. Verifies the restored wallet has the correct balance
/// 8. Checks that the original proofs are now marked as spent
///
/// This ensures wallet restoration works correctly and that
/// the mint properly tracks spent proofs across wallet instances.
#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_restore() {
    let seed = Mnemonic::generate(12).unwrap().to_seed_normalized("");
    let wallet = cdk_integration_tests::open_test_wallet(
        &get_mint_url_from_env(),
        CurrencyUnit::Sat,
        Arc::new(memory::empty().await.unwrap()),
        seed,
        None,
    )
    .expect("failed to create new wallet");

    mint_to_wallet(&wallet, 100.into(), SplitTarget::default()).await;

    assert_eq!(wallet.balance().await.unwrap().available, 100.into());

    let wallet_2 = cdk_integration_tests::open_test_wallet(
        &get_mint_url_from_env(),
        CurrencyUnit::Sat,
        Arc::new(memory::empty().await.unwrap()),
        seed,
        None,
    )
    .expect("failed to create new wallet");

    assert_eq!(wallet_2.balance().await.unwrap().available, 0.into());

    let restored = wallet_2
        .restore_from_seed(WalletRestoreRequest::default())
        .await
        .unwrap();
    let proofs = unspent_proofs(&wallet_2).await;

    assert!(!proofs.is_empty());

    let expected_fee = wallet_2
        .advanced()
        .estimate_fee(FeeEstimateRequest::Proofs(proofs.clone()))
        .await
        .unwrap()
        .total;
    reissue_all(&wallet_2, proofs).await;

    assert_eq!(restored.unspent, 100.into());

    // Since we have to do a swap we expect to restore amount - fee
    assert_eq!(
        wallet_2.balance().await.unwrap().available,
        Amount::from(100) - expected_fee
    );

    let proofs = unspent_proofs(&wallet).await;

    let states = wallet
        .advanced()
        .check_proofs(ProofCheckRequest { proofs })
        .await
        .unwrap();

    for state in states {
        if state.state != State::Spent {
            panic!("All proofs should be spent");
        }
    }
}

/// Tests wallet restoration with a large number of proofs (3000)
///
/// This test verifies the restore process works correctly with many proofs,
/// which is important for testing database performance (especially PostgreSQL)
/// and ensuring the restore batching logic handles large proof sets:
/// 1. Creates a wallet and mints 3000 sats as individual 1-sat proofs
/// 2. Creates a new wallet instance with the same seed but empty storage
/// 3. Restores the wallet state from the mint (requires ~30 restore batches)
/// 4. Verifies all 3000 proofs are correctly restored
/// 5. Swaps the proofs to ensure they're valid
/// 6. Checks that the original proofs are now marked as spent
#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_restore_large_proof_count() {
    let seed = Mnemonic::generate(12).unwrap().to_seed_normalized("");
    let wallet = cdk_integration_tests::open_test_wallet(
        &get_mint_url_from_env(),
        CurrencyUnit::Sat,
        Arc::new(memory::empty().await.unwrap()),
        seed,
        None,
    )
    .expect("failed to create new wallet");

    // This stress test runs against a controlled localhost mint. Disable pacing
    // before the first request so the default client rate limit does not dominate
    // the runtime of the 3,000-proof restore workload.
    wallet.disable_rate_limiting();

    let mint_amount: u64 = 3000;
    let batch_size: u64 = 999; // Keep under 1000 outputs per request

    // Mint in batches to avoid exceeding the 1000 output limit per request
    let mut remaining = mint_amount;

    while remaining > 0 {
        let batch = remaining.min(batch_size);

        mint_to_wallet(&wallet, batch.into(), SplitTarget::Value(1.into())).await;
        remaining -= batch;
    }

    assert_eq!(unspent_proofs(&wallet).await.len(), mint_amount as usize);
    assert_eq!(
        wallet.balance().await.unwrap().available,
        mint_amount.into()
    );

    let wallet_2 = cdk_integration_tests::open_test_wallet(
        &get_mint_url_from_env(),
        CurrencyUnit::Sat,
        Arc::new(memory::empty().await.unwrap()),
        seed,
        None,
    )
    .expect("failed to create new wallet");

    // This is a separate client and therefore has its own limiter. Disable it
    // before restore so all restore batches run without artificial delays.
    wallet_2.disable_rate_limiting();

    assert_eq!(wallet_2.balance().await.unwrap().available, 0.into());

    let restored = wallet_2
        .restore_from_seed(WalletRestoreRequest::default())
        .await
        .unwrap();
    let proofs = unspent_proofs(&wallet_2).await;

    assert_eq!(proofs.len(), mint_amount as usize);
    assert_eq!(restored.unspent, mint_amount.into());

    // Swap in batches to avoid exceeding the 1000 input limit per request
    let mut total_fee = Amount::ZERO;
    for batch in proofs.chunks(batch_size as usize) {
        let batch_vec = batch.to_vec();
        let batch_fee = wallet_2
            .advanced()
            .estimate_fee(FeeEstimateRequest::Proofs(batch_vec.clone()))
            .await
            .unwrap()
            .total;
        total_fee += batch_fee;
        reissue_all(&wallet_2, batch_vec).await;
    }

    // Since we have to do a swap we expect to restore amount - fee
    assert_eq!(
        wallet_2.balance().await.unwrap().available,
        Amount::from(mint_amount) - total_fee
    );

    let proofs = unspent_proofs(&wallet).await;

    // Check proofs in batches to avoid large queries
    for batch in proofs.chunks(100) {
        let states = wallet
            .advanced()
            .check_proofs(ProofCheckRequest {
                proofs: batch.to_vec(),
            })
            .await
            .unwrap();
        for state in states {
            if state.state != State::Spent {
                panic!("All proofs should be spent");
            }
        }
    }
}

/// Tests that wallet restore correctly handles non-sequential counter values
///
/// This test verifies that after restoring a wallet where there were gaps in the
/// counter sequence (e.g., due to failed operations or multi-device usage), the
/// wallet can continue to operate without errors.
///
/// Test scenario:
/// 1. Wallet mints proofs using counters 0-N
/// 2. Counter is incremented to simulate failed operations that consumed counter values
/// 3. Wallet mints more proofs using counters at higher values
/// 4. New wallet restores from seed and finds proofs at non-sequential counter positions
/// 5. Wallet should be able to continue normal operations (swaps) after restore
#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_restore_with_counter_gap() {
    let seed = Mnemonic::generate(12).unwrap().to_seed_normalized("");
    let store = Arc::new(memory::empty().await.unwrap());
    let wallet = cdk_integration_tests::open_test_wallet(
        &get_mint_url_from_env(),
        CurrencyUnit::Sat,
        store.clone(),
        seed,
        None,
    )
    .expect("failed to create new wallet");

    // This test runs against a controlled localhost mint. Disable pacing before
    // the first request so its timing reflects restore/counter-gap behavior.
    wallet.disable_rate_limiting();

    // Mint first batch of proofs (uses counters starting at 0)
    mint_to_wallet(&wallet, 100.into(), SplitTarget::default()).await;

    assert_eq!(wallet.balance().await.unwrap().available, 100.into());

    // Get the active keyset ID to increment counter
    let keyset_id = wallet
        .advanced()
        .mint_metadata(MintMetadataRequest {
            source: MetadataSource::Refresh,
        })
        .await
        .unwrap()
        .active_keyset()
        .expect("active keyset")
        .id;

    // Create a gap in the counter sequence
    // This simulates failed operations or multi-device usage where counter values
    // were consumed but no signatures were obtained
    let gap_size = 50u32;
    store
        .increment_keyset_counter(&keyset_id, gap_size)
        .await
        .unwrap();

    // Mint second batch of proofs (uses counters after the gap)
    mint_to_wallet(&wallet, 100.into(), SplitTarget::default()).await;

    assert_eq!(wallet.balance().await.unwrap().available, 200.into());

    // Create a new wallet with the same seed (simulating wallet restore scenario)
    let wallet_restored = cdk_integration_tests::open_test_wallet(
        &get_mint_url_from_env(),
        CurrencyUnit::Sat,
        Arc::new(memory::empty().await.unwrap()),
        seed,
        None,
    )
    .expect("failed to create restored wallet");

    // The restored wallet is a separate client with a separate limiter. Disable
    // it before restore and the post-restore swaps exercised below.
    wallet_restored.disable_rate_limiting();

    assert_eq!(wallet_restored.balance().await.unwrap().available, 0.into());

    // Restore the wallet - this should find proofs at non-sequential counter positions
    let restored = wallet_restored
        .restore_from_seed(WalletRestoreRequest::default())
        .await
        .unwrap();
    assert_eq!(restored.unspent, 200.into());

    let proofs = unspent_proofs(&wallet_restored).await;
    assert!(!proofs.is_empty());

    // Swap the restored proofs to verify they are valid
    let expected_fee = wallet_restored
        .advanced()
        .estimate_fee(FeeEstimateRequest::Proofs(proofs.clone()))
        .await
        .unwrap()
        .total;
    reissue_all(&wallet_restored, proofs).await;

    let balance_after_first_swap = Amount::from(200) - expected_fee;
    assert_eq!(
        wallet_restored.balance().await.unwrap().available,
        balance_after_first_swap
    );

    // Perform multiple swaps to verify the wallet can continue operating
    // after restore with non-sequential counter values
    for i in 0..gap_size {
        let proofs = unspent_proofs(&wallet_restored).await;
        if proofs.is_empty() {
            break;
        }

        let swap_result = wallet_restored
            .advanced()
            .reissue(ReissueRequest {
                proofs,
                amount: None,
                amount_split_target: SplitTarget::default(),
                conditions: None,
                fee_policy: ReissueFeePolicy::Deduct,
                protection: ReissueProtection::Plain,
            })
            .await;

        match swap_result {
            Ok(_) => {
                // Swap succeeded, continue
            }
            Err(e) => {
                let error_str = format!("{:?}", e);
                if error_str.contains("BlindedMessageAlreadySigned") {
                    panic!(
                        "Got 'blinded message already signed' error on swap {} after restore. \
                         Counter was not correctly set after restoring with non-sequential values.",
                        i + 1
                    );
                } else {
                    // Some other error - might be expected (e.g., insufficient funds due to fees)
                    break;
                }
            }
        }
    }
}

/// Tests that the melt quote status can be checked after a melt has completed
///
/// This test verifies:
/// 1. Mint tokens
/// 2. Create a melt quote and execute the melt
/// 3. Check the melt quote status via the wallet
/// 4. Verify the quote is in the Paid state
///
/// This ensures the mint correctly reports the melt quote status after completion.
#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_melt_quote_status_after_melt() {
    let wallet = cdk_integration_tests::open_test_wallet(
        &get_mint_url_from_env(),
        CurrencyUnit::Sat,
        Arc::new(memory::empty().await.unwrap()),
        Mnemonic::generate(12).unwrap().to_seed_normalized(""),
        None,
    )
    .expect("failed to create new wallet");

    let mint_amount = mint_to_wallet(&wallet, 100.into(), SplitTarget::default()).await;
    assert_eq!(mint_amount, 100.into());

    let invoice = create_invoice_for_env(Some(50)).await.unwrap();

    let payment = wallet
        .quote_payment(PaymentQuoteRequest::new(PaymentTarget::bolt11(invoice)))
        .await
        .unwrap()
        .into_single()
        .unwrap();
    payment.prepare().await.unwrap().execute().await.unwrap();

    let quote_status = payment.refresh().await.unwrap();
    assert_eq!(
        quote_status.state,
        PaymentState::Paid,
        "Melt quote should be in Paid state after successful melt"
    );
}

/// Tests that the melt quote status can be checked via WalletManager after a melt has completed
///
/// This test verifies the same flow as test_melt_quote_status_after_melt but using
/// the WalletManager abstraction:
/// 1. Create a WalletManager and add a mint
/// 2. Mint tokens via the wallet manager
/// 3. Create a melt quote and execute the melt
/// 4. Check the melt quote status via check_melt_quote
/// 5. Verify the quote is in the Paid state
#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_melt_quote_status_after_melt_wallet_manager() {
    let seed = Mnemonic::generate(12).unwrap().to_seed_normalized("");
    let localstore = Arc::new(memory::empty().await.unwrap());

    let wallet_manager = WalletManagerBuilder::new()
        .with_store(localstore.clone())
        .with_seed(seed)
        .build()
        .await
        .expect("failed to create wallet manager");

    let mint_url = MintUrl::from_str(&get_mint_url_from_env()).expect("invalid mint url");
    wallet_manager
        .register_mint(mint_url.clone())
        .await
        .expect("failed to add mint");

    // Get the wallet from the manager to call methods directly
    let wallet = wallet_manager
        .wallet(WalletIdentity::new(mint_url.clone(), CurrencyUnit::Sat))
        .await
        .expect("failed to get wallet");

    mint_to_wallet(&wallet, 100.into(), SplitTarget::default()).await;

    let balances = wallet_manager.balance_totals().await.unwrap();
    let balance = balances
        .get(&CurrencyUnit::Sat)
        .copied()
        .unwrap_or(Amount::ZERO);
    assert_eq!(balance, 100.into());

    let invoice = create_invoice_for_env(Some(50)).await.unwrap();

    let payment = wallet
        .quote_payment(PaymentQuoteRequest::new(PaymentTarget::bolt11(invoice)))
        .await
        .unwrap()
        .into_single()
        .unwrap();
    payment.prepare().await.unwrap().execute().await.unwrap();

    let quote_status = payment.refresh().await.unwrap();
    assert_eq!(
        quote_status.state,
        PaymentState::Paid,
        "Melt quote should be in Paid state after successful melt (via WalletManager)"
    );

    let db_quote = localstore
        .get_melt_quote(payment.id().as_str())
        .await
        .unwrap()
        .unwrap();

    assert_eq!(
        db_quote.state,
        MeltQuoteState::Paid,
        "Melt quote should be in Paid state after successful melt"
    );
}

/// Tests that change outputs in a melt quote are correctly handled
///
/// This test verifies the following workflow:
/// 1. Mint 100 sats of tokens
/// 2. Create a melt quote for 9 sats (which requires 100 sats input with 91 sats change)
/// 3. Manually construct a melt request with proofs and blinded messages for change
/// 4. Verify that the change proofs in the response match what's reported by the quote status
///
/// This ensures the mint correctly processes change outputs during melting operations
/// and that the wallet can properly verify the change amounts match expectations.
#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_fake_melt_change_in_quote() {
    let wallet = cdk_integration_tests::open_test_wallet(
        &get_mint_url_from_env(),
        CurrencyUnit::Sat,
        Arc::new(memory::empty().await.unwrap()),
        Mnemonic::generate(12).unwrap().to_seed_normalized(""),
        None,
    )
    .expect("failed to create new wallet");

    mint_to_wallet(&wallet, 100.into(), SplitTarget::default()).await;

    let invoice = create_invoice_for_env(Some(9)).await.unwrap();

    let proofs = unspent_proofs(&wallet).await;

    let payment = wallet
        .quote_payment(PaymentQuoteRequest::new(PaymentTarget::bolt11(invoice)))
        .await
        .unwrap()
        .into_single()
        .unwrap();
    let melt_quote = payment.quote();

    let keyset = wallet
        .advanced()
        .mint_metadata(MintMetadataRequest {
            source: MetadataSource::Refresh,
        })
        .await
        .unwrap()
        .active_keyset()
        .expect("active keyset")
        .clone();
    let fee_and_amounts = (0, ((0..32).map(|x| 2u64.pow(x)).collect::<Vec<_>>())).into();

    let premint_secrets = PreMintSecrets::random(
        keyset.id,
        100.into(),
        &SplitTarget::default(),
        &fee_and_amounts,
    )
    .unwrap();

    let client = HttpClient::new(get_mint_url_from_env().parse().unwrap(), None);

    let melt_request = MeltRequest::new(
        melt_quote.id.to_string(),
        proofs.clone(),
        Some(premint_secrets.blinded_messages()),
    );

    let melt_response = client
        .post_melt(&PaymentMethod::Known(KnownMethod::Bolt11), melt_request)
        .await
        .unwrap();

    assert!(melt_response.change().is_some());

    let check = client
        .get_melt_quote_status(PaymentMethod::BOLT11, melt_quote.id.as_str())
        .await
        .unwrap();
    let mut melt_change = melt_response.change().unwrap().clone();
    melt_change.sort_by_key(|a| a.amount);

    let mut check = match check {
        cdk_common::MeltQuoteResponse::Bolt11(r) => r.change.unwrap(),
        _ => panic!("Expected Bolt11 melt quote response"),
    };
    check.sort_by_key(|a| a.amount);

    assert_eq!(melt_change, check);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_pay_invoice_twice() {
    let payment_backend = match env::var("PAYMENT_BACKEND") {
        Ok(val) => Some(val),
        Err(_) => env::var("CDK_MINTD_PAYMENT_BACKEND").ok(),
    };

    if payment_backend.map(|backend| backend.to_uppercase()) == Some("FAKEWALLET".to_string()) {
        // We can only perform this test on regtest backends as fake wallet just marks the quote as paid
        return;
    }

    let wallet = cdk_integration_tests::open_test_wallet(
        &get_mint_url_from_env(),
        CurrencyUnit::Sat,
        Arc::new(memory::empty().await.unwrap()),
        Mnemonic::generate(12).unwrap().to_seed_normalized(""),
        None,
    )
    .expect("failed to create new wallet");

    let mint_amount = mint_to_wallet(&wallet, 100.into(), SplitTarget::default()).await;

    assert_eq!(mint_amount, 100.into());

    let invoice = create_invoice_for_env(Some(25)).await.unwrap();

    let payment = wallet
        .quote_payment(PaymentQuoteRequest::new(PaymentTarget::bolt11(
            invoice.clone(),
        )))
        .await
        .unwrap()
        .into_single()
        .unwrap();
    let melt = payment.prepare().await.unwrap().execute().await.unwrap();

    // Creating a second quote for the same invoice is allowed
    let payment_two = wallet
        .quote_payment(PaymentQuoteRequest::new(PaymentTarget::bolt11(invoice)))
        .await
        .unwrap()
        .into_single()
        .unwrap();

    // But attempting to melt (pay) the second quote should fail
    // since the first quote with the same lookup_id is already paid
    let melt_two = async { payment_two.prepare().await?.execute().await }.await;

    match melt_two {
        Err(err) => {
            let err_str = err.to_string().to_lowercase();
            if !err_str.contains("duplicate")
                && !err_str.contains("already paid")
                && !err_str.contains("request already paid")
            {
                panic!("Expected duplicate/already paid error, got: {}", err);
            }
        }
        Ok(_) => {
            panic!("Should not have allowed second payment");
        }
    }

    let balance = wallet.balance().await.unwrap().available;

    assert_eq!(balance, (Amount::from(100) - melt.fee_paid - melt.amount));
}
