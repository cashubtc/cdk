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
use std::time::Duration;

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

    // melt failed, but there is new code to reclaim unspent proofs
    assert!(!wallet
        .localstore
        .get_proofs(None, None, Some(vec![State::Pending]), None)
        .await
        .unwrap()
        .is_empty());
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

    let wallet_bal = wallet.total_balance().await.unwrap();
    assert_eq!(wallet_bal, 98.into());
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

    assert!(!wallet
        .localstore
        .get_proofs(None, None, Some(vec![State::Pending]), None)
        .await
        .unwrap()
        .is_empty());
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

    wallet.check_all_pending_proofs().await.unwrap();

    let pending = wallet
        .localstore
        .get_proofs(None, None, Some(vec![State::Pending]), None)
        .await
        .unwrap();

    assert!(pending.is_empty());

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

    wallet.check_all_pending_proofs().await.unwrap();

    assert!(!wallet
        .localstore
        .get_proofs(None, None, Some(vec![State::Pending]), None)
        .await
        .unwrap()
        .is_empty());
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
    assert!(melt.is_err());

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
    assert!(melt.is_err());

    assert!(!wallet
        .localstore
        .get_proofs(None, None, Some(vec![State::Pending]), None)
        .await
        .unwrap()
        .is_empty());
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

    let old_balance = wallet.total_balance().await.expect("balance");

    let fake_description = FakeInvoiceDescription {
        pay_invoice_state: MeltQuoteState::Failed,
        check_payment_state: MeltQuoteState::Paid,
        pay_err: true,
        check_err: false,
    };

    let invoice = create_fake_invoice(7000, serde_json::to_string(&fake_description).unwrap());

    let melt_quote = wallet.melt_quote(invoice.to_string(), None).await.unwrap();

    // The melt should error at the payment invoice command
    let melt = wallet.melt(&melt_quote.id).await.unwrap();

    assert!(melt.fee_paid == Amount::ZERO);
    assert!(melt.amount == Amount::from(7));

    // melt failed, but there is new code to reclaim unspent proofs
    assert_eq!(
        old_balance - melt.amount,
        wallet.total_balance().await.expect("new balance")
    );

    assert!(wallet
        .localstore
        .get_proofs(None, None, Some(vec![State::Pending]), None)
        .await
        .unwrap()
        .is_empty());
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

    let mut proofs = proof_streams
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

    let mut usd_proofs = proof_streams
        .next()
        .await
        .expect("payment")
        .expect("no error");

    usd_proofs.reverse();
    proofs.reverse();

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

/// Tests that wallet automatically recovers proofs after a failed melt operation
/// by swapping them to new proofs, preventing loss of funds
#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_wallet_proof_recovery_after_failed_melt() {
    let wallet = Wallet::new(
        MINT_URL,
        CurrencyUnit::Sat,
        Arc::new(memory::empty().await.unwrap()),
        Mnemonic::generate(12).unwrap().to_seed_normalized(""),
        None,
    )
    .expect("failed to create new wallet");

    // Mint 100 sats
    let mint_quote = wallet.mint_quote(100.into(), None).await.unwrap();
    let _roof_streams = wallet
        .wait_and_mint_quote(
            mint_quote.clone(),
            SplitTarget::default(),
            None,
            Duration::from_secs(1000),
        )
        .await;

    assert_eq!(wallet.total_balance().await.unwrap(), Amount::from(100));

    // Create a melt quote that will fail
    let fake_description = FakeInvoiceDescription {
        pay_invoice_state: MeltQuoteState::Unknown,
        check_payment_state: MeltQuoteState::Unpaid,
        pay_err: true,
        check_err: false,
    };

    let invoice = create_fake_invoice(1000, serde_json::to_string(&fake_description).unwrap());
    let melt_quote = wallet.melt_quote(invoice.to_string(), None).await.unwrap();

    // Attempt to melt - this should fail but trigger proof recovery
    let melt_result = wallet.melt(&melt_quote.id).await;
    assert!(melt_result.is_err(), "Melt should have failed");

    // Verify wallet still has balance (proofs recovered)
    assert_eq!(
        wallet.total_balance().await.unwrap(),
        Amount::from(100),
        "Balance should be recovered"
    );

    // Verify we can still spend the recovered proofs
    let valid_invoice = create_fake_invoice(7000, "".to_string());
    let valid_melt_quote = wallet
        .melt_quote(valid_invoice.to_string(), None)
        .await
        .unwrap();

    let successful_melt = wallet.melt(&valid_melt_quote.id).await;
    assert!(
        successful_melt.is_ok(),
        "Should be able to spend recovered proofs"
    );
}

