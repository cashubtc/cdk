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
use cdk::nuts::nut00::{KnownMethod, ProofsMethods};
use cdk::nuts::{
    CurrencyUnit, MeltQuoteState, MeltRequest, MintRequest, PaymentMethod, PreMintSecrets, Proofs,
    SecretKey, State, SwapRequest,
};
use cdk::wallet::advanced::{
    HttpClient, MetadataSource, MintClaimOptions, MintConnector, MintMetadataRequest,
    MintSessionFilter, PaymentFunding, PaymentPrepareOptions, ProofQuery, ReissueFeePolicy,
    ReissueProtection, ReissueRequest,
};
use cdk::wallet::history::HistoryQuery;
use cdk::wallet::mint::{MintRequest as WalletMintRequest, MintSession, MintState};
use cdk::wallet::operation::SyncPolicy;
use cdk::wallet::payment::{
    PaymentConfirmation, PaymentQuoteRequest, PaymentSession, PaymentTarget,
};
use cdk::wallet::Wallet;
use cdk_common::database::WalletDatabase;
use cdk_common::wallet::TransactionDirection;
use cdk_fake_wallet::{create_fake_invoice, FakeInvoiceDescription};
use cdk_sqlite::wallet::memory;

const MINT_URL: &str = "http://127.0.0.1:8086";

async fn fund_wallet(wallet: &Wallet, amount: Amount, split: SplitTarget) -> Proofs {
    let before = wallet
        .advanced()
        .proofs(ProofQuery::default())
        .await
        .expect("proof records")
        .into_iter()
        .map(|record| record.y)
        .collect::<std::collections::HashSet<_>>();
    let session = wallet
        .request_mint(WalletMintRequest::bolt11(amount))
        .await
        .expect("mint session");
    session
        .wait_with(
            MintClaimOptions {
                amount_split_target: split,
                conditions: None,
            },
            Duration::from_secs(30),
        )
        .await
        .expect("mint receipt");
    wallet
        .advanced()
        .proofs(ProofQuery::default())
        .await
        .expect("proof records")
        .into_iter()
        .filter(|record| !before.contains(&record.y))
        .map(|record| record.proof)
        .collect()
}

async fn paid_mint_session(
    wallet: &Wallet,
    method: PaymentMethod,
    amount: Option<Amount>,
) -> MintSession {
    let session = wallet
        .request_mint(WalletMintRequest::new(method, amount))
        .await
        .expect("mint session");
    tokio::time::timeout(Duration::from_secs(15), async {
        loop {
            let state = session.refresh().await.expect("mint state");
            if matches!(state.state, MintState::Paid | MintState::Issued) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .expect("paid mint quote");
    session
}

async fn quote_payment(wallet: &Wallet, invoice: impl ToString) -> PaymentSession {
    wallet
        .quote_payment(PaymentQuoteRequest::new(PaymentTarget::bolt11(
            invoice.to_string(),
        )))
        .await
        .expect("payment quote")
        .into_single()
        .expect("single payment quote")
}

async fn proof_count(wallet: &Wallet, state: State) -> usize {
    wallet
        .advanced()
        .proofs(ProofQuery {
            states: vec![state],
            conditions: None,
        })
        .await
        .expect("proof records")
        .len()
}

async fn unspent_proofs(wallet: &Wallet) -> Proofs {
    wallet
        .advanced()
        .proofs(ProofQuery::default())
        .await
        .expect("proof records")
        .into_iter()
        .map(|record| record.proof)
        .collect()
}

async fn active_keyset_id(wallet: &Wallet) -> cdk::nuts::Id {
    wallet
        .advanced()
        .mint_metadata(MintMetadataRequest {
            source: MetadataSource::Refresh,
        })
        .await
        .expect("mint metadata")
        .active_keyset()
        .expect("active keyset")
        .id
}

/// Tests that when both pay and check return pending status, input proofs should remain pending
#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_fake_tokens_pending() {
    let wallet = cdk_integration_tests::open_test_wallet(
        MINT_URL,
        CurrencyUnit::Sat,
        Arc::new(memory::empty().await.unwrap()),
        Mnemonic::generate(12).unwrap().to_seed_normalized(""),
        None,
    )
    .expect("failed to create new wallet");

    fund_wallet(&wallet, 100.into(), SplitTarget::default()).await;

    let fake_description = FakeInvoiceDescription {
        pay_invoice_state: MeltQuoteState::Pending,
        check_payment_state: MeltQuoteState::Pending,
        pay_err: false,
        check_err: false,
    };

    let invoice = create_fake_invoice(1000, serde_json::to_string(&fake_description).unwrap());

    let payment = quote_payment(&wallet, invoice).await;
    let confirmation = payment.prepare().await.unwrap().submit().await.unwrap();
    assert!(matches!(confirmation, PaymentConfirmation::Pending(_)));

    // melt failed, but there is new code to reclaim unspent proofs
    assert!(proof_count(&wallet, State::Pending).await > 0);
}

/// Tests that an unknown follow-up keeps proofs pending while a confirmed
/// failure returns only that payment's proofs to the wallet.
#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_fake_melt_payment_fail() {
    let wallet = cdk_integration_tests::open_test_wallet(
        MINT_URL,
        CurrencyUnit::Sat,
        Arc::new(memory::empty().await.unwrap()),
        Mnemonic::generate(12).unwrap().to_seed_normalized(""),
        None,
    )
    .expect("Failed to create new wallet");

    fund_wallet(&wallet, 100.into(), SplitTarget::default()).await;

    let fake_description = FakeInvoiceDescription {
        pay_invoice_state: MeltQuoteState::Unknown,
        check_payment_state: MeltQuoteState::Unknown,
        pay_err: true,
        check_err: false,
    };

    let invoice = create_fake_invoice(1000, serde_json::to_string(&fake_description).unwrap());

    let payment = quote_payment(&wallet, invoice).await;
    let outcome = payment.prepare().await.unwrap().submit().await.unwrap();
    assert!(matches!(outcome, PaymentConfirmation::Pending(_)));

    let pending_before_failed = proof_count(&wallet, State::Pending).await;
    assert!(pending_before_failed > 0);

    // Model an authoritative backend failure response. A dispatch error remains
    // ambiguous even if an immediate follow-up check reports Failed.
    let fake_description = FakeInvoiceDescription {
        pay_invoice_state: MeltQuoteState::Failed,
        check_payment_state: MeltQuoteState::Failed,
        pay_err: false,
        check_err: false,
    };

    let invoice = create_fake_invoice(1000, serde_json::to_string(&fake_description).unwrap());

    let payment = quote_payment(&wallet, invoice).await;

    // A confirmed failure should return an error and release its proofs.
    let melt = tokio::time::timeout(Duration::from_secs(15), async {
        payment.prepare().await?.execute().await
    })
    .await
    .expect("authoritative failure should not remain pending");
    assert!(melt.is_err());

    let pending_after_failed = proof_count(&wallet, State::Pending).await;
    assert_eq!(pending_after_failed, pending_before_failed);
}

/// Tests that when both the pay_invoice and check_invoice both fail,
/// the proofs should remain in pending state
#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_fake_melt_payment_fail_and_check() {
    let wallet = cdk_integration_tests::open_test_wallet(
        MINT_URL,
        CurrencyUnit::Sat,
        Arc::new(memory::empty().await.unwrap()),
        Mnemonic::generate(12).unwrap().to_seed_normalized(""),
        None,
    )
    .expect("Failed to create new wallet");

    fund_wallet(&wallet, 100.into(), SplitTarget::default()).await;

    let fake_description = FakeInvoiceDescription {
        pay_invoice_state: MeltQuoteState::Unknown,
        check_payment_state: MeltQuoteState::Unknown,
        pay_err: true,
        check_err: true,
    };

    let invoice = create_fake_invoice(7000, serde_json::to_string(&fake_description).unwrap());

    let payment = quote_payment(&wallet, invoice).await;

    // The melt should error at the payment invoice command
    payment.prepare().await.unwrap().submit().await.unwrap();

    assert!(proof_count(&wallet, State::Pending).await > 0);
}

