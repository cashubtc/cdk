//! Fake Wallet Integration Tests
//!
//! This file contains tests for the fake wallet backend functionality.
//! The fake wallet simulates Lightning Network behavior for testing purposes,
//! allowing verification of mint behavior in various payment scenarios without
//! requiring a real Lightning node.
//!
//! Test Scenarios:
//! - Pending payment states and proof handling
//! - Payment failure cases and proof state management
//! - Change output verification in melt operations
//! - Witness signature validation
//! - Cross-unit transaction validation
//! - Overflow and balance validation
//! - Duplicate proof detection

use std::sync::Arc;

use bip39::Mnemonic;
use cashu::Amount;
use cdk::amount::SplitTarget;
use cdk::nuts::nut00::ProofsMethods;
use cdk::nuts::{
    CurrencyUnit, MeltQuoteState, MeltRequest, MintRequest, PreMintSecrets, Proofs, SecretKey,
    State, SwapRequest,
};
use cdk::wallet::types::TransactionDirection;
use cdk::wallet::{HttpClient, MintConnector, Wallet};
use cdk::StreamExt;
use cdk_fake_wallet::{create_fake_invoice, FakeInvoiceDescription};
use cdk_integration_tests::attempt_to_swap_pending;
use cdk_sqlite::wallet::memory;

const MINT_URL: &str = "http://127.0.0.1:8086";

/// Tests that when both pay and check return pending status, input proofs should remain pending
#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_fake_tokens_pending() {
    let wallet = Wallet::new(
        MINT_URL,
        CurrencyUnit::Sat,
        Arc::new(memory::empty().await.unwrap()),
        Mnemonic::generate(12).unwrap().to_seed_normalized(""),
        None,
    )
    .expect("failed to create new wallet");

    let mint_quote = wallet.mint_quote(100.into(), None).await.unwrap();

    let mut proof_streams = wallet.proof_stream(mint_quote.clone(), SplitTarget::default(), None);

    let _proofs = proof_streams
        .next()
        .await
        .expect("payment")
        .expect("no error");

    let fake_description = FakeInvoiceDescription {
        pay_invoice_state: MeltQuoteState::Pending,
        check_payment_state: MeltQuoteState::Pending,
        pay_err: false,
        check_err: false,
    };

    let invoice = create_fake_invoice(1000, serde_json::to_string(&fake_description).unwrap());

    let melt_quote = wallet.melt_quote(invoice.to_string(), None).await.unwrap();

    let melt = wallet.melt(&melt_quote.id).await;

    assert!(melt.is_err());

    attempt_to_swap_pending(&wallet).await.unwrap();
}

/// Tests that if the pay error fails and the check returns unknown or failed,
/// the input proofs should be unset as spending (returned to unspent state)
#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_fake_melt_payment_fail() {
    let wallet = Wallet::new(
        MINT_URL,
        CurrencyUnit::Sat,
        Arc::new(memory::empty().await.unwrap()),
        Mnemonic::generate(12).unwrap().to_seed_normalized(""),
        None,
    )
    .expect("Failed to create new wallet");

    let mint_quote = wallet.mint_quote(100.into(), None).await.unwrap();

    let mut proof_streams = wallet.proof_stream(mint_quote.clone(), SplitTarget::default(), None);

    let _proofs = proof_streams
        .next()
        .await
        .expect("payment")
        .expect("no error");

    let fake_description = FakeInvoiceDescription {
        pay_invoice_state: MeltQuoteState::Unknown,
        check_payment_state: MeltQuoteState::Unknown,
        pay_err: true,
        check_err: false,
    };

    let invoice = create_fake_invoice(1000, serde_json::to_string(&fake_description).unwrap());

    let melt_quote = wallet.melt_quote(invoice.to_string(), None).await.unwrap();

    // The melt should error at the payment invoice command
    let melt = wallet.melt(&melt_quote.id).await;
    assert!(melt.is_err());

    let fake_description = FakeInvoiceDescription {
        pay_invoice_state: MeltQuoteState::Failed,
        check_payment_state: MeltQuoteState::Failed,
        pay_err: true,
        check_err: false,
    };

    let invoice = create_fake_invoice(1000, serde_json::to_string(&fake_description).unwrap());

    let melt_quote = wallet.melt_quote(invoice.to_string(), None).await.unwrap();

    // The melt should error at the payment invoice command
    let melt = wallet.melt(&melt_quote.id).await;
    assert!(melt.is_err());

    // The mint should have unset proofs from pending since payment failed
    let all_proof = wallet.get_unspent_proofs().await.unwrap();
    let states = wallet.check_proofs_spent(all_proof).await.unwrap();
    for state in states {
        assert!(state.state == State::Unspent);
    }

    let wallet_bal = wallet.total_balance().await.unwrap();
    assert_eq!(wallet_bal, 100.into());
}