/// Tests that concurrent melt attempts for the same invoice result in exactly one success
///
/// This test verifies the race condition protection: when multiple melt quotes exist for the
/// same invoice and all are attempted concurrently, only one should succeed due to
/// the FOR UPDATE locking on quotes with the same request_lookup_id.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn test_concurrent_melt_same_invoice() {
    const NUM_WALLETS: usize = 4;

    // Create multiple wallets to simulate concurrent requests
    let mut wallets = Vec::with_capacity(NUM_WALLETS);
    for i in 0..NUM_WALLETS {
        let wallet = Arc::new(
            Wallet::new(
                MINT_URL,
                CurrencyUnit::Sat,
                Arc::new(memory::empty().await.unwrap()),
                Mnemonic::generate(12).unwrap().to_seed_normalized(""),
                None,
            )
            .expect(&format!("failed to create wallet {}", i)),
        );
        wallets.push(wallet);
    }

    // Mint proofs for all wallets
    for (i, wallet) in wallets.iter().enumerate() {
        let mint_quote = wallet.mint_quote(100.into(), None).await.unwrap();
        let mut proof_streams =
            wallet.proof_stream(mint_quote.clone(), SplitTarget::default(), None);
        proof_streams
            .next()
            .await
            .expect(&format!("payment for wallet {}", i))
            .expect("no error");
    }

    // Create a single invoice that all wallets will try to pay
    let fake_description = FakeInvoiceDescription::default();
    let invoice = create_fake_invoice(9000, serde_json::to_string(&fake_description).unwrap());

    // All wallets create melt quotes for the same invoice
    let mut melt_quotes = Vec::with_capacity(NUM_WALLETS);
    for wallet in &wallets {
        let melt_quote = wallet.melt_quote(invoice.to_string(), None).await.unwrap();
        melt_quotes.push(melt_quote);
    }

    // Verify all quotes have the same request (same invoice = same lookup_id)
    for quote in &melt_quotes[1..] {
        assert_eq!(
            melt_quotes[0].request, quote.request,
            "All quotes should be for the same invoice"
        );
    }

    // Attempt all melts concurrently
    let mut handles = Vec::with_capacity(NUM_WALLETS);
    for (wallet, quote) in wallets.iter().zip(melt_quotes.iter()) {
        let wallet_clone = Arc::clone(wallet);
        let quote_id = quote.id.clone();
        handles.push(tokio::spawn(
            async move { wallet_clone.melt(&quote_id).await },
        ));
    }

    // Collect results
    let mut results = Vec::with_capacity(NUM_WALLETS);
    for handle in handles {
        results.push(handle.await.expect("task panicked"));
    }

    // Count successes and failures
    let success_count = results.iter().filter(|r| r.is_ok()).count();
    let failure_count = results.iter().filter(|r| r.is_err()).count();

    assert_eq!(
        success_count, 1,
        "Expected exactly one successful melt, got {}. Results: {:?}",
        success_count, results
    );
    assert_eq!(
        failure_count,
        NUM_WALLETS - 1,
        "Expected {} failed melts, got {}",
        NUM_WALLETS - 1,
        failure_count
    );

    // Verify all failures were due to duplicate detection
    for result in &results {
        if let Err(err) = result {
            let err_str = err.to_string().to_lowercase();
            assert!(
                err_str.contains("duplicate")
                    || err_str.contains("already paid")
                    || err_str.contains("pending"),
                "Expected duplicate/already paid/pending error, got: {}",
                err
            );
        }
    }
}