/// Tests that a failed backend status releases proofs while an unknown status
/// keeps them pending.
#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_fake_melt_payment_return_fail_status() {
    let wallet = cdk_integration_tests::open_test_wallet(
        MINT_URL,
        CurrencyUnit::Sat,
        Arc::new(memory::empty().await.unwrap()),
        Mnemonic::generate(12).unwrap().to_seed_normalized(""),
        None,
    )
    .expect("Failed to create new wallet");

    fund_wallet(&wallet, 100.into(), SplitTarget::default()).await;

    let fake_description = FakeInvoiceDescription {
        pay_invoice_state: MeltQuoteState::Failed,
        check_payment_state: MeltQuoteState::Failed,
        pay_err: false,
        check_err: false,
    };

    let invoice = create_fake_invoice(7000, serde_json::to_string(&fake_description).unwrap());

    let payment = quote_payment(&wallet, invoice).await;

    // A confirmed failure should return an error and release its proofs.
    let melt = async { payment.prepare().await?.execute().await }.await;
    assert!(melt.is_err());

    wallet.synchronize(SyncPolicy::Online).await.unwrap();
    assert_eq!(proof_count(&wallet, State::Pending).await, 0);

    let fake_description = FakeInvoiceDescription {
        pay_invoice_state: MeltQuoteState::Unknown,
        check_payment_state: MeltQuoteState::Unknown,
        pay_err: false,
        check_err: false,
    };

    let invoice = create_fake_invoice(7000, serde_json::to_string(&fake_description).unwrap());

    let payment = quote_payment(&wallet, invoice).await;

    let outcome = payment.prepare().await.unwrap().submit().await.unwrap();
    assert!(matches!(outcome, PaymentConfirmation::Pending(_)));

    wallet.synchronize(SyncPolicy::Online).await.unwrap();

    assert!(proof_count(&wallet, State::Pending).await > 0);
}

/// Tests that when the payment backend returns an error with unknown status,
/// the mint should do a second check and keep proofs pending.
#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_fake_melt_payment_error_unknown() {
    let wallet = cdk_integration_tests::open_test_wallet(
        MINT_URL,
        CurrencyUnit::Sat,
        Arc::new(memory::empty().await.unwrap()),
        Mnemonic::generate(12).unwrap().to_seed_normalized(""),
        None,
    )
    .unwrap();

    fund_wallet(&wallet, 100.into(), SplitTarget::default()).await;

    let fake_description = FakeInvoiceDescription {
        pay_invoice_state: MeltQuoteState::Failed,
        check_payment_state: MeltQuoteState::Unknown,
        pay_err: true,
        check_err: false,
    };

    let invoice = create_fake_invoice(7000, serde_json::to_string(&fake_description).unwrap());

    let payment = quote_payment(&wallet, invoice).await;

    let outcome = payment.prepare().await.unwrap().submit().await.unwrap();
    assert!(matches!(outcome, PaymentConfirmation::Pending(_)));

    let fake_description = FakeInvoiceDescription {
        pay_invoice_state: MeltQuoteState::Unknown,
        check_payment_state: MeltQuoteState::Unknown,
        pay_err: true,
        check_err: false,
    };

    let invoice = create_fake_invoice(7000, serde_json::to_string(&fake_description).unwrap());

    let payment = quote_payment(&wallet, invoice).await;

    let outcome = payment.prepare().await.unwrap().submit().await.unwrap();
    assert!(matches!(outcome, PaymentConfirmation::Pending(_)));

    assert!(proof_count(&wallet, State::Pending).await > 0);
}

/// Tests that when the payment backend returns an error but the second check returns paid,
/// proofs should remain in pending state
#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_fake_melt_payment_err_paid() {
    let wallet = cdk_integration_tests::open_test_wallet(
        MINT_URL,
        CurrencyUnit::Sat,
        Arc::new(memory::empty().await.unwrap()),
        Mnemonic::generate(12).unwrap().to_seed_normalized(""),
        None,
    )
    .expect("Failed to create new wallet");

    fund_wallet(&wallet, 100.into(), SplitTarget::default()).await;

    let old_balance = wallet.balance().await.expect("balance").available;

    let fake_description = FakeInvoiceDescription {
        pay_invoice_state: MeltQuoteState::Failed,
        check_payment_state: MeltQuoteState::Paid,
        pay_err: true,
        check_err: false,
    };

    let invoice = create_fake_invoice(7000, serde_json::to_string(&fake_description).unwrap());

    let payment = quote_payment(&wallet, invoice).await;

    // The melt should complete successfully
    let melt = payment.prepare().await.unwrap().execute().await.unwrap();

    assert_eq!(melt.fee_paid, Amount::ZERO);
    assert_eq!(melt.amount, Amount::from(7));

    // melt failed, but there is new code to reclaim unspent proofs
    assert_eq!(
        old_balance - melt.amount,
        wallet.balance().await.expect("new balance").available
    );

    assert_eq!(proof_count(&wallet, State::Pending).await, 0);
}

/// Tests that change outputs in a melt quote are correctly handled
#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_fake_melt_change_in_quote() {
    let wallet = cdk_integration_tests::open_test_wallet(
        MINT_URL,
        CurrencyUnit::Sat,
        Arc::new(memory::empty().await.unwrap()),
        Mnemonic::generate(12).unwrap().to_seed_normalized(""),
        None,
    )
    .expect("Failed to create new wallet");

    fund_wallet(&wallet, 100.into(), SplitTarget::default()).await;

    let transaction = wallet
        .history(HistoryQuery {
            direction: Some(TransactionDirection::Incoming),
            limit: None,
        })
        .await
        .unwrap()
        .pop()
        .expect("No transaction found");
    assert_eq!(wallet.identity(), transaction.wallet);
    assert_eq!(TransactionDirection::Incoming, transaction.direction);
    assert_eq!(Amount::from(100), transaction.amount);
    assert_eq!(Amount::from(0), transaction.fee);
    assert_eq!(CurrencyUnit::Sat, transaction.wallet.unit);

    let fake_description = FakeInvoiceDescription::default();

    let invoice = create_fake_invoice(9000, serde_json::to_string(&fake_description).unwrap());

    let proofs = unspent_proofs(&wallet).await;

    let payment = quote_payment(&wallet, invoice).await;
    let melt_quote = payment.quote();

    let keyset_id = active_keyset_id(&wallet).await;
    let fee_and_amounts = (0, ((0..32).map(|x| 2u64.pow(x)).collect::<Vec<_>>())).into();

    let premint_secrets = PreMintSecrets::random(
        keyset_id,
        100.into(),
        &SplitTarget::default(),
        &fee_and_amounts,
    )
    .unwrap();

    let client = HttpClient::new(MINT_URL.parse().unwrap(), None);

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