/// Tests that when both the pay_invoice and check_invoice both fail,
/// the proofs should remain in pending state
#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_fake_melt_payment_fail_and_check() {
    let wallet = Wallet::new(
        MINT_URL,
        CurrencyUnit::Sat,
        Arc::new(memory::empty().await.unwrap()),
        Mnemonic::generate(12).unwrap().to_seed_normalized(""),
        None,
    )
    .expect("Failed to create new wallet");

    let mint_quote = wallet.mint_quote(100.into(), None).await.unwrap();

    let mut proof_streams = wallet.proof_stream(mint_quote.clone(), SplitTarget::default(), None);

    let _proofs = proof_streams
        .next()
        .await
        .expect("payment")
        .expect("no error");

    let fake_description = FakeInvoiceDescription {
        pay_invoice_state: MeltQuoteState::Unknown,
        check_payment_state: MeltQuoteState::Unknown,
        pay_err: true,
        check_err: true,
    };

    let invoice = create_fake_invoice(7000, serde_json::to_string(&fake_description).unwrap());

    let melt_quote = wallet.melt_quote(invoice.to_string(), None).await.unwrap();

    // The melt should error at the payment invoice command
    let melt = wallet.melt(&melt_quote.id).await;
    assert!(melt.is_err());

    let pending = wallet
        .localstore
        .get_proofs(None, None, Some(vec![State::Pending]), None)
        .await
        .unwrap();

    assert!(!pending.is_empty());
}

/// Tests that when the ln backend returns a failed status but does not error,
/// the mint should do a second check, then remove proofs from pending state
#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_fake_melt_payment_return_fail_status() {
    let wallet = Wallet::new(
        MINT_URL,
        CurrencyUnit::Sat,
        Arc::new(memory::empty().await.unwrap()),
        Mnemonic::generate(12).unwrap().to_seed_normalized(""),
        None,
    )
    .expect("Failed to create new wallet");

    let mint_quote = wallet.mint_quote(100.into(), None).await.unwrap();

    let mut proof_streams = wallet.proof_stream(mint_quote.clone(), SplitTarget::default(), None);

    let _proofs = proof_streams
        .next()
        .await
        .expect("payment")
        .expect("no error");

    let fake_description = FakeInvoiceDescription {
        pay_invoice_state: MeltQuoteState::Failed,
        check_payment_state: MeltQuoteState::Failed,
        pay_err: false,
        check_err: false,
    };

    let invoice = create_fake_invoice(7000, serde_json::to_string(&fake_description).unwrap());

    let melt_quote = wallet.melt_quote(invoice.to_string(), None).await.unwrap();

    // The melt should error at the payment invoice command
    let melt = wallet.melt(&melt_quote.id).await;
    assert!(melt.is_err());

    let fake_description = FakeInvoiceDescription {
        pay_invoice_state: MeltQuoteState::Unknown,
        check_payment_state: MeltQuoteState::Unknown,
        pay_err: false,
        check_err: false,
    };

    let invoice = create_fake_invoice(7000, serde_json::to_string(&fake_description).unwrap());

    let melt_quote = wallet.melt_quote(invoice.to_string(), None).await.unwrap();

    // The melt should error at the payment invoice command
    let melt = wallet.melt(&melt_quote.id).await;
    assert!(melt.is_err());

    let pending = wallet
        .localstore
        .get_proofs(None, None, Some(vec![State::Pending]), None)
        .await
        .unwrap();

    assert!(pending.is_empty());
}

/// Tests that when the ln backend returns an error with unknown status,
/// the mint should do a second check, then remove proofs from pending state
#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_fake_melt_payment_error_unknown() {
    let wallet = Wallet::new(
        MINT_URL,
        CurrencyUnit::Sat,
        Arc::new(memory::empty().await.unwrap()),
        Mnemonic::generate(12).unwrap().to_seed_normalized(""),
        None,
    )
    .unwrap();

    let mint_quote = wallet.mint_quote(100.into(), None).await.unwrap();

    let mut proof_streams = wallet.proof_stream(mint_quote.clone(), SplitTarget::default(), None);

    let _proofs = proof_streams
        .next()
        .await
        .expect("payment")
        .expect("no error");

    let fake_description = FakeInvoiceDescription {
        pay_invoice_state: MeltQuoteState::Failed,
        check_payment_state: MeltQuoteState::Unknown,
        pay_err: true,
        check_err: false,
    };

    let invoice = create_fake_invoice(7000, serde_json::to_string(&fake_description).unwrap());

    let melt_quote = wallet.melt_quote(invoice.to_string(), None).await.unwrap();

    // The melt should error at the payment invoice command
    let melt = wallet.melt(&melt_quote.id).await;
    assert_eq!(melt.unwrap_err().to_string(), "Payment failed");

    let fake_description = FakeInvoiceDescription {
        pay_invoice_state: MeltQuoteState::Unknown,
        check_payment_state: MeltQuoteState::Unknown,
        pay_err: true,
        check_err: false,
    };

    let invoice = create_fake_invoice(7000, serde_json::to_string(&fake_description).unwrap());

    let melt_quote = wallet.melt_quote(invoice.to_string(), None).await.unwrap();

    // The melt should error at the payment invoice command
    let melt = wallet.melt(&melt_quote.id).await;
    assert_eq!(melt.unwrap_err().to_string(), "Payment failed");

    let pending = wallet
        .localstore
        .get_proofs(None, None, Some(vec![State::Pending]), None)
        .await
        .unwrap();

    assert!(pending.is_empty());
}