/// Tests that wallet automatically recovers proofs after a failed swap operation
#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_wallet_proof_recovery_after_failed_swap() {
    let wallet = Wallet::new(
        MINT_URL,
        CurrencyUnit::Sat,
        Arc::new(memory::empty().await.unwrap()),
        Mnemonic::generate(12).unwrap().to_seed_normalized(""),
        None,
    )
    .expect("failed to create new wallet");

    // Mint 100 sats
    let mint_quote = wallet.mint_quote(100.into(), None).await.unwrap();
    let mut proof_streams = wallet.proof_stream(mint_quote.clone(), SplitTarget::default(), None);
    let initial_proofs = proof_streams
        .next()
        .await
        .expect("payment")
        .expect("no error");

    let initial_ys: Vec<_> = initial_proofs.iter().map(|p| p.y().unwrap()).collect();

    assert_eq!(wallet.total_balance().await.unwrap(), Amount::from(100));

    let unspent_proofs = wallet.get_unspent_proofs().await.unwrap();

    // Create an invalid swap by manually constructing a request that will fail
    // We'll use the wallet's swap with invalid parameters to trigger a failure
    let active_keyset_id = wallet.fetch_active_keyset().await.unwrap().id;
    let fee_and_amounts = (0, ((0..32).map(|x| 2u64.pow(x)).collect::<Vec<_>>())).into();

    // Create invalid swap request (requesting more than we have)
    let preswap = PreMintSecrets::random(
        active_keyset_id,
        1000.into(), // More than the 100 we have
        &SplitTarget::default(),
        &fee_and_amounts,
    )
    .unwrap();

    let swap_request = SwapRequest::new(unspent_proofs.clone(), preswap.blinded_messages());

    // Use HTTP client directly to bypass wallet's validation and trigger recovery
    let http_client = HttpClient::new(MINT_URL.parse().unwrap(), None);
    let response = http_client.post_swap(swap_request).await;
    assert!(response.is_err(), "Swap should have failed");

    // Note: The HTTP client doesn't trigger the wallet's try_proof_operation wrapper
    // So we need to test through the wallet's own methods
    // After the failed HTTP request, the proofs are still in the wallet's database

    // Verify balance is still available after the failed operation
    assert_eq!(
        wallet.total_balance().await.unwrap(),
        Amount::from(100),
        "Balance should still be available"
    );

    // Verify we can perform a successful swap operation
    let successful_swap = wallet
        .swap(None, SplitTarget::None, unspent_proofs, None, false)
        .await;

    assert!(
        successful_swap.is_ok(),
        "Should be able to swap after failed operation"
    );

    // Verify the proofs were swapped to new ones
    let final_proofs = wallet.get_unspent_proofs().await.unwrap();
    let final_ys: Vec<_> = final_proofs.iter().map(|p| p.y().unwrap()).collect();

    // The Ys should be different after the successful swap
    assert!(
        initial_ys.iter().any(|y| !final_ys.contains(y)),
        "Proofs should have been swapped to new ones"
    );
}