/// Tests minting tokens with a valid witness signature
#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_fake_mint_with_witness() {
    let wallet = cdk_integration_tests::open_test_wallet(
        MINT_URL,
        CurrencyUnit::Sat,
        Arc::new(memory::empty().await.unwrap()),
        Mnemonic::generate(12).unwrap().to_seed_normalized(""),
        None,
    )
    .expect("failed to create new wallet");
    let proofs = fund_wallet(&wallet, 100.into(), SplitTarget::default()).await;

    let mint_amount = proofs.total_amount().unwrap();

    assert!(mint_amount == 100.into());
}

/// Tests that minting without a witness signature fails with the correct error
#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_fake_mint_without_witness() {
    let wallet = cdk_integration_tests::open_test_wallet(
        MINT_URL,
        CurrencyUnit::Sat,
        Arc::new(memory::empty().await.unwrap()),
        Mnemonic::generate(12).unwrap().to_seed_normalized(""),
        None,
    )
    .expect("failed to create new wallet");

    let session = paid_mint_session(&wallet, PaymentMethod::BOLT11, Some(100.into())).await;

    let http_client = HttpClient::new(MINT_URL.parse().unwrap(), None);

    let sat_keyset_id = active_keyset_id(&wallet).await;
    let fee_and_amounts = (0, ((0..32).map(|x| 2u64.pow(x)).collect::<Vec<_>>())).into();

    let premint_secrets = PreMintSecrets::random(
        sat_keyset_id,
        100.into(),
        &SplitTarget::default(),
        &fee_and_amounts,
    )
    .unwrap();

    let request = MintRequest {
        quote: session.id().to_string(),
        outputs: premint_secrets.blinded_messages(),
        signature: None,
    };

    let response = http_client
        .post_mint(&PaymentMethod::Known(KnownMethod::Bolt11), request.clone())
        .await;

    match response {
        Err(cdk::error::Error::SignatureMissingOrInvalid) => {} //pass
        Err(err) => panic!("Wrong mint response for minting without witness: {}", err),
        Ok(_) => panic!("Minting should not have succeed without a witness"),
    }
}

/// Tests that minting with an incorrect witness signature fails with the correct error
#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_fake_mint_with_wrong_witness() {
    let wallet = cdk_integration_tests::open_test_wallet(
        MINT_URL,
        CurrencyUnit::Sat,
        Arc::new(memory::empty().await.unwrap()),
        Mnemonic::generate(12).unwrap().to_seed_normalized(""),
        None,
    )
    .expect("failed to create new wallet");

    let session = paid_mint_session(&wallet, PaymentMethod::BOLT11, Some(100.into())).await;

    let http_client = HttpClient::new(MINT_URL.parse().unwrap(), None);

    let sat_keyset_id = active_keyset_id(&wallet).await;
    let fee_and_amounts = (0, ((0..32).map(|x| 2u64.pow(x)).collect::<Vec<_>>())).into();

    let premint_secrets = PreMintSecrets::random(
        sat_keyset_id,
        100.into(),
        &SplitTarget::default(),
        &fee_and_amounts,
    )
    .unwrap();

    let mut request = MintRequest {
        quote: session.id().to_string(),
        outputs: premint_secrets.blinded_messages(),
        signature: None,
    };

    let secret_key = SecretKey::generate();

    request
        .sign(&secret_key)
        .expect("failed to sign the mint request");

    let response = http_client
        .post_mint(&PaymentMethod::Known(KnownMethod::Bolt11), request.clone())
        .await;

    match response {
        Err(cdk::error::Error::SignatureMissingOrInvalid) => {} //pass
        Err(err) => panic!("Wrong mint response for minting without witness: {}", err),
        Ok(_) => panic!("Minting should not have succeed without a witness"),
    }
}

/// Tests that attempting to mint more tokens than allowed by the quote fails
#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_fake_mint_inflated() {
    let store = Arc::new(memory::empty().await.unwrap());
    let wallet = cdk_integration_tests::open_test_wallet(
        MINT_URL,
        CurrencyUnit::Sat,
        store.clone(),
        Mnemonic::generate(12).unwrap().to_seed_normalized(""),
        None,
    )
    .expect("failed to create new wallet");

    let session = paid_mint_session(&wallet, PaymentMethod::BOLT11, Some(100.into())).await;

    let sat_keyset_id = active_keyset_id(&wallet).await;
    let fee_and_amounts = (0, ((0..32).map(|x| 2u64.pow(x)).collect::<Vec<_>>())).into();

    let pre_mint = PreMintSecrets::random(
        sat_keyset_id,
        500.into(),
        &SplitTarget::None,
        &fee_and_amounts,
    )
    .unwrap();

    let quote_info = store
        .get_mint_quote(session.id().as_str())
        .await
        .unwrap()
        .expect("there is a quote");

    let mut mint_request = MintRequest {
        quote: session.id().to_string(),
        outputs: pre_mint.blinded_messages(),
        signature: None,
    };

    if let Some(secret_key) = quote_info.secret_key {
        mint_request
            .sign(&secret_key)
            .expect("failed to sign the mint request");
    }
    let http_client = HttpClient::new(MINT_URL.parse().unwrap(), None);

    let response = http_client
        .post_mint(
            &PaymentMethod::Known(KnownMethod::Bolt11),
            mint_request.clone(),
        )
        .await;

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

/// Tests that a failed inflated mint attempt does not consume the quote
#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_fake_mint_inflated_does_not_consume_quote() {
    let store = Arc::new(memory::empty().await.unwrap());
    let wallet = cdk_integration_tests::open_test_wallet(
        MINT_URL,
        CurrencyUnit::Sat,
        store.clone(),
        Mnemonic::generate(12).unwrap().to_seed_normalized(""),
        None,
    )
    .expect("failed to create new wallet");

    let session = paid_mint_session(&wallet, PaymentMethod::BOLT11, Some(100.into())).await;

    let sat_keyset_id = active_keyset_id(&wallet).await;
    let fee_and_amounts = (0, ((0..32).map(|x| 2u64.pow(x)).collect::<Vec<_>>())).into();
    let quote_info = store
        .get_mint_quote(session.id().as_str())
        .await
        .unwrap()
        .expect("there is a quote");
    let secret_key = quote_info.secret_key.expect("Secret key on quote");

    let inflated_pre_mint = PreMintSecrets::random(
        sat_keyset_id,
        500.into(),
        &SplitTarget::None,
        &fee_and_amounts,
    )
    .unwrap();

    let mut inflated_request = MintRequest {
        quote: session.id().to_string(),
        outputs: inflated_pre_mint.blinded_messages(),
        signature: None,
    };
    inflated_request
        .sign(&secret_key)
        .expect("failed to sign inflated mint request");

    let http_client = HttpClient::new(MINT_URL.parse().unwrap(), None);
    let inflated_response = http_client
        .post_mint(
            &PaymentMethod::Known(KnownMethod::Bolt11),
            inflated_request.clone(),
        )
        .await;

    assert!(matches!(
        inflated_response,
        Err(cdk::Error::TransactionUnbalanced(_, _, _))
    ));

    let valid_pre_mint = PreMintSecrets::random(
        sat_keyset_id,
        100.into(),
        &SplitTarget::None,
        &fee_and_amounts,
    )
    .unwrap();
    let mut valid_request = MintRequest {
        quote: session.id().to_string(),
        outputs: valid_pre_mint.blinded_messages(),
        signature: None,
    };
    valid_request
        .sign(&secret_key)
        .expect("failed to sign valid mint request");

    let valid_response = http_client
        .post_mint(
            &PaymentMethod::Known(KnownMethod::Bolt11),
            valid_request.clone(),
        )
        .await
        .expect("valid mint request should succeed");
    let total_issued: u64 = valid_response
        .signatures
        .iter()
        .map(|sig| sig.amount.to_u64())
        .sum();
    assert_eq!(total_issued, 100);

    let second_valid_pre_mint = PreMintSecrets::random(
        sat_keyset_id,
        100.into(),
        &SplitTarget::None,
        &fee_and_amounts,
    )
    .unwrap();
    let mut second_valid_request = MintRequest {
        quote: session.id().to_string(),
        outputs: second_valid_pre_mint.blinded_messages(),
        signature: None,
    };
    second_valid_request
        .sign(&secret_key)
        .expect("failed to sign second valid mint request");

    let second_response = http_client
        .post_mint(
            &PaymentMethod::Known(KnownMethod::Bolt11),
            second_valid_request,
        )
        .await;

    assert!(matches!(
        second_response,
        Err(cdk::Error::IssuedQuote) | Err(cdk::Error::TransactionUnbalanced(_, _, _))
    ));
}