/// Tests that when the ln backend returns an error but the second check returns paid,
/// proofs should remain in pending state
#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_fake_melt_payment_err_paid() {
    let wallet = Wallet::new(
        MINT_URL,
        CurrencyUnit::Sat,
        Arc::new(memory::empty().await.unwrap()),
        Mnemonic::generate(12).unwrap().to_seed_normalized(""),
        None,
    )
    .expect("Failed to create new wallet");

    let mint_quote = wallet.mint_quote(100.into(), None).await.unwrap();

    let mut proof_streams = wallet.proof_stream(mint_quote.clone(), SplitTarget::default(), None);

    let _proofs = proof_streams
        .next()
        .await
        .expect("payment")
        .expect("no error");

    let fake_description = FakeInvoiceDescription {
        pay_invoice_state: MeltQuoteState::Failed,
        check_payment_state: MeltQuoteState::Paid,
        pay_err: true,
        check_err: false,
    };

    let invoice = create_fake_invoice(7000, serde_json::to_string(&fake_description).unwrap());

    let melt_quote = wallet.melt_quote(invoice.to_string(), None).await.unwrap();

    // The melt should error at the payment invoice command
    let melt = wallet.melt(&melt_quote.id).await;
    assert!(melt.is_err());

    attempt_to_swap_pending(&wallet).await.unwrap();
}

/// Tests that change outputs in a melt quote are correctly handled
#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_fake_melt_change_in_quote() {
    let wallet = Wallet::new(
        MINT_URL,
        CurrencyUnit::Sat,
        Arc::new(memory::empty().await.unwrap()),
        Mnemonic::generate(12).unwrap().to_seed_normalized(""),
        None,
    )
    .expect("Failed to create new wallet");

    let mint_quote = wallet.mint_quote(100.into(), None).await.unwrap();

    let mut proof_streams = wallet.proof_stream(mint_quote.clone(), SplitTarget::default(), None);

    let _proofs = proof_streams
        .next()
        .await
        .expect("payment")
        .expect("no error");

    let transaction = wallet
        .list_transactions(Some(TransactionDirection::Incoming))
        .await
        .unwrap()
        .pop()
        .expect("No transaction found");
    assert_eq!(wallet.mint_url, transaction.mint_url);
    assert_eq!(TransactionDirection::Incoming, transaction.direction);
    assert_eq!(Amount::from(100), transaction.amount);
    assert_eq!(Amount::from(0), transaction.fee);
    assert_eq!(CurrencyUnit::Sat, transaction.unit);

    let fake_description = FakeInvoiceDescription::default();

    let invoice = create_fake_invoice(9000, serde_json::to_string(&fake_description).unwrap());

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

    let client = HttpClient::new(MINT_URL.parse().unwrap(), None);

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

/// Tests minting tokens with a valid witness signature
#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_fake_mint_with_witness() {
    let wallet = Wallet::new(
        MINT_URL,
        CurrencyUnit::Sat,
        Arc::new(memory::empty().await.unwrap()),
        Mnemonic::generate(12).unwrap().to_seed_normalized(""),
        None,
    )
    .expect("failed to create new wallet");
    let mint_quote = wallet.mint_quote(100.into(), None).await.unwrap();

    let mut proof_streams = wallet.proof_stream(mint_quote.clone(), SplitTarget::default(), None);

    let proofs = proof_streams
        .next()
        .await
        .expect("payment")
        .expect("no error");

    let mint_amount = proofs.total_amount().unwrap();

    assert!(mint_amount == 100.into());
}

/// Tests that minting without a witness signature fails with the correct error
#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_fake_mint_without_witness() {
    let wallet = Wallet::new(
        MINT_URL,
        CurrencyUnit::Sat,
        Arc::new(memory::empty().await.unwrap()),
        Mnemonic::generate(12).unwrap().to_seed_normalized(""),
        None,
    )
    .expect("failed to create new wallet");

    let mint_quote = wallet.mint_quote(100.into(), None).await.unwrap();

    let mut payment_streams = wallet.payment_stream(&mint_quote);

    payment_streams
        .next()
        .await
        .expect("payment")
        .expect("no error");

    let http_client = HttpClient::new(MINT_URL.parse().unwrap(), None);

    let active_keyset_id = wallet.fetch_active_keyset().await.unwrap().id;
    let fee_and_amounts = (0, ((0..32).map(|x| 2u64.pow(x)).collect::<Vec<_>>())).into();

    let premint_secrets = PreMintSecrets::random(
        active_keyset_id,
        100.into(),
        &SplitTarget::default(),
        &fee_and_amounts,
    )
    .unwrap();

    let request = MintRequest {
        quote: mint_quote.id,
        outputs: premint_secrets.blinded_messages(),
        signature: None,
    };

    let response = http_client.post_mint(request.clone()).await;

    match response {
        Err(cdk::error::Error::SignatureMissingOrInvalid) => {} //pass
        Err(err) => panic!("Wrong mint response for minting without witness: {}", err),
        Ok(_) => panic!("Minting should not have succeed without a witness"),
    }
}

