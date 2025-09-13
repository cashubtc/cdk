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
use std::env;
use std::fmt::Debug;
use std::path::PathBuf;
use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;

use bip39::Mnemonic;
use cashu::{MeltRequest, PreMintSecrets};
use cdk::amount::{Amount, SplitTarget};
use cdk::nuts::nut00::ProofsMethods;
use cdk::nuts::{CurrencyUnit, MeltQuoteState, NotificationPayload, State};
use cdk::wallet::{HttpClient, MintConnector, Wallet};
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

async fn get_notification<T: StreamExt<Item = Result<Message, E>> + Unpin, E: Debug>(
    reader: &mut T,
    timeout_to_wait: Duration,
) -> (String, NotificationPayload<String>) {
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

    (
        params_map
            .remove("subId")
            .unwrap()
            .as_str()
            .unwrap()
            .to_string(),
        serde_json::from_value(params_map.remove("payload").unwrap()).unwrap(),
    )
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
    let wallet = Wallet::new(
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

    let mint_quote = wallet.mint_quote(100.into(), None).await.unwrap();

    let invoice = Bolt11Invoice::from_str(&mint_quote.request).unwrap();
    pay_if_regtest(&get_test_temp_dir(), &invoice)
        .await
        .unwrap();

    let proofs = wallet
        .wait_and_mint_quote(
            mint_quote.clone(),
            SplitTarget::default(),
            None,
            tokio::time::Duration::from_secs(60),
        )
        .await
        .expect("payment");

    let mint_amount = proofs.total_amount().unwrap();

    assert!(mint_amount == 100.into());

    let invoice = create_invoice_for_env(Some(50)).await.unwrap();

    let melt = wallet.melt_quote(invoice, None).await.unwrap();

    write
        .send(Message::Text(
            serde_json::to_string(&json!({
                    "jsonrpc": "2.0",
                    "id": 2,
                    "method": "subscribe",
                    "params": {
                      "kind": "bolt11_melt_quote",
                      "filters": [
                        melt.id.clone(),
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

    let melt_response = wallet.melt(&melt.id).await.unwrap();
    assert!(melt_response.preimage.is_some());
    assert!(melt_response.state == MeltQuoteState::Paid);

    let (sub_id, payload) = get_notification(&mut reader, Duration::from_millis(15000)).await;
    // first message is the current state
    assert_eq!("test-sub", sub_id);
    let payload = match payload {
        NotificationPayload::MeltQuoteBolt11Response(melt) => melt,
        _ => panic!("Wrong payload"),
    };

    // assert_eq!(payload.amount + payload.fee_reserve, 50.into());
    assert_eq!(payload.quote.to_string(), melt.id);
    assert_eq!(payload.state, MeltQuoteState::Unpaid);

    // get current state
    let (sub_id, payload) = get_notification(&mut reader, Duration::from_millis(15000)).await;
    assert_eq!("test-sub", sub_id);
    let payload = match payload {
        NotificationPayload::MeltQuoteBolt11Response(melt) => melt,
        _ => panic!("Wrong payload"),
    };
    assert_eq!(payload.quote.to_string(), melt.id);
    assert_eq!(payload.state, MeltQuoteState::Pending);

    // get current state
    let (sub_id, payload) = get_notification(&mut reader, Duration::from_millis(15000)).await;
    assert_eq!("test-sub", sub_id);
    let payload = match payload {
        NotificationPayload::MeltQuoteBolt11Response(melt) => melt,
        _ => panic!("Wrong payload"),
    };
    assert_eq!(payload.amount, 50.into());
    assert_eq!(payload.quote.to_string(), melt.id);
    assert_eq!(payload.state, MeltQuoteState::Paid);
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
    let wallet = Wallet::new(
        &get_mint_url_from_env(),
        CurrencyUnit::Sat,
        Arc::new(memory::empty().await.unwrap()),
        Mnemonic::generate(12).unwrap().to_seed_normalized(""),
        None,
    )
    .expect("failed to create new wallet");

    let mint_amount = Amount::from(100);

    let mint_quote = wallet.mint_quote(mint_amount, None).await.unwrap();

    assert_eq!(mint_quote.amount, Some(mint_amount));

    let invoice = Bolt11Invoice::from_str(&mint_quote.request).unwrap();
    pay_if_regtest(&get_test_temp_dir(), &invoice)
        .await
        .unwrap();

    let proofs = wallet
        .wait_and_mint_quote(
            mint_quote.clone(),
            SplitTarget::default(),
            None,
            tokio::time::Duration::from_secs(60),
        )
        .await
        .expect("payment");

    let mint_amount = proofs.total_amount().unwrap();

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
    let wallet = Wallet::new(
        &get_mint_url_from_env(),
        CurrencyUnit::Sat,
        Arc::new(memory::empty().await.unwrap()),
        seed,
        None,
    )
    .expect("failed to create new wallet");

    let mint_quote = wallet.mint_quote(100.into(), None).await.unwrap();

    let invoice = Bolt11Invoice::from_str(&mint_quote.request).unwrap();
    pay_if_regtest(&get_test_temp_dir(), &invoice)
        .await
        .unwrap();

    let _proofs = wallet
        .wait_and_mint_quote(
            mint_quote.clone(),
            SplitTarget::default(),
            None,
            tokio::time::Duration::from_secs(60),
        )
        .await
        .expect("payment");

    assert_eq!(wallet.total_balance().await.unwrap(), 100.into());

    let wallet_2 = Wallet::new(
        &get_mint_url_from_env(),
        CurrencyUnit::Sat,
        Arc::new(memory::empty().await.unwrap()),
        seed,
        None,
    )
    .expect("failed to create new wallet");

    assert_eq!(wallet_2.total_balance().await.unwrap(), 0.into());

    let restored = wallet_2.restore().await.unwrap();
    let proofs = wallet_2.get_unspent_proofs().await.unwrap();

    assert!(!proofs.is_empty());

    let expected_fee = wallet.get_proofs_fee(&proofs).await.unwrap();
    wallet_2
        .swap(None, SplitTarget::default(), proofs, None, false)
        .await
        .unwrap();

    assert_eq!(restored, 100.into());

    // Since we have to do a swap we expect to restore amount - fee
    assert_eq!(
        wallet_2.total_balance().await.unwrap(),
        Amount::from(100) - expected_fee
    );

    let proofs = wallet.get_unspent_proofs().await.unwrap();

    let states = wallet.check_proofs_spent(proofs).await.unwrap();

    for state in states {
        if state.state != State::Spent {
            panic!("All proofs should be spent");
        }
    }
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
    let wallet = Wallet::new(
        &get_mint_url_from_env(),
        CurrencyUnit::Sat,
        Arc::new(memory::empty().await.unwrap()),
        Mnemonic::generate(12).unwrap().to_seed_normalized(""),
        None,
    )
    .expect("failed to create new wallet");

    let mint_quote = wallet.mint_quote(100.into(), None).await.unwrap();

    let bolt11 = Bolt11Invoice::from_str(&mint_quote.request).unwrap();

    pay_if_regtest(&get_test_temp_dir(), &bolt11).await.unwrap();

    let _proofs = wallet
        .wait_and_mint_quote(
            mint_quote.clone(),
            SplitTarget::default(),
            None,
            tokio::time::Duration::from_secs(60),
        )
        .await
        .expect("payment");

    let invoice = create_invoice_for_env(Some(9)).await.unwrap();

    let proofs = wallet.get_unspent_proofs().await.unwrap();

    let melt_quote = wallet.melt_quote(invoice.to_string(), None).await.unwrap();

    let keyset = wallet.fetch_active_keyset().await.unwrap();
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
        melt_quote.id.clone(),
        proofs.clone(),
        Some(premint_secrets.blinded_messages()),
    );

    let melt_response = client.post_melt(melt_request).await.unwrap();

    assert!(melt_response.change.is_some());

    let check = wallet.melt_quote_status(&melt_quote.id).await.unwrap();
    let mut melt_change = melt_response.change.unwrap();
    melt_change.sort_by(|a, b| a.amount.cmp(&b.amount));

    let mut check = check.change.unwrap();
    check.sort_by(|a, b| a.amount.cmp(&b.amount));

    assert_eq!(melt_change, check);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_pay_invoice_twice() {
    let ln_backend = match env::var("LN_BACKEND") {
        Ok(val) => Some(val),
        Err(_) => env::var("CDK_MINTD_LN_BACKEND").ok(),
    };

    if ln_backend.map(|ln| ln.to_uppercase()) == Some("FAKEWALLET".to_string()) {
        // We can only perform this test on regtest backends as fake wallet just marks the quote as paid
        return;
    }

    let wallet = Wallet::new(
        &get_mint_url_from_env(),
        CurrencyUnit::Sat,
        Arc::new(memory::empty().await.unwrap()),
        Mnemonic::generate(12).unwrap().to_seed_normalized(""),
        None,
    )
    .expect("failed to create new wallet");

    let mint_quote = wallet.mint_quote(100.into(), None).await.unwrap();

    pay_if_regtest(&get_test_temp_dir(), &mint_quote.request.parse().unwrap())
        .await
        .unwrap();

    let proofs = wallet
        .wait_and_mint_quote(
            mint_quote.clone(),
            SplitTarget::default(),
            None,
            tokio::time::Duration::from_secs(60),
        )
        .await
        .expect("payment");

    let mint_amount = proofs.total_amount().unwrap();

    assert_eq!(mint_amount, 100.into());

    let invoice = create_invoice_for_env(Some(25)).await.unwrap();

    let melt_quote = wallet.melt_quote(invoice.clone(), None).await.unwrap();

    let melt = wallet.melt(&melt_quote.id).await.unwrap();

    let melt_two = wallet.melt_quote(invoice, None).await;

    match melt_two {
        Err(err) => match err {
            cdk::Error::RequestAlreadyPaid => (),
            err => {
                if !err.to_string().contains("Duplicate entry") {
                    panic!("Wrong invoice already paid: {}", err.to_string());
                }
            }
        },
        Ok(_) => {
            panic!("Should not have allowed second payment");
        }
    }

    let balance = wallet.total_balance().await.unwrap();

    assert_eq!(balance, (Amount::from(100) - melt.fee_paid - melt.amount));
}