/// Tests concurrent mint attempts with different outputs for the same quote
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_fake_mint_concurrent_same_quote_different_outputs() {
    let store = Arc::new(memory::empty().await.unwrap());
    let wallet = cdk_integration_tests::open_test_wallet(
        MINT_URL,
        CurrencyUnit::Sat,
        store.clone(),
        Mnemonic::generate(12).unwrap().to_seed_normalized(""),
        None,
    )
    .expect("failed to create new wallet");

    let session = paid_mint_session(&wallet, PaymentMethod::BOLT11, Some(100.into())).await;

    let active_keyset_id = active_keyset_id(&wallet).await;
    let fee_and_amounts = (0, ((0..32).map(|x| 2u64.pow(x)).collect::<Vec<_>>())).into();
    let quote_info = store
        .get_mint_quote(session.id().as_str())
        .await
        .unwrap()
        .expect("there is a quote");
    let secret_key = quote_info.secret_key.expect("Secret key on quote");

    let pre_mint_one = PreMintSecrets::random(
        active_keyset_id,
        100.into(),
        &SplitTarget::None,
        &fee_and_amounts,
    )
    .unwrap();
    let pre_mint_two = PreMintSecrets::random(
        active_keyset_id,
        100.into(),
        &SplitTarget::None,
        &fee_and_amounts,
    )
    .unwrap();

    let mut request_one = MintRequest {
        quote: session.id().to_string(),
        outputs: pre_mint_one.blinded_messages(),
        signature: None,
    };
    request_one
        .sign(&secret_key)
        .expect("failed to sign first request");

    let mut request_two = MintRequest {
        quote: session.id().to_string(),
        outputs: pre_mint_two.blinded_messages(),
        signature: None,
    };
    request_two
        .sign(&secret_key)
        .expect("failed to sign second request");

    let (result_one, result_two) = tokio::join!(
        async {
            HttpClient::new(MINT_URL.parse().unwrap(), None)
                .post_mint(&PaymentMethod::Known(KnownMethod::Bolt11), request_one)
                .await
        },
        async {
            HttpClient::new(MINT_URL.parse().unwrap(), None)
                .post_mint(&PaymentMethod::Known(KnownMethod::Bolt11), request_two)
                .await
        }
    );

    let results = [result_one, result_two];
    let success_count = results.iter().filter(|res| res.is_ok()).count();
    assert_eq!(
        success_count, 1,
        "exactly one concurrent mint should succeed"
    );

    let failure = results
        .into_iter()
        .find_map(Result::err)
        .expect("one concurrent mint should fail");
    assert!(matches!(
        failure,
        cdk::Error::IssuedQuote | cdk::Error::TransactionUnbalanced(_, _, _)
    ));
}