/// Tests that minting with an incorrect witness signature fails with the correct error
#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_fake_mint_with_wrong_witness() {
    let wallet = Wallet::new(
        MINT_URL,
        CurrencyUnit::Sat,
        Arc::new(memory::empty().await.unwrap()),
        Mnemonic::generate(12).unwrap().to_seed_normalized(""),
        None,
    )
    .expect("failed to create new wallet");

    let mint_quote = wallet.mint_quote(100.into(), None).await.unwrap();

    let mut payment_streams = wallet.payment_stream(&mint_quote);

    payment_streams
        .next()
        .await
        .expect("payment")
        .expect("no error");

    let http_client = HttpClient::new(MINT_URL.parse().unwrap(), None);

    let active_keyset_id = wallet.fetch_active_keyset().await.unwrap().id;
    let fee_and_amounts = (0, ((0..32).map(|x| 2u64.pow(x)).collect::<Vec<_>>())).into();

    let premint_secrets = PreMintSecrets::random(
        active_keyset_id,
        100.into(),
        &SplitTarget::default(),
        &fee_and_amounts,
    )
    .unwrap();

    let mut request = MintRequest {
        quote: mint_quote.id,
        outputs: premint_secrets.blinded_messages(),
        signature: None,
    };

    let secret_key = SecretKey::generate();

    request
        .sign(secret_key)
        .expect("failed to sign the mint request");

    let response = http_client.post_mint(request.clone()).await;

    match response {
        Err(cdk::error::Error::SignatureMissingOrInvalid) => {} //pass
        Err(err) => panic!("Wrong mint response for minting without witness: {}", err),
        Ok(_) => panic!("Minting should not have succeed without a witness"),
    }
}

/// Tests that attempting to mint more tokens than allowed by the quote fails
#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_fake_mint_inflated() {
    let wallet = Wallet::new(
        MINT_URL,
        CurrencyUnit::Sat,
        Arc::new(memory::empty().await.unwrap()),
        Mnemonic::generate(12).unwrap().to_seed_normalized(""),
        None,
    )
    .expect("failed to create new wallet");

    let mint_quote = wallet.mint_quote(100.into(), None).await.unwrap();

    let mut payment_streams = wallet.payment_stream(&mint_quote);

    payment_streams
        .next()
        .await
        .expect("payment")
        .expect("no error");

    let active_keyset_id = wallet.fetch_active_keyset().await.unwrap().id;
    let fee_and_amounts = (0, ((0..32).map(|x| 2u64.pow(x)).collect::<Vec<_>>())).into();

    let pre_mint = PreMintSecrets::random(
        active_keyset_id,
        500.into(),
        &SplitTarget::None,
        &fee_and_amounts,
    )
    .unwrap();

    let quote_info = wallet
        .localstore
        .get_mint_quote(&mint_quote.id)
        .await
        .unwrap()
        .expect("there is a quote");

    let mut mint_request = MintRequest {
        quote: mint_quote.id,
        outputs: pre_mint.blinded_messages(),
        signature: None,
    };

    if let Some(secret_key) = quote_info.secret_key {
        mint_request
            .sign(secret_key)
            .expect("failed to sign the mint request");
    }
    let http_client = HttpClient::new(MINT_URL.parse().unwrap(), None);

    let response = http_client.post_mint(mint_request.clone()).await;

    match response {
        Err(err) => match err {
            cdk::Error::TransactionUnbalanced(_, _, _) => (),
            err => {
                panic!("Wrong mint error returned: {}", err);
            }
        },
        Ok(_) => {
            panic!("Should not have allowed second payment");
        }
    }
}

/// Tests that attempting to mint with multiple currency units in the same request fails
#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_fake_mint_multiple_units() {
    let wallet = Wallet::new(
        MINT_URL,
        CurrencyUnit::Sat,
        Arc::new(memory::empty().await.unwrap()),
        Mnemonic::generate(12).unwrap().to_seed_normalized(""),
        None,
    )
    .expect("failed to create new wallet");

    let mint_quote = wallet.mint_quote(100.into(), None).await.unwrap();

    let mut payment_streams = wallet.payment_stream(&mint_quote);

    payment_streams
        .next()
        .await
        .expect("payment")
        .expect("no error");

    let active_keyset_id = wallet.fetch_active_keyset().await.unwrap().id;
    let fee_and_amounts = (0, ((0..32).map(|x| 2u64.pow(x)).collect::<Vec<_>>())).into();

    let pre_mint = PreMintSecrets::random(
        active_keyset_id,
        50.into(),
        &SplitTarget::None,
        &fee_and_amounts,
    )
    .unwrap();

    let wallet_usd = Wallet::new(
        MINT_URL,
        CurrencyUnit::Usd,
        Arc::new(memory::empty().await.unwrap()),
        Mnemonic::generate(12).unwrap().to_seed_normalized(""),
        None,
    )
    .expect("failed to create new wallet");

    let active_keyset_id = wallet_usd.fetch_active_keyset().await.unwrap().id;

    let usd_pre_mint = PreMintSecrets::random(
        active_keyset_id,
        50.into(),
        &SplitTarget::None,
        &fee_and_amounts,
    )
    .unwrap();

    let quote_info = wallet
        .localstore
        .get_mint_quote(&mint_quote.id)
        .await
        .unwrap()
        .expect("there is a quote");

    let mut sat_outputs = pre_mint.blinded_messages();

    let mut usd_outputs = usd_pre_mint.blinded_messages();

    sat_outputs.append(&mut usd_outputs);

    let mut mint_request = MintRequest {
        quote: mint_quote.id,
        outputs: sat_outputs,
        signature: None,
    };

    if let Some(secret_key) = quote_info.secret_key {
        mint_request
            .sign(secret_key)
            .expect("failed to sign the mint request");
    }
    let http_client = HttpClient::new(MINT_URL.parse().unwrap(), None);

    let response = http_client.post_mint(mint_request.clone()).await;

    match response {
        Err(err) => match err {
            cdk::Error::MultipleUnits => (),
            err => {
                panic!("Wrong mint error returned: {}", err);
            }
        },
        Ok(_) => {
            panic!("Should not have allowed to mint with multiple units");
        }
    }
}

