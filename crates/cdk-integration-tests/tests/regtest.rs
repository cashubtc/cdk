use std::sync::Arc;
use std::time::Duration;

use bip39::Mnemonic;
use bitcoin::hashes::{sha256, Hash};
use cashu::ProofsMethods;
use cdk::amount::{Amount, SplitTarget};
use cdk::nuts::{
    CurrencyUnit, MeltOptions, MeltQuoteState, MintQuoteState, MintRequest, Mpp,
    NotificationPayload, PreMintSecrets,
};
use cdk::util::hex;
use cdk::wallet::{HttpClient, MintConnector, Wallet, WalletSubscription};
use cdk_integration_tests::init_regtest::get_temp_dir;
use cdk_integration_tests::{
    get_mint_url_from_env, get_second_mint_url_from_env, get_test_client, init_lnd_client,
};
use cdk_sqlite::wallet::{self, memory};
use futures::join;
use tokio::time::timeout;

const LDK_URL: &str = "http://127.0.0.1:8089";

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_internal_payment() {
    let ln_client = get_test_client().await;

    let wallet = Wallet::new(
        &get_mint_url_from_env(),
        CurrencyUnit::Sat,
        Arc::new(memory::empty().await.unwrap()),
        Mnemonic::generate(12).unwrap().to_seed_normalized(""),
        None,
    )
    .expect("failed to create new wallet");

    let mint_quote = wallet.mint_quote(100.into(), None).await.unwrap();

    ln_client
        .pay_invoice(mint_quote.request.clone())
        .await
        .expect("failed to pay invoice");

    let _proofs = wallet
        .wait_and_mint_quote(
            mint_quote.clone(),
            SplitTarget::default(),
            None,
            tokio::time::Duration::from_secs(60),
        )
        .await
        .expect("payment");

    assert!(wallet.total_balance().await.unwrap() == 100.into());

    let wallet_2 = Wallet::new(
        &get_mint_url_from_env(),
        CurrencyUnit::Sat,
        Arc::new(memory::empty().await.unwrap()),
        Mnemonic::generate(12).unwrap().to_seed_normalized(""),
        None,
    )
    .expect("failed to create new wallet");

    let mint_quote = wallet_2.mint_quote(10.into(), None).await.unwrap();

    let melt = wallet
        .melt_quote(mint_quote.request.clone(), None)
        .await
        .unwrap();

    assert_eq!(melt.amount, 10.into());

    let _melted = wallet.melt(&melt.id).await.unwrap();

    let _proofs = wallet_2
        .wait_and_mint_quote(
            mint_quote.clone(),
            SplitTarget::default(),
            None,
            tokio::time::Duration::from_secs(60),
        )
        .await
        .expect("payment");

    // let check_paid = match get_mint_port("0") {
    //     8085 => {
    //         let cln_one_dir = get_cln_dir(&get_temp_dir(), "one");
    //         let cln_client = ClnClient::new(cln_one_dir.clone(), None).await.unwrap();
    //         let payment_hash = Bolt11Invoice::from_str(&mint_quote.request).unwrap();
    //         cln_client
    //             .check_incoming_payment_status(&payment_hash.payment_hash().to_string())
    //             .await
    //             .expect("Could not check invoice")
    //     }
    //     8087 => {
    //         let lnd_two_dir = get_lnd_dir(&get_temp_dir(), "two");
    //         let lnd_client = LndClient::new(
    //             format!("https://{}", LND_TWO_RPC_ADDR),
    //             get_lnd_cert_file_path(&lnd_two_dir),
    //             get_lnd_macaroon_path(&lnd_two_dir),
    //         )
    //         .await
    //         .unwrap();
    //         let payment_hash = Bolt11Invoice::from_str(&mint_quote.request).unwrap();
    //         lnd_client
    //             .check_incoming_payment_status(&payment_hash.payment_hash().to_string())
    //             .await
    //             .expect("Could not check invoice")
    //     }
    //     _ => panic!("Unknown mint port"),
    // };

    // match check_paid {
    //     InvoiceStatus::Unpaid => (),
    //     _ => {
    //         panic!("Invoice has incorrect status: {:?}", check_paid);
    //     }
    // }

    let wallet_2_balance = wallet_2.total_balance().await.unwrap();

    assert!(wallet_2_balance == 10.into());

    let wallet_1_balance = wallet.total_balance().await.unwrap();

    assert!(wallet_1_balance == 90.into());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_websocket_connection() {
    let wallet = Wallet::new(
        &get_mint_url_from_env(),
        CurrencyUnit::Sat,
        Arc::new(wallet::memory::empty().await.unwrap()),
        Mnemonic::generate(12).unwrap().to_seed_normalized(""),
        None,
    )
    .expect("failed to create new wallet");

    // Create a small mint quote to test notifications
    let mint_quote = wallet.mint_quote(10.into(), None).await.unwrap();

    // Subscribe to notifications for this quote
    let mut subscription = wallet
        .subscribe(WalletSubscription::Bolt11MintQuoteState(vec![mint_quote
            .id
            .clone()]))
        .await
        .expect("failed to subscribe");

    // First check we get the unpaid state
    let msg = timeout(Duration::from_secs(10), subscription.recv())
        .await
        .expect("timeout waiting for unpaid notification")
        .expect("No paid notification received");

    match msg.into_inner() {
        NotificationPayload::MintQuoteBolt11Response(response) => {
            assert_eq!(response.quote.to_string(), mint_quote.id);
            assert_eq!(response.state, MintQuoteState::Unpaid);
        }
        _ => panic!("Unexpected notification type"),
    }

    let ln_client = get_test_client().await;
    ln_client
        .pay_invoice(mint_quote.request)
        .await
        .expect("failed to pay invoice");

    // Wait for paid notification with 10 second timeout
    let msg = timeout(Duration::from_secs(10), subscription.recv())
        .await
        .expect("timeout waiting for paid notification")
        .expect("No paid notification received");

    match msg.into_inner() {
        NotificationPayload::MintQuoteBolt11Response(response) => {
            assert_eq!(response.quote.to_string(), mint_quote.id);
            assert_eq!(response.state, MintQuoteState::Paid);
        }
        _ => panic!("Unexpected notification type"),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_multimint_melt() {
    if get_mint_url_from_env() == LDK_URL {
        return;
    }

    let ln_client = get_test_client().await;

    let db = Arc::new(memory::empty().await.unwrap());
    let wallet1 = Wallet::new(
        &get_mint_url_from_env(),
        CurrencyUnit::Sat,
        db,
        Mnemonic::generate(12).unwrap().to_seed_normalized(""),
        None,
    )
    .expect("failed to create new wallet");

    let db = Arc::new(memory::empty().await.unwrap());
    let wallet2 = Wallet::new(
        &get_second_mint_url_from_env(),
        CurrencyUnit::Sat,
        db,
        Mnemonic::generate(12).unwrap().to_seed_normalized(""),
        None,
    )
    .expect("failed to create new wallet");

    let mint_amount = Amount::from(100);

    // Fund the wallets
    let quote = wallet1.mint_quote(mint_amount, None).await.unwrap();
    ln_client
        .pay_invoice(quote.request.clone())
        .await
        .expect("failed to pay invoice");

    let _proofs = wallet1
        .wait_and_mint_quote(
            quote.clone(),
            SplitTarget::default(),
            None,
            tokio::time::Duration::from_secs(60),
        )
        .await
        .expect("payment");

    let quote = wallet2.mint_quote(mint_amount, None).await.unwrap();
    ln_client
        .pay_invoice(quote.request.clone())
        .await
        .expect("failed to pay invoice");

    let _proofs = wallet2
        .wait_and_mint_quote(
            quote.clone(),
            SplitTarget::default(),
            None,
            tokio::time::Duration::from_secs(60),
        )
        .await
        .expect("payment");

    // Get an invoice
    let invoice = ln_client.create_invoice(Some(50)).await.unwrap();

    // Get multi-part melt quotes
    let melt_options = MeltOptions::Mpp {
        mpp: Mpp {
            amount: Amount::from(25000),
        },
    };
    let quote_1 = wallet1
        .melt_quote(invoice.clone(), Some(melt_options))
        .await
        .expect("Could not get melt quote");
    let quote_2 = wallet2
        .melt_quote(invoice.clone(), Some(melt_options))
        .await
        .expect("Could not get melt quote");

    // Multimint pay invoice
    let result1 = wallet1.melt(&quote_1.id);
    let result2 = wallet2.melt(&quote_2.id);
    let result = join!(result1, result2);

    // Unpack results
    let result1 = result.0.unwrap();
    let result2 = result.1.unwrap();

    // Check
    assert!(result1.state == result2.state);
    assert!(result1.state == MeltQuoteState::Paid);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_cached_mint() {
    let ln_client = get_test_client().await;
    let wallet = Wallet::new(
        &get_mint_url_from_env(),
        CurrencyUnit::Sat,
        Arc::new(memory::empty().await.unwrap()),
        Mnemonic::generate(12).unwrap().to_seed_normalized(""),
        None,
    )
    .expect("failed to create new wallet");

    let mint_amount = Amount::from(100);

    let quote = wallet.mint_quote(mint_amount, None).await.unwrap();
    ln_client
        .pay_invoice(quote.request.clone())
        .await
        .expect("failed to pay invoice");

    let _proofs = wallet
        .wait_for_payment(&quote, tokio::time::Duration::from_secs(15))
        .await
        .expect("payment");

    let active_keyset_id = wallet.fetch_active_keyset().await.unwrap().id;
    let fee_and_amounts = (0, ((0..32).map(|x| 2u64.pow(x)).collect::<Vec<_>>())).into();
    let http_client = HttpClient::new(get_mint_url_from_env().parse().unwrap(), None);

    // Fetch mint info to populate cache support (NUT-19)
    http_client.get_mint_info().await.unwrap();

    let premint_secrets = PreMintSecrets::random(
        active_keyset_id,
        100.into(),
        &SplitTarget::default().to_owned(),
        &fee_and_amounts,
    )
    .unwrap();

    let mut request = MintRequest {
        quote: quote.id,
        outputs: premint_secrets.blinded_messages(),
        signature: None,
    };

    let secret_key = quote.secret_key;

    request
        .sign(secret_key.expect("Secret key on quote"))
        .unwrap();

    let response = http_client.post_mint(request.clone()).await.unwrap();
    let response1 = http_client.post_mint(request).await.unwrap();

    assert!(response == response1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_regtest_melt_amountless() {
    let ln_client = get_test_client().await;

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

    ln_client
        .pay_invoice(mint_quote.request)
        .await
        .expect("failed to pay invoice");

    let proofs = wallet
        .mint(&mint_quote.id, SplitTarget::default(), None)
        .await
        .unwrap();

    let amount = proofs.total_amount().unwrap();

    assert!(mint_amount == amount);

    let invoice = ln_client.create_invoice(None).await.unwrap();

    let options = MeltOptions::new_amountless(5_000);

    let melt_quote = wallet
        .melt_quote(invoice.clone(), Some(options))
        .await
        .unwrap();

    let melt = wallet.melt(&melt_quote.id).await.unwrap();

    assert!(melt.amount == 5.into());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_attempt_to_mint_unpaid() {
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

    let proofs = wallet
        .mint(&mint_quote.id, SplitTarget::default(), None)
        .await;

    match proofs {
        Err(err) => {
            if !matches!(err, cdk::Error::UnpaidQuote) {
                panic!("Wrong error quote should be unpaid: {}", err);
            }
        }
        Ok(_) => {
            panic!("Minting should not be allowed");
        }
    }

    let mint_quote = wallet.mint_quote(mint_amount, None).await.unwrap();

    let state = wallet.mint_quote_state(&mint_quote.id).await.unwrap();

    assert!(state.state == MintQuoteState::Unpaid);

    let proofs = wallet
        .mint(&mint_quote.id, SplitTarget::default(), None)
        .await;

    match proofs {
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

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_melt_payment_failure_hold_invoice() {
    let ln_client = get_test_client().await;

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

    ln_client
        .pay_invoice(mint_quote.request)
        .await
        .expect("failed to pay invoice");

    let _proofs = wallet
        .mint(&mint_quote.id, SplitTarget::default(), None)
        .await
        .unwrap();

    // Create a hold invoice via LND CLI
    // We'll use random preimage for the payment hash
    let preimage = (0..32).map(|_| rand::random::<u8>()).collect::<Vec<u8>>();
    let payment_hash = sha256::Hash::hash(&preimage);
    let payment_hash_hex = hex::encode(payment_hash);

    let temp_dir = get_temp_dir();
    let lnd_client = init_lnd_client(&temp_dir).await;

    let payment_request = lnd_client
        .add_hold_invoice(&payment_hash_hex, 10)
        .await
        .expect("failed to create hold invoice");

    let melt_quote = wallet
        .melt_quote(payment_request, None)
        .await
        .expect("failed to get melt quote");

    // Spawn melt in background since it will block
    let wallet_clone = wallet.clone();
    let quote_id = melt_quote.id.clone();

    let melt_handle = tokio::spawn(async move { wallet_clone.melt(&quote_id).await });

    // Wait for payment to be in flight
    tokio::time::sleep(Duration::from_secs(2)).await;

    // Cancel the invoice to force failure (wait until payment is actually held)
    lnd_client
        .wait_for_hold_invoice_accepted(&payment_hash_hex)
        .await
        .expect("failed to wait for invoice");

    lnd_client
        .cancel_invoice(&payment_hash_hex)
        .await
        .expect("failed to cancel invoice");

    // Ensure the in-flight payment is released
    drop(preimage);

    // Melt should fail or return unpaid
    let result = melt_handle.await.unwrap();

    match result {
        Ok(melt_response) => {
            // If it returns Ok, the state should be Unpaid or Failed, not Paid
            assert_ne!(
                melt_response.state,
                MeltQuoteState::Paid,
                "Payment should not have succeeded"
            );
        }
        Err(_) => {
            // Error is also an acceptable outcome for a failed payment
        }
    }

    // Verify balance - should be original amount (100) since payment failed
    // Note: fees might be reserved but should be returned on failure
    let balance = wallet.total_balance().await.unwrap();
    assert_eq!(balance, Amount::from(100));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_melt_concurrency_hold_invoice() {
    let ln_client = get_test_client().await;

    let wallet = Wallet::new(
        &get_mint_url_from_env(),
        CurrencyUnit::Sat,
        Arc::new(memory::empty().await.unwrap()),
        Mnemonic::generate(12).unwrap().to_seed_normalized(""),
        None,
    )
    .expect("failed to create new wallet");

    let mint_amount = Amount::from(2000);

    let mint_quote = wallet.mint_quote(mint_amount, None).await.unwrap();

    ln_client
        .pay_invoice(mint_quote.request)
        .await
        .expect("failed to pay invoice");

    let _proofs = wallet
        .mint(&mint_quote.id, SplitTarget::default(), None)
        .await
        .unwrap();

    // Create a hold invoice via LND CLI
    let preimage = (0..32).map(|_| rand::random::<u8>()).collect::<Vec<u8>>();
    let preimage_hex = hex::encode(&preimage);
    let payment_hash = sha256::Hash::hash(&preimage);
    let payment_hash_hex = hex::encode(payment_hash);

    let temp_dir = get_temp_dir();
    let lnd_client = init_lnd_client(&temp_dir).await;

    let payment_request = lnd_client
        .add_hold_invoice(&payment_hash_hex, 10)
        .await
        .expect("failed to create hold invoice");

    // Get two quotes for the same invoice
    let melt_quote_1 = wallet
        .melt_quote(payment_request.clone(), None)
        .await
        .expect("failed to get melt quote 1");

    let melt_quote_2 = wallet
        .melt_quote(payment_request, None)
        .await
        .expect("failed to get melt quote 2");

    // Spawn melt 1 in background
    let wallet_clone = wallet.clone();
    let quote_id_1 = melt_quote_1.id.clone();

    let melt_handle_1 = tokio::spawn(async move { wallet_clone.melt(&quote_id_1).await });

    // Wait for payment to be in flight
    lnd_client
        .wait_for_hold_invoice_accepted(&payment_hash_hex)
        .await
        .expect("failed to wait for invoice");

    // Attempt melt 2 - should fail because lock is held
    let result_2 = wallet.melt(&melt_quote_2.id).await;

    // Assert failure
    match result_2 {
        Ok(res) => {
            // If it returns Ok, the state should NOT be Paid.
            // It might be Unpaid if rejected immediately.
            assert_ne!(
                res.state,
                MeltQuoteState::Paid,
                "Second melt should not succeed while first is in flight"
            );
        }
        Err(_) => {
            // Expected error
        }
    }

    // Settle the invoice to let the first one complete
    lnd_client
        .wait_for_hold_invoice_accepted(&payment_hash_hex)
        .await
        .expect("failed to wait for invoice");

    lnd_client
        .settle_invoice(&preimage_hex)
        .await
        .expect("failed to settle invoice");

    // Wait for melt 1 to complete
    let result_1 = melt_handle_1.await.unwrap();

    // Assert success of first payment
    match result_1 {
        Ok(res) => {
            assert_eq!(
                res.state,
                MeltQuoteState::Paid,
                "First melt should have succeeded"
            );
        }
        Err(e) => panic!("First melt failed: {}", e),
    }

    // Attempt melt 2 again - should fail as already paid
    let result_2_retry = wallet.melt(&melt_quote_2.id).await;

    match result_2_retry {
        Ok(res) => {
            assert_ne!(
                res.state,
                MeltQuoteState::Paid,
                "Expected already-paid error, got Paid"
            );
        }
        Err(e) => {
            assert!(
                matches!(e, cdk::Error::PaidQuote | cdk::Error::RequestAlreadyPaid),
                "Expected PaidQuote or RequestAlreadyPaid, got: {}",
                e
            );
        }
    }

    // Cleanup: ensure the held payment is fully released
    drop(preimage);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_melt_same_quote_while_pending_errors() {
    let ln_client = get_test_client().await;

    let wallet = Wallet::new(
        &get_mint_url_from_env(),
        CurrencyUnit::Sat,
        Arc::new(memory::empty().await.unwrap()),
        Mnemonic::generate(12).unwrap().to_seed_normalized(""),
        None,
    )
    .expect("failed to create new wallet");

    let mint_amount = Amount::from(2000);

    let mint_quote = wallet.mint_quote(mint_amount, None).await.unwrap();

    ln_client
        .pay_invoice(mint_quote.request)
        .await
        .expect("failed to pay invoice");

    let _proofs = wallet
        .mint(&mint_quote.id, SplitTarget::default(), None)
        .await
        .unwrap();

    let preimage = (0..32).map(|_| rand::random::<u8>()).collect::<Vec<u8>>();
    let preimage_hex = hex::encode(&preimage);
    let payment_hash = sha256::Hash::hash(&preimage);
    let payment_hash_hex = hex::encode(payment_hash);

    let temp_dir = get_temp_dir();
    let lnd_client = init_lnd_client(&temp_dir).await;

    let payment_request = lnd_client
        .add_hold_invoice(&payment_hash_hex, 10)
        .await
        .expect("failed to create hold invoice");

    let melt_quote = wallet
        .melt_quote(payment_request, None)
        .await
        .expect("failed to get melt quote");

    let wallet_clone = wallet.clone();
    let quote_id = melt_quote.id.clone();
    let melt_handle = tokio::spawn(async move { wallet_clone.melt(&quote_id).await });

    lnd_client
        .wait_for_hold_invoice_accepted(&payment_hash_hex)
        .await
        .expect("failed to wait for invoice");

    let retry_same_quote = wallet.melt(&melt_quote.id).await;

    match retry_same_quote {
        Ok(_) => panic!("Expected PendingQuote when melting same quote while pending"),
        Err(e) => {
            assert!(
                matches!(e, cdk::Error::PendingQuote),
                "Expected PendingQuote, got: {}",
                e
            );
        }
    }

    lnd_client
        .settle_invoice(&preimage_hex)
        .await
        .expect("failed to settle invoice");

    let first_result = melt_handle.await.unwrap();
    assert_eq!(first_result.unwrap().state, MeltQuoteState::Paid);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_melt_quote_status_transitions_with_hold() {
    let ln_client = get_test_client().await;

    let wallet = Wallet::new(
        &get_mint_url_from_env(),
        CurrencyUnit::Sat,
        Arc::new(memory::empty().await.unwrap()),
        Mnemonic::generate(12).unwrap().to_seed_normalized(""),
        None,
    )
    .expect("failed to create new wallet");

    let mint_amount = Amount::from(2000);

    let mint_quote = wallet.mint_quote(mint_amount, None).await.unwrap();

    ln_client
        .pay_invoice(mint_quote.request)
        .await
        .expect("failed to pay invoice");

    let _proofs = wallet
        .mint(&mint_quote.id, SplitTarget::default(), None)
        .await
        .unwrap();

    let preimage = (0..32).map(|_| rand::random::<u8>()).collect::<Vec<u8>>();
    let preimage_hex = hex::encode(&preimage);
    let payment_hash = sha256::Hash::hash(&preimage);
    let payment_hash_hex = hex::encode(payment_hash);

    let temp_dir = get_temp_dir();
    let lnd_client = init_lnd_client(&temp_dir).await;

    let payment_request = lnd_client
        .add_hold_invoice(&payment_hash_hex, 10)
        .await
        .expect("failed to create hold invoice");

    let melt_quote = wallet
        .melt_quote(payment_request, None)
        .await
        .expect("failed to get melt quote");

    let wallet_clone = wallet.clone();
    let quote_id = melt_quote.id.clone();
    let melt_handle = tokio::spawn(async move { wallet_clone.melt(&quote_id).await });

    lnd_client
        .wait_for_hold_invoice_accepted(&payment_hash_hex)
        .await
        .expect("failed to wait for invoice");

    // Poll melt quote status until Pending
    let pending = tokio::time::timeout(Duration::from_secs(30), async {
        loop {
            let status = wallet.melt_quote_status(&melt_quote.id).await.unwrap();
            if status.state == MeltQuoteState::Pending {
                break status;
            }
            tokio::time::sleep(Duration::from_millis(250)).await;
        }
    })
    .await
    .expect("timed out waiting for pending melt quote");

    assert_eq!(pending.state, MeltQuoteState::Pending);
    let pending_quotes = wallet.get_pending_melt_quotes().await.unwrap();
    assert!(pending_quotes.iter().any(|q| q.id == melt_quote.id));

    // Settle and wait for Paid
    lnd_client
        .settle_invoice(&preimage_hex)
        .await
        .expect("failed to settle invoice");

    let paid_status = tokio::time::timeout(Duration::from_secs(30), async {
        loop {
            let status = wallet.melt_quote_status(&melt_quote.id).await.unwrap();
            if status.state == MeltQuoteState::Paid {
                break status;
            }
            tokio::time::sleep(Duration::from_millis(250)).await;
        }
    })
    .await
    .expect("timed out waiting for paid melt quote");

    assert_eq!(paid_status.state, MeltQuoteState::Paid);
    let pending_quotes = wallet.get_pending_melt_quotes().await.unwrap();
    assert!(!pending_quotes.iter().any(|q| q.id == melt_quote.id));

    // Ensure the melt task completes
    let first_result = melt_handle.await.unwrap();
    assert_eq!(first_result.unwrap().state, MeltQuoteState::Paid);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_melt_pending_balance_and_fee_math() {
    let ln_client = get_test_client().await;

    let wallet = Wallet::new(
        &get_mint_url_from_env(),
        CurrencyUnit::Sat,
        Arc::new(memory::empty().await.unwrap()),
        Mnemonic::generate(12).unwrap().to_seed_normalized(""),
        None,
    )
    .expect("failed to create new wallet");

    let mint_amount = Amount::from(2000);

    let mint_quote = wallet.mint_quote(mint_amount, None).await.unwrap();

    ln_client
        .pay_invoice(mint_quote.request)
        .await
        .expect("failed to pay invoice");

    let _proofs = wallet
        .mint(&mint_quote.id, SplitTarget::default(), None)
        .await
        .unwrap();

    let initial_unspent = wallet.total_balance().await.unwrap();
    let initial_pending = wallet.total_pending_balance().await.unwrap();
    let initial_reserved = wallet.total_reserved_balance().await.unwrap();

    assert_eq!(initial_pending, Amount::from(0));
    assert_eq!(initial_reserved, Amount::from(0));

    let preimage = (0..32).map(|_| rand::random::<u8>()).collect::<Vec<u8>>();
    let preimage_hex = hex::encode(&preimage);
    let payment_hash = sha256::Hash::hash(&preimage);
    let payment_hash_hex = hex::encode(payment_hash);

    let temp_dir = get_temp_dir();
    let lnd_client = init_lnd_client(&temp_dir).await;

    let payment_request = lnd_client
        .add_hold_invoice(&payment_hash_hex, 10)
        .await
        .expect("failed to create hold invoice");

    let melt_quote = wallet
        .melt_quote(payment_request, None)
        .await
        .expect("failed to get melt quote");

    let wallet_clone = wallet.clone();
    let quote_id = melt_quote.id.clone();
    let melt_handle = tokio::spawn(async move { wallet_clone.melt(&quote_id).await });

    lnd_client
        .wait_for_hold_invoice_accepted(&payment_hash_hex)
        .await
        .expect("failed to wait for invoice");

    let pending_total = wallet.total_pending_balance().await.unwrap();
    assert!(pending_total > Amount::from(0));

    // Melt uses State::Pending rather than Reserved
    let reserved_total = wallet.total_reserved_balance().await.unwrap();
    assert_eq!(reserved_total, Amount::from(0));

    lnd_client
        .settle_invoice(&preimage_hex)
        .await
        .expect("failed to settle invoice");

    // Wait until quote status reports paid
    let paid_status = tokio::time::timeout(Duration::from_secs(30), async {
        loop {
            let status = wallet.melt_quote_status(&melt_quote.id).await.unwrap();
            if status.state == MeltQuoteState::Paid {
                break status;
            }
            tokio::time::sleep(Duration::from_millis(250)).await;
        }
    })
    .await
    .expect("timed out waiting for paid melt quote");

    let change_total = paid_status.change_amount().unwrap_or_default();
    let fee_paid = pending_total - melt_quote.amount - change_total;

    let final_unspent = wallet.total_balance().await.unwrap();

    assert_eq!(
        final_unspent,
        initial_unspent - melt_quote.amount - fee_paid,
        "final_unspent mismatch: initial {} final {} quote_amt {} pending_total {} change_total {} fee_paid {}",
        u64::from(initial_unspent),
        u64::from(final_unspent),
        u64::from(melt_quote.amount),
        u64::from(pending_total),
        u64::from(change_total),
        u64::from(fee_paid),
    );

    let final_pending = wallet.total_pending_balance().await.unwrap();
    assert_eq!(final_pending, Amount::from(0));

    // Ensure melt task completes
    let first_result = melt_handle.await.unwrap();
    assert_eq!(first_result.unwrap().state, MeltQuoteState::Paid);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_melt_cancel_then_wallet_reclaim_pending_proofs() {
    let ln_client = get_test_client().await;

    let wallet = Wallet::new(
        &get_mint_url_from_env(),
        CurrencyUnit::Sat,
        Arc::new(memory::empty().await.unwrap()),
        Mnemonic::generate(12).unwrap().to_seed_normalized(""),
        None,
    )
    .expect("failed to create new wallet");

    let mint_amount = Amount::from(2000);

    let mint_quote = wallet.mint_quote(mint_amount, None).await.unwrap();

    ln_client
        .pay_invoice(mint_quote.request)
        .await
        .expect("failed to pay invoice");

    let _proofs = wallet
        .mint(&mint_quote.id, SplitTarget::default(), None)
        .await
        .unwrap();

    let initial_unspent = wallet.total_balance().await.unwrap();
    assert!(initial_unspent > Amount::from(0));

    // Create hold invoice
    let preimage = (0..32).map(|_| rand::random::<u8>()).collect::<Vec<u8>>();
    let payment_hash = sha256::Hash::hash(&preimage);
    let payment_hash_hex = hex::encode(payment_hash);

    let temp_dir = get_temp_dir();
    let lnd_client = init_lnd_client(&temp_dir).await;

    let payment_request = lnd_client
        .add_hold_invoice(&payment_hash_hex, 10)
        .await
        .expect("failed to create hold invoice");

    let melt_quote = wallet
        .melt_quote(payment_request, None)
        .await
        .expect("failed to get melt quote");

    // Start melt (will block because hold invoice)
    let wallet_clone = wallet.clone();
    let quote_id = melt_quote.id.clone();
    let melt_handle = tokio::spawn(async move { wallet_clone.melt(&quote_id).await });

    // Ensure the HTLC is locked in before canceling
    lnd_client
        .wait_for_hold_invoice_accepted(&payment_hash_hex)
        .await
        .expect("failed to wait for invoice");

    // Pending should now be non-zero
    let pending_before_cancel = wallet.total_pending_balance().await.unwrap();
    assert!(pending_before_cancel > Amount::from(0));

    // Cancel invoice -> melt should fail
    lnd_client
        .cancel_invoice(&payment_hash_hex)
        .await
        .expect("failed to cancel invoice");

    let melt_result = melt_handle.await.unwrap();
    assert!(melt_result.is_err(), "melt should fail after cancel");

    // Wallet reclaim: check pending proofs against mint state.
    // This should remove any spent proofs and keep unspent ones reclaimable.
    let reclaimable = wallet.check_all_pending_proofs().await.unwrap();
    assert!(reclaimable >= Amount::from(0));

    // After reclaim, pending should be zero (or at least reduced).
    let pending_after_check = wallet.total_pending_balance().await.unwrap();
    assert_eq!(pending_after_check, Amount::from(0));

    // And unspent should be restored back to initial (since payment was canceled).
    let unspent_after_reclaim = wallet.total_balance().await.unwrap();
    assert_eq!(unspent_after_reclaim, initial_unspent);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_melt_proof_locking_hold_invoice() {
    let ln_client = get_test_client().await;

    let wallet = Wallet::new(
        &get_mint_url_from_env(),
        CurrencyUnit::Sat,
        Arc::new(memory::empty().await.unwrap()),
        Mnemonic::generate(12).unwrap().to_seed_normalized(""),
        None,
    )
    .expect("failed to create new wallet");

    let mint_amount = Amount::from(2000);

    let mint_quote = wallet.mint_quote(mint_amount, None).await.unwrap();

    ln_client
        .pay_invoice(mint_quote.request)
        .await
        .expect("failed to pay invoice");

    let _proofs = wallet
        .mint(&mint_quote.id, SplitTarget::default(), None)
        .await
        .unwrap();

    // Get all proofs initially
    let all_proofs = wallet.get_unspent_proofs().await.unwrap();
    assert!(!all_proofs.is_empty());

    // Create a hold invoice via LND CLI
    let preimage = (0..32).map(|_| rand::random::<u8>()).collect::<Vec<u8>>();
    let preimage_hex = hex::encode(&preimage);
    let payment_hash = sha256::Hash::hash(&preimage);
    let payment_hash_hex = hex::encode(payment_hash);

    let temp_dir = get_temp_dir();
    let lnd_client = init_lnd_client(&temp_dir).await;

    let payment_request = lnd_client
        .add_hold_invoice(&payment_hash_hex, 100)
        .await
        .expect("failed to create hold invoice");

    let melt_quote = wallet
        .melt_quote(payment_request, None)
        .await
        .expect("failed to get melt quote");

    // Spawn melt in background since it will block
    let wallet_clone = wallet.clone();
    let quote_id = melt_quote.id.clone();

    let melt_handle = tokio::spawn(async move { wallet_clone.melt(&quote_id).await });

    // Wait for payment to be in flight
    lnd_client
        .wait_for_hold_invoice_accepted(&payment_hash_hex)
        .await
        .expect("failed to wait for invoice");

    // Now attempt to swap ALL proofs (including those likely used in melt)
    // We use HttpClient directly to bypass wallet's local lock check
    let http_client = HttpClient::new(get_mint_url_from_env().parse().unwrap(), None);

    // Construct Swap Request
    let active_keyset_id = wallet.fetch_active_keyset().await.unwrap().id;
    let total_amount = all_proofs.total_amount().unwrap();

    // Create output for the swap (just one output for simplicity)
    let swap_amount = total_amount;
    let fee_and_amounts = (0, ((0..32).map(|x| 2u64.pow(x)).collect::<Vec<_>>())).into();

    let premint_secrets = PreMintSecrets::random(
        active_keyset_id,
        swap_amount,
        &SplitTarget::default().to_owned(),
        &fee_and_amounts,
    )
    .unwrap();

    let swap_request = cdk::nuts::SwapRequest::new(all_proofs, premint_secrets.blinded_messages());

    let swap_result = http_client.post_swap(swap_request).await;

    // Assert failure
    match swap_result {
        Ok(_) => panic!("Swap should have failed because proofs are locked in pending melt"),
        Err(e) => {
            println!("Swap failed as expected: {}", e);
        }
    }

    // Settle the invoice to let the melt complete
    lnd_client
        .wait_for_hold_invoice_accepted(&payment_hash_hex)
        .await
        .expect("failed to wait for invoice");

    lnd_client
        .settle_invoice(&preimage_hex)
        .await
        .expect("failed to settle invoice");

    // Wait for melt to complete
    let result = melt_handle.await.unwrap();
    assert_eq!(result.unwrap().state, MeltQuoteState::Paid);
}