/// Tests that melt_proofs works correctly with proofs that are not already in the wallet's database.
/// This is similar to the receive flow where proofs come from an external source.
///
/// Flow:
/// 1. Wallet A mints proofs (proofs ARE in Wallet A's database)
/// 2. Wallet B creates a melt quote
/// 3. Wallet B calls melt_proofs with proofs from Wallet A (proofs are NOT in Wallet B's database)
#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_melt_proofs_external() {
    // Create sender wallet (Wallet A) and mint some proofs
    let wallet_sender = Wallet::new(
        MINT_URL,
        CurrencyUnit::Sat,
        Arc::new(memory::empty().await.unwrap()),
        Mnemonic::generate(12).unwrap().to_seed_normalized(""),
        None,
    )
    .expect("failed to create sender wallet");

    let mint_quote = wallet_sender.mint_quote(100.into(), None).await.unwrap();

    let mut proof_streams =
        wallet_sender.proof_stream(mint_quote.clone(), SplitTarget::default(), None);

    let proofs = proof_streams
        .next()
        .await
        .expect("payment")
        .expect("no error");

    assert_eq!(proofs.total_amount().unwrap(), Amount::from(100));

    // Create receiver/melter wallet (Wallet B) with a separate database
    // These proofs are NOT in Wallet B's database
    let wallet_melter = Wallet::new(
        MINT_URL,
        CurrencyUnit::Sat,
        Arc::new(memory::empty().await.unwrap()),
        Mnemonic::generate(12).unwrap().to_seed_normalized(""),
        None,
    )
    .expect("failed to create melter wallet");

    // Verify proofs are not in the melter wallet's database
    let melter_proofs = wallet_melter.get_unspent_proofs().await.unwrap();
    assert!(
        melter_proofs.is_empty(),
        "Melter wallet should have no proofs initially"
    );

    // Create a fake invoice for melting
    let fake_description = FakeInvoiceDescription::default();
    let invoice = create_fake_invoice(9000, serde_json::to_string(&fake_description).unwrap());

    // Wallet B creates a melt quote
    let melt_quote = wallet_melter
        .melt_quote(invoice.to_string(), None)
        .await
        .unwrap();

    // Wallet B calls melt_proofs with external proofs (from Wallet A)
    // These proofs are NOT in wallet_melter's database
    let melted = wallet_melter
        .melt_proofs(&melt_quote.id, proofs.clone())
        .await
        .unwrap();

    // Verify the melt succeeded
    assert_eq!(melted.amount, Amount::from(9));
    assert_eq!(melted.fee_paid, 1.into());

    // Verify change was returned (100 input - 9 melt amount = 91 change, minus fee reserve)
    assert!(melted.change.is_some());
    let change_amount = melted.change.unwrap().total_amount().unwrap();
    assert!(change_amount > Amount::ZERO, "Should have received change");

    // Verify the melter wallet now has the change proofs
    let melter_balance = wallet_melter.total_balance().await.unwrap();
    assert_eq!(melter_balance, change_amount);

    // Verify a transaction was recorded
    let transactions = wallet_melter
        .list_transactions(Some(TransactionDirection::Outgoing))
        .await
        .unwrap();
    assert_eq!(transactions.len(), 1);
    assert_eq!(transactions[0].amount, Amount::from(9));
}

/// Tests that melt automatically performs a swap when proofs don't exactly match
/// the required amount (quote + fee_reserve + input_fee).
///
/// This test verifies the swap-before-melt optimization:
/// 1. Mint proofs that will NOT exactly match a melt amount
/// 2. Create a melt quote for a specific amount
/// 3. Call melt() - it should automatically swap proofs to get exact denominations
/// 4. Verify the melt succeeded
#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_melt_with_swap_for_exact_amount() {
    let wallet = Wallet::new(
        MINT_URL,
        CurrencyUnit::Sat,
        Arc::new(memory::empty().await.unwrap()),
        Mnemonic::generate(12).unwrap().to_seed_normalized(""),
        None,
    )
    .expect("failed to create new wallet");

    // Mint 100 sats - this will give us proofs in standard denominations
    let mint_quote = wallet.mint_quote(100.into(), None).await.unwrap();

    let mut proof_streams = wallet.proof_stream(mint_quote.clone(), SplitTarget::default(), None);

    let proofs = proof_streams
        .next()
        .await
        .expect("payment")
        .expect("no error");

    let initial_balance = wallet.total_balance().await.unwrap();
    assert_eq!(initial_balance, Amount::from(100));

    // Log the proof denominations we received
    let proof_amounts: Vec<u64> = proofs.iter().map(|p| u64::from(p.amount)).collect();
    tracing::info!("Initial proof denominations: {:?}", proof_amounts);

    // Create a melt quote for an amount that likely won't match our proof denominations exactly
    // Using 7 sats (7000 msats) which requires specific denominations
    let fake_description = FakeInvoiceDescription::default();
    let invoice = create_fake_invoice(7000, serde_json::to_string(&fake_description).unwrap());

    let melt_quote = wallet.melt_quote(invoice.to_string(), None).await.unwrap();

    tracing::info!(
        "Melt quote: amount={}, fee_reserve={}",
        melt_quote.amount,
        melt_quote.fee_reserve
    );

    // Call melt() - this should trigger swap-before-melt if proofs don't match exactly
    let melted = wallet.melt(&melt_quote.id).await.unwrap();

    // Verify the melt succeeded
    assert_eq!(melted.amount, Amount::from(7));

    tracing::info!(
        "Melt completed: amount={}, fee_paid={}",
        melted.amount,
        melted.fee_paid
    );

    // Verify final balance is correct (initial - melt_amount - fees)
    let final_balance = wallet.total_balance().await.unwrap();
    tracing::info!(
        "Balance: initial={}, final={}, paid={}",
        initial_balance,
        final_balance,
        melted.amount + melted.fee_paid
    );

    assert!(
        final_balance < initial_balance,
        "Balance should have decreased after melt"
    );
    assert_eq!(
        final_balance,
        initial_balance - melted.amount - melted.fee_paid,
        "Final balance should be initial - amount - fees"
    );
}

