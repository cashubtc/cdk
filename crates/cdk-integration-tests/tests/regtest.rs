//! Regtest Integration Tests
//!
//! This file contains tests that run against actual Lightning Network nodes in regtest mode.
//! These tests require a local development environment with LND nodes configured for regtest.
//!
//! Test Environment Setup:
//! - Uses actual LND nodes connected to a regtest Bitcoin network
//! - Tests real Lightning payment flows including invoice creation and payment
//! - Verifies mint behavior with actual Lightning Network interactions
//!
//! Running Tests:
//! - Requires CDK_TEST_REGTEST=1 environment variable to be set
//! - Requires properly configured LND nodes with TLS certificates and macaroons
//! - Uses real Bitcoin transactions in regtest mode

use std::sync::Arc;
use std::time::Duration;

use bip39::Mnemonic;
use cdk::amount::{Amount, SplitTarget};
use cdk::nuts::nut00::KnownMethod;
use cdk::nuts::{
    CurrencyUnit, MintQuoteState, MintRequest, NotificationPayload, PaymentMethod, PreMintSecrets,
};
use cdk::wallet::advanced::{
    HttpClient, MetadataSource, MintConnector, MintMetadataRequest, SubscriptionRequest,
};
use cdk::wallet::mint::{MintRequest as WalletMintRequest, MintSession, MintState};
use cdk::wallet::payment::{PaymentQuoteRequest, PaymentTarget};
use cdk_common::database::WalletDatabase;
use cdk_integration_tests::{
    attempt_manual_mint, get_mint_url_from_env, get_second_mint_url_from_env, get_test_client,
};
use cdk_sqlite::wallet::{self, memory};
use futures::join;
use tokio::time::timeout;

const LDK_URL: &str = "http://127.0.0.1:8089";