/// Tests that attempting to swap tokens with multiple currency units fails
#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_fake_mint_multiple_unit_swap() {
    let wallet = Wallet::new(
        MINT_URL,
        CurrencyUnit::Sat,
        Arc::new(memory::empty().await.unwrap()),
        Mnemonic::generate(12).unwrap().to_seed_normalized(""),
        None,
    )
    .expect("failed to create new wallet");

    wallet.refresh_keysets().await.unwrap();

    let mint_quote = wallet.mint_quote(100.into(), None).await.unwrap();

    let mut proof_streams = wallet.proof_stream(mint_quote.clone(), SplitTarget::default(), None);

    let proofs = proof_streams
        .next()
        .await
        .expect("payment")
        .expect("no error");

    let wallet_usd = Wallet::new(
        MINT_URL,
        CurrencyUnit::Usd,
        Arc::new(memory::empty().await.unwrap()),
        Mnemonic::generate(12).unwrap().to_seed_normalized(""),
        None,
    )
    .expect("failed to create usd wallet");
    wallet_usd.refresh_keysets().await.unwrap();

    let mint_quote = wallet_usd.mint_quote(100.into(), None).await.unwrap();

    let mut proof_streams =
        wallet_usd.proof_stream(mint_quote.clone(), SplitTarget::default(), None);

    let usd_proofs = proof_streams
        .next()
        .await
        .expect("payment")
        .expect("no error");

    let active_keyset_id = wallet.fetch_active_keyset().await.unwrap().id;
    let fee_and_amounts = (0, ((0..32).map(|x| 2u64.pow(x)).collect::<Vec<_>>())).into();

    {
        let inputs: Proofs = vec![
            proofs.first().expect("There is a proof").clone(),
            usd_proofs.first().expect("There is a proof").clone(),
        ];

        let pre_mint = PreMintSecrets::random(
            active_keyset_id,
            inputs.total_amount().unwrap(),
            &SplitTarget::None,
            &fee_and_amounts,
        )
        .unwrap();

        let swap_request = SwapRequest::new(inputs, pre_mint.blinded_messages());

        let http_client = HttpClient::new(MINT_URL.parse().unwrap(), None);
        let response = http_client.post_swap(swap_request.clone()).await;

        match response {
            Err(err) => match err {
                cdk::Error::MultipleUnits => (),
                err => {
                    panic!("Wrong mint error returned: {}", err);
                }
            },
            Ok(_) => {
                panic!("Should not have allowed to mint with multiple units");
            }
        }
    }

    {
        let usd_active_keyset_id = wallet_usd.fetch_active_keyset().await.unwrap().id;
        let inputs: Proofs = proofs.into_iter().take(2).collect();

        let total_inputs = inputs.total_amount().unwrap();
        let fee_and_amounts = (0, ((0..32).map(|x| 2u64.pow(x)).collect::<Vec<_>>())).into();

        let half = total_inputs / 2.into();
        let usd_pre_mint = PreMintSecrets::random(
            usd_active_keyset_id,
            half,
            &SplitTarget::None,
            &fee_and_amounts,
        )
        .unwrap();
        let pre_mint = PreMintSecrets::random(
            active_keyset_id,
            total_inputs - half,
            &SplitTarget::None,
            &fee_and_amounts,
        )
        .unwrap();

        let mut usd_outputs = usd_pre_mint.blinded_messages();
        let mut sat_outputs = pre_mint.blinded_messages();

        usd_outputs.append(&mut sat_outputs);

        let swap_request = SwapRequest::new(inputs, usd_outputs);

        let http_client = HttpClient::new(MINT_URL.parse().unwrap(), None);
        let response = http_client.post_swap(swap_request.clone()).await;

        match response {
            Err(err) => match err {
                cdk::Error::MultipleUnits => (),
                err => {
                    panic!("Wrong mint error returned: {}", err);
                }
            },
            Ok(_) => {
                panic!("Should not have allowed to mint with multiple units");
            }
        }
    }
}