/// Tests that attempting to mint with multiple currency units in the same request fails
#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_fake_mint_multiple_units() {
    let store = Arc::new(memory::empty().await.unwrap());
    let wallet = cdk_integration_tests::open_test_wallet(
        MINT_URL,
        CurrencyUnit::Sat,
        store.clone(),
        Mnemonic::generate(12).unwrap().to_seed_normalized(""),
        None,
    )
    .expect("failed to create new wallet");

    let session = paid_mint_session(&wallet, PaymentMethod::BOLT11, Some(100.into())).await;

    let sat_keyset_id = active_keyset_id(&wallet).await;
    let fee_and_amounts = (0, ((0..32).map(|x| 2u64.pow(x)).collect::<Vec<_>>())).into();

    let pre_mint = PreMintSecrets::random(
        sat_keyset_id,
        50.into(),
        &SplitTarget::None,
        &fee_and_amounts,
    )
    .unwrap();

    let wallet_usd = cdk_integration_tests::open_test_wallet(
        MINT_URL,
        CurrencyUnit::Usd,
        Arc::new(memory::empty().await.unwrap()),
        Mnemonic::generate(12).unwrap().to_seed_normalized(""),
        None,
    )
    .expect("failed to create new wallet");

    let usd_keyset_id = active_keyset_id(&wallet_usd).await;

    let usd_pre_mint = PreMintSecrets::random(
        usd_keyset_id,
        50.into(),
        &SplitTarget::None,
        &fee_and_amounts,
    )
    .unwrap();

    let quote_info = store
        .get_mint_quote(session.id().as_str())
        .await
        .unwrap()
        .expect("there is a quote");

    let mut sat_outputs = pre_mint.blinded_messages();

    let mut usd_outputs = usd_pre_mint.blinded_messages();

    sat_outputs.append(&mut usd_outputs);

    let mut mint_request = MintRequest {
        quote: session.id().to_string(),
        outputs: sat_outputs,
        signature: None,
    };

    if let Some(secret_key) = quote_info.secret_key {
        mint_request
            .sign(&secret_key)
            .expect("failed to sign the mint request");
    }
    let http_client = HttpClient::new(MINT_URL.parse().unwrap(), None);

    let response = http_client
        .post_mint(
            &PaymentMethod::Known(KnownMethod::Bolt11),
            mint_request.clone(),
        )
        .await;

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
    let wallet = cdk_integration_tests::open_test_wallet(
        MINT_URL,
        CurrencyUnit::Sat,
        Arc::new(memory::empty().await.unwrap()),
        Mnemonic::generate(12).unwrap().to_seed_normalized(""),
        None,
    )
    .expect("failed to create new wallet");

    wallet
        .advanced()
        .mint_metadata(MintMetadataRequest::default())
        .await
        .unwrap();
    let proofs = fund_wallet(&wallet, 100.into(), SplitTarget::default()).await;

    let wallet_usd = cdk_integration_tests::open_test_wallet(
        MINT_URL,
        CurrencyUnit::Usd,
        Arc::new(memory::empty().await.unwrap()),
        Mnemonic::generate(12).unwrap().to_seed_normalized(""),
        None,
    )
    .expect("failed to create usd wallet");
    wallet_usd
        .advanced()
        .mint_metadata(MintMetadataRequest::default())
        .await
        .unwrap();
    let usd_proofs = fund_wallet(&wallet_usd, 100.into(), SplitTarget::default()).await;

    let sat_keyset_id = active_keyset_id(&wallet).await;
    let fee_and_amounts = (0, ((0..32).map(|x| 2u64.pow(x)).collect::<Vec<_>>())).into();

    {
        let inputs: Proofs = vec![
            proofs.first().expect("There is a proof").clone(),
            usd_proofs.first().expect("There is a proof").clone(),
        ];

        let pre_mint = PreMintSecrets::random(
            sat_keyset_id,
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
        let usd_active_keyset_id = active_keyset_id(&wallet_usd).await;
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
            sat_keyset_id,
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
    let wallet = cdk_integration_tests::open_test_wallet(
        MINT_URL,
        CurrencyUnit::Sat,
        Arc::new(memory::empty().await.unwrap()),
        Mnemonic::generate(12).unwrap().to_seed_normalized(""),
        None,
    )
    .expect("failed to create new wallet");

    let mut proofs = fund_wallet(&wallet, 100.into(), SplitTarget::default()).await;

    println!("Minted sat");

    let wallet_usd = cdk_integration_tests::open_test_wallet(
        MINT_URL,
        CurrencyUnit::Usd,
        Arc::new(memory::empty().await.unwrap()),
        Mnemonic::generate(12).unwrap().to_seed_normalized(""),
        None,
    )
    .expect("failed to create new wallet");

    println!("Minted quote usd");
    let mut usd_proofs = fund_wallet(&wallet_usd, 100.into(), SplitTarget::default()).await;

    usd_proofs.reverse();
    proofs.reverse();

    {
        let inputs: Proofs = vec![
            proofs.first().expect("There is a proof").clone(),
            usd_proofs.first().expect("There is a proof").clone(),
        ];

        let input_amount: u64 = inputs.total_amount().unwrap().into();
        let invoice = create_fake_invoice((input_amount - 1) * 1000, "".to_string());
        let payment = quote_payment(&wallet, invoice).await;
        let melt_request = MeltRequest::new(payment.id().to_string(), inputs, None);

        let http_client = HttpClient::new(MINT_URL.parse().unwrap(), None);
        let response = http_client
            .post_melt(
                &PaymentMethod::Known(KnownMethod::Bolt11),
                melt_request.clone(),
            )
            .await;

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
        let sat_keyset_id = active_keyset_id(&wallet).await;
        let usd_active_keyset_id = active_keyset_id(&wallet_usd).await;

        let usd_pre_mint = PreMintSecrets::random(
            usd_active_keyset_id,
            inputs.total_amount().unwrap() + 100.into(),
            &SplitTarget::None,
            &fee_and_amounts,
        )
        .unwrap();
        let pre_mint = PreMintSecrets::random(
            sat_keyset_id,
            100.into(),
            &SplitTarget::None,
            &fee_and_amounts,
        )
        .unwrap();

        let mut usd_outputs = usd_pre_mint.blinded_messages();
        let mut sat_outputs = pre_mint.blinded_messages();

        usd_outputs.append(&mut sat_outputs);
        let payment = quote_payment(&wallet, invoice).await;
        let melt_request = MeltRequest::new(payment.id().to_string(), inputs, Some(usd_outputs));

        let http_client = HttpClient::new(MINT_URL.parse().unwrap(), None);

        let response = http_client
            .post_melt(
                &PaymentMethod::Known(KnownMethod::Bolt11),
                melt_request.clone(),
            )
            .await;

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
    let wallet = cdk_integration_tests::open_test_wallet(
        MINT_URL,
        CurrencyUnit::Sat,
        Arc::new(memory::empty().await.unwrap()),
        Mnemonic::generate(12).unwrap().to_seed_normalized(""),
        None,
    )
    .expect("failed to create new wallet");

    let proofs = fund_wallet(&wallet, 100.into(), SplitTarget::default()).await;

    let wallet_usd = cdk_integration_tests::open_test_wallet(
        MINT_URL,
        CurrencyUnit::Usd,
        Arc::new(memory::empty().await.unwrap()),
        Mnemonic::generate(12).unwrap().to_seed_normalized(""),
        None,
    )
    .expect("failed to create new  usd wallet");
    let usd_active_keyset_id = active_keyset_id(&wallet_usd).await;
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
    let wallet = cdk_integration_tests::open_test_wallet(
        MINT_URL,
        CurrencyUnit::Sat,
        Arc::new(memory::empty().await.unwrap()),
        Mnemonic::generate(12).unwrap().to_seed_normalized(""),
        None,
    )
    .expect("failed to create new wallet");

    let fee_and_amounts = (0, ((0..32).map(|x| 2u64.pow(x)).collect::<Vec<_>>())).into();
    let proofs = fund_wallet(&wallet, 100.into(), SplitTarget::default()).await;

    let active_keyset_id = active_keyset_id(&wallet).await;
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
    let wallet = cdk_integration_tests::open_test_wallet(
        MINT_URL,
        CurrencyUnit::Sat,
        Arc::new(memory::empty().await.unwrap()),
        Mnemonic::generate(12).unwrap().to_seed_normalized(""),
        None,
    )
    .expect("failed to create new wallet");

    let proofs = fund_wallet(&wallet, 100.into(), SplitTarget::default()).await;

    let active_keyset_id = active_keyset_id(&wallet).await;
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
    let wallet = cdk_integration_tests::open_test_wallet(
        MINT_URL,
        CurrencyUnit::Sat,
        Arc::new(memory::empty().await.unwrap()),
        Mnemonic::generate(12).unwrap().to_seed_normalized(""),
        None,
    )
    .expect("failed to create new wallet");

    let proofs = fund_wallet(&wallet, 100.into(), SplitTarget::default()).await;

    let active_keyset_id = active_keyset_id(&wallet).await;
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
    let payment = quote_payment(&wallet, invoice).await;
    let melt_request = MeltRequest::new(payment.id().to_string(), proofs, None);

    let http_client = HttpClient::new(MINT_URL.parse().unwrap(), None);
    let response = http_client
        .post_melt(
            &PaymentMethod::Known(KnownMethod::Bolt11),
            melt_request.clone(),
        )
        .await;

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
    let wallet = cdk_integration_tests::open_test_wallet(
        MINT_URL,
        CurrencyUnit::Sat,
        Arc::new(memory::empty().await.unwrap()),
        Mnemonic::generate(12).unwrap().to_seed_normalized(""),
        None,
    )
    .expect("failed to create new wallet");

    let proofs = fund_wallet(&wallet, 100.into(), SplitTarget::default()).await;

    let active_keyset_id = active_keyset_id(&wallet).await;
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
    let wallet = cdk_integration_tests::open_test_wallet(
        MINT_URL,
        CurrencyUnit::Sat,
        Arc::new(memory::empty().await.unwrap()),
        Mnemonic::generate(12).unwrap().to_seed_normalized(""),
        None,
    )
    .expect("failed to create new wallet");

    let proofs = fund_wallet(&wallet, 100.into(), SplitTarget::default()).await;

    let inputs = vec![proofs[0].clone(), proofs[0].clone()];

    let invoice = create_fake_invoice(7000, "".to_string());

    let payment = quote_payment(&wallet, invoice).await;
    let melt_request = MeltRequest::new(payment.id().to_string(), inputs, None);

    let http_client = HttpClient::new(MINT_URL.parse().unwrap(), None);
    let response = http_client
        .post_melt(
            &PaymentMethod::Known(KnownMethod::Bolt11),
            melt_request.clone(),
        )
        .await;

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
    let wallet = cdk_integration_tests::open_test_wallet(
        MINT_URL,
        CurrencyUnit::Sat,
        Arc::new(memory::empty().await.unwrap()),
        Mnemonic::generate(12).unwrap().to_seed_normalized(""),
        None,
    )
    .expect("failed to create new wallet");

    fund_wallet(&wallet, 100.into(), SplitTarget::default()).await;

    assert_eq!(wallet.balance().await.unwrap().available, Amount::from(100));

    // Model an authoritative backend failure response. An error from the
    // dispatch call would be ambiguous even if an immediate status check
    // reported Unpaid.
    let fake_description = FakeInvoiceDescription {
        pay_invoice_state: MeltQuoteState::Failed,
        check_payment_state: MeltQuoteState::Failed,
        pay_err: false,
        check_err: false,
    };

    let invoice = create_fake_invoice(1000, serde_json::to_string(&fake_description).unwrap());
    let payment = quote_payment(&wallet, invoice).await;

    // Attempt to melt - this should fail but trigger proof recovery
    let melt_result = tokio::time::timeout(Duration::from_secs(15), async {
        payment.prepare().await?.execute().await
    })
    .await
    .expect("authoritative failure should not remain pending");
    assert!(melt_result.is_err(), "Melt should have failed");

    // Verify wallet still has balance (proofs recovered)
    assert_eq!(
        wallet.balance().await.unwrap().available,
        Amount::from(100),
        "Balance should be recovered"
    );

    // Verify we can still spend the recovered proofs
    let valid_invoice = create_fake_invoice(7000, "".to_string());
    let valid_payment = quote_payment(&wallet, valid_invoice).await;

    let successful_melt = async { valid_payment.prepare().await?.execute().await }.await;
    assert!(
        successful_melt.is_ok(),
        "Should be able to spend recovered proofs"
    );
}

/// Regression test for cashubtc/cdk#1891.
///
/// Exercises the chained `PaymentSession::prepare().await?.execute().await?`
/// pattern where the plan is consumed on failure. The wallet should recover via
/// the persisted melt saga without requiring `check_all_pending_proofs()`.
#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_chained_confirm_auto_releases_proofs_on_failure() {
    let wallet = cdk_integration_tests::open_test_wallet(
        MINT_URL,
        CurrencyUnit::Sat,
        Arc::new(memory::empty().await.unwrap()),
        Mnemonic::generate(12).unwrap().to_seed_normalized(""),
        None,
    )
    .expect("failed to create new wallet");

    fund_wallet(&wallet, 100.into(), SplitTarget::default()).await;

    assert_eq!(wallet.balance().await.unwrap().available, Amount::from(100));

    let fake_description = FakeInvoiceDescription {
        pay_invoice_state: MeltQuoteState::Failed,
        check_payment_state: MeltQuoteState::Failed,
        pay_err: false,
        check_err: false,
    };
    let invoice = create_fake_invoice(1000, serde_json::to_string(&fake_description).unwrap());
    let payment = quote_payment(&wallet, invoice).await;

    let melt_result = payment.prepare().await.unwrap().execute().await;
    assert!(melt_result.is_err(), "Melt should fail on Failed state");

    assert!(
        proof_count(&wallet, State::Pending).await == 0,
        "no proofs should remain Pending after automatic saga recovery"
    );

    assert_eq!(
        wallet.balance().await.unwrap().available,
        Amount::from(100),
        "balance should be fully recovered without explicit recovery calls"
    );
}

/// Regression test for the recovered-paid confirm path.
///
/// If the initial melt request errors locally but recovery sees the quote as
/// `Paid`, chained payment-plan preparation and confirmation should still
/// return success.
#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_chained_confirm_recovers_paid_melt() {
    let wallet = cdk_integration_tests::open_test_wallet(
        MINT_URL,
        CurrencyUnit::Sat,
        Arc::new(memory::empty().await.unwrap()),
        Mnemonic::generate(12).unwrap().to_seed_normalized(""),
        None,
    )
    .expect("Failed to create new wallet");

    fund_wallet(&wallet, 100.into(), SplitTarget::default()).await;

    let old_balance = wallet.balance().await.expect("balance").available;

    let fake_description = FakeInvoiceDescription {
        pay_invoice_state: MeltQuoteState::Failed,
        check_payment_state: MeltQuoteState::Paid,
        pay_err: true,
        check_err: false,
    };
    let invoice = create_fake_invoice(7000, serde_json::to_string(&fake_description).unwrap());
    let payment = quote_payment(&wallet, invoice).await;

    let melt = payment
        .prepare()
        .await
        .unwrap()
        .execute()
        .await
        .expect("confirm should recover to Paid");

    assert_eq!(melt.fee_paid, Amount::ZERO);
    assert_eq!(melt.amount, Amount::from(7));
    assert_eq!(
        old_balance - melt.amount,
        wallet.balance().await.expect("new balance").available
    );

    assert!(
        proof_count(&wallet, State::Pending).await == 0,
        "paid recovery should not leave pending proofs"
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
            cdk_integration_tests::open_test_wallet(
                MINT_URL,
                CurrencyUnit::Sat,
                Arc::new(memory::empty().await.unwrap()),
                Mnemonic::generate(12).unwrap().to_seed_normalized(""),
                None,
            )
            .unwrap_or_else(|_| panic!("failed to create wallet {}", i)),
        );
        wallets.push(wallet);
    }

    // Mint proofs for all wallets
    for wallet in &wallets {
        fund_wallet(wallet, 100.into(), SplitTarget::default()).await;
    }

    // Create a single invoice that all wallets will try to pay
    let fake_description = FakeInvoiceDescription::default();
    let invoice = create_fake_invoice(9000, serde_json::to_string(&fake_description).unwrap());

    // All wallets create melt quotes for the same invoice
    let mut payment_sessions = Vec::with_capacity(NUM_WALLETS);
    for wallet in &wallets {
        payment_sessions.push(quote_payment(wallet, &invoice).await);
    }

    for session in &payment_sessions[1..] {
        assert_eq!(payment_sessions[0].quote().amount, session.quote().amount);
        assert_ne!(payment_sessions[0].id(), session.id());
    }

    // Attempt all melts concurrently
    let mut handles = Vec::with_capacity(NUM_WALLETS);
    for session in payment_sessions {
        let plan = session.prepare().await.expect("payment plan");
        handles.push(tokio::spawn(async move { plan.execute().await }));
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
                    || err_str.contains("pending")
                    || err_str.contains("payment failed"),
                "Expected duplicate/already paid/pending/payment failed error, got: {}",
                err
            );
        }
    }
}