/// Tests that melt works correctly when proofs already exactly match the required amount.
/// In this case, no swap should be needed.
#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_melt_exact_proofs_no_swap_needed() {
    let wallet = Wallet::new(
        MINT_URL,
        CurrencyUnit::Sat,
        Arc::new(memory::empty().await.unwrap()),
        Mnemonic::generate(12).unwrap().to_seed_normalized(""),
        None,
    )
    .expect("failed to create new wallet");

    // Mint a larger amount to have more denomination options
    let mint_quote = wallet.mint_quote(1000.into(), None).await.unwrap();

    let mut proof_streams = wallet.proof_stream(mint_quote.clone(), SplitTarget::default(), None);

    let _proofs = proof_streams
        .next()
        .await
        .expect("payment")
        .expect("no error");

    let initial_balance = wallet.total_balance().await.unwrap();
    assert_eq!(initial_balance, Amount::from(1000));

    // Create a melt for a power-of-2 amount that's more likely to match existing denominations
    let fake_description = FakeInvoiceDescription::default();
    let invoice = create_fake_invoice(64_000, serde_json::to_string(&fake_description).unwrap()); // 64 sats

    let melt_quote = wallet.melt_quote(invoice.to_string(), None).await.unwrap();

    // Melt should succeed
    let melted = wallet.melt(&melt_quote.id).await.unwrap();

    assert_eq!(melted.amount, Amount::from(64));

    let final_balance = wallet.total_balance().await.unwrap();
    assert_eq!(
        final_balance,
        initial_balance - melted.amount - melted.fee_paid
    );
}

/// Tests the check_all_mint_quotes functionality for Bolt11 quotes
///
/// This test verifies that:
/// 1. Paid mint quotes are automatically minted when check_all_mint_quotes is called
/// 2. The total amount returned matches the minted proofs
/// 3. Quote state is properly updated after minting
#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_check_all_mint_quotes_bolt11() {
    let wallet = Wallet::new(
        MINT_URL,
        CurrencyUnit::Sat,
        Arc::new(memory::empty().await.unwrap()),
        Mnemonic::generate(12).unwrap().to_seed_normalized(""),
        None,
    )
    .expect("failed to create new wallet");

    // Create first mint quote and pay it (using proof_stream triggers fake wallet payment)
    let mint_quote_1 = wallet.mint_quote(100.into(), None).await.unwrap();

    // Wait for the payment to be registered (fake wallet auto-pays)
    let mut payment_stream_1 = wallet.payment_stream(&mint_quote_1);
    payment_stream_1
        .next()
        .await
        .expect("payment")
        .expect("no error");

    // Create second mint quote and pay it
    let mint_quote_2 = wallet.mint_quote(50.into(), None).await.unwrap();

    let mut payment_stream_2 = wallet.payment_stream(&mint_quote_2);
    payment_stream_2
        .next()
        .await
        .expect("payment")
        .expect("no error");

    // Verify no proofs have been minted yet
    assert_eq!(wallet.total_balance().await.unwrap(), Amount::ZERO);

    // Call check_all_mint_quotes - this should mint both paid quotes
    let total_minted = wallet.check_all_mint_quotes().await.unwrap();

    // Verify the total amount minted is correct (100 + 50 = 150)
    assert_eq!(total_minted, Amount::from(150));

    // Verify wallet balance matches
    assert_eq!(wallet.total_balance().await.unwrap(), Amount::from(150));

    // Calling check_all_mint_quotes again should return 0 (quotes already minted)
    let second_check = wallet.check_all_mint_quotes().await.unwrap();
    assert_eq!(second_check, Amount::ZERO);
}