/// Tests that attempting to melt tokens with multiple currency units fails
#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_fake_mint_multiple_unit_melt() {
    let wallet = Wallet::new(
        MINT_URL,
        CurrencyUnit::Sat,
        Arc::new(memory::empty().await.unwrap()),
        Mnemonic::generate(12).unwrap().to_seed_normalized(""),
        None,
    )
    .expect("failed to create new wallet");

    let mint_quote = wallet.mint_quote(100.into(), None).await.unwrap();

    let mut proof_streams = wallet.proof_stream(mint_quote.clone(), SplitTarget::default(), None);

    let proofs = proof_streams
        .next()
        .await
        .expect("payment")
        .expect("no error");

    println!("Minted sat");

    let wallet_usd = Wallet::new(
        MINT_URL,
        CurrencyUnit::Usd,
        Arc::new(memory::empty().await.unwrap()),
        Mnemonic::generate(12).unwrap().to_seed_normalized(""),
        None,
    )
    .expect("failed to create new wallet");

    let mint_quote = wallet_usd.mint_quote(100.into(), None).await.unwrap();
    println!("Minted quote usd");

    let mut proof_streams =
        wallet_usd.proof_stream(mint_quote.clone(), SplitTarget::default(), None);

    let usd_proofs = proof_streams
        .next()
        .await
        .expect("payment")
        .expect("no error");

    {
        let inputs: Proofs = vec![
            proofs.first().expect("There is a proof").clone(),
            usd_proofs.first().expect("There is a proof").clone(),
        ];

        let input_amount: u64 = inputs.total_amount().unwrap().into();
        let invoice = create_fake_invoice((input_amount - 1) * 1000, "".to_string());
        let melt_quote = wallet.melt_quote(invoice.to_string(), None).await.unwrap();

        let melt_request = MeltRequest::new(melt_quote.id, inputs, None);

        let http_client = HttpClient::new(MINT_URL.parse().unwrap(), None);
        let response = http_client.post_melt(melt_request.clone()).await;

        match response {
            Err(err) => match err {
                cdk::Error::MultipleUnits => (),
                err => {
                    panic!("Wrong mint error returned: {}", err);
                }
            },
            Ok(_) => {
                panic!("Should not have allowed to melt with multiple units");
            }
        }
    }

    {
        let fee_and_amounts = (0, ((0..32).map(|x| 2u64.pow(x)).collect::<Vec<_>>())).into();
        let inputs: Proofs = vec![proofs.first().expect("There is a proof").clone()];

        let input_amount: u64 = inputs.total_amount().unwrap().into();

        let invoice = create_fake_invoice((input_amount - 1) * 1000, "".to_string());
        let active_keyset_id = wallet.fetch_active_keyset().await.unwrap().id;
        let usd_active_keyset_id = wallet_usd.fetch_active_keyset().await.unwrap().id;

        let usd_pre_mint = PreMintSecrets::random(
            usd_active_keyset_id,
            inputs.total_amount().unwrap() + 100.into(),
            &SplitTarget::None,
            &fee_and_amounts,
        )
        .unwrap();
        let pre_mint = PreMintSecrets::random(
            active_keyset_id,
            100.into(),
            &SplitTarget::None,
            &fee_and_amounts,
        )
        .unwrap();

        let mut usd_outputs = usd_pre_mint.blinded_messages();
        let mut sat_outputs = pre_mint.blinded_messages();

        usd_outputs.append(&mut sat_outputs);
        let quote = wallet.melt_quote(invoice.to_string(), None).await.unwrap();

        let melt_request = MeltRequest::new(quote.id, inputs, Some(usd_outputs));

        let http_client = HttpClient::new(MINT_URL.parse().unwrap(), None);

        let response = http_client.post_melt(melt_request.clone()).await;

        match response {
            Err(err) => match err {
                cdk::Error::MultipleUnits => (),
                err => {
                    panic!("Wrong mint error returned: {}", err);
                }
            },
            Ok(_) => {
                panic!("Should not have allowed to melt with multiple units");
            }
        }
    }
}

/// Tests that swapping tokens where input unit doesn't match output unit fails
#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_fake_mint_input_output_mismatch() {
    let wallet = Wallet::new(
        MINT_URL,
        CurrencyUnit::Sat,
        Arc::new(memory::empty().await.unwrap()),
        Mnemonic::generate(12).unwrap().to_seed_normalized(""),
        None,
    )
    .expect("failed to create new wallet");

    let mint_quote = wallet.mint_quote(100.into(), None).await.unwrap();

    let mut proof_streams = wallet.proof_stream(mint_quote.clone(), SplitTarget::default(), None);

    let proofs = proof_streams
        .next()
        .await
        .expect("payment")
        .expect("no error");

    let wallet_usd = Wallet::new(
        MINT_URL,
        CurrencyUnit::Usd,
        Arc::new(memory::empty().await.unwrap()),
        Mnemonic::generate(12).unwrap().to_seed_normalized(""),
        None,
    )
    .expect("failed to create new  usd wallet");
    let usd_active_keyset_id = wallet_usd.fetch_active_keyset().await.unwrap().id;
    let fee_and_amounts = (0, ((0..32).map(|x| 2u64.pow(x)).collect::<Vec<_>>())).into();

    let inputs = proofs;

    let pre_mint = PreMintSecrets::random(
        usd_active_keyset_id,
        inputs.total_amount().unwrap(),
        &SplitTarget::None,
        &fee_and_amounts,
    )
    .unwrap();

    let swap_request = SwapRequest::new(inputs, pre_mint.blinded_messages());

    let http_client = HttpClient::new(MINT_URL.parse().unwrap(), None);
    let response = http_client.post_swap(swap_request.clone()).await;

    match response {
        Err(err) => match err {
            cdk::Error::UnitMismatch => (),
            err => panic!("Wrong error returned: {}", err),
        },
        Ok(_) => {
            panic!("Should not have allowed to mint with multiple units");
        }
    }
}

