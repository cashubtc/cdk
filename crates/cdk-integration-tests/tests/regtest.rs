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
use cashu::ProofsMethods;
use cdk::amount::{Amount, SplitTarget};
use cdk::nuts::{
    CurrencyUnit, MeltOptions, MeltQuoteState, MintQuoteState, MintRequest, Mpp,
    NotificationPayload, PreMintSecrets,
};
use cdk::wallet::{HttpClient, MintConnector, Wallet, WalletSubscription};
use cdk_integration_tests::{get_mint_url_from_env, get_second_mint_url_from_env, get_test_client};
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
        .await;

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