async fn wait_until_paid(session: &MintSession) {
    timeout(Duration::from_secs(60), async {
        loop {
            if session.refresh().await.expect("mint state").state == MintState::Paid {
                break;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    })
    .await
    .expect("mint payment timeout");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_internal_payment() {
    let ln_client = get_test_client().await;

    let wallet = cdk_integration_tests::open_test_wallet(
        &get_mint_url_from_env(),
        CurrencyUnit::Sat,
        Arc::new(memory::empty().await.unwrap()),
        Mnemonic::generate(12).unwrap().to_seed_normalized(""),
        None,
    )
    .expect("failed to create new wallet");

    let mint_session = wallet
        .request_mint(WalletMintRequest::bolt11(100.into()))
        .await
        .unwrap();

    ln_client
        .pay_invoice(mint_session.initial_state().payment_request.clone())
        .await
        .expect("failed to pay invoice");

    mint_session
        .wait(Duration::from_secs(60))
        .await
        .expect("payment");

    assert!(wallet.balance().await.unwrap().available == 100.into());

    let wallet_2 = cdk_integration_tests::open_test_wallet(
        &get_mint_url_from_env(),
        CurrencyUnit::Sat,
        Arc::new(memory::empty().await.unwrap()),
        Mnemonic::generate(12).unwrap().to_seed_normalized(""),
        None,
    )
    .expect("failed to create new wallet");

    let mint_session = wallet_2
        .request_mint(WalletMintRequest::bolt11(10.into()))
        .await
        .unwrap();

    let payment = wallet
        .quote_payment(PaymentQuoteRequest::new(PaymentTarget::bolt11(
            mint_session.initial_state().payment_request.clone(),
        )))
        .await
        .unwrap()
        .into_single()
        .unwrap();

    assert_eq!(payment.quote().amount, 10.into());

    payment.prepare().await.unwrap().execute().await.unwrap();

    mint_session
        .wait(Duration::from_secs(60))
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

    let wallet_2_balance = wallet_2.balance().await.unwrap().available;

    assert!(wallet_2_balance == 10.into());

    let wallet_1_balance = wallet.balance().await.unwrap().available;

    assert!(wallet_1_balance == 90.into());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_websocket_connection() {
    let wallet = cdk_integration_tests::open_test_wallet(
        &get_mint_url_from_env(),
        CurrencyUnit::Sat,
        Arc::new(wallet::memory::empty().await.unwrap()),
        Mnemonic::generate(12).unwrap().to_seed_normalized(""),
        None,
    )
    .expect("failed to create new wallet");

    // Create a small mint quote to test notifications
    let mint_session = wallet
        .request_mint(WalletMintRequest::bolt11(10.into()))
        .await
        .unwrap();

    // Subscribe to notifications for this quote
    let mut subscription = wallet
        .advanced()
        .subscribe(SubscriptionRequest::Bolt11MintQuotes(vec![mint_session
            .id()
            .to_string()]))
        .await
        .expect("failed to subscribe");

    // First check we get the unpaid state
    let msg = timeout(Duration::from_secs(10), subscription.recv())
        .await
        .expect("timeout waiting for unpaid notification")
        .expect("No paid notification received");

    match msg.into_inner() {
        NotificationPayload::MintQuoteBolt11Response(response) => {
            assert_eq!(response.quote.to_string(), mint_session.id().as_str());
            assert_eq!(response.state, MintQuoteState::Unpaid);
        }
        _ => panic!("Unexpected notification type"),
    }

    let ln_client = get_test_client().await;
    ln_client
        .pay_invoice(mint_session.initial_state().payment_request.clone())
        .await
        .expect("failed to pay invoice");

    // Wait for paid notification with 10 second timeout
    let msg = timeout(Duration::from_secs(10), subscription.recv())
        .await
        .expect("timeout waiting for paid notification")
        .expect("No paid notification received");

    match msg.into_inner() {
        NotificationPayload::MintQuoteBolt11Response(response) => {
            assert_eq!(response.quote.to_string(), mint_session.id().as_str());
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
    let wallet1 = cdk_integration_tests::open_test_wallet(
        &get_mint_url_from_env(),
        CurrencyUnit::Sat,
        db,
        Mnemonic::generate(12).unwrap().to_seed_normalized(""),
        None,
    )
    .expect("failed to create new wallet");

    let db = Arc::new(memory::empty().await.unwrap());
    let wallet2 = cdk_integration_tests::open_test_wallet(
        &get_second_mint_url_from_env(),
        CurrencyUnit::Sat,
        db,
        Mnemonic::generate(12).unwrap().to_seed_normalized(""),
        None,
    )
    .expect("failed to create new wallet");

    let mint_amount = Amount::from(100);

    // Fund the wallets
    let session = wallet1
        .request_mint(WalletMintRequest::bolt11(mint_amount))
        .await
        .unwrap();
    ln_client
        .pay_invoice(session.initial_state().payment_request.clone())
        .await
        .expect("failed to pay invoice");

    session
        .wait(Duration::from_secs(60))
        .await
        .expect("payment");

    let session = wallet2
        .request_mint(WalletMintRequest::bolt11(mint_amount))
        .await
        .unwrap();
    ln_client
        .pay_invoice(session.initial_state().payment_request.clone())
        .await
        .expect("failed to pay invoice");

    session
        .wait(Duration::from_secs(60))
        .await
        .expect("payment");

    // Get an invoice
    let invoice = ln_client.create_invoice(Some(50)).await.unwrap();

    // Get multi-part melt quotes
    let quote_1 = wallet1
        .quote_payment(PaymentQuoteRequest::new(PaymentTarget::bolt11_mpp(
            invoice.clone(),
            Amount::from(25000),
        )))
        .await
        .expect("Could not get melt quote")
        .into_single()
        .expect("single quote");
    let quote_2 = wallet2
        .quote_payment(PaymentQuoteRequest::new(PaymentTarget::bolt11_mpp(
            invoice,
            Amount::from(25000),
        )))
        .await
        .expect("Could not get melt quote")
        .into_single()
        .expect("single quote");

    // Multimint pay invoice - prepare both melts
    let prepared1 = quote_1.prepare().await.expect("Could not prepare melt 1");
    let prepared2 = quote_2.prepare().await.expect("Could not prepare melt 2");

    // Confirm both in parallel
    let result = join!(prepared1.execute(), prepared2.execute());

    // Unpack results
    let result1 = result.0.unwrap();
    let result2 = result.1.unwrap();

    // Check
    assert_eq!(result1.amount, result2.amount);
    assert_eq!(result1.amount, Amount::from(25));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_cached_mint() {
    let ln_client = get_test_client().await;
    let store: Arc<dyn WalletDatabase<cdk_common::database::Error> + Send + Sync> =
        Arc::new(memory::empty().await.unwrap());
    let wallet = cdk_integration_tests::open_test_wallet(
        &get_mint_url_from_env(),
        CurrencyUnit::Sat,
        store.clone(),
        Mnemonic::generate(12).unwrap().to_seed_normalized(""),
        None,
    )
    .expect("failed to create new wallet");

    let mint_amount = Amount::from(100);

    let session = wallet
        .request_mint(WalletMintRequest::bolt11(mint_amount))
        .await
        .unwrap();
    ln_client
        .pay_invoice(session.initial_state().payment_request.clone())
        .await
        .expect("failed to pay invoice");

    wait_until_paid(&session).await;

    let active_keyset_id = wallet
        .advanced()
        .mint_metadata(MintMetadataRequest {
            source: MetadataSource::Refresh,
        })
        .await
        .unwrap()
        .active_keyset()
        .expect("active keyset")
        .id;
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
        quote: session.id().to_string(),
        outputs: premint_secrets.blinded_messages(),
        signature: None,
    };

    let secret_key = store
        .get_mint_quote(session.id().as_str())
        .await
        .unwrap()
        .expect("mint quote")
        .secret_key
        .expect("Secret key on quote");

    request.sign(&secret_key).unwrap();

    let response = http_client
        .post_mint(&PaymentMethod::Known(KnownMethod::Bolt11), request.clone())
        .await
        .unwrap();
    let response1 = http_client
        .post_mint(&PaymentMethod::Known(KnownMethod::Bolt11), request)
        .await
        .unwrap();

    assert!(response == response1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_regtest_melt_amountless() {
    let ln_client = get_test_client().await;

    let wallet = cdk_integration_tests::open_test_wallet(
        &get_mint_url_from_env(),
        CurrencyUnit::Sat,
        Arc::new(memory::empty().await.unwrap()),
        Mnemonic::generate(12).unwrap().to_seed_normalized(""),
        None,
    )
    .expect("failed to create new wallet");

    let mint_amount = Amount::from(100);

    let mint_session = wallet
        .request_mint(WalletMintRequest::bolt11(mint_amount))
        .await
        .unwrap();

    assert_eq!(mint_session.initial_state().amount, Some(mint_amount));

    ln_client
        .pay_invoice(mint_session.initial_state().payment_request.clone())
        .await
        .expect("failed to pay invoice");

    let receipt = mint_session.wait(Duration::from_secs(60)).await.unwrap();

    let amount = receipt.amount;

    assert!(mint_amount == amount);

    let invoice = ln_client.create_invoice(None).await.unwrap();

    let payment = wallet
        .quote_payment(PaymentQuoteRequest::new(PaymentTarget::bolt11_amountless(
            invoice,
            Amount::from(5_000),
        )))
        .await
        .unwrap()
        .into_single()
        .unwrap();

    let prepared = payment.prepare().await.unwrap();
    let melt = prepared.execute().await.unwrap();

    assert!(melt.amount == 5.into());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_attempt_to_mint_unpaid() {
    let store: Arc<dyn WalletDatabase<cdk_common::database::Error> + Send + Sync> =
        Arc::new(memory::empty().await.unwrap());
    let wallet = cdk_integration_tests::open_test_wallet(
        &get_mint_url_from_env(),
        CurrencyUnit::Sat,
        store.clone(),
        Mnemonic::generate(12).unwrap().to_seed_normalized(""),
        None,
    )
    .expect("failed to create new wallet");

    let mint_amount = Amount::from(100);

    let mint_session = wallet
        .request_mint(WalletMintRequest::bolt11(mint_amount))
        .await
        .unwrap();

    assert_eq!(mint_session.initial_state().amount, Some(mint_amount));

    let response = attempt_manual_mint(
        &wallet,
        &get_mint_url_from_env(),
        &store,
        &mint_session,
        mint_amount,
        PaymentMethod::Known(KnownMethod::Bolt11),
    )
    .await;

    match response {
        Err(err) => {
            if !matches!(err, cdk::Error::UnpaidQuote) {
                panic!("Wrong error quote should be unpaid: {}", err);
            }
        }
        Ok(_) => {
            panic!("Minting should not be allowed");
        }
    }

    let mint_session = wallet
        .request_mint(WalletMintRequest::bolt11(mint_amount))
        .await
        .unwrap();

    let state = mint_session.refresh().await.unwrap();

    assert!(state.state == MintState::Unpaid);

    let response = attempt_manual_mint(
        &wallet,
        &get_mint_url_from_env(),
        &store,
        &mint_session,
        mint_amount,
        PaymentMethod::Known(KnownMethod::Bolt11),
    )
    .await;

    match response {
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