/// Tests that swapping tokens where output amount is greater than input amount fails
#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_fake_mint_swap_inflated() {
    let wallet = Wallet::new(
        MINT_URL,
        CurrencyUnit::Sat,
        Arc::new(memory::empty().await.unwrap()),
        Mnemonic::generate(12).unwrap().to_seed_normalized(""),
        None,
    )
    .expect("failed to create new wallet");

    let mint_quote = wallet.mint_quote(100.into(), None).await.unwrap();

    let mut proof_streams = wallet.proof_stream(mint_quote.clone(), SplitTarget::default(), None);
    let fee_and_amounts = (0, ((0..32).map(|x| 2u64.pow(x)).collect::<Vec<_>>())).into();

    let proofs = proof_streams
        .next()
        .await
        .expect("payment")
        .expect("no error");

    let active_keyset_id = wallet.fetch_active_keyset().await.unwrap().id;
    let pre_mint = PreMintSecrets::random(
        active_keyset_id,
        101.into(),
        &SplitTarget::None,
        &fee_and_amounts,
    )
    .unwrap();

    let swap_request = SwapRequest::new(proofs, pre_mint.blinded_messages());

    let http_client = HttpClient::new(MINT_URL.parse().unwrap(), None);
    let response = http_client.post_swap(swap_request.clone()).await;

    match response {
        Err(err) => match err {
            cdk::Error::TransactionUnbalanced(_, _, _) => (),
            err => {
                panic!("Wrong mint error returned: {}", err);
            }
        },
        Ok(_) => {
            panic!("Should not have allowed to mint with multiple units");
        }
    }
}

/// Tests that tokens cannot be spent again after a failed swap attempt
#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_fake_mint_swap_spend_after_fail() {
    let wallet = Wallet::new(
        MINT_URL,
        CurrencyUnit::Sat,
        Arc::new(memory::empty().await.unwrap()),
        Mnemonic::generate(12).unwrap().to_seed_normalized(""),
        None,
    )
    .expect("failed to create new wallet");

    let mint_quote = wallet.mint_quote(100.into(), None).await.unwrap();

    let mut proof_streams = wallet.proof_stream(mint_quote.clone(), SplitTarget::default(), None);

    let proofs = proof_streams
        .next()
        .await
        .expect("payment")
        .expect("no error");

    let active_keyset_id = wallet.fetch_active_keyset().await.unwrap().id;
    let fee_and_amounts = (0, ((0..32).map(|x| 2u64.pow(x)).collect::<Vec<_>>())).into();

    let pre_mint = PreMintSecrets::random(
        active_keyset_id,
        100.into(),
        &SplitTarget::None,
        &fee_and_amounts,
    )
    .unwrap();

    let swap_request = SwapRequest::new(proofs.clone(), pre_mint.blinded_messages());

    let http_client = HttpClient::new(MINT_URL.parse().unwrap(), None);
    let response = http_client.post_swap(swap_request.clone()).await;

    assert!(response.is_ok());

    let pre_mint = PreMintSecrets::random(
        active_keyset_id,
        101.into(),
        &SplitTarget::None,
        &fee_and_amounts,
    )
    .unwrap();

    let swap_request = SwapRequest::new(proofs.clone(), pre_mint.blinded_messages());

    let http_client = HttpClient::new(MINT_URL.parse().unwrap(), None);
    let response = http_client.post_swap(swap_request.clone()).await;

    match response {
        Err(err) => match err {
            cdk::Error::TransactionUnbalanced(_, _, _) => (),
            err => panic!("Wrong mint error returned expected TransactionUnbalanced, got: {err}"),
        },
        Ok(_) => panic!("Should not have allowed swap with unbalanced"),
    }

    let pre_mint = PreMintSecrets::random(
        active_keyset_id,
        100.into(),
        &SplitTarget::None,
        &fee_and_amounts,
    )
    .unwrap();

    let swap_request = SwapRequest::new(proofs, pre_mint.blinded_messages());

    let http_client = HttpClient::new(MINT_URL.parse().unwrap(), None);
    let response = http_client.post_swap(swap_request.clone()).await;

    match response {
        Err(err) => match err {
            cdk::Error::TokenAlreadySpent => (),
            err => {
                panic!("Wrong mint error returned: {}", err);
            }
        },
        Ok(_) => {
            panic!("Should not have allowed to mint with multiple units");
        }
    }
}