/// Tests that wallet automatically recovers proofs after a failed swap operation
#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_wallet_proof_recovery_after_failed_swap() {
    let wallet = cdk_integration_tests::open_test_wallet(
        MINT_URL,
        CurrencyUnit::Sat,
        Arc::new(memory::empty().await.unwrap()),
        Mnemonic::generate(12).unwrap().to_seed_normalized(""),
        None,
    )
    .expect("failed to create new wallet");

    let initial_proofs = fund_wallet(&wallet, 100.into(), SplitTarget::default()).await;

    let initial_ys: Vec<_> = initial_proofs.iter().map(|p| p.y().unwrap()).collect();

    assert_eq!(wallet.balance().await.unwrap().available, Amount::from(100));

    let available_proofs = unspent_proofs(&wallet).await;

    // Create an invalid swap by manually constructing a request that will fail
    // We'll use the wallet's swap with invalid parameters to trigger a failure
    let active_keyset_id = active_keyset_id(&wallet).await;
    let fee_and_amounts = (0, ((0..32).map(|x| 2u64.pow(x)).collect::<Vec<_>>())).into();

    // Create invalid swap request (requesting more than we have)
    let preswap = PreMintSecrets::random(
        active_keyset_id,
        1000.into(), // More than the 100 we have
        &SplitTarget::default(),
        &fee_and_amounts,
    )
    .unwrap();

    let swap_request = SwapRequest::new(available_proofs.clone(), preswap.blinded_messages());

    // Use HTTP client directly to bypass wallet's validation and trigger recovery
    let http_client = HttpClient::new(MINT_URL.parse().unwrap(), None);
    let response = http_client.post_swap(swap_request).await;
    assert!(response.is_err(), "Swap should have failed");

    // Note: The HTTP client doesn't trigger the wallet's try_proof_operation wrapper
    // So we need to test through the wallet's own methods
    // After the failed HTTP request, the proofs are still in the wallet's database

    // Verify balance is still available after the failed operation
    assert_eq!(
        wallet.balance().await.unwrap().available,
        Amount::from(100),
        "Balance should still be available"
    );

    // Verify we can perform a successful swap operation
    let successful_swap = wallet
        .advanced()
        .reissue(ReissueRequest {
            proofs: available_proofs,
            amount: None,
            amount_split_target: SplitTarget::None,
            conditions: None,
            fee_policy: ReissueFeePolicy::Deduct,
            protection: ReissueProtection::Plain,
        })
        .await;

    assert!(
        successful_swap.is_ok(),
        "Should be able to swap after failed operation"
    );

    // Verify the proofs were swapped to new ones
    let final_proofs = unspent_proofs(&wallet).await;
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
    let wallet_sender = cdk_integration_tests::open_test_wallet(
        MINT_URL,
        CurrencyUnit::Sat,
        Arc::new(memory::empty().await.unwrap()),
        Mnemonic::generate(12).unwrap().to_seed_normalized(""),
        None,
    )
    .expect("failed to create sender wallet");

    let proofs = fund_wallet(&wallet_sender, 100.into(), SplitTarget::default()).await;

    assert_eq!(proofs.total_amount().unwrap(), Amount::from(100));

    // Create receiver/melter wallet (Wallet B) with a separate database
    // These proofs are NOT in Wallet B's database
    let wallet_melter = cdk_integration_tests::open_test_wallet(
        MINT_URL,
        CurrencyUnit::Sat,
        Arc::new(memory::empty().await.unwrap()),
        Mnemonic::generate(12).unwrap().to_seed_normalized(""),
        None,
    )
    .expect("failed to create melter wallet");

    // Verify proofs are not in the melter wallet's database
    let melter_proofs = unspent_proofs(&wallet_melter).await;
    assert!(
        melter_proofs.is_empty(),
        "Melter wallet should have no proofs initially"
    );

    // Create a fake invoice for melting
    let fake_description = FakeInvoiceDescription::default();
    let invoice = create_fake_invoice(9000, serde_json::to_string(&fake_description).unwrap());

    // Wallet B creates a melt quote
    let payment = quote_payment(&wallet_melter, invoice).await;

    // Wallet B calls melt_proofs with external proofs (from Wallet A)
    // These proofs are NOT in wallet_melter's database
    let prepared = payment
        .prepare_with(PaymentPrepareOptions {
            funding: PaymentFunding::Proofs(proofs.clone()),
        })
        .await
        .unwrap();
    let melted = prepared.execute().await.unwrap();

    // Verify the melt succeeded
    assert_eq!(melted.amount, Amount::from(9));
    assert_eq!(melted.fee_paid, 1.into());

    // Change from externally funded proofs is persisted in the paying wallet.
    let melter_balance = wallet_melter.balance().await.unwrap().available;
    assert_eq!(melter_balance, Amount::from(90));

    // Verify a transaction was recorded
    let transactions = wallet_melter
        .history(HistoryQuery {
            direction: Some(TransactionDirection::Outgoing),
            limit: None,
        })
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
    let wallet = cdk_integration_tests::open_test_wallet(
        MINT_URL,
        CurrencyUnit::Sat,
        Arc::new(memory::empty().await.unwrap()),
        Mnemonic::generate(12).unwrap().to_seed_normalized(""),
        None,
    )
    .expect("failed to create new wallet");

    // Mint 100 sats - this will give us proofs in standard denominations
    let proofs = fund_wallet(&wallet, 100.into(), SplitTarget::default()).await;

    let initial_balance = wallet.balance().await.unwrap().available;
    assert_eq!(initial_balance, Amount::from(100));

    // Log the proof denominations we received
    let proof_amounts: Vec<u64> = proofs.iter().map(|p| u64::from(p.amount)).collect();
    tracing::info!("Initial proof denominations: {:?}", proof_amounts);

    // Create a melt quote for an amount that likely won't match our proof denominations exactly
    // Using 7 sats (7000 msats) which requires specific denominations
    let fake_description = FakeInvoiceDescription::default();
    let invoice = create_fake_invoice(7000, serde_json::to_string(&fake_description).unwrap());

    let payment = quote_payment(&wallet, invoice).await;
    let quote = payment.quote();

    tracing::info!(
        "Melt quote: amount={}, fee_reserve={}",
        quote.amount,
        quote.fee_reserve
    );

    // Call melt() - this should trigger swap-before-melt if proofs don't match exactly
    let prepared = payment.prepare().await.unwrap();
    let melted = prepared.execute().await.unwrap();

    // Verify the melt succeeded
    assert_eq!(melted.amount, Amount::from(7));

    tracing::info!(
        "Melt completed: amount={}, fee_paid={}",
        melted.amount,
        melted.fee_paid
    );

    // Verify final balance is correct (initial - melt_amount - fees)
    let final_balance = wallet.balance().await.unwrap().available;
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
    let wallet = cdk_integration_tests::open_test_wallet(
        MINT_URL,
        CurrencyUnit::Sat,
        Arc::new(memory::empty().await.unwrap()),
        Mnemonic::generate(12).unwrap().to_seed_normalized(""),
        None,
    )
    .expect("failed to create new wallet");

    // Mint a larger amount to have more denomination options
    fund_wallet(&wallet, 1000.into(), SplitTarget::default()).await;

    let initial_balance = wallet.balance().await.unwrap().available;
    assert_eq!(initial_balance, Amount::from(1000));

    // Create a melt for a power-of-2 amount that's more likely to match existing denominations
    let fake_description = FakeInvoiceDescription::default();
    let invoice = create_fake_invoice(64_000, serde_json::to_string(&fake_description).unwrap()); // 64 sats

    let payment = quote_payment(&wallet, invoice).await;

    // Melt should succeed
    let prepared = payment.prepare().await.unwrap();
    let melted = prepared.execute().await.unwrap();

    assert_eq!(melted.amount, Amount::from(64));

    let final_balance = wallet.balance().await.unwrap().available;
    assert_eq!(
        final_balance,
        initial_balance - melted.amount - melted.fee_paid
    );
}