/// Tests the get_unissued_mint_quotes wallet method
///
/// This test verifies that:
/// 1. Unpaid quotes are included (wallet needs to check with mint)
/// 2. Paid but not issued quotes are included
/// 3. Fully issued quotes are excluded
/// 4. Only quotes for the current mint URL are returned
#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_get_unissued_mint_quotes_wallet() {
    let wallet = Wallet::new(
        MINT_URL,
        CurrencyUnit::Sat,
        Arc::new(memory::empty().await.unwrap()),
        Mnemonic::generate(12).unwrap().to_seed_normalized(""),
        None,
    )
    .expect("failed to create new wallet");

    // Create a quote but don't pay it (stays unpaid)
    let unpaid_quote = wallet.mint_quote(100.into(), None).await.unwrap();

    // Create another quote and pay it but don't mint
    let paid_quote = wallet.mint_quote(50.into(), None).await.unwrap();
    let mut payment_stream = wallet.payment_stream(&paid_quote);
    payment_stream
        .next()
        .await
        .expect("payment")
        .expect("no error");

    // Create a third quote and fully mint it
    let minted_quote = wallet.mint_quote(25.into(), None).await.unwrap();
    let mut proof_stream = wallet.proof_stream(minted_quote.clone(), SplitTarget::default(), None);
    proof_stream
        .next()
        .await
        .expect("payment")
        .expect("no error");

    // Get unissued quotes
    let unissued_quotes = wallet.get_unissued_mint_quotes().await.unwrap();

    // Should have 2 quotes: unpaid and paid-but-not-issued
    // The fully minted quote should be excluded
    assert_eq!(
        unissued_quotes.len(),
        2,
        "Should have 2 unissued quotes (unpaid and paid-not-issued)"
    );

    let quote_ids: Vec<&str> = unissued_quotes.iter().map(|q| q.id.as_str()).collect();
    assert!(
        quote_ids.contains(&unpaid_quote.id.as_str()),
        "Unpaid quote should be included"
    );
    assert!(
        quote_ids.contains(&paid_quote.id.as_str()),
        "Paid but not issued quote should be included"
    );
    assert!(
        !quote_ids.contains(&minted_quote.id.as_str()),
        "Fully minted quote should NOT be included"
    );
}

/// Tests that mint quote state is properly updated after minting
///
/// This test verifies that:
/// 1. amount_issued is updated after successful minting
/// 2. Quote state is updated correctly
/// 3. The quote is stored properly in the localstore
#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_mint_quote_state_updates_after_minting() {
    let wallet = Wallet::new(
        MINT_URL,
        CurrencyUnit::Sat,
        Arc::new(memory::empty().await.unwrap()),
        Mnemonic::generate(12).unwrap().to_seed_normalized(""),
        None,
    )
    .expect("failed to create new wallet");

    let mint_amount = Amount::from(100);
    let mint_quote = wallet.mint_quote(mint_amount, None).await.unwrap();

    // Get the quote from localstore before minting
    let quote_before = wallet
        .localstore
        .get_mint_quote(&mint_quote.id)
        .await
        .unwrap()
        .expect("Quote should exist");

    // Verify initial state
    assert_eq!(quote_before.amount_issued, Amount::ZERO);

    // Mint the tokens using wait_and_mint_quote
    let proofs = wallet
        .wait_and_mint_quote(
            mint_quote.clone(),
            SplitTarget::default(),
            None,
            Duration::from_secs(60),
        )
        .await
        .expect("minting should succeed");

    let minted_amount = proofs.total_amount().unwrap();
    assert_eq!(minted_amount, mint_amount);

    // Check the quote is now either removed or updated in the localstore
    // After minting, the quote should be removed from localstore (it's fully issued)
    let quote_after = wallet
        .localstore
        .get_mint_quote(&mint_quote.id)
        .await
        .unwrap();

    // The quote should either be removed or have amount_issued updated
    match quote_after {
        Some(quote) => {
            // If still present, amount_issued should equal the minted amount
            assert_eq!(
                quote.amount_issued, minted_amount,
                "amount_issued should be updated after minting"
            );
        }
        None => {
            // Quote was removed after being fully issued - this is also valid behavior
        }
    }

    // Verify the unissued quotes no longer contains this quote
    let unissued = wallet.get_unissued_mint_quotes().await.unwrap();
    let unissued_ids: Vec<&str> = unissued.iter().map(|q| q.id.as_str()).collect();
    assert!(
        !unissued_ids.contains(&mint_quote.id.as_str()),
        "Fully minted quote should not appear in unissued quotes"
    );
}