/// Tests that tokens cannot be melted after a failed swap attempt
#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_fake_mint_melt_spend_after_fail() {
    let wallet = Wallet::new(
        MINT_URL,
        CurrencyUnit::Sat,
        Arc::new(memory::empty().await.unwrap()),
        Mnemonic::generate(12).unwrap().to_seed_normalized(""),
        None,
    )
    .expect("failed to create new wallet");

    let mint_quote = wallet.mint_quote(100.into(), None).await.unwrap();

    let mut proof_streams = wallet.proof_stream(mint_quote.clone(), SplitTarget::default(), None);

    let proofs = proof_streams
        .next()
        .await
        .expect("payment")
        .expect("no error");

    let active_keyset_id = wallet.fetch_active_keyset().await.unwrap().id;
    let fee_and_amounts = (0, ((0..32).map(|x| 2u64.pow(x)).collect::<Vec<_>>())).into();

    let pre_mint = PreMintSecrets::random(
        active_keyset_id,
        100.into(),
        &SplitTarget::None,
        &fee_and_amounts,
    )
    .unwrap();

    let swap_request = SwapRequest::new(proofs.clone(), pre_mint.blinded_messages());

    let http_client = HttpClient::new(MINT_URL.parse().unwrap(), None);
    let response = http_client.post_swap(swap_request.clone()).await;

    assert!(response.is_ok());

    let pre_mint = PreMintSecrets::random(
        active_keyset_id,
        101.into(),
        &SplitTarget::None,
        &fee_and_amounts,
    )
    .unwrap();

    let swap_request = SwapRequest::new(proofs.clone(), pre_mint.blinded_messages());

    let http_client = HttpClient::new(MINT_URL.parse().unwrap(), None);
    let response = http_client.post_swap(swap_request.clone()).await;

    match response {
        Err(err) => match err {
            cdk::Error::TransactionUnbalanced(_, _, _) => (),
            err => panic!("Wrong mint error returned expected TransactionUnbalanced, got: {err}"),
        },
        Ok(_) => panic!("Should not have allowed swap with unbalanced"),
    }

    let input_amount: u64 = proofs.total_amount().unwrap().into();
    let invoice = create_fake_invoice((input_amount - 1) * 1000, "".to_string());
    let melt_quote = wallet.melt_quote(invoice.to_string(), None).await.unwrap();

    let melt_request = MeltRequest::new(melt_quote.id, proofs, None);

    let http_client = HttpClient::new(MINT_URL.parse().unwrap(), None);
    let response = http_client.post_melt(melt_request.clone()).await;

    match response {
        Err(err) => match err {
            cdk::Error::TokenAlreadySpent => (),
            err => {
                panic!("Wrong mint error returned: {}", err);
            }
        },
        Ok(_) => {
            panic!("Should not have allowed to melt with multiple units");
        }
    }
}

/// Tests that attempting to swap with duplicate proofs fails
#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_fake_mint_duplicate_proofs_swap() {
    let wallet = Wallet::new(
        MINT_URL,
        CurrencyUnit::Sat,
        Arc::new(memory::empty().await.unwrap()),
        Mnemonic::generate(12).unwrap().to_seed_normalized(""),
        None,
    )
    .expect("failed to create new wallet");

    let mint_quote = wallet.mint_quote(100.into(), None).await.unwrap();

    let mut proof_streams = wallet.proof_stream(mint_quote.clone(), SplitTarget::default(), None);

    let proofs = proof_streams
        .next()
        .await
        .expect("payment")
        .expect("no error");

    let active_keyset_id = wallet.fetch_active_keyset().await.unwrap().id;
    let fee_and_amounts = (0, ((0..32).map(|x| 2u64.pow(x)).collect::<Vec<_>>())).into();

    let inputs = vec![proofs[0].clone(), proofs[0].clone()];

    let pre_mint = PreMintSecrets::random(
        active_keyset_id,
        inputs.total_amount().unwrap(),
        &SplitTarget::None,
        &fee_and_amounts,
    )
    .unwrap();

    let swap_request = SwapRequest::new(inputs.clone(), pre_mint.blinded_messages());

    let http_client = HttpClient::new(MINT_URL.parse().unwrap(), None);
    let response = http_client.post_swap(swap_request.clone()).await;

    match response {
        Err(err) => match err {
            cdk::Error::DuplicateInputs => (),
            err => {
                panic!(
                    "Wrong mint error returned, expected duplicate inputs: {}",
                    err
                );
            }
        },
        Ok(_) => {
            panic!("Should not have allowed duplicate inputs");
        }
    }

    let blinded_message = pre_mint.blinded_messages();

    let inputs = vec![proofs[0].clone()];
    let outputs = vec![blinded_message[0].clone(), blinded_message[0].clone()];

    let swap_request = SwapRequest::new(inputs, outputs);

    let http_client = HttpClient::new(MINT_URL.parse().unwrap(), None);
    let response = http_client.post_swap(swap_request.clone()).await;

    match response {
        Err(err) => match err {
            cdk::Error::DuplicateOutputs => (),
            err => {
                panic!(
                    "Wrong mint error returned, expected duplicate outputs: {}",
                    err
                );
            }
        },
        Ok(_) => {
            panic!("Should not have allow duplicate inputs");
        }
    }
}

/// Tests that attempting to melt with duplicate proofs fails
#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_fake_mint_duplicate_proofs_melt() {
    let wallet = Wallet::new(
        MINT_URL,
        CurrencyUnit::Sat,
        Arc::new(memory::empty().await.unwrap()),
        Mnemonic::generate(12).unwrap().to_seed_normalized(""),
        None,
    )
    .expect("failed to create new wallet");

    let mint_quote = wallet.mint_quote(100.into(), None).await.unwrap();

    let mut proof_streams = wallet.proof_stream(mint_quote.clone(), SplitTarget::default(), None);

    let proofs = proof_streams
        .next()
        .await
        .expect("payment")
        .expect("no error");

    let inputs = vec![proofs[0].clone(), proofs[0].clone()];

    let invoice = create_fake_invoice(7000, "".to_string());

    let melt_quote = wallet.melt_quote(invoice.to_string(), None).await.unwrap();

    let melt_request = MeltRequest::new(melt_quote.id, inputs, None);

    let http_client = HttpClient::new(MINT_URL.parse().unwrap(), None);
    let response = http_client.post_melt(melt_request.clone()).await;

    match response {
        Err(err) => match err {
            cdk::Error::DuplicateInputs => (),
            err => {
                panic!("Wrong mint error returned: {}", err);
            }
        },
        Ok(_) => {
            panic!("Should not have allow duplicate inputs");
        }
    }
}