/// Tests that synchronization claims all paid Bolt11 mint sessions.
///
/// This test verifies that:
/// 1. Paid mint quotes are automatically minted when check_all_mint_quotes is called
/// 2. The total amount returned matches the minted proofs
/// 3. Quote state is properly updated after minting
#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_synchronize_claims_paid_bolt11_sessions() {
    let wallet = cdk_integration_tests::open_test_wallet(
        MINT_URL,
        CurrencyUnit::Sat,
        Arc::new(memory::empty().await.unwrap()),
        Mnemonic::generate(12).unwrap().to_seed_normalized(""),
        None,
    )
    .expect("failed to create new wallet");

    paid_mint_session(&wallet, PaymentMethod::BOLT11, Some(100.into())).await;
    paid_mint_session(&wallet, PaymentMethod::BOLT11, Some(50.into())).await;

    // Verify no proofs have been minted yet
    assert_eq!(wallet.balance().await.unwrap().available, Amount::ZERO);

    // Call mint_unissued_quotes - this should mint both paid quotes
    let report = wallet.synchronize(SyncPolicy::Online).await.unwrap();

    // Verify the total amount minted is correct (100 + 50 = 150)
    assert_eq!(report.claimed_amount, Amount::from(150));

    // Verify wallet balance matches
    assert_eq!(report.balance.available, Amount::from(150));

    // Calling mint_unissued_quotes again should return 0 (quotes already minted)
    let second_report = wallet.synchronize(SyncPolicy::Online).await.unwrap();
    assert_eq!(second_report.claimed_amount, Amount::ZERO);
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
    let wallet = cdk_integration_tests::open_test_wallet(
        MINT_URL,
        CurrencyUnit::Sat,
        Arc::new(memory::empty().await.unwrap()),
        Mnemonic::generate(12).unwrap().to_seed_normalized(""),
        None,
    )
    .expect("failed to create new wallet");

    // Create a quote but don't pay it (stays unpaid)
    let unpaid_session = wallet
        .request_mint(WalletMintRequest::bolt11(100.into()))
        .await
        .unwrap();

    // Create another quote and pay it but don't mint
    let paid_session = paid_mint_session(&wallet, PaymentMethod::BOLT11, Some(50.into())).await;

    // Create a third quote and fully mint it
    let minted_session = wallet
        .request_mint(WalletMintRequest::bolt11(25.into()))
        .await
        .unwrap();
    minted_session
        .wait(Duration::from_secs(30))
        .await
        .expect("mint receipt");

    // Get unissued quotes
    let unissued_quotes = wallet
        .advanced()
        .mint_sessions(MintSessionFilter::Unissued)
        .await
        .unwrap();

    // Should have 2 quotes: unpaid and paid-but-not-issued
    // The fully minted quote should be excluded
    assert_eq!(
        unissued_quotes.len(),
        2,
        "Should have 2 unissued quotes (unpaid and paid-not-issued)"
    );

    let quote_ids: Vec<&str> = unissued_quotes
        .iter()
        .map(|session| session.id().as_str())
        .collect();
    assert!(
        quote_ids.contains(&unpaid_session.id().as_str()),
        "Unpaid quote should be included"
    );
    assert!(
        quote_ids.contains(&paid_session.id().as_str()),
        "Paid but not issued quote should be included"
    );
    assert!(
        !quote_ids.contains(&minted_session.id().as_str()),
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
async fn test_check_mint_quote_status_updates_after_minting() {
    let wallet = cdk_integration_tests::open_test_wallet(
        MINT_URL,
        CurrencyUnit::Sat,
        Arc::new(memory::empty().await.unwrap()),
        Mnemonic::generate(12).unwrap().to_seed_normalized(""),
        None,
    )
    .expect("failed to create new wallet");

    let mint_amount = Amount::from(100);
    let session = wallet
        .request_mint(WalletMintRequest::bolt11(mint_amount))
        .await
        .unwrap();

    // Verify initial state
    assert_eq!(session.initial_state().amount_claimed, Amount::ZERO);

    // Mint the tokens using wait_and_mint_quote
    let receipt = session
        .wait(Duration::from_secs(60))
        .await
        .expect("minting should succeed");

    let minted_amount = receipt.amount;
    assert_eq!(minted_amount, mint_amount);

    let state = session.refresh().await.expect("mint session state");
    assert_eq!(state.amount_claimed, minted_amount);
    assert_eq!(state.state, MintState::Issued);

    // Verify the unissued quotes no longer contains this quote
    let unissued = wallet
        .advanced()
        .mint_sessions(MintSessionFilter::Unissued)
        .await
        .unwrap();
    let unissued_ids: Vec<&str> = unissued
        .iter()
        .map(|session| session.id().as_str())
        .collect();
    assert!(
        !unissued_ids.contains(&session.id().as_str()),
        "Fully minted quote should not appear in unissued quotes"
    );
}

/// Tests that Bolt12 payment notifications persist `amount_issued` updates.
///
/// This catches regressions where the wallet updates `amount_paid` from the
/// notification stream but forgets to persist `amount_issued`, causing
/// `check_mint_quote_status` to drift from the notification-driven state.
#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_check_mint_quote_status_updates_amount_issued_for_bolt12() {
    let wallet = cdk_integration_tests::open_test_wallet(
        MINT_URL,
        CurrencyUnit::Sat,
        Arc::new(memory::empty().await.unwrap()),
        Mnemonic::generate(12).unwrap().to_seed_normalized(""),
        None,
    )
    .expect("failed to create new wallet");

    let session = paid_mint_session(&wallet, PaymentMethod::BOLT12, Some(Amount::from(100))).await;
    let quote_after_notification = session.refresh().await.expect("paid quote state");
    let paid_amount = quote_after_notification.amount_paid;

    assert_eq!(paid_amount, Amount::from(100));
    assert_eq!(
        quote_after_notification.amount_claimed,
        Amount::ZERO,
        "payment notification should not mark funds as issued before minting"
    );

    let minted_amount = session
        .claim()
        .await
        .expect("minting should succeed")
        .amount;

    assert_eq!(minted_amount, paid_amount);

    let quote_after_mint = session.refresh().await.expect("quote status check");

    assert_eq!(quote_after_mint.method, PaymentMethod::BOLT12);
    assert_eq!(quote_after_mint.amount_paid, minted_amount);
    assert_eq!(
        quote_after_mint.amount_claimed, minted_amount,
        "amount_issued should match the issued amount after Bolt12 minting"
    );
    assert_eq!(
        quote_after_mint.state,
        MintState::Issued,
        "quote should be fully issued after minting"
    );

    let http_client = HttpClient::new(MINT_URL.parse().unwrap(), None);
    let mint_response = http_client
        .get_mint_quote_status(PaymentMethod::BOLT12, session.id().as_str())
        .await
        .expect("mint quote status request should succeed");

    match mint_response {
        cdk_common::MintQuoteResponse::Bolt12(response) => {
            assert_eq!(response.amount_paid, minted_amount);
            assert_eq!(response.amount_issued, minted_amount);
        }
        response => panic!("expected Bolt12 quote response, got {response:?}"),
    }
}
